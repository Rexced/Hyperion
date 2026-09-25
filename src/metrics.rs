#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    Cpu,
    Memory,
    Storage,
    Network,
}

impl Category {
    pub const ALL: [Category; 4] = [
        Category::Cpu,
        Category::Memory,
        Category::Storage,
        Category::Network,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::Cpu => "CPU",
            Category::Memory => "Memory",
            Category::Storage => "Storage",
            Category::Network => "Network",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MetricId {
    CpuTotal,
    CpuPerCore,
    RamUsage,
    SwapUsage,
    DiskSpace,
    DiskIo,
    NetThroughput,
}

impl MetricId {
    pub const ALL: [MetricId; 7] = [
        MetricId::CpuTotal,
        MetricId::CpuPerCore,
        MetricId::RamUsage,
        MetricId::SwapUsage,
        MetricId::DiskSpace,
        MetricId::DiskIo,
        MetricId::NetThroughput,
    ];

    /// Stable identifier used in the config file; never rename.
    pub fn key(self) -> &'static str {
        match self {
            MetricId::CpuTotal => "cpu.total",
            MetricId::CpuPerCore => "cpu.per_core",
            MetricId::RamUsage => "mem.ram",
            MetricId::SwapUsage => "mem.swap",
            MetricId::DiskSpace => "storage.space",
            MetricId::DiskIo => "storage.io",
            MetricId::NetThroughput => "net.throughput",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            MetricId::CpuTotal => "Total usage",
            MetricId::CpuPerCore => "Per-core usage",
            MetricId::RamUsage => "RAM usage",
            MetricId::SwapUsage => "Swap usage",
            MetricId::DiskSpace => "Disk space",
            MetricId::DiskIo => "Disk read/write speed",
            MetricId::NetThroughput => "Upload/download speed",
        }
    }

    pub fn category(self) -> Category {
        match self {
            MetricId::CpuTotal | MetricId::CpuPerCore => Category::Cpu,
            MetricId::RamUsage | MetricId::SwapUsage => Category::Memory,
            MetricId::DiskSpace | MetricId::DiskIo => Category::Storage,
            MetricId::NetThroughput => Category::Network,
        }
    }

    pub fn default_enabled(self) -> bool {
        true
    }

    /// Prefix of the history series this metric produces, if any.
    pub fn series_prefix(self) -> Option<&'static str> {
        match self {
            MetricId::CpuTotal => Some("cpu"),
            MetricId::RamUsage => Some("ram"),
            MetricId::SwapUsage => Some("swap"),
            MetricId::DiskIo => Some("disk:"),
            MetricId::NetThroughput => Some("net:"),
            MetricId::CpuPerCore | MetricId::DiskSpace => None,
        }
    }

    fn bit(self) -> u64 {
        1 << (self as u32)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EnabledSet(u64);

impl EnabledSet {
    pub fn contains(self, id: MetricId) -> bool {
        self.0 & id.bit() != 0
    }

    pub fn set(&mut self, id: MetricId, on: bool) {
        if on {
            self.0 |= id.bit();
        } else {
            self.0 &= !id.bit();
        }
    }

    pub fn any(self, ids: &[MetricId]) -> bool {
        ids.iter().any(|&id| self.contains(id))
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CpuSample {
    /// Percent, 0..=100.
    pub total: f32,
    pub per_core: Vec<f32>,
}

/// All values in bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemSample {
    pub total: u64,
    pub used: u64,
    pub swap_total: u64,
    pub swap_used: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DiskIo {
    pub name: String,
    pub read_bps: f64,
    pub write_bps: f64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FsUsage {
    pub device: String,
    pub mount: String,
    pub fstype: String,
    pub total: u64,
    pub used: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NetIo {
    pub name: String,
    pub rx_bps: f64,
    pub tx_bps: f64,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    /// Seconds since the sampler started.
    pub t: f64,
    pub cpu: Option<CpuSample>,
    pub mem: Option<MemSample>,
    pub disks: Vec<DiskIo>,
    /// `None` when not refreshed this tick (it runs on a slower cadence).
    pub filesystems: Option<Vec<FsUsage>>,
    pub net: Vec<NetIo>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metric_keys_are_unique() {
        let mut keys: Vec<_> = MetricId::ALL.iter().map(|m| m.key()).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), MetricId::ALL.len());
    }

    #[test]
    fn enabled_set_roundtrip() {
        let mut set = EnabledSet::default();
        set.set(MetricId::DiskIo, true);
        assert!(set.contains(MetricId::DiskIo));
        assert!(!set.contains(MetricId::CpuTotal));
        set.set(MetricId::DiskIo, false);
        assert_eq!(set, EnabledSet::default());
    }
}
