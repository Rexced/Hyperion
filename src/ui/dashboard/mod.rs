//! The tile dashboard: a grid you can rearrange by dragging, like a phone home screen,
//! plus tiles popped out into their own windows.

mod grid;
mod popout;
mod tiles;

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use eframe::egui::{self, Layout, RichText};

use crate::config::DetachedTile;
use crate::history::History;
use crate::hypr::Hypr;
use crate::metrics::{EnabledSet, MetricId};
use crate::ui::theme;
use grid::{Grid, Span};
use tiles::Card;

/// How long tiles take to slide into a new slot.
const SLIDE_SECS: f32 = 0.18;
/// A torn-off tile only re-docks once the cursor is this far back inside the window,
/// so hovering right on the edge doesn't flip it in and out.
const REENTRY_MARGIN: f32 = 24.0;
/// Size of the resize grip in a tile's bottom-right corner.
const GRIP: f32 = 16.0;

/// A tile lifted out of the grid and following the pointer.
struct GridDrag {
    key: String,
    /// Pointer position within the tile when it was picked up.
    grab: egui::Vec2,
    size: egui::Vec2,
    /// Where it would land if dropped now: an index into the grid without it.
    slot: usize,
    /// Where it was picked up from, for cancelling.
    origin_slot: usize,
    /// Carried outside the main window as its own window.
    torn: bool,
}

/// A tile being resized by its corner grip.
struct Resize {
    key: String,
    /// The tile's top-left, relative to the grid origin (so scrolling is harmless).
    anchor: egui::Vec2,
    /// Grip position minus the pointer at the start: grabbing the grip a few pixels
    /// off the exact corner shouldn't change the size.
    corner_offset: egui::Vec2,
    /// Cell steps and columns when it started (see `grid::span_for`).
    step: [f32; 2],
    columns: usize,
    /// For Escape.
    original: Span,
}

/// Interaction state for the dashboard that doesn't belong in the config file.
pub struct TileBoard {
    hypr: Option<Hypr>,
    grid_drag: Option<GridDrag>,
    resize: Option<Resize>,
    window_drag: Option<popout::WindowDrag>,
    windows: HashMap<String, popout::WindowState>,
    last_poll: Option<Instant>,
    /// Keeps the slide animation running briefly after a drop.
    settle_until: Option<Instant>,
}

impl Default for TileBoard {
    fn default() -> Self {
        Self {
            hypr: Hypr::detect(),
            grid_drag: None,
            resize: None,
            window_drag: None,
            windows: HashMap::new(),
            last_poll: None,
            settle_until: None,
        }
    }
}

fn tile_id(key: &str) -> egui::Id {
    egui::Id::new(("hyperion-tile", key))
}

fn own_pid() -> i64 {
    i64::from(std::process::id())
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

/// Puts `key` at `slot`, counted among the other tiles (as shown while it was lifted).
fn drop_at(order: &mut Vec<String>, key: &str, slot: usize) {
    order.retain(|k| k != key);
    order.insert(slot.min(order.len()), key.to_owned());
}

/// Puts a popped-out tile back into the grid: into the slot it was dropped on, else
/// where it was torn off from, else at the end.
fn return_to_grid(
    order: &mut Vec<String>,
    detached: &mut BTreeMap<String, DetachedTile>,
    key: &str,
    slot: Option<usize>,
) {
    let home = detached.remove(key).and_then(|t| t.home);
    drop_at(order, key, slot.or(home).unwrap_or(usize::MAX));
}

/// Everything a tile needs to draw itself.
#[derive(Clone, Copy)]
pub struct View<'a> {
    pub history: &'a History,
    pub enabled: EnabledSet,
    pub palette: &'a theme::Palette,
    /// Drive id -> shown; drives not listed show only if they hold the root filesystem.
    pub drives: &'a BTreeMap<String, bool>,
    /// GPU id -> shown; GPUs not listed show if dedicated (or the only kind present).
    pub gpus: &'a BTreeMap<String, bool>,
}

