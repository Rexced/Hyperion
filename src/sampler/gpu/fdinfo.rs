//! GPU usage and memory from DRM `fdinfo`: the kernel lists, for every process with
//! a GPU file open, how busy it kept each engine and how much memory it holds. Used
//! for Intel, whose driver has no single "busy %" file. Only processes of the current
//! user are readable, which in practice covers the desktop, games and browsers.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Client {
    pub pdev: String,
    pub id: u64,
    /// `(engine, busy, total)`: busy nanoseconds (`total` None), or busy cycles out of
    /// `total` cycles (the xe driver).
    pub engines: Vec<(String, u64, Option<u64>)>,
    pub resident_system: u64,
    pub resident_vram: u64,
}

fn bytes(value: &str) -> Option<u64> {
    let mut parts = value.split_whitespace();
    let n: u64 = parts.next()?.parse().ok()?;
    let unit = match parts.next() {
        None => 1,
        Some("KiB") => 1 << 10,
        Some("MiB") => 1 << 20,
        Some("GiB") => 1 << 30,
        Some(_) => return None,
    };
    Some(n * unit)
}

pub fn parse(text: &str) -> Option<Client> {
    let mut client = Client::default();
    let mut cycles: HashMap<String, u64> = HashMap::new();
    let mut totals: HashMap<String, u64> = HashMap::new();
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if key == "drm-pdev" {
            client.pdev = value.to_owned();
        } else if key == "drm-client-id" {
            client.id = value.parse().ok()?;
        } else if let Some(engine) = key.strip_prefix("drm-engine-") {
            let ns = value.trim_end_matches(" ns").parse().ok()?;
            client.engines.push((engine.to_owned(), ns, None));
        } else if let Some(engine) = key.strip_prefix("drm-total-cycles-") {
            totals.insert(engine.to_owned(), value.parse().ok()?);
        } else if let Some(engine) = key.strip_prefix("drm-cycles-") {
            cycles.insert(engine.to_owned(), value.parse().ok()?);
        } else if let Some(region) = key.strip_prefix("drm-resident-") {
            let b = bytes(value)?;
            if region.starts_with("system") || region.starts_with("gtt") {
                client.resident_system += b;
            } else if region.starts_with("vram") || region.starts_with("local") {
                client.resident_vram += b;
            }
        }
    }
    for (engine, busy) in cycles {
        let total = totals.get(&engine).copied();
        client.engines.push((engine, busy, total));
    }
    client.engines.sort_by(|a, b| a.0.cmp(&b.0));
    (!client.pdev.is_empty()).then_some(client)
}

/// Busiest engine's load in percent, from two snapshots of all clients.
pub fn usage_percent(
    prev: &HashMap<u64, Client>,
    cur: &HashMap<u64, Client>,
    elapsed_ns: f64,
) -> Option<f32> {
    let mut per_engine: HashMap<&str, f64> = HashMap::new();
    for (id, client) in cur {
        let Some(before) = prev.get(id) else { continue };
        for (engine, busy, total) in &client.engines {
            let Some((_, busy0, total0)) = before.engines.iter().find(|(e, ..)| e == engine) else {
                continue;
            };
            let share = match (total, total0) {
                (Some(t), Some(t0)) if t > t0 => {
                    busy.saturating_sub(*busy0) as f64 / (t - t0) as f64
                }
                (None, None) if elapsed_ns > 0.0 => busy.saturating_sub(*busy0) as f64 / elapsed_ns,
                _ => continue,
            };
            *per_engine.entry(engine.as_str()).or_default() += share;
        }
    }
    per_engine
        .values()
        .copied()
        .reduce(f64::max)
        .map(|v| (v * 100.0).min(100.0) as f32)
}

pub struct Scanner {
    slot: String,
    files: Vec<PathBuf>,
    prev: HashMap<u64, Client>,
    prev_t: Option<Instant>,
}

impl Scanner {
    pub fn new(slot: &str) -> Self {
        let mut s = Self {
            slot: slot.to_owned(),
            files: Vec::new(),
            prev: HashMap::new(),
            prev_t: None,
        };
        s.rescan();
        s
    }

