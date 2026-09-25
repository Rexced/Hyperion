//! Friendly GPU names from the system's PCI ID database (`pci.ids`, from the hwdata
//! or pciutils packages on every major distro).

const PATHS: [&str; 3] = [
    "/usr/share/hwdata/pci.ids",
    "/usr/share/misc/pci.ids",
    "/usr/share/pci.ids",
];

pub fn load() -> Option<String> {
    PATHS.iter().find_map(|p| std::fs::read_to_string(p).ok())
}

/// The device's name, preferring the marketing name in brackets
/// ("Navi 23 [Radeon RX 6600/6600 XT/6600M]" -> "Radeon RX 6600/6600 XT/6600M").
pub fn lookup(db: &str, vendor: u16, device: u16) -> Option<String> {
    let vendor_prefix = format!("{vendor:04x}  ");
    let device_prefix = format!("\t{device:04x}  ");
    let mut in_vendor = false;
    for line in db.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if !line.starts_with('\t') {
            // A new top-level entry: a vendor (or the device-class section at the end).
            if in_vendor {
                return None;
            }
            in_vendor = line.starts_with(&vendor_prefix);
            continue;
        }
        if in_vendor && let Some(name) = line.strip_prefix(&device_prefix) {
            let name = name.trim();
            let marketing = name
                .rfind('[')
                .zip(name.rfind(']'))
                .filter(|(open, close)| open < close)
                .map(|(open, close)| &name[open + 1..close]);
            return Some(marketing.unwrap_or(name).to_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const DB: &str = "\
# comment
1002  Advanced Micro Devices, Inc. [AMD/ATI]
\t73ff  Navi 23 [Radeon RX 6600/6600 XT/6600M]
\t\t1043 05e3  Dual Radeon RX 6600
\t15bf  Phoenix1
10de  NVIDIA Corporation
\t2684  AD102 [GeForce RTX 4090]
8086  Intel Corporation
\t46a6  Alder Lake-P GT2 [Iris Xe Graphics]
C 03  Display controller
";

    #[test]
    fn prefers_the_bracketed_marketing_name() {
        assert_eq!(
            lookup(DB, 0x1002, 0x73ff).as_deref(),
            Some("Radeon RX 6600/6600 XT/6600M")
        );
        assert_eq!(
            lookup(DB, 0x8086, 0x46a6).as_deref(),
            Some("Iris Xe Graphics")
        );
    }

    #[test]
    fn falls_back_to_the_codename_and_misses_cleanly() {
        assert_eq!(lookup(DB, 0x1002, 0x15bf).as_deref(), Some("Phoenix1"));
        // Same device id under another vendor must not match.
        assert_eq!(lookup(DB, 0x10de, 0x73ff), None);
        assert_eq!(lookup(DB, 0xabcd, 0x0001), None);
    }
}