/// What the dashboard asks the app to do after this frame.
#[derive(Default)]
pub struct Shown {
    /// A pop-out's "bring back the main window" button was clicked.
    pub restore_main: bool,
}

/// The user's tile layout, kept in the config file.
pub struct Arrangement<'a> {
    /// Docked tiles in grid order.
    pub order: &'a mut Vec<String>,
    /// Tiles popped out into their own windows (never also in `order`).
    pub detached: &'a mut BTreeMap<String, DetachedTile>,
    /// `[columns, rows]` of resized tiles; missing means one cell.
    pub spans: &'a mut BTreeMap<String, [u8; 2]>,
}

enum Slot<'a> {
    Tile(&'a str),
    /// Where the tile with this key will land if dropped (it's being dragged).
    Empty(&'a str),
}

impl<'a> Slot<'a> {
    fn key(&self) -> &'a str {
        match *self {
            Slot::Tile(k) | Slot::Empty(k) => k,
        }
    }
}

fn span_of(spans: &BTreeMap<String, [u8; 2]>, key: &str) -> Span {
    spans
        .get(key)
        .copied()
        .map_or(grid::ONE_CELL, grid::clamp_span)
}

fn set_span(spans: &mut BTreeMap<String, [u8; 2]>, key: &str, span: Span) {
    if span == grid::ONE_CELL {
        spans.remove(key);
    } else {
        // Spans are clamped to MAX_COLUMNS/MAX_ROWS, far below u8::MAX.
        spans.insert(key.to_owned(), span.map(|n| n as u8));
    }
}

