use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use eframe::egui::{self, Layout, RichText};

use super::theme;
use super::widgets::{
    GraphLine, Scale, card, core_grid, drag_ghost, drag_placeholder, graph, metric_colors,
    usage_bar,
};
use crate::config::DetachedTile;
use crate::format;
use crate::history::History;
use crate::hypr::{self, Hypr};
use crate::metrics::{EnabledSet, MetricId, MetricId::*};

const MIN_COLUMN_WIDTH: f32 = 420.0;
const GRAPH_HEIGHT: f32 = 110.0;
/// Gap between tiles, both between columns in a row and between rows.
const TILE_GAP: f32 = 12.0;
/// Used only if a tile is detached before it was ever measured.
const FALLBACK_TILE_HEIGHT: f32 = 176.0;
/// Card inner margin plus stroke: the offset from a tile's corner to its header.
const CARD_INSET: f32 = 15.0;
/// Wayland app_id (Hyprland "class") of popped-out tile windows.
const DETACHED_APP_ID: &str = "hyperion-tile";
/// App id of the main window, set in `main.rs`.
const MAIN_APP_ID: &str = "hyperion";
/// Poll rate while waiting for a new pop-out window to appear so it can be placed.
const PLACEMENT_POLL: Duration = Duration::from_millis(150);
/// Poll rate for picking up moves/resizes done with the compositor's own controls.
const SYNC_POLL: Duration = Duration::from_secs(2);

/// A tile being dragged around the grid.
struct GridDrag {
    key: String,
    grab_offset: egui::Vec2,
    /// The tile's real height, so its placeholder keeps the grid geometry unchanged.
    height: f32,
    /// Tile we last moved onto. We don't move onto it again until the pointer leaves it:
    /// when tiles of different heights shift under a still pointer, the pointer can end
    /// up over the same neighbour again, and without this the pair would swap every frame.
    last_target: Option<String>,
}

/// A pop-out window being moved by its header, driven through Hyprland IPC.
struct WindowDrag {
    key: String,
    address: String,
    /// Cursor minus window top-left at drag start, in global logical pixels.
    grab: [f32; 2],
    pos: [f32; 2],
}

#[derive(Default)]
struct WindowState {
    rule_registered: bool,
    address: Option<String>,
    placed: bool,
}

/// Interaction state for the dashboard that doesn't belong in the config file.
pub struct TileBoard {
    hypr: Option<Hypr>,
    grid_drag: Option<GridDrag>,
    window_drag: Option<WindowDrag>,
    heights: HashMap<String, f32>,
    windows: HashMap<String, WindowState>,
    last_poll: Option<Instant>,
}

impl Default for TileBoard {
    fn default() -> Self {
        Self {
            hypr: Hypr::detect(),
            grid_drag: None,
            window_drag: None,
            heights: HashMap::new(),
            windows: HashMap::new(),
            last_poll: None,
        }
    }
}

#[cfg(test)]
impl TileBoard {
    fn headless() -> Self {
        Self {
            hypr: None,
            ..Self::default()
        }
    }
}

