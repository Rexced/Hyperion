use std::io;

use super::procfile::ProcFile;
use crate::metrics::MemSample;

pub fn parse_meminfo(text: &str) -> MemSample {
    let (mut total, mut available, mut free) = (0, None, 0);
    let (mut swap_total, mut swap_free) = (0, 0);
    for line in text.lines() {
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let kib: u64 = rest
            .split_ascii_whitespace()
            .next()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let bytes = kib * 1024;
        match key {
            "MemTotal" => total = bytes,
            "MemAvailable" => available = Some(bytes),
            "MemFree" => free = bytes,
            "SwapTotal" => swap_total = bytes,
            "SwapFree" => swap_free = bytes,
            _ => {}
        }
    }
    MemSample {
        total,
        used: total.saturating_sub(available.unwrap_or(free)),
        swap_total,
        swap_used: swap_total.saturating_sub(swap_free),
    }
}

pub struct MemCollector {
    meminfo: ProcFile,
}

impl MemCollector {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            meminfo: ProcFile::open("/proc/meminfo")?,
        })
    }

    pub fn sample(&mut self) -> io::Result<MemSample> {
        Ok(parse_meminfo(self.meminfo.read()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_meminfo() {
        let text = "\
MemTotal:       16284692 kB
MemFree:         2000000 kB
MemAvailable:    8000000 kB
Buffers:          100000 kB
SwapCached:            0 kB
SwapTotal:      16285692 kB
SwapFree:       16285232 kB
";
        let m = parse_meminfo(text);
        assert_eq!(m.total, 16_284_692 * 1024);
        assert_eq!(m.used, (16_284_692 - 8_000_000) * 1024);
        assert_eq!(m.swap_total, 16_285_692 * 1024);
        assert_eq!(m.swap_used, 460 * 1024);
    }

    #[test]
    fn falls_back_to_memfree_without_memavailable() {
        let m = parse_meminfo("MemTotal: 1000 kB\nMemFree: 400 kB\n");
        assert_eq!(m.used, 600 * 1024);
        assert_eq!(m.swap_total, 0);
    }
}