pub fn show(
    ui: &mut egui::Ui,
    view: View<'_>,
    arrangement: Arrangement<'_>,
    board: &mut TileBoard,
    main_hidden: bool,
) -> Shown {
    let View {
        history, enabled, ..
    } = view;
    let Arrangement {
        order,
        detached,
        spans,
    } = arrangement;
    let ctx = ui.ctx().clone();
    // Dropping a tile outside this (the top bar, the settings panel, or outside the
    // window) pops it out into its own window.
    let area = ui.max_rect();
    let latest = history.latest();

    let cards = tiles::collect(latest, enabled, view.drives, view.gpus);
    if cards.is_empty() {
        ui.centered_and_justified(|ui| {
            let msg = if !enabled.any(&MetricId::ALL) {
                "All metrics are turned off. Enable some in Settings."
            } else {
                "Collecting data…"
            };
            ui.label(RichText::new(msg).weak());
        });
        return Shown::default();
    }

    let keys: Vec<String> = cards.iter().map(Card::key).collect();
    detached.retain(|k, _| keys.contains(k));
    board.windows.retain(|k, _| detached.contains_key(k));
    reconcile_order(order, detached, &keys);
    // Only a tile that no longer exists ends a drag: a torn-off tile is legitimately
    // missing from the grid order while it's carried as a window.
    if board
        .grid_drag
        .as_ref()
        .is_some_and(|d| !keys.contains(&d.key))
    {
        board.grid_drag = None;
    }
    if board
        .window_drag
        .as_ref()
        .is_some_and(|d| !detached.contains_key(d.key()))
    {
        board.window_drag = None;
    }
    let by_key: HashMap<String, &Card<'_>> = cards.iter().map(|c| (c.key(), c)).collect();

    let pointer = ctx.input(|i| i.pointer.interact_pos());
    let over_dashboard = pointer.is_some_and(|p| area.contains(p));
    let spacing = ui.spacing().item_spacing;

    // A pop-out window being dragged over the main window: where it is over the grid.
    let dock = if main_hidden {
        None
    } else {
        popout::dock_pointer(board).filter(|(_, p)| area.contains(*p))
    };

    // The grid shows every docked tile except the lifted one, plus a gap where a
    // dragged tile would land. For a tile dragged inside the window: over the grid,
    // the gap opens where it would land; elsewhere in the window (top bar, margins,
    // settings) letting go puts it back, so the gap stays at its original slot
    // instead of the tiles reshuffling near the edges. Once it is carried outside as
    // a window, the grid closes up.
    let lifted: Option<String> = board.grid_drag.as_ref().map(|d| d.key.clone());
    let gap: Option<(String, usize)> = match (&board.grid_drag, &dock) {
        (Some(d), _) if !d.torn => {
            let at = if over_dashboard {
                d.slot
            } else {
                d.origin_slot
            };
            Some((d.key.clone(), at))
        }
        (None, Some((key, _))) => {
            Some((key.clone(), popout::dock_slot(board).unwrap_or(usize::MAX)))
        }
        _ => None,
    };
    let mut slots: Vec<Slot<'_>> = order
        .iter()
        .map(String::as_str)
        .filter(|k| lifted.as_deref() != Some(*k))
        .map(Slot::Tile)
        .collect();
    if let Some((key, at)) = &gap {
        slots.insert((*at).min(slots.len()), Slot::Empty(key));
    }

    let animating = board.grid_drag.is_some()
        || gap.is_some()
        || board.resize.is_some()
        || board.settle_until.is_some_and(|t| Instant::now() < t);
    let slide = if animating { SLIDE_SECS } else { 0.0 };

    let mut picked_up = None;
    let mut started_resize = None;
    let mut next_slot = None;
    let mut next_dock_slot = None;
    let mut grid_origin = area.min;
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            let origin = ui.cursor().min;
            grid_origin = origin;
            let width = ui.available_width();
            let slot_spans: Vec<Span> = slots.iter().map(|s| span_of(spans, s.key())).collect();
            let columns = grid::columns_for(width, slot_spans.iter().map(|s| s[0]).sum());
            let column_width = grid::column_width(width, columns);
            let cells: Vec<(Span, f32)> = slots
                .iter()
                .zip(&slot_spans)
                .map(|(s, &span)| {
                    let w = grid::span_width(column_width, span[0].min(columns));
                    let card = by_key[s.key()];
                    (span, tiles::min_height(card, latest, enabled, w, spacing))
                })
                .collect();
            let layout = Grid::new(origin, width, area.height(), &cells);
            ui.allocate_space(egui::vec2(width, layout.height()));

            if let Some(p) = pointer
                && over_dashboard
                && board.grid_drag.as_ref().is_some_and(|d| !d.torn)
            {
                next_slot = Some(layout.slot_at(p));
            }
            if let Some((_, p)) = &dock {
                next_dock_slot = Some(layout.slot_at(*p));
            }

            for (i, slot) in slots.iter().enumerate() {
                let Slot::Tile(key) = *slot else { continue };
                let target = layout.cell(i);
                let id = tile_id(key);
                // Animate the offset from the grid origin, not the screen position, so
                // tiles don't lag behind when the grid scrolls.
                let offset = target.min - origin;
                let x = ctx.animate_value_with_time(id.with("x"), offset.x, slide);
                let y = ctx.animate_value_with_time(id.with("y"), offset.y, slide);
                let w = ctx.animate_value_with_time(id.with("w"), target.width(), slide);
                let h = ctx.animate_value_with_time(id.with("h"), target.height(), slide);
                let rect = egui::Rect::from_min_size(origin + egui::vec2(x, y), egui::vec2(w, h));

                // Registered before the content: hover (graph tooltips) still reaches
                // the content, while a drag anywhere on the tile picks it up.
                let handle = ui.interact(rect, id, egui::Sense::click_and_drag());
                let mut tile_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(rect)
                        .layout(Layout::top_down(egui::Align::Min)),
                );
                tiles::draw(&mut tile_ui, by_key[key], view);

                // The resize grip goes on top of everything else in the tile.
                let grip = egui::Rect::from_min_max(rect.max - egui::vec2(GRIP, GRIP), rect.max);
                let grip_response = ui
                    .interact(grip, id.with("resize"), egui::Sense::drag())
                    .on_hover_cursor(egui::CursorIcon::ResizeNwSe);
                let resizing = board.resize.as_ref().is_some_and(|r| r.key == key);
                if resizing || (board.grid_drag.is_none() && ui.rect_contains_pointer(rect)) {
                    paint_grip(
                        ui.painter(),
                        grip,
                        view.palette,
                        resizing || grip_response.hovered(),
                    );
                }
                if grip_response.drag_started() && board.resize.is_none() {
                    let p = ctx
                        .input(|i| i.pointer.press_origin())
                        .or(grip_response.interact_pointer_pos())
                        .unwrap_or(target.max);
                    started_resize = Some(Resize {
                        key: key.to_owned(),
                        anchor: target.min - origin,
                        corner_offset: target.max - p,
                        step: layout.step(i),
                        columns: layout.columns(),
                        original: slot_spans[i],
                    });
                }

                if handle.drag_started() && picked_up.is_none() {
                    // Where the button went down, not where the drag was recognised a
                    // few pixels later, so the tile stays exactly under the grab point.
                    let p = ctx
                        .input(|i| i.pointer.press_origin())
                        .or(handle.interact_pointer_pos())
                        .unwrap_or(rect.min);
                    picked_up = Some(GridDrag {
                        key: key.to_owned(),
                        grab: p - rect.min,
                        size: rect.size(),
                        slot: i,
                        origin_slot: i,
                        torn: false,
                    });
                }
            }
        });

    if let Some(d) = picked_up {
        board.grid_drag = Some(d);
    } else if let (Some(d), Some(slot)) = (board.grid_drag.as_mut(), next_slot) {
        d.slot = slot;
    }
    popout::set_dock_slot(board, next_dock_slot);
    if let Some(r) = started_resize {
        board.resize = Some(r);
    }
    if let Some(r) = board.resize.take() {
        if !keys.contains(&r.key) {
            // The tile went away mid-resize.
        } else if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            set_span(spans, &r.key, r.original);
            settle_slide(&ctx, board, &r.key, None, grid_origin);
        } else if ctx.input(|i| i.pointer.primary_down()) {
            if let Some(p) = pointer {
                let corner = p + r.corner_offset;
                let span = grid::span_for(grid_origin + r.anchor, corner, r.step, r.columns);
                set_span(spans, &r.key, span);
            }
            ctx.set_cursor_icon(egui::CursorIcon::ResizeNwSe);
            ctx.request_repaint();
            board.resize = Some(r);
        } else {
            settle_slide(&ctx, board, &r.key, None, grid_origin);
        }
    }

    let window = ctx.content_rect();
    let held = board
        .grid_drag
        .as_ref()
        .map(|d| (d.key.clone(), d.grab, d.size, d.slot, d.origin_slot, d.torn));
    if let Some((key, grab, size, slot, origin_slot, mut torn)) = held {
        let c = by_key.get(&key).copied();
        let cancelled = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let outside_window = pointer.is_none_or(|p| !window.contains(p));
        let restore = |order: &mut Vec<String>| drop_at(order, &key, origin_slot);

        if cancelled {
            board.grid_drag = None;
            if torn {
                detached.remove(&key);
                board.windows.remove(&key);
            }
            restore(order);
            settle_slide(&ctx, board, &key, None, grid_origin);
        } else if ctx.input(|i| i.pointer.primary_down()) {
            ctx.request_repaint();
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
            let back_inside = pointer.is_some_and(|p| window.shrink(REENTRY_MARGIN).contains(p));
            if !torn && outside_window {
                torn = true;
                order.retain(|k| k != &key);
                popout::tear_off(&ctx, board, detached, &key, grab, size, pointer);
                if let Some(t) = detached.get_mut(&key) {
                    t.home = Some(origin_slot);
                }
            } else if torn && back_inside {
                torn = false;
                detached.remove(&key);
                board.windows.remove(&key);
            }
            if let Some(d) = board.grid_drag.as_mut() {
                d.torn = torn;
            }
            match (c, torn, pointer) {
                (Some(c), true, _) => popout::carry_torn(&ctx, board, detached, c, grab, pointer),
                (Some(c), false, Some(p)) => show_lifted(&ctx, c, &key, p - grab, size, view),
                _ => {}
            }
        } else {
            board.grid_drag = None;
            let main = popout::main_window_rect(&ctx, board);
            if torn {
                if let Some(c) = c {
                    popout::settle(&ctx, board, detached, c, main);
                }
            } else if over_dashboard {
                // Start the slide from where it was let go, so it glides into its slot.
                drop_at(order, &key, slot);
                settle_slide(&ctx, board, &key, pointer.map(|p| p - grab), grid_origin);
            } else if outside_window {
                // Released outside before any frame saw it leave: pop out right here.
                order.retain(|k| k != &key);
                popout::tear_off(&ctx, board, detached, &key, grab, size, pointer);
                if let Some(t) = detached.get_mut(&key) {
                    t.home = Some(origin_slot);
                }
                if let Some(c) = c {
                    popout::settle(&ctx, board, detached, c, main);
                }
            } else {
                // Over the top bar or settings panel: back to where it came from.
                restore(order);
                settle_slide(&ctx, board, &key, pointer.map(|p| p - grab), grid_origin);
            }
        }
    }

    let torn = board
        .grid_drag
        .as_ref()
        .filter(|d| d.torn)
        .map(|d| d.key.clone());
    let (redock, restore_main) = popout::show_windows(
        &ctx,
        view,
        detached,
        board,
        &by_key,
        torn.as_deref(),
        main_hidden,
    );
    for back in redock {
        board.windows.remove(&back.key);
        return_to_grid(order, detached, &back.key, back.slot);
        settle_slide(&ctx, board, &back.key, back.from, grid_origin);
    }
    let busy = torn.or_else(|| board.window_drag.as_ref().map(|d| d.key().to_owned()));
    popout::place_windows(&ctx, detached, board, &by_key, busy.as_deref());
    Shown { restore_main }
}

