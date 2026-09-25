//! Intel GPUs (`i915` and the newer `xe` driver): clock, temperature and power from
//! sysfs/hwmon, usage and memory from DRM fdinfo.

use std::path::Path;
use std::time::Instant;

use super::fdinfo::Scanner;
use super::{hwmon_power, hwmon_temps, read_f64};
use crate::metrics::GpuSample;

/// Built-in Intel graphics always sits on PCI bus 0; Arc cards don't.
pub fn is_integrated(slot: &str) -> bool {
    slot.split(':').nth(1) == Some("00")
}

pub struct Intel {
    scanner: Scanner,
    integrated: bool,
    /// Last `energy1_input` reading, for cards that report energy but not power.
    energy: Option<(f64, Instant)>,
}

impl Intel {
    pub fn new(slot: &str) -> Self {
        Self {
            scanner: Scanner::new(slot),
            integrated: is_integrated(slot),
            energy: None,
        }
    }

    pub fn refresh(&mut self) {
        self.scanner.rescan();
    }

    pub fn sample(&mut self, card: &Path, dev: &Path, hwmon: Option<&Path>, s: &mut GpuSample) {
        s.core_mhz = [
            card.join("gt_act_freq_mhz"),
            card.join("gt/gt0/rps_act_freq_mhz"),
            dev.join("tile0/gt0/freq0/act_freq"),
        ]
        .iter()
        .find_map(|p| read_f64(p))
        .map(|mhz| mhz as f32);

        if let Some(h) = hwmon {
            s.temps = hwmon_temps(h);
            let (power, cap) = hwmon_power(h);
            s.power_cap_w = cap;
            s.power_w = power.or_else(|| {
                let uj = read_f64(&h.join("energy1_input"))?;
                let now = Instant::now();
                let (prev, t) = self.energy.replace((uj, now))?;
                let secs = now.duration_since(t).as_secs_f64();
                (secs > 0.0 && uj >= prev).then(|| ((uj - prev) / 1e6 / secs) as f32)
            });
        }

        let (usage, system, vram) = self.scanner.sample();
        s.usage = usage;
        if self.integrated {
            // Integrated graphics has no VRAM; all of its memory is system RAM.
            s.system_used = Some(system);
        } else {
            s.vram_used = Some(vram);
            s.system_used = Some(system);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bus_zero_is_integrated() {
        assert!(is_integrated("0000:00:02.0"));
        assert!(!is_integrated("0000:03:00.0"));
    }
}