enum Card<'a> {
    Cpu,
    Memory,
    Net(&'a str),
    Disk(&'a str),
    Space,
}

fn card_key(c: &Card<'_>) -> String {
    match *c {
        Card::Cpu => "cpu".to_owned(),
        Card::Memory => "memory".to_owned(),
        Card::Net(name) => format!("net:{name}"),
        Card::Disk(name) => format!("disk:{name}"),
        Card::Space => "space".to_owned(),
    }
}

fn tile_title(c: &Card<'_>) -> String {
    match *c {
        Card::Cpu => "CPU".to_owned(),
        Card::Memory => "Memory".to_owned(),
        Card::Net(name) => format!("Network · {name}"),
        Card::Disk(name) => format!("Disk · {name}"),
        Card::Space => "Disk space".to_owned(),
    }
}

fn window_title(tile_title: &str) -> String {
    format!("Hyperion — {tile_title}")
}

/// Drops keys for tiles that no longer exist and appends any new, not-detached ones at
/// the end, keeping the rest of the user's saved order intact.
fn reconcile_order(
    order: &mut Vec<String>,
    detached: &BTreeMap<String, DetachedTile>,
    keys: &[String],
) {
    order.retain(|k| keys.contains(k) && !detached.contains_key(k));
    for k in keys {
        if !order.contains(k) && !detached.contains_key(k) {
            order.push(k.clone());
        }
    }
}

/// Moves `from` into the slot `to` currently occupies; the tiles in between shift one
/// step toward `from`'s old slot (a plain swap when they're neighbours).
fn move_in_order(order: &mut Vec<String>, from: &str, to: &str) {
    let (Some(from_idx), Some(to_idx)) = (
        order.iter().position(|k| k == from),
        order.iter().position(|k| k == to),
    ) else {
        return;
    };
    if from_idx != to_idx {
        let item = order.remove(from_idx);
        order.insert(to_idx, item);
    }
}

fn own_pid() -> i64 {
    i64::from(std::process::id())
}

pub fn show(
    ui: &mut egui::Ui,
    history: &History,
    enabled: EnabledSet,
    palette: &theme::Palette,
    order: &mut Vec<String>,
    detached: &mut BTreeMap<String, DetachedTile>,
    board: &mut TileBoard,
) {
    // Dropping a tile anywhere outside this rect (top bar, settings panel, or outside
    // the window entirely) pops it out into its own window.
    let dashboard_rect = ui.max_rect();
    let ctx = ui.ctx().clone();

    let latest = history.latest();
    let mut cards = Vec::new();
    if enabled.any(&[CpuTotal, CpuPerCore]) && latest.cpu.is_some() {
        cards.push(Card::Cpu);
    }
    if enabled.any(&[RamUsage, SwapUsage]) && latest.mem.is_some() {
        cards.push(Card::Memory);
    }
    if enabled.contains(NetThroughput) {
        cards.extend(latest.net.iter().map(|n| Card::Net(&n.name)));
    }
    if enabled.contains(DiskIo) {
        cards.extend(latest.disks.iter().map(|d| Card::Disk(&d.name)));
    }
    if enabled.contains(DiskSpace) && latest.filesystems.as_ref().is_some_and(|f| !f.is_empty()) {
        cards.push(Card::Space);
    }

    if cards.is_empty() {
        ui.centered_and_justified(|ui| {
            let msg = if !enabled.any(&MetricId::ALL) {
                "All metrics are turned off. Enable some in Settings."
            } else {
                "Collecting data…"
            };
            ui.label(RichText::new(msg).weak());
        });
        return;
    }

    let keys: Vec<String> = cards.iter().map(card_key).collect();
    detached.retain(|k, _| keys.contains(k));
    board.windows.retain(|k, _| detached.contains_key(k));
    reconcile_order(order, detached, &keys);
    if board
        .grid_drag
        .as_ref()
        .is_some_and(|d| !order.contains(&d.key))
    {
        board.grid_drag = None;
    }
    if board
        .window_drag
        .as_ref()
        .is_some_and(|d| !detached.contains_key(&d.key))
    {
        board.window_drag = None;
    }

    let by_key: HashMap<String, &Card<'_>> = cards.iter().map(|c| (card_key(c), c)).collect();
    let sorted: Vec<&Card<'_>> = order
        .iter()
        .filter_map(|k| by_key.get(k).copied())
        .collect();

    let columns = ((dashboard_rect.width() / MIN_COLUMN_WIDTH).floor() as usize).clamp(1, 4);
    let tile_width = (dashboard_rect.width() - TILE_GAP * (columns as f32 - 1.0)) / columns as f32;

    let pointer = ctx.input(|i| i.pointer.interact_pos());
    let mut hovered_target: Option<String> = None;

    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            let clip = ui.clip_rect();
            for row in sorted.chunks(columns) {
                // `horizontal_top`: plain `horizontal` centers children vertically,
                // which made shorter tiles float and zig-zag.
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = TILE_GAP;
                    for c in row {
                        let key = card_key(c);
                        let placeholder = board
                            .grid_drag
                            .as_ref()
                            .filter(|d| d.key == key)
                            .map(|d| d.height);
                        let id = egui::Id::new(("hyperion-tile", key.as_str()));

                        let cell = ui.allocate_ui_with_layout(
                            egui::vec2(tile_width, 0.0),
                            Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.set_width(tile_width);
                                match placeholder {
                                    Some(height) => {
                                        drag_placeholder(ui, palette, height);
                                        None
                                    }
                                    None => draw(ui, c, history, enabled, palette, id),
                                }
                            },
                        );
                        let rect = cell.response.rect;

                        if placeholder.is_none() {
                            board.heights.insert(key.clone(), rect.height());
                            if let Some(header) = cell.inner
                                && header.drag_started()
                            {
                                let grab = header.interact_pointer_pos().unwrap_or(header.rect.min)
                                    - header.rect.min;
                                board.grid_drag = Some(GridDrag {
                                    key: key.clone(),
                                    grab_offset: grab,
                                    height: rect.height(),
                                    last_target: None,
                                });
                            }
                        }

                        // Plain geometry, not `rect_contains_pointer`: that also requires
                        // the grid's layer to be topmost under the pointer, which isn't
                        // true while the drag ghost or a menu is over it.
                        if board.grid_drag.as_ref().is_some_and(|d| d.key != key)
                            && pointer.is_some_and(|p| rect.contains(p) && clip.contains(p))
                        {
                            hovered_target = Some(key);
                        }
                    }
                });
                ui.add_space(TILE_GAP);
            }
        });

    if let Some(d) = board.grid_drag.as_mut() {
        match hovered_target {
            Some(target) if d.last_target.as_ref() != Some(&target) => {
                move_in_order(order, &d.key, &target);
                d.last_target = Some(target);
            }
            Some(_) => {}
            None => d.last_target = None,
        }
    }

    finish_grid_drag(
        &ctx,
        board,
        order,
        detached,
        &by_key,
        palette,
        pointer,
        dashboard_rect,
        tile_width,
    );

    let redock = show_detached_windows(&ctx, history, enabled, palette, detached, board, &by_key);
    for key in redock {
        detached.remove(&key);
        board.windows.remove(&key);
        if !order.contains(&key) {
            order.push(key);
        }
    }

    place_new_windows(&ctx, detached, board, &by_key);
}

