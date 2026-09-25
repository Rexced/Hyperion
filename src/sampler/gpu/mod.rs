//! GPUs: AMD and Intel through Linux sysfs, NVIDIA through NVML (loaded at runtime,
//! only if the NVIDIA driver is installed), and temperatures for anything else with
//! a hwmon sensor (e.g. nouveau).

mod amd;
mod fdinfo;
mod intel;
mod nvidia;
mod pci_ids;

use std::path::{Path, PathBuf};

use crate::metrics::GpuSample;

/// A GPU as the kernel's DRM subsystem lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    /// PCI slot, e.g. `0000:0c:00.0`.
    pub slot: String,
    pub driver: String,
    pub vendor: u16,
    pub device: u16,
    /// `/sys/class/drm/cardN`.
    pub card_dir: PathBuf,
    /// `/sys/class/drm/cardN/device` (the PCI device).
    pub dev_dir: PathBuf,
}

fn file_name_of(path: &Path) -> Option<String> {
    let resolved = std::fs::canonicalize(path).ok()?;
    Some(resolved.file_name()?.to_string_lossy().into_owned())
}

fn read_hex_u16(path: &Path) -> Option<u16> {
    let text = std::fs::read_to_string(path).ok()?;
    u16::from_str_radix(text.trim().trim_start_matches("0x"), 16).ok()
}

/// Every PCI GPU under `sys` (normally `/sys`), one per PCI slot.
pub fn discover(sys: &Path) -> Vec<Card> {
    let Ok(entries) = std::fs::read_dir(sys.join("class/drm")) else {
        return Vec::new();
    };
    let mut cards: Vec<Card> = entries
        .flatten()
        .filter(|e| {
            // `card0`, not connectors like `card0-DP-1` or render nodes.
            let name = e.file_name().to_string_lossy().into_owned();
            name.strip_prefix("card")
                .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        })
        .filter_map(|e| {
            let card_dir = e.path();
            let dev_dir = card_dir.join("device");
            Some(Card {
                slot: file_name_of(&dev_dir)?,
                driver: file_name_of(&dev_dir.join("driver")).unwrap_or_default(),
                vendor: read_hex_u16(&dev_dir.join("vendor"))?,
                device: read_hex_u16(&dev_dir.join("device"))?,
                card_dir,
                dev_dir,
            })
        })
        .collect();
    cards.sort_by(|a, b| a.slot.cmp(&b.slot));
    cards.dedup_by(|a, b| a.slot == b.slot);
    cards
}

pub(super) fn read_f64(path: &Path) -> Option<f64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// The GPU's hwmon directory (`device/hwmon/hwmonN`).
pub(super) fn hwmon_dir(dev_dir: &Path) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(dev_dir.join("hwmon"))
        .ok()?
        .flatten()
        .map(|e| e.path())
        .collect();
    dirs.sort();
    dirs.into_iter().next()
}

/// All temperature sensors, with readable names ("junction" becomes "Hotspot").
pub(super) fn hwmon_temps(hwmon: &Path) -> Vec<(String, f32)> {
    let mut temps = Vec::new();
    for n in 1..=8 {
        let Some(milli) = read_f64(&hwmon.join(format!("temp{n}_input"))) else {
            continue;
        };
        let label = std::fs::read_to_string(hwmon.join(format!("temp{n}_label")))
            .map(|l| l.trim().to_lowercase())
            .unwrap_or_default();
        let name = match label.as_str() {
            "edge" => "Edge",
            "junction" => "Hotspot",
            "mem" | "vram" => "Memory",
            "pkg" => "Package",
            "" => "GPU",
            other => {
                temps.push((other.to_owned(), (milli / 1000.0) as f32));
                continue;
            }
        };
        temps.push((name.to_owned(), (milli / 1000.0) as f32));
    }
    temps
}

/// Current power and power cap in watts (hwmon reports microwatts).
pub(super) fn hwmon_power(hwmon: &Path) -> (Option<f32>, Option<f32>) {
    let watts = |file: &str| read_f64(&hwmon.join(file)).map(|uw| (uw / 1e6) as f32);
    (
        watts("power1_average").or_else(|| watts("power1_input")),
        watts("power1_cap").filter(|&w| w > 0.0),
    )
}

enum Backend {
    Amd,
    Intel(intel::Intel),
    Nvidia(u32),
    /// Unknown driver: whatever hwmon offers (temperatures, maybe power).
    Basic,
}

struct Gpu {
    id: String,
    name: String,
    integrated: bool,
    card_dir: PathBuf,
    dev_dir: PathBuf,
    hwmon: Option<PathBuf>,
    backend: Backend,
}

pub struct GpuCollector {
    gpus: Vec<Gpu>,
    nvidia: Option<nvidia::Nvidia>,
}

