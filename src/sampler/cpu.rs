use std::io;

use super::procfile::ProcFile;
use crate::metrics::CpuSample;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CpuTimes {
    pub busy: u64,
    pub total: u64,
}

/// Parses the aggregate `cpu` line and every `cpuN` line of `/proc/stat`.
pub fn parse_stat(text: &str) -> (Option<CpuTimes>, Vec<CpuTimes>) {
    let mut aggregate = None;
    let mut cores = Vec::new();
    for line in text.lines() {
        if !line.starts_with("cpu") {
            // cpu lines come first; the rest (notably `intr`) is long and irrelevant.
            break;
        }
        let mut parts = line.split_ascii_whitespace();
        let Some(label) = parts.next() else { continue };
        // user nice system idle iowait irq softirq steal (guest time is already inside user/nice)
        let mut v = [0u64; 8];
        for (slot, field) in v.iter_mut().zip(parts) {
            *slot = field.parse().unwrap_or(0);
        }
        let total: u64 = v.iter().sum();
        let idle = v[3] + v[4];
        let times = CpuTimes {
            busy: total.saturating_sub(idle),
            total,
        };
        if label == "cpu" {
            aggregate = Some(times);
        } else {
            cores.push(times);
        }
    }
    (aggregate, cores)
}

pub fn usage_percent(prev: CpuTimes, cur: CpuTimes) -> f32 {
    let total = cur.total.saturating_sub(prev.total);
    if total == 0 {
        return 0.0;
    }
    let busy = cur.busy.saturating_sub(prev.busy);
    (busy as f32 / total as f32 * 100.0).clamp(0.0, 100.0)
}

pub struct CpuCollector {
    stat: ProcFile,
    prev_total: CpuTimes,
    prev_cores: Vec<CpuTimes>,
}

impl CpuCollector {
    pub fn new() -> io::Result<Self> {
        let mut stat = ProcFile::open("/proc/stat")?;
        let (total, cores) = parse_stat(stat.read()?);
        Ok(Self {
            stat,
            prev_total: total.unwrap_or_default(),
            prev_cores: cores,
        })
    }

    pub fn sample(&mut self) -> io::Result<CpuSample> {
        let (total, cores) = parse_stat(self.stat.read()?);
        let total = total.unwrap_or_default();
        let sample = CpuSample {
            total: usage_percent(self.prev_total, total),
            per_core: if cores.len() == self.prev_cores.len() {
                self.prev_cores
                    .iter()
                    .zip(&cores)
                    .map(|(&p, &c)| usage_percent(p, c))
                    .collect()
            } else {
                // CPU hotplug changed the core count; skip one delta.
                vec![0.0; cores.len()]
            },
            ..Default::default()
        };
        self.prev_total = total;
        self.prev_cores = cores;
        Ok(sample)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAT_A: &str = "\
cpu  100 0 50 800 50 0 0 0 0 0
cpu0 50 0 25 400 25 0 0 0 0 0
cpu1 50 0 25 400 25 0 0 0 0 0
intr 12345 0 0 0
ctxt 999
";
    const STAT_B: &str = "\
cpu  200 0 100 850 50 0 0 0 0 0
cpu0 150 0 75 400 25 0 0 0 0 0
cpu1 50 0 25 450 25 0 0 0 0 0
intr 12400 0 0 0
";

    #[test]
    fn parses_aggregate_and_cores() {
        let (total, cores) = parse_stat(STAT_A);
        assert_eq!(
            total,
            Some(CpuTimes {
                busy: 150,
                total: 1000
            })
        );
        assert_eq!(cores.len(), 2);
        assert_eq!(
            cores[0],
            CpuTimes {
                busy: 75,
                total: 500
            }
        );
    }

    #[test]
    fn computes_usage_from_deltas() {
        let (a, ca) = parse_stat(STAT_A);
        let (b, cb) = parse_stat(STAT_B);
        // total: busy +150 over +200 ticks
        assert_eq!(usage_percent(a.unwrap(), b.unwrap()), 75.0);
        assert_eq!(usage_percent(ca[0], cb[0]), 100.0);
        assert_eq!(usage_percent(ca[1], cb[1]), 0.0);
    }

    #[test]
    fn zero_delta_is_zero_usage() {
        let t = CpuTimes { busy: 5, total: 10 };
        assert_eq!(usage_percent(t, t), 0.0);
    }
}