/// Draws the drag ghost while a grid tile is held, and on release either leaves it in
/// its (already live-updated) grid slot or pops it out if it was dropped outside.
#[allow(clippy::too_many_arguments)]
fn finish_grid_drag(
    ctx: &egui::Context,
    board: &mut TileBoard,
    order: &mut Vec<String>,
    detached: &mut BTreeMap<String, DetachedTile>,
    by_key: &HashMap<String, &Card<'_>>,
    palette: &theme::Palette,
    pointer: Option<egui::Pos2>,
    dashboard_rect: egui::Rect,
    tile_width: f32,
) {
    let Some(d) = board.grid_drag.as_ref() else {
        return;
    };
    // No position means the pointer left the window mid-drag.
    let outside = pointer.is_none_or(|p| !dashboard_rect.contains(p));

    if ctx.input(|i| i.pointer.primary_down()) {
        ctx.request_repaint();
        if let Some(pos) = pointer {
            let title = by_key
                .get(&d.key)
                .map(|c| tile_title(c))
                .unwrap_or_default();
            let label = if outside {
                format!("Pop out · {title}")
            } else {
                title
            };
            drag_ghost(ctx, palette, pos - d.grab_offset, &label);
        }
        return;
    }

    let Some(d) = board.grid_drag.take() else {
        return;
    };
    let hypr_cursor = board.hypr.as_ref().and_then(Hypr::cursor_pos);
    // Backstop for when egui never saw the pointer leave: ask Hyprland whether the
    // release happened outside the main window.
    let outside_main = match (&board.hypr, hypr_cursor) {
        (Some(h), Some(cursor)) => h
            .clients()
            .iter()
            .find(|c| c.pid == own_pid() && c.class == MAIN_APP_ID)
            .is_some_and(|main| !main.rect().contains(cursor)),
        _ => false,
    };
    if !(outside || outside_main) {
        return;
    }

    let inset = egui::vec2(CARD_INSET, CARD_INSET);
    let pos = if let Some(cursor) = hypr_cursor {
        Some([
            cursor[0] - d.grab_offset.x - inset.x,
            cursor[1] - d.grab_offset.y - inset.y,
        ])
    } else if let (Some(inner), Some(p)) = (ctx.input(|i| i.viewport().inner_rect), pointer) {
        // X11 reports the window's screen position; plain Wayland doesn't (None).
        let global = inner.min + p.to_vec2() - d.grab_offset - inset;
        Some([global.x, global.y])
    } else {
        None
    };
    let height = board
        .heights
        .get(&d.key)
        .copied()
        .unwrap_or(FALLBACK_TILE_HEIGHT);
    order.retain(|k| k != &d.key);
    board.windows.remove(&d.key);
    detached.insert(
        d.key,
        DetachedTile {
            pos,
            size: [tile_width, height],
        },
    );
}