    /// Finds the open GPU files for this card. Walking every process's fd table is
    /// too slow for every sample, so it runs on the slow tick.
    pub fn rescan(&mut self) {
        self.files.clear();
        let Ok(procs) = std::fs::read_dir("/proc") else {
            return;
        };
        for proc_dir in procs.flatten() {
            let pid = proc_dir.file_name();
            if !pid.to_string_lossy().chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let Ok(fds) = std::fs::read_dir(proc_dir.path().join("fd")) else {
                continue;
            };
            for fd in fds.flatten() {
                let is_gpu = std::fs::read_link(fd.path())
                    .is_ok_and(|target| target.starts_with("/dev/dri/"));
                if !is_gpu {
                    continue;
                }
                let info = proc_dir.path().join("fdinfo").join(fd.file_name());
                let for_us = std::fs::read_to_string(&info)
                    .ok()
                    .and_then(|t| parse(&t))
                    .is_some_and(|c| c.pdev == self.slot);
                if for_us {
                    self.files.push(info);
                }
            }
        }
    }

    /// `(busiest engine %, bytes in system RAM, bytes in VRAM)`.
    pub fn sample(&mut self) -> (Option<f32>, u64, u64) {
        let now = Instant::now();
        let mut cur: HashMap<u64, Client> = HashMap::new();
        for file in &self.files {
            if let Some(client) = std::fs::read_to_string(file).ok().and_then(|t| parse(&t))
                && client.pdev == self.slot
            {
                // The same client can be reachable through several (dup'd) fds.
                cur.entry(client.id).or_insert(client);
            }
        }
        let system = cur.values().map(|c| c.resident_system).sum();
        let vram = cur.values().map(|c| c.resident_vram).sum();
        let usage = self.prev_t.and_then(|t| {
            let elapsed = now.duration_since(t).as_nanos() as f64;
            usage_percent(&self.prev, &cur, elapsed)
        });
        self.prev = cur;
        self.prev_t = Some(now);
        (usage, system, vram)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const I915: &str = "pos:\t0\nflags:\t02100002\nmnt_id:\t26\ndrm-driver:\ti915\n\
drm-pdev:\t0000:00:02.0\ndrm-client-id:\t7\ndrm-engine-render:\t1000000000 ns\n\
drm-engine-video:\t0 ns\ndrm-resident-system0:\t2048 KiB\n";

    const XE: &str = "drm-driver:\txe\ndrm-pdev:\t0000:03:00.0\ndrm-client-id:\t3\n\
drm-cycles-rcs:\t500\ndrm-total-cycles-rcs:\t1000\ndrm-resident-vram0:\t1 MiB\n\
drm-resident-gtt:\t4096\n";

    #[test]
    fn parses_i915_engine_time_and_memory() {
        let c = parse(I915).unwrap();
        assert_eq!(c.pdev, "0000:00:02.0");
        assert_eq!(c.id, 7);
        assert_eq!(
            c.engines,
            [
                ("render".to_owned(), 1_000_000_000, None),
                ("video".to_owned(), 0, None)
            ]
        );
        assert_eq!(c.resident_system, 2048 * 1024);
    }

    #[test]
    fn parses_xe_cycles_and_vram() {
        let c = parse(XE).unwrap();
        assert_eq!(c.engines, [("rcs".to_owned(), 500, Some(1000))]);
        assert_eq!(c.resident_vram, 1 << 20);
        assert_eq!(c.resident_system, 4096);
    }

    #[test]
    fn not_a_gpu_file_is_none() {
        assert_eq!(parse("pos:\t0\nflags:\t02\n"), None);
    }

    #[test]
    fn usage_is_the_busiest_engine_summed_over_clients() {
        let snap = |render_ns: u64, id: u64| {
            let mut c = parse(I915).unwrap();
            c.id = id;
            c.engines[0].1 = render_ns;
            c
        };
        let prev = HashMap::from([(1, snap(0, 1)), (2, snap(0, 2))]);
        // Over 1 s: client 1 kept render busy 300 ms, client 2 200 ms.
        let cur = HashMap::from([(1, snap(300_000_000, 1)), (2, snap(200_000_000, 2))]);
        assert_eq!(usage_percent(&prev, &cur, 1e9), Some(50.0));
    }

    #[test]
    fn cycle_based_usage() {
        let mut before = parse(XE).unwrap();
        let mut after = before.clone();
        before.engines[0] = ("rcs".into(), 1000, Some(10_000));
        after.engines[0] = ("rcs".into(), 1750, Some(11_000));
        let prev = HashMap::from([(3, before)]);
        let cur = HashMap::from([(3, after)]);
        assert_eq!(usage_percent(&prev, &cur, 0.0), Some(75.0));
    }
}
