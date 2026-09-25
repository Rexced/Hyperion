use std::collections::{HashMap, VecDeque};

use crate::metrics::Snapshot;

/// Time series of `[t, value]` with `t` in sampler seconds.
#[derive(Default)]
pub struct Series {
    pts: VecDeque<[f64; 2]>,
}

pub struct History {
    window: f64,
    now: f64,
    latest: Snapshot,
    series: HashMap<String, Series>,
}

impl History {
    pub fn new(window_secs: f64) -> Self {
        Self {
            window: window_secs,
            now: 0.0,
            latest: Snapshot::default(),
            series: HashMap::new(),
        }
    }

    pub fn window(&self) -> f64 {
        self.window
    }

    pub fn set_window(&mut self, secs: f64) {
        self.window = secs;
        self.trim();
    }

    pub fn latest(&self) -> &Snapshot {
        &self.latest
    }

    #[cfg(test)]
    pub fn series(&self, key: &str) -> Option<&Series> {
        self.series.get(key)
    }

    pub fn push(&mut self, mut snap: Snapshot) {
        self.now = snap.t;
        if let Some(cpu) = &snap.cpu {
            self.record("cpu", cpu.total as f64);
        }
        if let Some(m) = snap.mem {
            if m.total > 0 {
                self.record("ram", m.used as f64 / m.total as f64 * 100.0);
            }
            if m.swap_total > 0 {
                self.record("swap", m.swap_used as f64 / m.swap_total as f64 * 100.0);
            }
        }
        for d in &snap.disks {
            self.record(&format!("disk:{}:r", d.name), d.read_bps);
            self.record(&format!("disk:{}:w", d.name), d.write_bps);
        }
        for n in &snap.net {
            self.record(&format!("net:{}:rx", n.name), n.rx_bps);
            self.record(&format!("net:{}:tx", n.name), n.tx_bps);
        }
        if snap.filesystems.is_none() {
            snap.filesystems = self.latest.filesystems.take();
        }
        self.latest = snap;
        self.trim();
    }

    /// Drops every series whose key starts with `prefix` (used when a metric is switched off).
    pub fn clear_prefix(&mut self, prefix: &str) {
        self.series.retain(|k, _| !k.starts_with(prefix));
    }

    /// Points with x relative to now (−window..=0), min/max-decimated to about `buckets` columns.
    pub fn plot_points(&self, key: &str, buckets: usize) -> Vec<[f64; 2]> {
        match self.series.get(key) {
            Some(s) => decimate(&s.pts, self.now, self.window, buckets.max(1)),
            None => Vec::new(),
        }
    }

    fn record(&mut self, key: &str, value: f64) {
        let point = [self.now, value];
        match self.series.get_mut(key) {
            Some(s) => s.pts.push_back(point),
            None => {
                self.series.insert(
                    key.to_owned(),
                    Series {
                        pts: VecDeque::from([point]),
                    },
                );
            }
        }
    }

    fn trim(&mut self) {
        let cutoff = self.now - self.window;
        self.series.retain(|_, s| {
            // Keep one point left of the window so the line reaches the plot's left edge.
            while s.pts.len() >= 2 && s.pts[1][0] <= cutoff {
                s.pts.pop_front();
            }
            s.pts.back().is_some_and(|p| p[0] >= cutoff)
        });
    }
}

/// Min/max decimation: keeps each bucket's extremes so spikes survive downsampling.
fn decimate(pts: &VecDeque<[f64; 2]>, now: f64, window: f64, buckets: usize) -> Vec<[f64; 2]> {
    let rel = |p: &[f64; 2]| [p[0] - now, p[1]];
    if pts.len() <= buckets * 2 {
        return pts.iter().map(rel).collect();
    }
    let width = window / buckets as f64;
    let mut out = Vec::with_capacity(buckets * 2 + 2);
    let mut current: Option<(i64, [f64; 2], [f64; 2])> = None;
    let flush = |out: &mut Vec<[f64; 2]>, lo: [f64; 2], hi: [f64; 2]| {
        if lo == hi {
            out.push(lo);
        } else if lo[0] < hi[0] {
            out.extend([lo, hi]);
        } else {
            out.extend([hi, lo]);
        }
    };
    for p in pts {
        let p = rel(p);
        let bucket = ((p[0] + window) / width).floor() as i64;
        match &mut current {
            Some((b, lo, hi)) if *b == bucket => {
                if p[1] < lo[1] {
                    *lo = p;
                }
                if p[1] > hi[1] {
                    *hi = p;
                }
            }
            _ => {
                if let Some((_, lo, hi)) = current {
                    flush(&mut out, lo, hi);
                }
                current = Some((bucket, p, p));
            }
        }
    }
    if let Some((_, lo, hi)) = current {
        flush(&mut out, lo, hi);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::{CpuSample, NetIo};

    fn cpu_snap(t: f64, total: f32) -> Snapshot {
        Snapshot {
            t,
            cpu: Some(CpuSample {
                total,
                per_core: vec![],
            }),
            ..Default::default()
        }
    }

    #[test]
    fn trims_to_window_keeping_one_leading_point() {
        let mut h = History::new(10.0);
        for i in 0..=30 {
            h.push(cpu_snap(i as f64, i as f32));
        }
        let pts = h.plot_points("cpu", 1000);
        assert_eq!(pts.first().unwrap()[0], -10.0);
        assert_eq!(pts.last().unwrap(), &[0.0, 30.0]);
        assert_eq!(pts.len(), 11);
    }

    #[test]
    fn vanished_devices_are_dropped() {
        let mut h = History::new(5.0);
        h.push(Snapshot {
            t: 0.0,
            net: vec![NetIo {
                name: "usb0".into(),
                rx_bps: 1.0,
                tx_bps: 1.0,
            }],
            ..Default::default()
        });
        assert!(h.series("net:usb0:rx").is_some());
        h.push(cpu_snap(10.0, 1.0));
        assert!(h.series("net:usb0:rx").is_none());
    }

    #[test]
    fn filesystems_persist_between_slow_ticks() {
        let mut h = History::new(60.0);
        h.push(Snapshot {
            t: 0.0,
            filesystems: Some(vec![]),
            ..Default::default()
        });
        h.push(cpu_snap(1.0, 5.0));
        assert!(h.latest().filesystems.is_some());
    }

    #[test]
    fn decimation_preserves_extremes_and_bounds_size() {
        let pts: VecDeque<[f64; 2]> = (0..18_000)
            .map(|i| {
                let v = if i == 9_001 {
                    100.0
                } else if i == 4_000 {
                    -5.0
                } else {
                    1.0
                };
                [i as f64 * 0.1, v]
            })
            .collect();
        let now = 1799.9;
        let out = decimate(&pts, now, 1800.0, 400);
        assert!(out.len() <= 2 * 401);
        assert!(out.iter().any(|p| p[1] == 100.0));
        assert!(out.iter().any(|p| p[1] == -5.0));
        assert!(out.windows(2).all(|w| w[0][0] <= w[1][0]));
    }

    #[test]
    fn clear_prefix_removes_matching_series() {
        let mut h = History::new(60.0);
        h.push(cpu_snap(0.0, 1.0));
        h.clear_prefix("cpu");
        assert!(h.series("cpu").is_none());
    }
}
