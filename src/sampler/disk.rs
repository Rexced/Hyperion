use std::collections::{HashMap, HashSet};
use std::ffi::CString;
use std::io;
use std::path::Path;
use std::time::Instant;

use super::procfile::{ProcFile, rate};
use crate::metrics::{DiskIo, FsUsage};

/// `/proc/diskstats` always counts in 512-byte sectors, regardless of the device's sector size.
const SECTOR_BYTES: u64 = 512;

#[derive(Debug, PartialEq, Eq)]
pub struct DiskCounters<'a> {
    pub name: &'a str,
    pub sectors_read: u64,
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
                sectors_read: f[5].parse().ok()?,
                sectors_written: f[9].parse().ok()?,
            })
        })
        .collect()
}

/// Whole physical disks have a `device` link; partitions, loop, zram and dm devices don't.
pub fn is_physical_disk(sys_root: &Path, name: &str) -> bool {
    sys_root.join("block").join(name).join("device").exists()
}

pub struct DiskCollector {
    diskstats: ProcFile,
    physical: HashSet<String>,
    prev: HashMap<String, (u64, u64)>,
    last: Instant,
}

impl DiskCollector {
    pub fn new() -> io::Result<Self> {
        let mut c = Self {
            diskstats: ProcFile::open("/proc/diskstats")?,
            physical: HashSet::new(),
            prev: HashMap::new(),
            last: Instant::now(),
        };
        c.refresh_devices();
        c.sample()?;
        Ok(c)
    }

    pub fn refresh_devices(&mut self) {
        let sys = Path::new("/sys");
        let Ok(entries) = std::fs::read_dir(sys.join("block")) else {
            return;
        };
        self.physical = entries
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|name| is_physical_disk(sys, name))
            .collect();
    }

    pub fn sample(&mut self) -> io::Result<Vec<DiskIo>> {
        let now = Instant::now();
        let secs = now.duration_since(self.last).as_secs_f64();
        self.last = now;
        let text = self.diskstats.read()?;
        let mut out = Vec::new();
        let mut seen = HashMap::with_capacity(self.physical.len());
        for d in parse_diskstats(text) {
            if !self.physical.contains(d.name) {
                continue;
            }
            let cur = (d.sectors_read, d.sectors_written);
            if let Some(&(pr, pw)) = self.prev.get(d.name) {
                out.push(DiskIo {
                    name: d.name.to_owned(),
                    read_bps: rate(pr, cur.0, secs) * SECTOR_BYTES as f64,
                    write_bps: rate(pw, cur.1, secs) * SECTOR_BYTES as f64,
                });
            }
            seen.insert(d.name.to_owned(), cur);
        }
        self.prev = seen;
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }
}

#[derive(Debug, PartialEq, Eq)]
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

pub fn filesystems() -> Vec<FsUsage> {
    let Ok(text) = std::fs::read_to_string("/proc/self/mounts") else {
        return Vec::new();
    };
    parse_mounts(&text)
        .into_iter()
        .filter_map(|m| {
            let (total, used) = statvfs(&m.mount)?;
            (total > 0).then_some(FsUsage {
                device: m.device,
                mount: m.mount,
                fstype: m.fstype,
                total,
                used,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
                sectors_read: 4_064_596,
                sectors_written: 10_879_235
            }
        );
        assert_eq!(d[1].name, "sda");
    }

    #[test]
    fn physical_disk_detection_uses_device_link() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("block/nvme0n1/device")).unwrap();
        std::fs::create_dir_all(root.path().join("block/zram0")).unwrap();
        assert!(is_physical_disk(root.path(), "nvme0n1"));
        assert!(!is_physical_disk(root.path(), "zram0"));
        assert!(!is_physical_disk(root.path(), "missing"));
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