impl GpuCollector {
    pub fn new() -> Self {
        let cards = discover(Path::new("/sys"));
        let db = pci_ids::load();
        let nvidia = cards
            .iter()
            .any(|c| c.driver == "nvidia")
            .then(nvidia::Nvidia::init)
            .flatten();
        let gpus = cards
            .into_iter()
            .map(|card| {
                let nvml_index = (card.driver == "nvidia")
                    .then(|| nvidia.as_ref()?.index_of(&card.slot))
                    .flatten();
                let backend = match (card.driver.as_str(), nvml_index) {
                    ("amdgpu", _) => Backend::Amd,
                    ("i915" | "xe", _) => Backend::Intel(intel::Intel::new(&card.slot)),
                    (_, Some(index)) => Backend::Nvidia(index),
                    _ => Backend::Basic,
                };
                let integrated = match &backend {
                    Backend::Amd => amd::is_integrated(&card.dev_dir),
                    Backend::Intel(_) => intel::is_integrated(&card.slot),
                    Backend::Nvidia(_) | Backend::Basic => false,
                };
                let name = nvml_index
                    .and_then(|i| nvidia.as_ref()?.name(i))
                    .or_else(|| pci_ids::lookup(db.as_deref()?, card.vendor, card.device))
                    .unwrap_or_else(|| format!("GPU {}", card.slot));
                Gpu {
                    id: card.slot,
                    name,
                    integrated,
                    hwmon: hwmon_dir(&card.dev_dir),
                    card_dir: card.card_dir,
                    dev_dir: card.dev_dir,
                    backend,
                }
            })
            .collect();
        Self { gpus, nvidia }
    }

    /// Slow periodic work (Intel: finding which processes use the GPU).
    pub fn refresh(&mut self) {
        for gpu in &mut self.gpus {
            if let Backend::Intel(intel) = &mut gpu.backend {
                intel.refresh();
            }
        }
    }

    pub fn sample(&mut self) -> Vec<GpuSample> {
        let nvidia = self.nvidia.as_ref();
        self.gpus
            .iter_mut()
            .map(|gpu| {
                let mut s = GpuSample {
                    id: gpu.id.clone(),
                    name: gpu.name.clone(),
                    integrated: gpu.integrated,
                    ..Default::default()
                };
                match &mut gpu.backend {
                    Backend::Amd => amd::sample(&gpu.dev_dir, gpu.hwmon.as_deref(), &mut s),
                    Backend::Intel(intel) => {
                        intel.sample(&gpu.card_dir, &gpu.dev_dir, gpu.hwmon.as_deref(), &mut s);
                    }
                    Backend::Nvidia(index) => {
                        if let Some(nv) = nvidia {
                            nv.sample(*index, &mut s);
                        }
                    }
                    Backend::Basic => {
                        if let Some(h) = &gpu.hwmon {
                            s.temps = hwmon_temps(h);
                            (s.power_w, s.power_cap_w) = hwmon_power(h);
                        }
                    }
                }
                s
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;

    /// A fake /sys with a GPU at `slot` bound to `driver`.
    fn add_gpu(sys: &Path, card: &str, slot: &str, driver: &str, vendor: &str, device: &str) {
        let dev = sys.join("devices").join(slot);
        fs::create_dir_all(&dev).unwrap();
        fs::write(dev.join("vendor"), format!("{vendor}\n")).unwrap();
        fs::write(dev.join("device"), format!("{device}\n")).unwrap();
        let drv = sys.join("drivers").join(driver);
        fs::create_dir_all(&drv).unwrap();
        symlink(&drv, dev.join("driver")).unwrap();
        let card_dir = sys.join("class/drm").join(card);
        fs::create_dir_all(&card_dir).unwrap();
        symlink(&dev, card_dir.join("device")).unwrap();
    }

    #[test]
    fn discovers_gpus_but_not_connectors_or_render_nodes() {
        let root = tempfile::tempdir().unwrap();
        let sys = root.path();
        add_gpu(sys, "card1", "0000:0c:00.0", "amdgpu", "0x1002", "0x73ff");
        add_gpu(sys, "card0", "0000:00:02.0", "i915", "0x8086", "0x46a6");
        fs::create_dir_all(sys.join("class/drm/card1-DP-1")).unwrap();
        fs::create_dir_all(sys.join("class/drm/renderD128")).unwrap();

        let cards = discover(sys);
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[0].slot, "0000:00:02.0");
        assert_eq!(cards[0].driver, "i915");
        assert_eq!((cards[1].vendor, cards[1].device), (0x1002, 0x73ff));
    }

    #[test]
    fn reads_labelled_temperatures_and_power() {
        let root = tempfile::tempdir().unwrap();
        let h = root.path();
        for (n, label, value) in [
            (1, "edge", "38000"),
            (2, "junction", "41000"),
            (3, "mem", "44000"),
        ] {
            fs::write(h.join(format!("temp{n}_input")), value).unwrap();
            fs::write(h.join(format!("temp{n}_label")), label).unwrap();
        }
        fs::write(h.join("power1_average"), "15000000").unwrap();
        fs::write(h.join("power1_cap"), "120000000").unwrap();
        assert_eq!(
            hwmon_temps(h),
            [
                ("Edge".to_owned(), 38.0),
                ("Hotspot".to_owned(), 41.0),
                ("Memory".to_owned(), 44.0)
            ]
        );
        assert_eq!(hwmon_power(h), (Some(15.0), Some(120.0)));
    }

    #[test]
    fn unlabelled_sensor_is_called_gpu() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("temp1_input"), "50000").unwrap();
        assert_eq!(hwmon_temps(root.path()), [("GPU".to_owned(), 50.0)]);
        assert_eq!(hwmon_power(root.path()), (None, None));
    }
}