/// Three short diagonal lines in a tile's bottom-right corner, the usual "drag here
/// to resize" mark; accent-coloured while hovered or in use.
fn paint_grip(painter: &egui::Painter, grip: egui::Rect, palette: &theme::Palette, active: bool) {
    let color = if active {
        palette.accent
    } else {
        palette.text_dim.gamma_multiply(0.7)
    };
    let stroke = egui::Stroke::new(1.5, color);
    let corner = grip.max - egui::vec2(5.0, 5.0);
    for d in [3.0, 7.0, 11.0] {
        painter.line_segment(
            [corner - egui::vec2(d, 0.0), corner - egui::vec2(0.0, d)],
            stroke,
        );
    }
}

/// Keeps tiles sliding for a moment after a drop; if `from` is given, the dropped
/// tile's slide starts there instead of its old slot.
fn settle_slide(
    ctx: &egui::Context,
    board: &mut TileBoard,
    key: &str,
    from: Option<egui::Pos2>,
    grid_origin: egui::Pos2,
) {
    if let Some(from) = from {
        let offset = from - grid_origin;
        let id = tile_id(key);
        ctx.animate_value_with_time(id.with("x"), offset.x, 0.0);
        ctx.animate_value_with_time(id.with("y"), offset.y, 0.0);
    }
    board.settle_until = Some(Instant::now() + Duration::from_secs_f32(SLIDE_SECS + 0.1));
    ctx.request_repaint();
}