/// Renders each popped-out tile in its own OS window and handles moving it by its
/// header. Returns the tiles to put back in the grid (closed, or dropped on the main
/// window).
fn show_detached_windows(
    ctx: &egui::Context,
    history: &History,
    enabled: EnabledSet,
    palette: &theme::Palette,
    detached: &mut BTreeMap<String, DetachedTile>,
    board: &mut TileBoard,
    by_key: &HashMap<String, &Card<'_>>,
) -> Vec<String> {
    let mut redock = Vec::new();
    for (key, tile) in detached.iter_mut() {
        let Some(c) = by_key.get(key).copied() else {
            continue;
        };
        let title = window_title(&tile_title(c));
        let state = board.windows.entry(key.clone()).or_default();
        if let Some(h) = board.hypr.as_ref()
            && !state.rule_registered
        {
            h.register_tile_rule(DETACHED_APP_ID, &title, tile.pos, tile.size);
            state.rule_registered = true;
        }
        let mut builder = egui::ViewportBuilder::default()
            .with_title(title)
            .with_app_id(DETACHED_APP_ID)
            .with_inner_size(tile.size)
            .with_decorations(false);
        if let Some(pos) = tile.pos {
            // Honored on X11; Wayland ignores it and Hyprland placement happens via IPC.
            builder = builder.with_position(pos);
        }
        let viewport = egui::ViewportId::from_hash_of(("hyperion-detached", key.as_str()));
        let id = egui::Id::new(("hyperion-tile", key.as_str()));

        let (drag_started, close, outer, primary_down) =
            ctx.show_viewport_immediate(viewport, builder, |ui, _class| {
                ui.painter()
                    .rect_filled(ui.max_rect(), 0, palette.bg_bottom);
                let header = draw(ui, c, history, enabled, palette, id);
                ui.input(|i| {
                    (
                        header.is_some_and(|h| h.drag_started()),
                        i.viewport().close_requested(),
                        i.viewport().outer_rect,
                        i.pointer.primary_down(),
                    )
                })
            });

        if close {
            redock.push(key.clone());
            continue;
        }
        if let Some(r) = outer {
            tile.pos = Some([r.min.x, r.min.y]);
        }

        if drag_started {
            let address = board.windows.get(key).and_then(|w| w.address.clone());
            let start = board.hypr.as_ref().zip(address).and_then(|(h, address)| {
                let cursor = h.cursor_pos()?;
                let at = h.clients().into_iter().find(|c| c.address == address)?.at;
                let pos = [at[0] as f32, at[1] as f32];
                Some(WindowDrag {
                    key: key.clone(),
                    address,
                    grab: [cursor[0] - pos[0], cursor[1] - pos[1]],
                    pos,
                })
            });
            match start {
                Some(drag) => board.window_drag = Some(drag),
                // Not on Hyprland (or not placed yet): let the compositor move it.
                None => ctx.send_viewport_cmd_to(viewport, egui::ViewportCommand::StartDrag),
            }
        }

        let (Some(h), Some(drag)) = (board.hypr.as_ref(), board.window_drag.as_mut()) else {
            continue;
        };
        if drag.key != *key {
            continue;
        }
        if primary_down {
            ctx.request_repaint();
            if let Some(cursor) = h.cursor_pos() {
                let pos = [cursor[0] - drag.grab[0], cursor[1] - drag.grab[1]];
                if pos != drag.pos {
                    h.move_to(&drag.address, pos);
                    drag.pos = pos;
                }
            }
            continue;
        }

        // Released: back into the grid if dropped on the main window, else snap.
        let clients = h.clients();
        let over_main = h.cursor_pos().is_some_and(|cursor| {
            clients
                .iter()
                .any(|c| c.pid == own_pid() && c.class == MAIN_APP_ID && c.rect().contains(cursor))
        });
        if over_main {
            redock.push(key.clone());
        } else {
            let win = hypr::Rect {
                x: drag.pos[0],
                y: drag.pos[1],
                w: tile.size[0],
                h: tile.size[1],
            };
            let snapped = hypr::snap(win, &usable_areas(h), &neighbours(&clients, &drag.address));
            if snapped != drag.pos {
                h.move_to(&drag.address, snapped);
            }
            tile.pos = Some(snapped);
        }
        board.window_drag = None;
    }
    redock
}

