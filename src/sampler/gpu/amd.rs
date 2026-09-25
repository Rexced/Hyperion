//! AMD GPUs (the `amdgpu` driver): everything comes straight from sysfs.

use std::path::Path;

use super::{hwmon_power, hwmon_temps, read_f64};
use crate::metrics::GpuSample;

/// APUs (Ryzen with built-in graphics) have no VRAM chips of their own, so the
/// driver doesn't publish a VRAM vendor; dedicated cards always do.
pub fn is_integrated(dev: &Path) -> bool {
    !dev.join("mem_info_vram_vendor").exists()
}

pub fn sample(dev: &Path, hwmon: Option<&Path>, s: &mut GpuSample) {
    let bytes = |file: &str| read_f64(&dev.join(file)).map(|v| v as u64);
    s.usage = read_f64(&dev.join("gpu_busy_percent")).map(|v| v as f32);
    s.vram_used = bytes("mem_info_vram_used");
    s.vram_total = bytes("mem_info_vram_total");
    // GTT: GPU buffers living in system RAM (spill on dedicated cards).
    s.system_used = bytes("mem_info_gtt_used");
    if let Some(h) = hwmon {
        (s.power_w, s.power_cap_w) = hwmon_power(h);
        // freq1 is the shader (core) clock, in Hz.
        s.core_mhz = read_f64(&h.join("freq1_input")).map(|hz| (hz / 1e6) as f32);
        s.temps = hwmon_temps(h);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn reads_an_rx_6600_like_card() {
        let root = tempfile::tempdir().unwrap();
        let dev = root.path();
        for (file, value) in [
            ("gpu_busy_percent", "37"),
            ("mem_info_vram_used", "716427264"),
            ("mem_info_vram_total", "8573157376"),
            ("mem_info_gtt_used", "59392000"),
            ("mem_info_vram_vendor", "samsung"),
        ] {
            fs::write(dev.join(file), value).unwrap();
        }
        let h = dev.join("hwmon/hwmon3");
        fs::create_dir_all(&h).unwrap();
        fs::write(h.join("freq1_input"), "2450000000").unwrap();
        fs::write(h.join("power1_average"), "98000000").unwrap();
        fs::write(h.join("temp1_input"), "61000").unwrap();
        fs::write(h.join("temp1_label"), "edge").unwrap();

        let mut s = GpuSample::default();
        sample(dev, Some(&h), &mut s);
        assert_eq!(s.usage, Some(37.0));
        assert_eq!(s.vram_total, Some(8_573_157_376));
        assert_eq!(s.system_used, Some(59_392_000));
        assert_eq!(s.core_mhz, Some(2450.0));
        assert_eq!(s.power_w, Some(98.0));
        assert_eq!(s.temps, [("Edge".to_owned(), 61.0)]);
        assert!(!is_integrated(dev));
    }

    #[test]
    fn no_vram_vendor_means_integrated() {
        let root = tempfile::tempdir().unwrap();
        assert!(is_integrated(root.path()));
    }
}
