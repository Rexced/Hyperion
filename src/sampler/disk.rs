//! Mounted physical drives: read/write speed, IOPS, temperature and partition space.

use std::collections::HashMap;
use std::ffi::CString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use super::procfile::{ProcFile, rate};
use crate::metrics::{DriveSample, FsUsage};

/// `/proc/diskstats` always counts in 512-byte sectors, regardless of the device's sector size.
const SECTOR_BYTES: u64 = 512;
/// UDisks refreshes SMART data about every 10 minutes, so asking often is pointless.
const UDISKS_POLL: Duration = Duration::from_secs(60);

#[derive(Debug, PartialEq, Eq)]
pub struct DiskCounters<'a> {
    pub name: &'a str,
    pub reads: u64,
    pub sectors_read: u64,
    pub writes: u64,
    pub sectors_written: u64,
}

pub fn parse_diskstats(text: &str) -> Vec<DiskCounters<'_>> {
    text.lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split_ascii_whitespace().collect();
            if f.len() < 10 {
                return None;
            }
            Some(DiskCounters {
                name: f[2],
                reads: f[3].parse().ok()?,
                sectors_read: f[5].parse().ok()?,
                writes: f[7].parse().ok()?,
                sectors_written: f[9].parse().ok()?,
            })
        })
        .collect()
}