/// On Hyprland: floats and positions pop-out windows once they appear, then keeps the
/// saved position/size in sync with any moves made with the compositor's own controls.
fn place_new_windows(
    ctx: &egui::Context,
    detached: &mut BTreeMap<String, DetachedTile>,
    board: &mut TileBoard,
    by_key: &HashMap<String, &Card<'_>>,
) {
    let Some(h) = board.hypr.as_ref() else {
        return;
    };
    if detached.is_empty() || board.window_drag.is_some() {
        return;
    }
    let waiting = detached
        .keys()
        .any(|k| !board.windows.get(k).is_some_and(|w| w.placed));
    let period = if waiting { PLACEMENT_POLL } else { SYNC_POLL };
    ctx.request_repaint_after(period);
    let now = Instant::now();
    if board
        .last_poll
        .is_some_and(|t| now.duration_since(t) < period)
    {
        return;
    }
    board.last_poll = Some(now);

    let mut clients = h.clients();
    let mut areas = None;
    for (key, tile) in detached.iter_mut() {
        let Some(c) = by_key.get(key) else { continue };
        let title = window_title(&tile_title(c));
        let Some(i) = clients
            .iter()
            .position(|cl| cl.pid == own_pid() && cl.title == title)
        else {
            continue;
        };
        let state = board.windows.entry(key.clone()).or_default();
        state.address = Some(clients[i].address.clone());
        if state.placed {
            tile.pos = Some([clients[i].at[0] as f32, clients[i].at[1] as f32]);
            tile.size = [clients[i].size[0] as f32, clients[i].size[1] as f32];
            continue;
        }
        let pos = tile
            .pos
            .unwrap_or([clients[i].at[0] as f32, clients[i].at[1] as f32]);
        let win = hypr::Rect {
            x: pos[0],
            y: pos[1],
            w: tile.size[0],
            h: tile.size[1],
        };
        let areas = areas.get_or_insert_with(|| usable_areas(h));
        let snapped = hypr::snap(win, areas, &neighbours(&clients, &clients[i].address));
        h.place(&clients[i], snapped, tile.size);
        // Later windows in this same pass must snap against where this one went now.
        clients[i].at = snapped.map(|v| v.round() as i32);
        clients[i].size = tile.size.map(|v| v.round() as i32);
        clients[i].floating = true;
        tile.pos = Some(snapped);
        state.placed = true;
    }
}

fn usable_areas(h: &Hypr) -> Vec<hypr::Rect> {
    h.monitors().iter().map(hypr::Monitor::usable).collect()
}

/// Our other windows (the main window and other pop-outs) that a window can snap to.
fn neighbours(clients: &[hypr::Client], except: &str) -> Vec<hypr::Rect> {
    clients
        .iter()
        .filter(|c| {
            c.pid == own_pid()
                && c.address != except
                && (c.class == MAIN_APP_ID || c.class == DETACHED_APP_ID)
        })
        .map(hypr::Client::rect)
        .collect()
}

