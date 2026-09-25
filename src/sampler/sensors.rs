//! CPU temperature (hwmon) and CPU package power (RAPL energy counter), Linux sysfs.

use std::path::{Path, PathBuf};
use std::time::Instant;

use super::procfile::ProcFile;

/// Chips and labels that report the CPU temperature, best first. `None` = any label.
const TEMP_SOURCES: [(&str, Option<&str>); 6] = [
    ("zenpower", Some("Tdie")),
    ("k10temp", Some("Tdie")),
    ("k10temp", Some("Tctl")),
    ("coretemp", Some("Package id 0")),
    ("cpu_thermal", None),
    ("k10temp", None),
];

/// Whether this machine can report CPU power, and whether we may read it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerAccess {
    /// No RAPL energy counter (e.g. a VM, or not a Linux system).
    Unavailable,
    /// The counter exists but is root-only (the Linux default since 2020).
    NeedsPermission,
    Readable,
}

/// The hwmon `tempN_input` file for the CPU, under `sys` (normally `/sys`).
pub fn cpu_temp_input(sys: &Path) -> Option<PathBuf> {
    let chips: Vec<(String, PathBuf)> = std::fs::read_dir(sys.join("class/hwmon"))
        .ok()?
        .filter_map(|e| {
            let dir = e.ok()?.path();
            let name = std::fs::read_to_string(dir.join("name")).ok()?;
            Some((name.trim().to_owned(), dir))
        })
        .collect();
    for (chip, label) in TEMP_SOURCES {
        for (_, dir) in chips.iter().filter(|(n, _)| n == chip) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
            let mut inputs: Vec<PathBuf> = entries
                .filter_map(|e| Some(e.ok()?.path()))
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("temp") && n.ends_with("_input"))
                })
                .collect();
            inputs.sort();
            for input in inputs {
                let Some(file) = input.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                let label_path = input.with_file_name(file.replace("_input", "_label"));
                let found = std::fs::read_to_string(&label_path).ok();
                match label {
                    Some(want) if found.as_deref().map(str::trim) == Some(want) => {
                        return Some(input);
                    }
                    None => return Some(input),
                    _ => {}
                }
            }
        }
    }
    None
}

/// The RAPL package domain (`.../intel-rapl:0`, also used on AMD), under `sys`.
pub fn rapl_package_dir(sys: &Path) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(sys.join("class/powercap"))
        .ok()?
        .filter_map(|e| Some(e.ok()?.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("intel-rapl:") && n.matches(':').count() == 1)
        })
        .collect();
    dirs.sort();
    dirs.into_iter().find(|d| {
        std::fs::read_to_string(d.join("name")).is_ok_and(|n| n.trim().starts_with("package"))
    })
}

pub fn power_access() -> PowerAccess {
    power_access_in(Path::new("/sys"))
}

fn power_access_in(sys: &Path) -> PowerAccess {
    match rapl_package_dir(sys) {
        None => PowerAccess::Unavailable,
        Some(dir) if std::fs::File::open(dir.join("energy_uj")).is_ok() => PowerAccess::Readable,
        Some(_) => PowerAccess::NeedsPermission,
    }
}

/// Average power between two energy readings. The counter wraps at `max_uj`.
pub fn watts(prev_uj: u64, cur_uj: u64, max_uj: u64, secs: f64) -> Option<f32> {
    if secs <= 0.0 {
        return None;
    }
    let used = if cur_uj >= prev_uj {
        cur_uj - prev_uj
    } else {
        max_uj.checked_sub(prev_uj)? + cur_uj
    };
    Some((used as f64 / 1e6 / secs) as f32)
}

struct Rapl {
    energy: ProcFile,
    max_uj: u64,
    prev: Option<(u64, Instant)>,
}

pub struct CpuSensors {
    temp: Option<ProcFile>,
    rapl_dir: Option<PathBuf>,
    rapl: Option<Rapl>,
}

impl CpuSensors {
    pub fn new() -> Self {
        let sys = Path::new("/sys");
        let mut sensors = Self {
            temp: cpu_temp_input(sys).and_then(|p| ProcFile::open(p).ok()),
            rapl_dir: rapl_package_dir(sys),
            rapl: None,
        };
        sensors.retry_power();
        sensors
    }

