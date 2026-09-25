#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    Cpu,
    Gpu,
    Memory,
    Storage,
    Network,
}

impl Category {
    pub const ALL: [Category; 5] = [
        Category::Cpu,
        Category::Gpu,
        Category::Memory,
        Category::Storage,
        Category::Network,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::Cpu => "CPU",
            Category::Gpu => "GPU",
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
    CpuTemp,
    CpuPower,
    GpuUsage,
    GpuPower,
    GpuMemory,
    GpuTemp,
    RamUsage,
    SwapUsage,
    DiskSpace,
    DiskIo,
    DiskTemp,
    DiskIops,
    NetThroughput,
}

impl MetricId {
    pub const ALL: [MetricId; 15] = [
        MetricId::CpuTotal,
        MetricId::CpuPerCore,
        MetricId::CpuTemp,
        MetricId::CpuPower,
        MetricId::GpuUsage,
        MetricId::GpuPower,
        MetricId::GpuMemory,
        MetricId::GpuTemp,
        MetricId::RamUsage,
        MetricId::SwapUsage,
        MetricId::DiskIo,
        MetricId::DiskIops,
        MetricId::DiskTemp,
        MetricId::DiskSpace,
        MetricId::NetThroughput,
    ];

    /// Stable identifier used in the config file; never rename.
    pub fn key(self) -> &'static str {
        match self {
            MetricId::CpuTotal => "cpu.total",
            MetricId::CpuPerCore => "cpu.per_core",
            MetricId::CpuTemp => "cpu.temp",
            MetricId::CpuPower => "cpu.power",
            MetricId::GpuUsage => "gpu.usage",
            MetricId::GpuPower => "gpu.power",
            MetricId::GpuMemory => "gpu.memory",
            MetricId::GpuTemp => "gpu.temp",
            MetricId::RamUsage => "mem.ram",
            MetricId::SwapUsage => "mem.swap",
            MetricId::DiskSpace => "storage.space",
            MetricId::DiskIo => "storage.io",
            MetricId::DiskTemp => "storage.temp",
            MetricId::DiskIops => "storage.iops",
            MetricId::NetThroughput => "net.throughput",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            MetricId::CpuTotal => "Total usage",
            MetricId::CpuPerCore => "Per-core usage",
            MetricId::CpuTemp => "Temperature",
            MetricId::CpuPower => "Power draw",
            MetricId::GpuUsage => "Usage and core clock",
            MetricId::GpuPower => "Power draw",
            MetricId::GpuMemory => "Memory (VRAM and spill)",
            MetricId::GpuTemp => "Temperatures",
            MetricId::RamUsage => "RAM usage",
            MetricId::SwapUsage => "Swap usage",
            MetricId::DiskSpace => "Partition space",
            MetricId::DiskIo => "Read/write speed",
            MetricId::DiskTemp => "Temperature",
            MetricId::DiskIops => "IOPS",
            MetricId::NetThroughput => "Upload/download speed",
        }
    }

    pub fn category(self) -> Category {
        match self {
            MetricId::CpuTotal | MetricId::CpuPerCore | MetricId::CpuTemp | MetricId::CpuPower => {
                Category::Cpu
            }
            MetricId::GpuUsage | MetricId::GpuPower | MetricId::GpuMemory | MetricId::GpuTemp => {
                Category::Gpu
            }
            MetricId::RamUsage | MetricId::SwapUsage => Category::Memory,
            MetricId::DiskSpace | MetricId::DiskIo | MetricId::DiskTemp | MetricId::DiskIops => {
                Category::Storage
            }
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
            MetricId::GpuUsage => Some("gpu-usage:"),
            MetricId::GpuPower => Some("gpu-power:"),
            MetricId::GpuMemory => Some("gpu-mem:"),
            MetricId::GpuTemp => Some("gpu-temp:"),
            MetricId::CpuPerCore
            | MetricId::CpuTemp
            | MetricId::CpuPower
            | MetricId::DiskSpace
            | MetricId::DiskTemp
            | MetricId::DiskIops => None,
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
    pub temp_c: Option<f32>,
    /// Package power; `None` without permission to read the energy counter.
    pub power_w: Option<f32>,
}

/// All values in bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemSample {
    pub total: u64,
    pub used: u64,
    pub swap_total: u64,
    pub swap_used: u64,
}

/// One mounted physical drive.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DriveSample {
    /// Stable across reboots (udev serial), for remembering the user's choice.
    pub id: String,
    /// Kernel name, e.g. `nvme0n1`; can change between boots.
    pub name: String,
    pub model: String,
    /// Holds the root filesystem.
    pub boot: bool,
    pub read_bps: f64,
    pub write_bps: f64,
    /// Reads plus writes completed per second.
    pub iops: f64,
    pub temp_c: Option<f32>,
    pub partitions: Vec<FsUsage>,
}

/// One GPU. Everything is optional: vendors and drivers expose different subsets.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GpuSample {
    /// PCI slot, e.g. `0000:0c:00.0`; stable across reboots.
    pub id: String,
    pub name: String,
    /// Built into the CPU (shares system RAM) rather than a separate card.
    pub integrated: bool,
    pub usage: Option<f32>,
    pub core_mhz: Option<f32>,
    pub power_w: Option<f32>,
    pub power_cap_w: Option<f32>,
    pub vram_used: Option<u64>,
    pub vram_total: Option<u64>,
    /// GPU data held in system RAM: what spilled out of VRAM on a dedicated card,
    /// or everything on an integrated one.
    pub system_used: Option<u64>,
    /// `(label, °C)`, e.g. `("Edge", 45.0)`.
    pub temps: Vec<(String, f32)>,
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
    pub drives: Vec<DriveSample>,
    pub net: Vec<NetIo>,
    pub gpus: Vec<GpuSample>,
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