fn draw(
    ui: &mut egui::Ui,
    c: &Card<'_>,
    history: &History,
    enabled: EnabledSet,
    palette: &theme::Palette,
    id: egui::Id,
) -> Option<egui::Response> {
    let latest = history.latest();
    let colors = metric_colors(palette);
    let title = tile_title(c);
    match *c {
        Card::Cpu => {
            let cpu = latest.cpu.as_ref()?;
            let headline = if enabled.contains(CpuTotal) {
                format::percent(f64::from(cpu.total))
            } else {
                String::new()
            };
            Some(card(ui, palette, id, &title, &headline, |ui| {
                if enabled.contains(CpuTotal) {
                    graph(
                        ui,
                        palette,
                        "plot:cpu",
                        history,
                        &[GraphLine {
                            key: "cpu",
                            name: "CPU",
                            color: colors.cpu,
                        }],
                        Scale::Percent,
                        GRAPH_HEIGHT,
                    );
                }
                if enabled.contains(CpuPerCore) {
                    ui.add_space(6.0);
                    core_grid(ui, &cpu.per_core, colors.cpu);
                }
            }))
        }
        Card::Memory => {
            let m = latest.mem?;
            let headline = if enabled.contains(RamUsage) {
                format!(
                    "{} / {}",
                    format::bytes(m.used as f64),
                    format::bytes(m.total as f64)
                )
            } else {
                String::new()
            };
            Some(card(ui, palette, id, &title, &headline, |ui| {
                let mut lines = Vec::new();
                if enabled.contains(RamUsage) {
                    lines.push(GraphLine {
                        key: "ram",
                        name: "RAM",
                        color: colors.ram,
                    });
                }
                if enabled.contains(SwapUsage) && m.swap_total > 0 {
                    lines.push(GraphLine {
                        key: "swap",
                        name: "Swap",
                        color: colors.swap,
                    });
                }
                graph(
                    ui,
                    palette,
                    "plot:mem",
                    history,
                    &lines,
                    Scale::Percent,
                    GRAPH_HEIGHT,
                );
                ui.add_space(6.0);
                if enabled.contains(RamUsage) {
                    ratio_bar(ui, "RAM", m.used, m.total, colors.ram);
                }
                if enabled.contains(SwapUsage) {
                    if m.swap_total > 0 {
                        ratio_bar(ui, "Swap", m.swap_used, m.swap_total, colors.swap);
                    } else {
                        ui.label(RichText::new("No swap configured").weak());
                    }
                }
            }))
        }
        Card::Net(name) => {
            let n = latest.net.iter().find(|n| n.name == name)?;
            let headline = format!("↓ {}  ↑ {}", format::rate(n.rx_bps), format::rate(n.tx_bps));
            let (rx, tx) = (format!("net:{name}:rx"), format!("net:{name}:tx"));
            Some(card(ui, palette, id, &title, &headline, |ui| {
                graph(
                    ui,
                    palette,
                    ("plot:net", name),
                    history,
                    &[
                        GraphLine {
                            key: &rx,
                            name: "Download",
                            color: colors.rx,
                        },
                        GraphLine {
                            key: &tx,
                            name: "Upload",
                            color: colors.tx,
                        },
                    ],
                    Scale::BytesPerSec,
                    GRAPH_HEIGHT,
                );
            }))
        }
        Card::Disk(name) => {
            let d = latest.disks.iter().find(|d| d.name == name)?;
            let headline = format!(
                "R {}  W {}",
                format::rate(d.read_bps),
                format::rate(d.write_bps)
            );
            let (r, w) = (format!("disk:{name}:r"), format!("disk:{name}:w"));
            Some(card(ui, palette, id, &title, &headline, |ui| {
                graph(
                    ui,
                    palette,
                    ("plot:disk", name),
                    history,
                    &[
                        GraphLine {
                            key: &r,
                            name: "Read",
                            color: colors.read,
                        },
                        GraphLine {
                            key: &w,
                            name: "Write",
                            color: colors.write,
                        },
                    ],
                    Scale::BytesPerSec,
                    GRAPH_HEIGHT,
                );
            }))
        }
        Card::Space => {
            let fss = latest.filesystems.as_ref()?;
            Some(card(ui, palette, id, &title, "", |ui| {
                for fs in fss {
                    let device = fs.device.trim_start_matches("/dev/");
                    ui.label(
                        RichText::new(format!("{}  ·  {} · {}", fs.mount, fs.fstype, device))
                            .small(),
                    );
                    usage_bar(
                        ui,
                        fs.used as f32 / fs.total as f32,
                        format!(
                            "{} / {} used",
                            format::bytes(fs.used as f64),
                            format::bytes(fs.total as f64)
                        ),
                        colors.read,
                    );
                    ui.add_space(4.0);
                }
            }))
        }
    }
}

