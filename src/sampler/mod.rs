mod cpu;
mod disk;
mod mem;
mod net;
mod procfile;

use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use eframe::egui;

use crate::history::History;
use crate::metrics::{EnabledSet, MetricId, Snapshot};

/// Cadence for work that is too slow or too static to run every tick.
const SLOW_PERIOD: Duration = Duration::from_secs(5);
/// Cap on the delay before the first sample, so long intervals don't start with an empty window.
const FIRST_SAMPLE_MAX_DELAY: Duration = Duration::from_millis(500);

pub enum Control {
    Interval(Duration),
    Enabled(EnabledSet),
}

pub struct SamplerHandle {
    tx: Option<mpsc::Sender<Control>>,
    thread: Option<JoinHandle<()>>,
}

impl SamplerHandle {
    pub fn send(&self, control: Control) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(control);
        }
    }
}

impl Drop for SamplerHandle {
    fn drop(&mut self) {
        // Dropping the sender wakes the thread with `Disconnected`.
        self.tx.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub fn spawn(
    ctx: egui::Context,
    history: Arc<Mutex<History>>,
    interval: Duration,
    enabled: EnabledSet,
) -> SamplerHandle {
    let (tx, rx) = mpsc::channel();
    let thread = std::thread::Builder::new()
        .name("hyperion-sampler".into())
        .spawn(move || run(ctx, history, rx, interval, enabled))
        .expect("failed to spawn sampler thread");
    SamplerHandle {
        tx: Some(tx),
        thread: Some(thread),
    }
}

fn run(
    ctx: egui::Context,
    history: Arc<Mutex<History>>,
    rx: mpsc::Receiver<Control>,
    mut interval: Duration,
    mut enabled: EnabledSet,
) {
    let mut sampler = Sampler::new();
    let mut last_tick = Instant::now();
    let mut next_tick = last_tick + interval.min(FIRST_SAMPLE_MAX_DELAY);

    loop {
        let wait = next_tick.saturating_duration_since(Instant::now());
        match rx.recv_timeout(wait) {
            Ok(Control::Interval(d)) => {
                interval = d;
                next_tick = last_tick + interval;
            }
            Ok(Control::Enabled(e)) => {
                enabled = e;
                sampler.last_slow = None;
            }
            Err(RecvTimeoutError::Timeout) => {
                let snapshot = sampler.tick(enabled);
                history
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(snapshot);
                ctx.request_repaint();

                let now = Instant::now();
                last_tick = now;
                next_tick += interval;
                if next_tick <= now {
                    // Fell behind (suspend, slow I/O): resync instead of bursting.
                    next_tick = now + interval;
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

struct Sampler {
    start: Instant,
    last_slow: Option<Instant>,
    cpu: Option<cpu::CpuCollector>,
    mem: Option<mem::MemCollector>,
    disk: Option<disk::DiskCollector>,
    net: Option<net::NetCollector>,
}

impl Sampler {
    fn new() -> Self {
        Self {
            start: Instant::now(),
            last_slow: None,
            cpu: cpu::CpuCollector::new().ok(),
            mem: mem::MemCollector::new().ok(),
            disk: disk::DiskCollector::new().ok(),
            net: net::NetCollector::new().ok(),
        }
    }

    fn tick(&mut self, enabled: EnabledSet) -> Snapshot {
        use MetricId::*;

        let now = Instant::now();
        let slow = self
            .last_slow
            .is_none_or(|t| now.duration_since(t) >= SLOW_PERIOD);
        if slow {
            self.last_slow = Some(now);
            if let Some(d) = &mut self.disk {
                d.refresh_devices();
            }
            if let Some(n) = &mut self.net {
                n.refresh_devices();
            }
        }

        let mut snap = Snapshot {
            t: now.duration_since(self.start).as_secs_f64(),
            ..Default::default()
        };
        if enabled.any(&[CpuTotal, CpuPerCore]) {
            snap.cpu = self.cpu.as_mut().and_then(|c| c.sample().ok());
        }
        if enabled.any(&[RamUsage, SwapUsage]) {
            snap.mem = self.mem.as_mut().and_then(|m| m.sample().ok());
        }
        if enabled.contains(DiskIo) {
            snap.disks = self
                .disk
                .as_mut()
                .and_then(|d| d.sample().ok())
                .unwrap_or_default();
        }
        if enabled.contains(DiskSpace) && slow {
            snap.filesystems = Some(disk::filesystems());
        }
        if enabled.contains(NetThroughput) {
            snap.net = self
                .net
                .as_mut()
                .and_then(|n| n.sample().ok())
                .unwrap_or_default();
        }
        snap
    }
}
