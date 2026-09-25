//! NVIDIA GPUs through NVML, NVIDIA's own monitoring library. `nvml-wrapper` loads
//! `libnvidia-ml` at runtime, so Hyperion still runs on machines without the driver.
//! The same library exists on Windows, which keeps this backend portable.

use nvml_wrapper::Nvml;
use nvml_wrapper::enum_wrappers::device::{Clock, TemperatureSensor};

use crate::metrics::GpuSample;

pub struct Nvidia {
    nvml: Nvml,
}

/// NVML writes bus ids as `00000000:01:00.0` (8-digit domain, upper case); sysfs as
/// `0000:01:00.0`.
pub fn normalize_bus_id(bus_id: &str) -> Option<String> {
    let (domain, rest) = bus_id.trim().split_once(':')?;
    let (bus, rest) = rest.split_once(':')?;
    let (device, function) = rest.split_once('.')?;
    let hex = |s: &str| u32::from_str_radix(s, 16).ok();
    Some(format!(
        "{:04x}:{:02x}:{:02x}.{:x}",
        hex(domain)?,
        hex(bus)?,
        hex(device)?,
        hex(function)?
    ))
}

impl Nvidia {
    pub fn init() -> Option<Self> {
        Nvml::init().ok().map(|nvml| Self { nvml })
    }

    /// NVML's index for the GPU in PCI slot `slot`.
    pub fn index_of(&self, slot: &str) -> Option<u32> {
        (0..self.nvml.device_count().ok()?).find(|&i| {
            self.nvml
                .device_by_index(i)
                .and_then(|d| d.pci_info())
                .ok()
                .and_then(|pci| normalize_bus_id(&pci.bus_id))
                .as_deref()
                == Some(slot)
        })
    }

    pub fn name(&self, index: u32) -> Option<String> {
        let name = self.nvml.device_by_index(index).ok()?.name().ok()?;
        Some(name.trim_start_matches("NVIDIA ").to_owned())
    }

    pub fn sample(&self, index: u32, s: &mut GpuSample) {
        let Ok(d) = self.nvml.device_by_index(index) else {
            return;
        };
        s.usage = d.utilization_rates().ok().map(|u| u.gpu as f32);
        s.core_mhz = d.clock_info(Clock::Graphics).ok().map(|mhz| mhz as f32);
        s.power_w = d.power_usage().ok().map(|mw| mw as f32 / 1000.0);
        s.power_cap_w = d.enforced_power_limit().ok().map(|mw| mw as f32 / 1000.0);
        if let Ok(mem) = d.memory_info() {
            s.vram_used = Some(mem.used);
            s.vram_total = Some(mem.total);
        }
        // NVML reports one core temperature and no figure for data spilled to RAM.
        if let Ok(t) = d.temperature(TemperatureSensor::Gpu) {
            s.temps = vec![("GPU".to_owned(), t as f32)];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bus_ids_match_sysfs_slots() {
        assert_eq!(
            normalize_bus_id("00000000:01:00.0").as_deref(),
            Some("0000:01:00.0")
        );
        assert_eq!(
            normalize_bus_id("00000000:0A:00.1").as_deref(),
            Some("0000:0a:00.1")
        );
        assert_eq!(normalize_bus_id("garbage"), None);
    }
}