fn ratio_bar(ui: &mut egui::Ui, label: &str, used: u64, total: u64, color: egui::Color32) {
    let frac = used as f32 / total as f32;
    usage_bar(
        ui,
        frac,
        format!(
            "{label}  {} / {}  ({:.0}%)",
            format::bytes(used as f64),
            format::bytes(total as f64),
            frac * 100.0
        ),
        color,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(keys: &[&str]) -> Vec<String> {
        keys.iter().map(|s| s.to_string()).collect()
    }

    fn tile() -> DetachedTile {
        DetachedTile {
            pos: Some([10.0, 20.0]),
            size: [400.0, 200.0],
        }
    }

    #[test]
    fn dragging_onto_the_next_tile_swaps_them() {
        // Regression: this used to be a no-op, so forward drags "sometimes" failed.
        let mut order = v(&["cpu", "memory", "net"]);
        move_in_order(&mut order, "cpu", "memory");
        assert_eq!(order, v(&["memory", "cpu", "net"]));
    }

    #[test]
    fn dragging_onto_the_previous_tile_swaps_them() {
        let mut order = v(&["cpu", "memory", "net"]);
        move_in_order(&mut order, "memory", "cpu");
        assert_eq!(order, v(&["memory", "cpu", "net"]));
    }

    #[test]
    fn long_forward_drag_takes_target_slot_and_shifts_the_rest_back() {
        let mut order = v(&["a", "b", "c", "d"]);
        move_in_order(&mut order, "a", "c");
        assert_eq!(order, v(&["b", "c", "a", "d"]));
    }

    #[test]
    fn long_backward_drag_takes_target_slot_and_shifts_the_rest_forward() {
        let mut order = v(&["a", "b", "c", "d"]);
        move_in_order(&mut order, "d", "b");
        assert_eq!(order, v(&["a", "d", "b", "c"]));
    }

    #[test]
    fn move_to_self_or_unknown_is_a_no_op() {
        let mut order = v(&["a", "b"]);
        move_in_order(&mut order, "a", "a");
        move_in_order(&mut order, "a", "missing");
        assert_eq!(order, v(&["a", "b"]));
    }

    /// Drives `show` through egui's real input pipeline with synthetic pointer events.
    struct Harness {
        ctx: egui::Context,
        time: f64,
        size: egui::Vec2,
        history: History,
        order: Vec<String>,
        detached: BTreeMap<String, DetachedTile>,
        board: TileBoard,
    }

    impl Harness {
        fn new(size: egui::Vec2) -> Self {
            use crate::metrics::{CpuSample, MemSample, NetIo, Snapshot};
            let mut history = History::new(60.0);
            history.push(Snapshot {
                t: 1.0,
                cpu: Some(CpuSample {
                    total: 10.0,
                    per_core: vec![5.0; 4],
                }),
                mem: Some(MemSample {
                    total: 100,
                    used: 50,
                    swap_total: 10,
                    swap_used: 1,
                }),
                net: vec![NetIo {
                    name: "eth0".into(),
                    rx_bps: 1.0,
                    tx_bps: 1.0,
                }],
                ..Default::default()
            });
            let mut h = Self {
                ctx: egui::Context::default(),
                time: 0.0,
                size,
                history,
                order: Vec::new(),
                detached: BTreeMap::new(),
                board: TileBoard::headless(),
            };
            h.frame(vec![]);
            h
        }

        fn frame(&mut self, events: Vec<egui::Event>) {
            self.time += 1.0 / 60.0;
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, self.size)),
                time: Some(self.time),
                events,
                ..Default::default()
            };
            let enabled = crate::config::Config::default().enabled();
            let palette = theme::ThemeId::default().palette();
            let mut output = self.ctx.run_ui(raw, |ui| {
                show(
                    ui,
                    &self.history,
                    enabled,
                    &palette,
                    &mut self.order,
                    &mut self.detached,
                    &mut self.board,
                );
            });
            // No renderer here to upload font textures to.
            output.textures_delta.clear();
        }

        fn button(&mut self, pos: egui::Pos2, pressed: bool) {
            self.frame(vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::default(),
            }]);
        }

        /// Press on `from`, glide to `to` over several frames, then release there.
        fn drag(&mut self, from: egui::Pos2, to: egui::Pos2) {
            self.frame(vec![egui::Event::PointerMoved(from)]);
            self.button(from, true);
            for step in 1..=20 {
                let p = from + (to - from) * (step as f32 / 20.0);
                self.frame(vec![egui::Event::PointerMoved(p)]);
            }
            for _ in 0..5 {
                self.frame(vec![egui::Event::PointerMoved(to)]);
            }
            self.button(to, false);
            self.frame(vec![]);
        }
    }

    // A 1000px-wide screen gives two 494px columns; y=25 is inside the first row's
    // header strip (card margin 14px, header ~20px).
    const WIDE: egui::Vec2 = egui::vec2(1000.0, 800.0);
    const CPU_HEADER: egui::Pos2 = egui::pos2(40.0, 25.0);
    const MEMORY_HEADER: egui::Pos2 = egui::pos2(546.0, 25.0);

    #[test]
    fn e2e_forward_drag_onto_neighbour_swaps() {
        let mut h = Harness::new(WIDE);
        assert_eq!(h.order, v(&["cpu", "memory", "net:eth0"]));
        h.drag(CPU_HEADER, MEMORY_HEADER);
        assert_eq!(h.order, v(&["memory", "cpu", "net:eth0"]));
        assert!(h.detached.is_empty());
        assert!(h.board.grid_drag.is_none());
    }

    #[test]
    fn e2e_backward_drag_swaps_and_second_row_tile_can_move_up() {
        let mut h = Harness::new(WIDE);
        h.drag(MEMORY_HEADER, CPU_HEADER);
        assert_eq!(h.order, v(&["memory", "cpu", "net:eth0"]));
        let net_header = egui::pos2(
            40.0,
            h.board.heights["memory"].max(h.board.heights["cpu"]) + 12.0 + 25.0,
        );
        h.drag(net_header, MEMORY_HEADER - egui::vec2(506.0, 0.0));
        assert_eq!(h.order, v(&["net:eth0", "memory", "cpu"]));
    }

    #[test]
    fn e2e_dropping_outside_the_window_pops_the_tile_out() {
        let mut h = Harness::new(WIDE);
        h.drag(CPU_HEADER, egui::pos2(1200.0, 25.0));
        assert_eq!(h.order, v(&["memory", "net:eth0"]));
        let tile = &h.detached["cpu"];
        assert_eq!(tile.size[0], (1000.0 - TILE_GAP) / 2.0);
        assert!(tile.size[1] > 100.0);
    }

    #[test]
    fn reconcile_drops_missing_and_appends_new() {
        let mut order = v(&["disk:sdb", "cpu", "gone"]);
        let keys = v(&["cpu", "memory", "disk:sdb"]);
        reconcile_order(&mut order, &BTreeMap::new(), &keys);
        assert_eq!(order, v(&["disk:sdb", "cpu", "memory"]));
    }

    #[test]
    fn reconcile_keeps_detached_tiles_out_of_the_grid() {
        let mut order = v(&["cpu", "memory"]);
        let detached = BTreeMap::from([("memory".to_string(), tile())]);
        let keys = v(&["cpu", "memory"]);
        reconcile_order(&mut order, &detached, &keys);
        assert_eq!(order, v(&["cpu"]));
    }
}
