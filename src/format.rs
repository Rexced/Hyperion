const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];

pub fn bytes(value: f64) -> String {
    let mut v = value.max(0.0);
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{v:.0} {}", UNITS[0])
    } else if v >= 100.0 {
        format!("{v:.0} {}", UNITS[unit])
    } else {
        format!("{v:.1} {}", UNITS[unit])
    }
}

pub fn rate(bytes_per_sec: f64) -> String {
    format!("{}/s", bytes(bytes_per_sec))
}

pub fn percent(value: f64) -> String {
    format!("{value:.0}%")
}

pub fn interval_ms(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms} ms")
    } else {
        format!("{} s", ms / 1000)
    }
}

pub fn span_secs(secs: u64) -> String {
    if secs < 60 {
        format!("{secs} s")
    } else {
        format!("{} min", secs / 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_bytes() {
        assert_eq!(bytes(512.0), "512 B");
        assert_eq!(bytes(1536.0), "1.5 KiB");
        assert_eq!(bytes(16_284_692.0 * 1024.0), "15.5 GiB");
        assert_eq!(bytes(500.0 * 1024.0 * 1024.0), "500 MiB");
        assert_eq!(rate(2048.0), "2.0 KiB/s");
    }

    #[test]
    fn formats_durations() {
        assert_eq!(interval_ms(100), "100 ms");
        assert_eq!(interval_ms(3000), "3 s");
        assert_eq!(span_secs(30), "30 s");
        assert_eq!(span_secs(1800), "30 min");
    }
}