/// The picked-up tile, live, drawn above everything at the pointer.
fn show_lifted(
    ctx: &egui::Context,
    c: &Card<'_>,
    key: &str,
    pos: egui::Pos2,
    size: egui::Vec2,
    view: View<'_>,
) {
    egui::Area::new(tile_id(key).with("lifted"))
        .order(egui::Order::Tooltip)
        .interactable(false)
        // Areas are kept inside the window by default, which pinned the tile to the edge
        // while the cursor moved on, and made it jump once it popped out. Let it follow
        // the cursor past the edge (clipped), so the pop-out appears right where it is.
        .constrain(false)
        .fixed_pos(pos)
        .show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
            let mut tile_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(rect)
                    .layout(Layout::top_down(egui::Align::Min)),
            );
            tiles::draw(&mut tile_ui, c, view);
        });
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
            home: None,
        }
    }

    #[test]
    fn drop_at_inserts_among_the_other_tiles() {
        let mut order = v(&["cpu", "memory", "net"]);
        drop_at(&mut order, "cpu", 1);
        assert_eq!(order, v(&["memory", "cpu", "net"]));
        drop_at(&mut order, "net", 0);
        assert_eq!(order, v(&["net", "memory", "cpu"]));
        drop_at(&mut order, "net", 99);
        assert_eq!(order, v(&["memory", "cpu", "net"]));
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

    /// Drives `show` through egui's real input pipeline with synthetic pointer events.
    struct Harness {
        ctx: egui::Context,
        time: f64,
        size: egui::Vec2,
        history: History,
        order: Vec<String>,
        detached: BTreeMap<String, DetachedTile>,
        spans: BTreeMap<String, [u8; 2]>,
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
                    ..Default::default()
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
                spans: BTreeMap::new(),
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
            let (library, _) = theme::Library::load(None);
            let palette = library.get(theme::DEFAULT_KEY).spec.palette();
            let mut output = self.ctx.run_ui(raw, |ui| {
                let (drives, gpus) = (BTreeMap::new(), BTreeMap::new());
                let view = View {
                    history: &self.history,
                    enabled,
                    palette: &palette,
                    drives: &drives,
                    gpus: &gpus,
                };
                let arrangement = Arrangement {
                    order: &mut self.order,
                    detached: &mut self.detached,
                    spans: &mut self.spans,
                };
                show(ui, view, arrangement, &mut self.board, false);
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

        fn glide(&mut self, from: egui::Pos2, to: egui::Pos2) {
            for step in 1..=20 {
                let p = from + (to - from) * (step as f32 / 20.0);
                self.frame(vec![egui::Event::PointerMoved(p)]);
            }
            for _ in 0..5 {
                self.frame(vec![egui::Event::PointerMoved(to)]);
            }
        }

        /// Press at `from`, glide to `to` over several frames, then release there.
        fn escape(&mut self) {
            self.frame(vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::default(),
            }]);
        }

        /// Where a tile was drawn last frame.
        fn tile_rect(&self, key: &str) -> egui::Rect {
            self.ctx.read_response(tile_id(key)).unwrap().rect
        }

        fn drag(&mut self, from: egui::Pos2, to: egui::Pos2) {
            self.frame(vec![egui::Event::PointerMoved(from)]);
            self.button(from, true);
            self.glide(from, to);
            self.button(to, false);
            self.frame(vec![]);
        }
    }

    // 1000x800 with 3 tiles: 2 columns of 494px, 2 rows of 394px sharing the height.
    const SCREEN: egui::Vec2 = egui::vec2(1000.0, 800.0);
    const CELL_0: egui::Pos2 = egui::pos2(100.0, 200.0);
    const CELL_1: egui::Pos2 = egui::pos2(600.0, 200.0);
    const CELL_2: egui::Pos2 = egui::pos2(100.0, 600.0);

    #[test]
    fn e2e_drag_from_the_graph_area_moves_the_tile() {
        // Regression: drags that started on the graph were swallowed by the plot.
        let mut h = Harness::new(SCREEN);
        h.drag(CELL_0, CELL_1);
        assert_eq!(h.order, v(&["memory", "cpu", "net:eth0"]));
        assert!(h.detached.is_empty());
        assert!(h.board.grid_drag.is_none());
    }

    #[test]
    fn e2e_backward_and_cross_row_drags_reflow_the_rest() {
        let mut h = Harness::new(SCREEN);
        h.drag(CELL_1, CELL_0);
        assert_eq!(h.order, v(&["memory", "cpu", "net:eth0"]));
        h.drag(CELL_2, CELL_0);
        assert_eq!(h.order, v(&["net:eth0", "memory", "cpu"]));
    }

    #[test]
    fn e2e_dropping_on_empty_end_of_grid_moves_tile_last() {
        let mut h = Harness::new(SCREEN);
        h.drag(CELL_0, egui::pos2(600.0, 600.0));
        assert_eq!(h.order, v(&["memory", "net:eth0", "cpu"]));
    }

    #[test]
    fn e2e_escape_cancels_the_drag() {
        let mut h = Harness::new(SCREEN);
        h.frame(vec![egui::Event::PointerMoved(CELL_0)]);
        h.button(CELL_0, true);
        for step in 1..=10 {
            let p = CELL_0 + (CELL_1 - CELL_0) * (step as f32 / 10.0);
            h.frame(vec![egui::Event::PointerMoved(p)]);
        }
        h.frame(vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        }]);
        h.button(CELL_1, false);
        assert_eq!(h.order, v(&["cpu", "memory", "net:eth0"]));
    }

    #[test]
    fn e2e_tile_tears_off_at_the_window_edge_and_docks_again_on_return() {
        let mut h = Harness::new(SCREEN);
        h.frame(vec![egui::Event::PointerMoved(CELL_0)]);
        h.button(CELL_0, true);
        let outside = egui::pos2(1200.0, 200.0);
        h.glide(CELL_0, outside);
        // Still holding it: already its own window, and the grid has closed the gap.
        assert!(h.detached.contains_key("cpu"));
        assert_eq!(h.order, v(&["memory", "net:eth0"]));
        h.glide(outside, CELL_1);
        assert!(h.detached.is_empty());
        h.button(CELL_1, false);
        assert_eq!(h.order, v(&["memory", "cpu", "net:eth0"]));
    }

    #[test]
    fn e2e_escape_while_torn_off_puts_the_tile_back() {
        let mut h = Harness::new(SCREEN);
        h.frame(vec![egui::Event::PointerMoved(CELL_1)]);
        h.button(CELL_1, true);
        h.glide(CELL_1, egui::pos2(1200.0, 200.0));
        assert!(h.detached.contains_key("memory"));
        h.frame(vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        }]);
        assert!(h.detached.is_empty());
        assert_eq!(h.order, v(&["cpu", "memory", "net:eth0"]));
    }

    #[test]
    fn e2e_dropping_outside_the_window_pops_the_tile_out() {
        let mut h = Harness::new(SCREEN);
        h.drag(CELL_0, egui::pos2(1200.0, 200.0));
        assert_eq!(h.order, v(&["memory", "net:eth0"]));
        let tile = &h.detached["cpu"];
        assert_eq!(
            tile.size,
            [(1000.0 - grid::GAP) / 2.0, (800.0 - grid::GAP) / 2.0]
        );
    }

    #[test]
    fn return_to_grid_prefers_the_drop_slot_then_home_then_the_end() {
        let with_home =
            |home| BTreeMap::from([("cpu".to_string(), DetachedTile { home, ..tile() })]);
        let mut order = v(&["memory", "net"]);
        return_to_grid(&mut order, &mut with_home(Some(0)), "cpu", None);
        assert_eq!(order, v(&["cpu", "memory", "net"]));

        let mut order = v(&["memory", "net"]);
        let mut detached = with_home(Some(0));
        return_to_grid(&mut order, &mut detached, "cpu", Some(1));
        assert_eq!(order, v(&["memory", "cpu", "net"]));
        assert!(detached.is_empty());

        let mut order = v(&["memory", "net"]);
        return_to_grid(&mut order, &mut with_home(None), "cpu", None);
        assert_eq!(order, v(&["memory", "net", "cpu"]));
    }

    #[test]
    fn e2e_dragged_tile_follows_the_cursor_past_the_window_edge() {
        // Regression: the lifted tile was kept inside the window, so it stopped at
        // the edge while the cursor went on, then jumped when it popped out.
        let mut h = Harness::new(SCREEN);
        h.frame(vec![egui::Event::PointerMoved(CELL_0)]);
        h.button(CELL_0, true);
        h.glide(CELL_0, egui::pos2(5.0, 200.0));
        let lifted = h
            .ctx
            .memory(|m| m.area_rect(tile_id("cpu").with("lifted")))
            .unwrap();
        // Grabbed 100px from its left edge, so its left edge is 95px past the window's.
        assert_eq!(lifted.min.x, 5.0 - 100.0);
    }

    #[test]
    fn e2e_tear_off_remembers_where_the_tile_came_from() {
        let mut h = Harness::new(SCREEN);
        h.drag(CELL_1, egui::pos2(1200.0, 200.0));
        assert_eq!(h.detached["memory"].home, Some(1));
        return_to_grid(&mut h.order, &mut h.detached, "memory", None);
        assert_eq!(h.order, v(&["cpu", "memory", "net:eth0"]));
    }

    #[test]
    fn e2e_pop_out_dragged_over_the_grid_opens_a_gap_and_docks_there() {
        let mut h = Harness::new(SCREEN);
        h.order.retain(|k| k != "net:eth0");
        h.detached.insert("net:eth0".into(), tile());
        let main = crate::placement::Rect {
            x: 0.0,
            y: 0.0,
            w: SCREEN.x,
            h: SCREEN.y,
        };
        h.board.window_drag = Some(popout::test_window_drag("net:eth0", main, [100.0, 200.0]));
        // The button is held (in the pop-out) and the cursor is over the first cell.
        h.button(egui::pos2(990.0, 790.0), true);
        for _ in 0..30 {
            h.frame(vec![]);
        }
        // The first tile made way: it slid into the second cell.
        assert_eq!(h.tile_rect("cpu").min, egui::pos2(506.0, 0.0));
        assert_eq!(popout::dock_slot(&h.board), Some(0));
        h.button(egui::pos2(990.0, 790.0), false);
        assert!(h.detached.is_empty());
        assert_eq!(h.order, v(&["net:eth0", "cpu", "memory"]));
    }

    // CPU is cell 0 (0,0)-(494,394); its grip is the 16px square in that corner.
    const CPU_GRIP: egui::Pos2 = egui::pos2(488.0, 388.0);

    #[test]
    fn e2e_dragging_the_corner_grip_resizes_the_tile_to_whole_cells() {
        let mut h = Harness::new(SCREEN);
        // One column to the right (506px per column): the tile spans both columns.
        h.drag(CPU_GRIP, CPU_GRIP + egui::vec2(506.0, 0.0));
        assert_eq!(h.spans.get("cpu"), Some(&[2, 1]));
        // It was resized, not moved.
        assert_eq!(h.order, v(&["cpu", "memory", "net:eth0"]));
        assert!(h.board.grid_drag.is_none() && h.board.resize.is_none());
        for _ in 0..30 {
            h.frame(vec![]);
        }
        assert_eq!(h.tile_rect("cpu").width(), 1000.0);
        // The others moved down a row.
        assert!(h.tile_rect("memory").min.y > h.tile_rect("cpu").max.y);

        // Back to one cell: the entry is removed rather than stored as [1, 1].
        let grip = h.tile_rect("cpu").max - egui::vec2(6.0, 6.0);
        h.drag(grip, grip - egui::vec2(506.0, 0.0));
        assert!(h.spans.is_empty());
    }

    #[test]
    fn e2e_escape_cancels_a_resize() {
        let mut h = Harness::new(SCREEN);
        h.frame(vec![egui::Event::PointerMoved(CPU_GRIP)]);
        h.button(CPU_GRIP, true);
        h.glide(CPU_GRIP, CPU_GRIP + egui::vec2(506.0, 406.0));
        assert_eq!(h.spans.get("cpu"), Some(&[2, 2]));
        h.escape();
        h.button(CPU_GRIP, false);
        assert!(h.spans.is_empty());
    }
}