/// The physical disks behind a block device: itself if it is one, a partition's
/// disk, or every disk under a device-mapper (LUKS/LVM) or RAID device.
pub fn physical_disks(sys: &Path, dev: &str) -> Vec<String> {
    fn walk(sys: &Path, dev: &str, depth: u8, out: &mut Vec<String>) {
        if depth > 8 {
            return;
        }
        let block = sys.join("block").join(dev);
        if block.join("device").exists() {
            out.push(dev.to_owned());
            return;
        }
        let class = sys.join("class/block").join(dev);
        if class.join("partition").exists() {
            // A partition lives inside its disk's directory.
            if let Some(disk) = std::fs::canonicalize(&class)
                .ok()
                .and_then(|p| Some(p.parent()?.file_name()?.to_string_lossy().into_owned()))
            {
                walk(sys, &disk, depth + 1, out);
            }
            return;
        }
        if let Ok(slaves) = std::fs::read_dir(block.join("slaves")) {
            for slave in slaves.flatten() {
                walk(sys, &slave.file_name().to_string_lossy(), depth + 1, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(sys, dev, 0, &mut out);
    out.sort();
    out.dedup();
    out
}

/// `E:KEY=VALUE` lines of a udev database entry.
pub fn udev_props(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|l| l.strip_prefix("E:")?.split_once('='))
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriveInfo {
    pub name: String,
    /// udev's `ID_SERIAL` (model + serial): the same drive keeps it across reboots.
    pub id: String,
    pub serial: Option<String>,
    pub model: String,
}

pub fn drive_info(sys: &Path, udev: &Path, name: &str) -> DriveInfo {
    let props = std::fs::read_to_string(sys.join("block").join(name).join("dev"))
        .ok()
        .and_then(|dev| std::fs::read_to_string(udev.join(format!("b{}", dev.trim()))).ok())
        .map(|text| udev_props(&text))
        .unwrap_or_default();
    let model = props
        .get("ID_MODEL")
        .map(|m| m.replace('_', " ").trim().to_owned())
        .filter(|m| !m.is_empty())
        .or_else(|| {
            std::fs::read_to_string(sys.join("block").join(name).join("device/model"))
                .ok()
                .map(|m| m.trim().to_owned())
                .filter(|m| !m.is_empty())
        })
        .unwrap_or_else(|| name.to_owned());
    DriveInfo {
        name: name.to_owned(),
        id: props
            .get("ID_SERIAL")
            .cloned()
            .unwrap_or_else(|| name.to_owned()),
        serial: props.get("ID_SERIAL_SHORT").cloned(),
        model,
    }
}

/// The drive's own temperature sensor: NVMe's hwmon, or SATA's with the `drivetemp`
/// kernel module loaded.
pub fn temp_input(sys: &Path, name: &str) -> Option<PathBuf> {
    let device = sys.join("block").join(name).join("device");
    let hwmons = |dir: PathBuf| -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.path())
                    .filter(|p| {
                        p.file_name()
                            .is_some_and(|n| n.to_string_lossy().starts_with("hwmon"))
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut candidates = hwmons(device.clone());
    candidates.extend(hwmons(device.join("hwmon")));
    candidates.sort();
    candidates
        .into_iter()
        .map(|dir| dir.join("temp1_input"))
        .find(|p| p.exists())
}

/// Serial -> °C for drives UDisks has SMART temperatures for, from `busctl -j` output
/// of `GetManagedObjects`.
pub fn parse_udisks(json: &str) -> HashMap<String, f32> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return HashMap::new();
    };
    let Some(objects) = value["data"][0].as_object() else {
        return HashMap::new();
    };
    objects
        .values()
        .filter_map(|ifaces| {
            let serial = ifaces["org.freedesktop.UDisks2.Drive"]["Serial"]["data"].as_str()?;
            let kelvin =
                ifaces["org.freedesktop.UDisks2.Drive.Ata"]["SmartTemperature"]["data"].as_f64()?;
            // 0 means "unknown".
            (kelvin > 0.0 && !serial.is_empty())
                .then(|| (serial.to_owned(), (kelvin - 273.15) as f32))
        })
        .collect()
}

fn udisks_temps() -> HashMap<String, f32> {
    Command::new("busctl")
        .args([
            "--system",
            "-j",
            "call",
            "org.freedesktop.UDisks2",
            "/org/freedesktop/UDisks2",
            "org.freedesktop.DBus.ObjectManager",
            "GetManagedObjects",
        ])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| parse_udisks(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default()
}

/// Kernel device name a mount's source refers to (follows /dev/mapper symlinks).
fn dev_name(source: &str) -> Option<String> {
    let resolved = std::fs::canonicalize(source).unwrap_or_else(|_| PathBuf::from(source));
    Some(resolved.file_name()?.to_string_lossy().into_owned())
}

struct Drive {
    info: DriveInfo,
    boot: bool,
    temp: Option<ProcFile>,
    partitions: Vec<FsUsage>,
}

pub struct DiskCollector {
    diskstats: ProcFile,
    drives: Vec<Drive>,
    prev: HashMap<String, [u64; 4]>,
    last: Instant,
    udisks: HashMap<String, f32>,
    udisks_at: Option<Instant>,
}

impl DiskCollector {
    pub fn new() -> io::Result<Self> {
        let mut c = Self {
            diskstats: ProcFile::open("/proc/diskstats")?,
            drives: Vec::new(),
            prev: HashMap::new(),
            last: Instant::now(),
            udisks: HashMap::new(),
            udisks_at: None,
        };
        c.refresh(false);
        c.sample(false)?;
        Ok(c)
    }

    /// Re-finds the mounted drives and their partitions' free space. Slow-ish, so
    /// it runs every few seconds rather than every sample.
    pub fn refresh(&mut self, want_temp: bool) {
        let sys = Path::new("/sys");
        let udev = Path::new("/run/udev/data");
        let Ok(text) = std::fs::read_to_string("/proc/self/mounts") else {
            return;
        };
        let mut by_disk: Vec<(String, Vec<MountEntry>)> = Vec::new();
        for mount in parse_mounts(&text) {
            let Some(dev) = dev_name(&mount.device) else {
                continue;
            };
            for disk in physical_disks(sys, &dev) {
                match by_disk.iter_mut().find(|(d, _)| *d == disk) {
                    Some((_, mounts)) => mounts.push(mount.clone()),
                    None => by_disk.push((disk, vec![mount.clone()])),
                }
            }
        }

        let mut old: Vec<Drive> = std::mem::take(&mut self.drives);
        for (name, mounts) in by_disk {
            let existing = old
                .iter()
                .position(|d| d.info.name == name)
                .map(|i| old.remove(i));
            let (info, temp) = match existing {
                Some(d) => (d.info, d.temp),
                None => (
                    drive_info(sys, udev, &name),
                    temp_input(sys, &name).and_then(|p| ProcFile::open(p).ok()),
                ),
            };
            let partitions = mounts
                .iter()
                .filter_map(|m| {
                    let (total, used) = statvfs(&m.mount)?;
                    (total > 0).then(|| FsUsage {
                        device: m.device.clone(),
                        mount: m.mount.clone(),
                        fstype: m.fstype.clone(),
                        total,
                        used,
                    })
                })
                .collect();
            self.drives.push(Drive {
                info,
                boot: mounts.iter().any(|m| m.mount == "/"),
                temp,
                partitions,
            });
        }
        self.drives
            .sort_by(|a, b| b.boot.cmp(&a.boot).then(a.info.name.cmp(&b.info.name)));

        let needs_udisks = self.drives.iter().any(|d| d.temp.is_none());
        if want_temp && needs_udisks && self.udisks_at.is_none_or(|t| t.elapsed() >= UDISKS_POLL) {
            self.udisks_at = Some(Instant::now());
            self.udisks = udisks_temps();
        }
    }

    pub fn sample(&mut self, want_temp: bool) -> io::Result<Vec<DriveSample>> {
        let now = Instant::now();
        let secs = now.duration_since(self.last).as_secs_f64();
        self.last = now;
        let text = self.diskstats.read()?.to_owned();
        let counters = parse_diskstats(&text);

        let mut out = Vec::with_capacity(self.drives.len());
        let mut seen = HashMap::with_capacity(self.drives.len());
        for drive in &mut self.drives {
            let Some(c) = counters.iter().find(|c| c.name == drive.info.name) else {
                continue;
            };
            let cur = [c.reads, c.sectors_read, c.writes, c.sectors_written];
            let prev = self.prev.get(&drive.info.name).copied().unwrap_or(cur);
            seen.insert(drive.info.name.clone(), cur);
            let temp_c = if want_temp {
                match drive.temp.as_mut() {
                    Some(f) => f
                        .read()
                        .ok()
                        .and_then(|t| t.trim().parse::<f32>().ok())
                        .map(|milli| milli / 1000.0),
                    None => drive
                        .info
                        .serial
                        .as_ref()
                        .and_then(|s| self.udisks.get(s).copied()),
                }
            } else {
                None
            };
            out.push(DriveSample {
                id: drive.info.id.clone(),
                name: drive.info.name.clone(),
                model: drive.info.model.clone(),
                boot: drive.boot,
                read_bps: rate(prev[1], cur[1], secs) * SECTOR_BYTES as f64,
                write_bps: rate(prev[3], cur[3], secs) * SECTOR_BYTES as f64,
                iops: rate(prev[0], cur[0], secs) + rate(prev[2], cur[2], secs),
                temp_c,
                partitions: drive.partitions.clone(),
            });
        }
        self.prev = seen;
        Ok(out)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MountEntry {
    pub device: String,
    pub mount: String,
    pub fstype: String,
}

/// Block-device-backed mounts from `/proc/self/mounts`, one per device
/// (btrfs subvolumes and bind mounts collapse to the shortest mount path).
pub fn parse_mounts(text: &str) -> Vec<MountEntry> {
    let mut by_device: HashMap<String, MountEntry> = HashMap::new();
    for line in text.lines() {
        let mut f = line.split_ascii_whitespace();
        let (Some(device), Some(mount), Some(fstype)) = (f.next(), f.next(), f.next()) else {
            continue;
        };
        if !device.starts_with("/dev/") || fstype == "squashfs" {
            continue;
        }
        let entry = MountEntry {
            device: unescape_octal(device),
            mount: unescape_octal(mount),
            fstype: fstype.to_owned(),
        };
        match by_device.get(&entry.device) {
            Some(existing) if existing.mount.len() <= entry.mount.len() => {}
            _ => {
                by_device.insert(entry.device.clone(), entry);
            }
        }
    }
    let mut mounts: Vec<_> = by_device.into_values().collect();
    mounts.sort_by(|a, b| a.mount.cmp(&b.mount));
    mounts
}

/// The kernel escapes space, tab, newline and backslash in mount paths as `\ooo`.
fn unescape_octal(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && i + 3 < bytes.len()
            && bytes[i + 1..i + 4]
                .iter()
                .all(|b| (b'0'..=b'7').contains(b))
        {
            let v = (bytes[i + 1] - b'0') * 64 + (bytes[i + 2] - b'0') * 8 + (bytes[i + 3] - b'0');
            out.push(v);
            i += 4;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Returns `(total, used)` bytes.
pub fn statvfs(path: &str) -> Option<(u64, u64)> {
    let c_path = CString::new(path).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: c_path is a valid NUL-terminated string and st is a properly sized out-parameter.
    if unsafe { libc::statvfs(c_path.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let frsize = st.f_frsize as u64;
    let total = st.f_blocks as u64 * frsize;
    let used = (st.f_blocks as u64).saturating_sub(st.f_bfree as u64) * frsize;
    Some((total, used))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn parses_diskstats_fields() {
        let text = "\
 259       0 nvme0n1 54842 9087 4064596 31473 297763 3255 10879235 154311 0 17636 187856 5104 0 7866592 1282 5043 789
   8       0 sda 266 0 19280 138 0 0 0 0 0 44 138 0 0 0 0 0 0
";
        let d = parse_diskstats(text);
        assert_eq!(
            d[0],
            DiskCounters {
                name: "nvme0n1",
                reads: 54_842,
                sectors_read: 4_064_596,
                writes: 297_763,
                sectors_written: 10_879_235,
            }
        );
        assert_eq!(d[1].name, "sda");
    }

    /// A fake /sys: a whole disk, one of its partitions, and a LUKS mapping on it.
    fn fake_sys() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        let sys = root.path();
        fs::create_dir_all(sys.join("block/nvme0n1/device")).unwrap();
        fs::write(sys.join("block/nvme0n1/dev"), "259:0\n").unwrap();
        fs::create_dir_all(sys.join("block/nvme0n1/nvme0n1p2")).unwrap();
        fs::write(sys.join("block/nvme0n1/nvme0n1p2/partition"), "2").unwrap();
        fs::create_dir_all(sys.join("class/block")).unwrap();
        std::os::unix::fs::symlink(
            sys.join("block/nvme0n1/nvme0n1p2"),
            sys.join("class/block/nvme0n1p2"),
        )
        .unwrap();
        fs::create_dir_all(sys.join("block/dm-0/slaves/nvme0n1p2")).unwrap();
        fs::create_dir_all(sys.join("block/zram0")).unwrap();
        root
    }

    #[test]
    fn follows_partitions_and_mappings_back_to_the_disk() {
        let root = fake_sys();
        let sys = root.path();
        assert_eq!(physical_disks(sys, "nvme0n1"), ["nvme0n1"]);
        assert_eq!(physical_disks(sys, "nvme0n1p2"), ["nvme0n1"]);
        assert_eq!(physical_disks(sys, "dm-0"), ["nvme0n1"]);
        assert!(physical_disks(sys, "zram0").is_empty());
    }

    #[test]
    fn model_and_stable_id_come_from_udev() {
        let root = fake_sys();
        let udev = root.path().join("udev");
        fs::create_dir_all(&udev).unwrap();
        fs::write(
            udev.join("b259:0"),
            "S:disk/by-id/x\nE:ID_MODEL=Samsung_SSD_860_EVO_500GB\nE:ID_SERIAL=Samsung_SSD_860_EVO_500GB_S59U\nE:ID_SERIAL_SHORT=S59U\n",
        )
        .unwrap();
        let info = drive_info(root.path(), &udev, "nvme0n1");
        assert_eq!(info.model, "Samsung SSD 860 EVO 500GB");
        assert_eq!(info.id, "Samsung_SSD_860_EVO_500GB_S59U");
        assert_eq!(info.serial.as_deref(), Some("S59U"));
    }

    #[test]
    fn falls_back_to_kernel_name_without_udev() {
        let root = fake_sys();
        let info = drive_info(root.path(), &root.path().join("none"), "nvme0n1");
        assert_eq!(info.model, "nvme0n1");
        assert_eq!(info.id, "nvme0n1");
    }

    #[test]
    fn finds_nvme_and_drivetemp_sensors() {
        let root = fake_sys();
        let sys = root.path();
        fs::create_dir_all(sys.join("block/nvme0n1/device/hwmon1")).unwrap();
        fs::write(sys.join("block/nvme0n1/device/hwmon1/temp1_input"), "39850").unwrap();
        assert!(
            temp_input(sys, "nvme0n1")
                .unwrap()
                .ends_with("hwmon1/temp1_input")
        );
        // drivetemp nests its hwmon one level deeper.
        fs::create_dir_all(sys.join("block/sda/device/hwmon/hwmon5")).unwrap();
        fs::write(
            sys.join("block/sda/device/hwmon/hwmon5/temp1_input"),
            "30000",
        )
        .unwrap();
        assert!(
            temp_input(sys, "sda")
                .unwrap()
                .ends_with("hwmon5/temp1_input")
        );
        assert_eq!(temp_input(sys, "zram0"), None);
    }

    #[test]
    fn reads_smart_temperatures_from_udisks_json() {
        let json = r#"{"type":"a{oa{sa{sv}}}","data":[{
            "/org/freedesktop/UDisks2/drives/Samsung":{
                "org.freedesktop.UDisks2.Drive":{"Serial":{"type":"s","data":"S59U"}},
                "org.freedesktop.UDisks2.Drive.Ata":{"SmartTemperature":{"type":"d","data":300.15}}},
            "/org/freedesktop/UDisks2/drives/Usb":{
                "org.freedesktop.UDisks2.Drive":{"Serial":{"type":"s","data":"301"}}},
            "/org/freedesktop/UDisks2/drives/Unknown":{
                "org.freedesktop.UDisks2.Drive":{"Serial":{"type":"s","data":"X"}},
                "org.freedesktop.UDisks2.Drive.Ata":{"SmartTemperature":{"type":"d","data":0.0}}}}]}"#;
        let temps = parse_udisks(json);
        assert_eq!(temps.len(), 1);
        assert!((temps["S59U"] - 27.0).abs() < 0.01);
        assert!(parse_udisks("not json").is_empty());
    }

    #[test]
    fn collapses_btrfs_subvolumes_and_skips_virtual() {
        let text = "\
proc /proc proc rw 0 0
/dev/nvme0n1p2 /home btrfs rw,subvol=/@home 0 0
/dev/nvme0n1p2 / btrfs rw,subvol=/@ 0 0
/dev/nvme0n1p1 /boot vfat rw 0 0
/dev/loop0 /snap/core squashfs ro 0 0
/dev/sdb1 /run/media/me/My\\040Drive exfat rw 0 0
";
        let m = parse_mounts(text);
        let mounts: Vec<_> = m.iter().map(|e| e.mount.as_str()).collect();
        assert_eq!(mounts, ["/", "/boot", "/run/media/me/My Drive"]);
        assert_eq!(m[0].device, "/dev/nvme0n1p2");
    }

    #[test]
    fn unescapes_octal_sequences() {
        assert_eq!(unescape_octal(r"a\040b\134c"), r"a b\c");
        assert_eq!(unescape_octal(r"trailing\04"), r"trailing\04");
    }

    #[test]
    fn statvfs_root_is_nonzero() {
        let (total, used) = statvfs("/").unwrap();
        assert!(total > 0 && used <= total);
    }
}
