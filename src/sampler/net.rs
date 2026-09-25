use std::collections::{HashMap, HashSet};
use std::io;
use std::path::Path;
use std::time::Instant;

use super::procfile::{ProcFile, rate};
use crate::metrics::NetIo;

#[derive(Debug, PartialEq, Eq)]
pub struct NetCounters<'a> {
    pub name: &'a str,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

pub fn parse_net_dev(text: &str) -> Vec<NetCounters<'_>> {
    text.lines()
        .skip(2)
        .filter_map(|line| {
            // Split on ':' first: old kernels print `eth0:123` with no space.
            let (name, rest) = line.split_once(':')?;
            let f: Vec<&str> = rest.split_ascii_whitespace().collect();
            if f.len() < 9 {
                return None;
            }
            Some(NetCounters {
                name: name.trim(),
                rx_bytes: f[0].parse().ok()?,
                tx_bytes: f[8].parse().ok()?,
            })
        })
        .collect()
}

/// Hardware NICs have a `device` link; lo, bridges, veth, docker and VPN tunnels don't.
pub fn is_physical_iface(sys_root: &Path, name: &str) -> bool {
    sys_root
        .join("class/net")
        .join(name)
        .join("device")
        .exists()
}

pub struct NetCollector {
    net_dev: ProcFile,
    physical: HashSet<String>,
    prev: HashMap<String, (u64, u64)>,
    last: Instant,
}

impl NetCollector {
    pub fn new() -> io::Result<Self> {
        let mut c = Self {
            net_dev: ProcFile::open("/proc/net/dev")?,
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
        let Ok(entries) = std::fs::read_dir(sys.join("class/net")) else {
            return;
        };
        self.physical = entries
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|name| is_physical_iface(sys, name))
            .collect();
    }

    pub fn sample(&mut self) -> io::Result<Vec<NetIo>> {
        let now = Instant::now();
        let secs = now.duration_since(self.last).as_secs_f64();
        self.last = now;
        let text = self.net_dev.read()?;
        let mut out = Vec::new();
        let mut seen = HashMap::with_capacity(self.physical.len());
        for n in parse_net_dev(text) {
            if !self.physical.contains(n.name) {
                continue;
            }
            let cur = (n.rx_bytes, n.tx_bytes);
            if let Some(&(prx, ptx)) = self.prev.get(n.name) {
                out.push(NetIo {
                    name: n.name.to_owned(),
                    rx_bps: rate(prx, cur.0, secs),
                    tx_bps: rate(ptx, cur.1, secs),
                });
            }
            seen.insert(n.name.to_owned(), cur);
        }
        self.prev = seen;
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NET_DEV: &str = "\
Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo:    6848      70    0    0    0     0          0         0     6848      70    0    0    0     0       0          0
enp9s0: 254623634  248455    0    0    0     0          0       823 41268245  165174    0    0    0     0       0          0
eth1:123 1 0 0 0 0 0 0 456 2 0 0 0 0 0 0
";

    #[test]
    fn parses_rx_and_tx_bytes() {
        let n = parse_net_dev(NET_DEV);
        assert_eq!(n.len(), 3);
        assert_eq!(
            n[1],
            NetCounters {
                name: "enp9s0",
                rx_bytes: 254_623_634,
                tx_bytes: 41_268_245
            }
        );
        assert_eq!(
            n[2],
            NetCounters {
                name: "eth1",
                rx_bytes: 123,
                tx_bytes: 456
            }
        );
    }

    #[test]
    fn physical_iface_detection_uses_device_link() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("class/net/enp9s0/device")).unwrap();
        std::fs::create_dir_all(root.path().join("class/net/docker0")).unwrap();
        assert!(is_physical_iface(root.path(), "enp9s0"));
        assert!(!is_physical_iface(root.path(), "docker0"));
    }
}