    /// Tries to open the energy counter again; permission may have been granted
    /// while we were running.
    pub fn retry_power(&mut self) {
        if self.rapl.is_some() {
            return;
        }
        let Some(dir) = &self.rapl_dir else { return };
        let Ok(energy) = ProcFile::open(dir.join("energy_uj")) else {
            return;
        };
        let max_uj = std::fs::read_to_string(dir.join("max_energy_range_uj"))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(u64::MAX);
        self.rapl = Some(Rapl {
            energy,
            max_uj,
            prev: None,
        });
    }

    pub fn temp_c(&mut self) -> Option<f32> {
        let milli: f32 = self.temp.as_mut()?.read().ok()?.trim().parse().ok()?;
        Some(milli / 1000.0)
    }

    /// Watts since the previous call; `None` on the first call or without access.
    pub fn power_w(&mut self) -> Option<f32> {
        let rapl = self.rapl.as_mut()?;
        let now = Instant::now();
        let cur: u64 = rapl.energy.read().ok()?.trim().parse().ok()?;
        let prev = rapl.prev.replace((cur, now));
        let (prev_uj, prev_t) = prev?;
        watts(
            prev_uj,
            cur,
            rapl.max_uj,
            now.duration_since(prev_t).as_secs_f64(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn chip(root: &Path, hwmon: &str, name: &str, temps: &[(&str, &str)]) {
        let dir = root.join("class/hwmon").join(hwmon);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("name"), format!("{name}\n")).unwrap();
        for (i, (label, value)) in temps.iter().enumerate() {
            fs::write(dir.join(format!("temp{}_input", i + 1)), value).unwrap();
            fs::write(dir.join(format!("temp{}_label", i + 1)), label).unwrap();
        }
    }

    #[test]
    fn prefers_tdie_then_tctl_and_ignores_other_chips() {
        let root = tempfile::tempdir().unwrap();
        chip(root.path(), "hwmon0", "nvme", &[("Composite", "40000")]);
        chip(
            root.path(),
            "hwmon1",
            "k10temp",
            &[("Tctl", "55000"), ("Tccd1", "50000")],
        );
        let found = cpu_temp_input(root.path()).unwrap();
        assert!(found.ends_with("hwmon1/temp1_input"));

        chip(root.path(), "hwmon2", "zenpower", &[("Tdie", "52000")]);
        assert!(
            cpu_temp_input(root.path())
                .unwrap()
                .ends_with("hwmon2/temp1_input")
        );
    }

    #[test]
    fn intel_package_temperature() {
        let root = tempfile::tempdir().unwrap();
        chip(
            root.path(),
            "hwmon3",
            "coretemp",
            &[("Core 0", "45000"), ("Package id 0", "48000")],
        );
        assert!(
            cpu_temp_input(root.path())
                .unwrap()
                .ends_with("temp2_input")
        );
    }

    #[test]
    fn no_cpu_sensor_is_none() {
        let root = tempfile::tempdir().unwrap();
        chip(root.path(), "hwmon0", "amdgpu", &[("edge", "40000")]);
        assert_eq!(cpu_temp_input(root.path()), None);
    }

    #[test]
    fn finds_the_package_domain_not_subdomains() {
        let root = tempfile::tempdir().unwrap();
        for (d, name) in [("intel-rapl:0:0", "core"), ("intel-rapl:0", "package-0")] {
            let dir = root.path().join("class/powercap").join(d);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("name"), name).unwrap();
        }
        let dir = rapl_package_dir(root.path()).unwrap();
        assert!(dir.ends_with("intel-rapl:0"));
        assert_eq!(power_access_in(root.path()), PowerAccess::NeedsPermission);
        fs::write(dir.join("energy_uj"), "123").unwrap();
        assert_eq!(power_access_in(root.path()), PowerAccess::Readable);
    }

    #[test]
    fn no_rapl_is_unavailable() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(power_access_in(root.path()), PowerAccess::Unavailable);
    }

    #[test]
    fn watts_from_energy_including_wraparound() {
        // 42 J over 1 s.
        assert_eq!(watts(1_000_000, 43_000_000, u64::MAX, 1.0), Some(42.0));
        // Counter wrapped at 262_143_328_850 µJ.
        let max = 262_143_328_850;
        assert_eq!(watts(max - 10_000_000, 10_000_000, max, 0.5), Some(40.0));
        assert_eq!(watts(0, 1, max, 0.0), None);
    }
}
