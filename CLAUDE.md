# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Rules

- Never use `rm -rf` without confirming with the user first.
- Think of yourself as a senior software developer with an insane amount of fullstack experience.
- Assume I am a fullstack software engineer intern who is under your supervision and you explain everything you do before actually doing it.

## Project

Hyperion is a live system resource monitor (CPU, GPU, memory, storage, network) written in Rust with `eframe`/`egui` (glow backend, Wayland + X11). It is Linux-only today; Windows and Android ports are planned, so platform-specific code is kept behind `cfg(target_os = ...)` or small per-platform backends. Requires Rust ≥ 1.95 (edition 2024).

## Commands

```sh
cargo run                         # debug build + launch
cargo build --release --locked    # what the installer builds
cargo test                        # all unit tests (inline #[cfg(test)] modules; no tests/ dir)
cargo test placement::            # tests in one module
cargo test near_screen_corner     # a single test by name substring
cargo clippy --all-targets
cargo fmt
./scripts/install.sh              # build + install to ~/.local/bin and .desktop file (run as normal user, not sudo)
```

Runtime files: config at `~/.config/hyperion/config.toml`, custom themes in `~/.config/hyperion/themes/*.toml`.

## Architecture

**Two threads, one shared buffer.** `sampler::spawn` starts a `hyperion-sampler` thread that runs `Sampler::tick` on the configured interval, builds a `metrics::Snapshot`, pushes it into `Arc<Mutex<History>>`, and calls `ctx.request_repaint()`. The UI thread (`app::HyperionApp`) only reads `History`. The UI tells the sampler about setting changes through `sampler::Control` messages (interval, enabled metrics) sent over an mpsc channel. Dropping `SamplerHandle` stops and joins the thread. Mutex poisoning is always recovered with `unwrap_or_else(PoisonError::into_inner)`.

**Sampler collectors** (`src/sampler/`) read Linux procfs/sysfs directly (no sysinfo crate). Each collector is optional: if it fails to initialise, its metric is absent rather than crashing the app. Slow or static work (device discovery, UDisks SMART temps, GPU re-enumeration, RAPL retry) runs at most every `SLOW_PERIOD` (5 s). Only collectors for enabled metrics are sampled. GPU backends: AMD/Intel via sysfs/fdinfo, NVIDIA via NVML loaded at runtime (absent driver means no NVIDIA backend), anything else via hwmon only. Parsers take a `sys: &Path` root so tests can build a fake sysfs tree with `tempfile`. Follow this pattern for new hardware code.

**Metrics and history keys.** `metrics::MetricId` is the list of toggleable metrics. `MetricId::key()` strings are persisted in the config and must never be renamed. `History` stores time series under string keys such as `cpu`, `disk:<name>:r`, and `gpu-usage:<id>`. `MetricId::series_prefix()` must match the prefixes used in `History::push`, because disabling a metric clears its series by prefix.

**Config flow.** `Config` (serde + TOML, `#[serde(default)]`, values snapped to the allowed steps on load) is the single source of truth for user state, including dashboard layout (`tile_order`, `tile_spans`, `detached_tiles`) and per-device visibility. Each frame, `HyperionApp::ui` clones the config, lets the UI mutate it, diffs the result against the clone in `apply_changes` (sends `Control` messages, clears history), and saves with a 600 ms debounce. Saves are atomic (write a `.tmp` file, then rename). New user-facing state should go into `Config` the same way.

**Dashboard** (`src/ui/dashboard/`): `grid.rs` is pure layout maths (CSS-grid-like auto-placement, no drawing). `tiles.rs` decides which tiles (`Card`) exist and draws them. `mod.rs` handles drag-to-reorder and resize. `popout.rs` tears tiles out into their own OS windows (egui viewports, app id `hyperion-tile`).

**Window placement is platform-specific.** Wayland doesn't let apps position their own windows, so:
- `hypr.rs` is a minimal Hyprland IPC client (control socket). It floats and moves pop-outs, reads the cursor and monitors, and hides the main window on the `special:hyperion` workspace.
- `main_window.rs` picks a Hyprland, plain-Wayland (minimise only), or native (hide/show) backend.
- `placement.rs` is platform-neutral snapping and overlap avoidance in global logical pixels.

Closing the main window while tiles are popped out only hides it.

**Rendering quirks.** Vsync is deliberately off (`main.rs`) because compositors never signal frames to windows on hidden workspaces, which froze the event loop on Hyprland. `frame_limit::FrameLimiter` caps redraws to the display refresh rate instead. Don't re-enable vsync.

**Privileged helper.** `hyperion --grant-sensors [cpu-power] [drivetemp]` (`privileged.rs`, intercepted at the top of `main`) is the only code that runs as root, via sudo from the installer or pkexec from Settings. It writes a udev rule so the invoking user's group can read RAPL energy counters, and/or enables the `drivetemp` module. Keep it limited to fixed, minimal steps.

**Themes** (`ui/theme.rs`): a theme is six base colours (`ThemeSpec`). All other shades are derived in `ThemeSpec::palette()` and can be overridden per field in `Advanced`. Presets are built in. Custom themes are TOML files keyed `custom:<file>`. Unknown theme keys fall back to `DEFAULT_KEY`.

## Conventions

- Errors are reported with `eprintln!("hyperion: ...")` and execution continues; the app degrades rather than failing.
- Code comments explain *why* (platform quirks, kernel behaviour), in plain prose. Match that style.
