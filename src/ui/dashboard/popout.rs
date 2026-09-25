//! Tiles popped out of the grid into their own OS windows.
//!
//! Moving our own windows is platform-specific, so it goes through the few helpers at
//! the top of this file:
//! - Hyprland: its IPC socket gives the global cursor and moves/floats windows.
//! - X11 and Windows: eframe reports window positions and can set them.
//! - Other Wayland desktops: apps may not position windows; the compositor places them
//!   and moving a pop-out is handed to the compositor (`StartDrag`).

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use eframe::egui;

use super::tiles::{self, Card};
use super::{TileBoard, own_pid, tile_id};
use crate::config::DetachedTile;
use crate::hypr;
use crate::main_window::APP_ID as MAIN_APP_ID;
use crate::placement::{self, Rect};
use crate::ui::theme;
use crate::ui::widgets::CARD_INSET;

/// App id (Wayland) / class (X11, Hyprland) of popped-out tile windows.
const DETACHED_APP_ID: &str = "hyperion-tile";
/// Poll rate while waiting for a new pop-out window to appear so it can be placed.
/// Short: until placed, other window rules (e.g. "center floating windows") may have
/// put it somewhere else.
const PLACEMENT_POLL: Duration = Duration::from_millis(30);
/// Poll rate for picking up moves/resizes done with the compositor's own controls.
const SYNC_POLL: Duration = Duration::from_secs(2);

/// A pop-out window being moved by dragging it.
pub(super) struct WindowDrag {
    key: String,
    /// Cursor minus window top-left at drag start, in global logical pixels.
    grab: [f32; 2],
    /// The main window, for "dropped back onto it" (it doesn't move mid-drag).
    main: Option<Rect>,
    /// Global position of the main window's content (its egui coordinates' origin).
    origin: Option<[f32; 2]>,
    /// Latest global cursor position.
    cursor: Option<[f32; 2]>,
    /// Grid slot under the cursor while it is over the main window's grid.
    dock_slot: Option<usize>,
}

impl WindowDrag {
    pub(super) fn key(&self) -> &str {
        &self.key
    }
}

/// While a pop-out is dragged over the main window: its key and the cursor in the
/// main window's coordinates, so the grid can open a gap there.
pub(super) fn dock_pointer(board: &TileBoard) -> Option<(String, egui::Pos2)> {
    let d = board.window_drag.as_ref()?;
    let cursor = d.cursor?;
    if !d.main?.contains(cursor) {
        return None;
    }
    let origin = d.origin?;
    Some((
        d.key.clone(),
        egui::pos2(cursor[0] - origin[0], cursor[1] - origin[1]),
    ))
}

pub(super) fn dock_slot(board: &TileBoard) -> Option<usize> {
    board.window_drag.as_ref()?.dock_slot
}

pub(super) fn set_dock_slot(board: &mut TileBoard, slot: Option<usize>) {
    if let Some(d) = board.window_drag.as_mut() {
        d.dock_slot = slot;
    }
}

#[cfg(test)]
pub(super) fn test_window_drag(key: &str, main: Rect, cursor: [f32; 2]) -> WindowDrag {
    WindowDrag {
        key: key.to_owned(),
        grab: [10.0, 10.0],
        main: Some(main),
        origin: Some([main.x, main.y]),
        cursor: Some(cursor),
        dock_slot: None,
    }
}

/// A pop-out going back into the grid.
pub(super) struct Redock {
    pub key: String,
    /// Grid slot it was dropped on; `None` means where it was torn off from.
    pub slot: Option<usize>,
    /// Where its window was, in main-window coordinates, so it glides in from there.
    pub from: Option<egui::Pos2>,
}

#[derive(Default)]
pub(super) struct WindowState {
    rule_registered: bool,
    address: Option<String>,
    placed: bool,
}

pub(super) fn viewport_id(key: &str) -> egui::ViewportId {
    egui::ViewportId::from_hash_of(("hyperion-detached", key))
}

fn window_title(tile_title: &str) -> String {
    format!("Hyperion — {tile_title}")
}

fn to_rect(r: egui::Rect) -> Rect {
    Rect {
        x: r.min.x,
        y: r.min.y,
        w: r.width(),
        h: r.height(),
    }
}

// ---------------------------------------------------------------------------------
// Platform helpers

/// The cursor in global screen coordinates, given its position `local` to the window
/// `viewport`. `None` on desktops that won't tell us (plain Wayland).
fn global_cursor(
    ctx: &egui::Context,
    board: &TileBoard,
    viewport: egui::ViewportId,
    local: Option<egui::Pos2>,
) -> Option<[f32; 2]> {
    if let Some(h) = &board.hypr {
        return h.cursor_pos();
    }
    let inner = ctx.input_for(viewport, |i| i.viewport().inner_rect)?;
    let global = inner.min + local?.to_vec2();
    Some([global.x, global.y])
}

/// The main window's screen rectangle, where the platform reports it.
pub(super) fn main_window_rect(ctx: &egui::Context, board: &TileBoard) -> Option<Rect> {
    if let Some(h) = &board.hypr {
        return h
            .clients()
            .iter()
            .find(|c| c.pid == own_pid() && c.class == MAIN_APP_ID)
            .map(hypr::Client::rect);
    }
    ctx.input_for(egui::ViewportId::ROOT, |i| i.viewport().outer_rect)
        .map(to_rect)
}

/// Global position of the main window's content area, where its egui coordinates
/// start. On Hyprland windows have no title bar, so it is the window's position.
fn main_content_origin(
    ctx: &egui::Context,
    board: &TileBoard,
    main: Option<Rect>,
) -> Option<[f32; 2]> {
    if board.hypr.is_some() {
        return main.map(|m| [m.x, m.y]);
    }
    ctx.input_for(egui::ViewportId::ROOT, |i| i.viewport().inner_rect)
        .map(|r| [r.min.x, r.min.y])
}

/// Hyprland address of a pop-out, looked up by its window title until found.
fn hypr_address(board: &mut TileBoard, key: &str, title: &str) -> Option<String> {
    let h = board.hypr.as_ref()?;
    let state = board.windows.entry(key.to_owned()).or_default();
    if state.address.is_none() {
        state.address = h
            .clients()
            .into_iter()
            .find(|c| c.pid == own_pid() && c.title == title)
            .map(|c| c.address);
    }
    state.address.clone()
}

fn move_window(
    ctx: &egui::Context,
    board: &mut TileBoard,
    key: &str,
    title: &str,
    pos: [f32; 2],
    size: [f32; 2],
) {
    if board.hypr.is_some() {
        if let Some(address) = hypr_address(board, key, title)
            && let Some(h) = &board.hypr
        {
            h.move_to(&address, pos, size);
        }
        return;
    }
    // Honored on X11 and Windows; Wayland compositors ignore it.
    ctx.send_viewport_cmd_to(
        viewport_id(key),
        egui::ViewportCommand::OuterPosition(egui::pos2(pos[0], pos[1])),
    );
}

/// Screen areas (minus bars) and our other windows, for snapping a released window.
fn surroundings(
    board: &TileBoard,
    detached: &BTreeMap<String, DetachedTile>,
    except: &str,
    main: Option<Rect>,
) -> (Vec<Rect>, Vec<Rect>) {
    if let Some(h) = &board.hypr {
        let skip = board.windows.get(except).and_then(|w| w.address.as_deref());
        let screens = h.monitors().iter().map(hypr::Monitor::usable).collect();
        let others = h
            .clients()
            .iter()
            .filter(|c| {
                c.pid == own_pid()
                    && Some(c.address.as_str()) != skip
                    && (c.class == MAIN_APP_ID || c.class == DETACHED_APP_ID)
            })
            .map(hypr::Client::rect)
            .collect();
        return (screens, others);
    }
    // Elsewhere we only know our own windows, and not the screen edges.
    let others = detached
        .iter()
        .filter(|(k, _)| k.as_str() != except)
        .filter_map(|(_, t)| {
            t.pos.map(|[x, y]| Rect {
                x,
                y,
                w: t.size[0],
                h: t.size[1],
            })
        })
        .chain(main)
        .collect();
    (Vec::new(), others)
}

// ---------------------------------------------------------------------------------
// Tearing a tile off the grid

/// Turns a tile held in the grid into its own window under the cursor. `local` is
/// the pointer relative to the main window; `grab` is where the tile was held.
pub(super) fn tear_off(
    ctx: &egui::Context,
    board: &mut TileBoard,
    detached: &mut BTreeMap<String, DetachedTile>,
    key: &str,
    grab: egui::Vec2,
    size: egui::Vec2,
    local: Option<egui::Pos2>,
) {
    let pos = global_cursor(ctx, board, egui::ViewportId::ROOT, local)
        .map(|c| [c[0] - grab.x, c[1] - grab.y]);
    board.windows.remove(key);
    detached.insert(
        key.to_owned(),
        DetachedTile {
            pos,
            size: [size.x, size.y],
            home: None,
        },
    );
}

/// Keeps a torn-off tile's window under the cursor while the button is still held.
pub(super) fn carry_torn(
    ctx: &egui::Context,
    board: &mut TileBoard,
    detached: &mut BTreeMap<String, DetachedTile>,
    c: &Card<'_>,
    grab: egui::Vec2,
    local: Option<egui::Pos2>,
) {
    let key = c.key();
    let Some(cursor) = global_cursor(ctx, board, egui::ViewportId::ROOT, local) else {
        return;
    };
    let pos = [cursor[0] - grab.x, cursor[1] - grab.y];
    let Some(tile) = detached.get_mut(&key) else {
        return;
    };
    if tile.pos == Some(pos) {
        return;
    }
    tile.pos = Some(pos);
    let size = tile.size;
    // Placement polling would snap it mid-drag; we are positioning it ourselves.
    board.windows.entry(key.clone()).or_default().placed = true;
    move_window(ctx, board, &key, &window_title(&c.title()), pos, size);
}

/// Where a released pop-out ends up: off any window it landed on, then gently snapped.
pub(super) fn settle(
    ctx: &egui::Context,
    board: &mut TileBoard,
    detached: &mut BTreeMap<String, DetachedTile>,
    c: &Card<'_>,
    main: Option<Rect>,
) {
    let key = c.key();
    let Some(tile) = detached.get(&key) else {
        return;
    };
    let Some([x, y]) = tile.pos else {
        return;
    };
    let win = Rect {
        x,
        y,
        w: tile.size[0],
        h: tile.size[1],
    };
    let (screens, others) = surroundings(board, detached, &key, main);
    let pos = placement::settle(win, &screens, &others);
    if let Some(tile) = detached.get_mut(&key) {
        tile.pos = Some(pos);
    }
    if pos != [x, y] {
        move_window(
            ctx,
            board,
            &key,
            &window_title(&c.title()),
            pos,
            [win.w, win.h],
        );
    }
}

// ---------------------------------------------------------------------------------
// Pop-out windows

/// Renders each popped-out tile in its own OS window and handles moving it. Returns
/// the tiles to put back in the grid (closed, or dropped on the main window).
pub(super) fn show_windows(
    ctx: &egui::Context,
    view: super::View<'_>,
    detached: &mut BTreeMap<String, DetachedTile>,
    board: &mut TileBoard,
    by_key: &HashMap<String, &Card<'_>>,
    torn: Option<&str>,
    main_hidden: bool,
) -> (Vec<Redock>, bool) {
    let palette = view.palette;
    let mut redock = Vec::new();
    let mut restore_main = false;
    let keys: Vec<String> = detached.keys().cloned().collect();
    for key in keys {
        let Some(c) = by_key.get(&key).copied() else {
            continue;
        };
        let Some(tile) = detached.get(&key).cloned() else {
            continue;
        };
        let title = window_title(&c.title());
        let state = board.windows.entry(key.clone()).or_default();
        if let Some(h) = &board.hypr
            && !state.rule_registered
        {
            h.register_tile_rule(DETACHED_APP_ID, &title, tile.pos, tile.size);
            state.rule_registered = true;
        }
        let mut builder = egui::ViewportBuilder::default()
            .with_title(title.clone())
            .with_app_id(DETACHED_APP_ID)
            .with_inner_size(tile.size)
            .with_decorations(false);
        if let Some(pos) = tile.pos {
            // Honored on X11 and Windows; Wayland ignores it (Hyprland is placed via IPC).
            builder = builder.with_position(pos);
        }
        let viewport = viewport_id(&key);
        let id = tile_id(&key);

        let (drag_started, close, outer, primary_down, local, buttons) = ctx
            .show_viewport_immediate(viewport, builder, |ui, _class| {
                ui.painter()
                    .rect_filled(ui.max_rect(), 0, palette.bg_bottom);
                // Registered before the content so hover (graph tooltips) still reaches
                // the content, while drags anywhere on the tile move the window.
                let handle = ui.interact(ui.max_rect(), id, egui::Sense::click_and_drag());
                tiles::draw(ui, c, view);
                let hovered = ui
                    .input(|i| i.pointer.hover_pos())
                    .is_some_and(|p| ui.max_rect().contains(p));
                let buttons = if hovered {
                    window_buttons(ui, palette, id, main_hidden)
                } else {
                    Buttons::default()
                };
                ui.input(|i| {
                    (
                        handle.drag_started(),
                        i.viewport().close_requested(),
                        i.viewport().outer_rect,
                        i.pointer.primary_down(),
                        i.pointer.interact_pos(),
                        buttons,
                    )
                })
            });

        restore_main |= buttons.restore;
        if close || buttons.close {
            redock.push(Redock {
                key,
                slot: None,
                from: None,
            });
            continue;
        }
        // A tile being torn off is positioned by the grid drag, not here.
        if torn == Some(key.as_str()) {
            continue;
        }
        if let Some(r) = outer
            && board.hypr.is_none()
            && let Some(t) = detached.get_mut(&key)
        {
            t.pos = Some([r.min.x, r.min.y]);
        }

        if drag_started {
            let cursor = global_cursor(ctx, board, viewport, local);
            let at = if board.hypr.is_some() {
                let address = hypr_address(board, &key, &title);
                address.zip(board.hypr.as_ref()).and_then(|(address, h)| {
                    let at = h.clients().into_iter().find(|c| c.address == address)?.at;
                    Some([at[0] as f32, at[1] as f32])
                })
            } else {
                outer.map(|r| [r.min.x, r.min.y])
            };
            match cursor.zip(at) {
                Some((cursor, at)) => {
                    let main = main_window_rect(ctx, board);
                    board.window_drag = Some(WindowDrag {
                        key: key.clone(),
                        grab: [cursor[0] - at[0], cursor[1] - at[1]],
                        main,
                        origin: main_content_origin(ctx, board, main),
                        cursor: Some(cursor),
                        dock_slot: None,
                    });
                }
                // We can't position windows here: let the compositor move it.
                None => ctx.send_viewport_cmd_to(viewport, egui::ViewportCommand::StartDrag),
            }
        }

        let Some(drag) = board.window_drag.as_ref().filter(|d| d.key == key) else {
            continue;
        };
        let (grab, main, origin, dock_slot) = (drag.grab, drag.main, drag.origin, drag.dock_slot);
        // A failed lookup (e.g. one IPC hiccup) keeps the last known position.
        let cursor = global_cursor(ctx, board, viewport, local).or(drag.cursor);
        if let Some(d) = board.window_drag.as_mut() {
            d.cursor = cursor;
        }
        if primary_down {
            ctx.request_repaint();
            if let Some(cursor) = cursor {
                let pos = [cursor[0] - grab[0], cursor[1] - grab[1]];
                if let Some(t) = detached.get_mut(&key)
                    && t.pos != Some(pos)
                {
                    t.pos = Some(pos);
                    let size = t.size;
                    move_window(ctx, board, &key, &title, pos, size);
                }
            }
            continue;
        }

        // Released: back into the grid if dropped on the main window, else settle.
        board.window_drag = None;
        if !main_hidden
            && let Some(cursor) = cursor
            && main.is_some_and(|main| main.contains(cursor))
        {
            let from =
                origin.map(|o| egui::pos2(cursor[0] - grab[0] - o[0], cursor[1] - grab[1] - o[1]));
            redock.push(Redock {
                key,
                slot: dock_slot,
                from,
            });
        } else {
            settle(ctx, board, detached, c, main);
        }
    }
    (redock, restore_main)
}

#[derive(Clone, Copy, Default)]
struct Buttons {
    restore: bool,
    close: bool,
}

/// Hover controls in the top-right of a pop-out's title row: "bring back the main
/// window" (only while it is hidden) and "send this tile back". They sit on a small
/// pill so they read cleanly over the headline value underneath.
fn window_buttons(
    ui: &mut egui::Ui,
    palette: &theme::Palette,
    id: egui::Id,
    show_restore: bool,
) -> Buttons {
    const SIZE: f32 = 20.0;
    const SPACING: f32 = 6.0;
    let area = ui.max_rect();
    let center_y = area.top() + CARD_INSET + SIZE / 2.0;
    let close_center = egui::pos2(area.right() - CARD_INSET - SIZE / 2.0, center_y);
    let restore_center = close_center - egui::vec2(SIZE + SPACING, 0.0);
    let leftmost = if show_restore {
        restore_center
    } else {
        close_center
    };

    let pill = egui::Rect::from_min_max(
        egui::pos2(leftmost.x - SIZE / 2.0 - 5.0, center_y - SIZE / 2.0 - 4.0),
        egui::pos2(
            close_center.x + SIZE / 2.0 + 5.0,
            center_y + SIZE / 2.0 + 4.0,
        ),
    );
    let painter = ui.painter().clone();
    painter.rect(
        pill,
        pill.height() / 2.0,
        palette.well,
        egui::Stroke::new(1.0, palette.card_stroke),
        egui::StrokeKind::Inside,
    );

    let mut buttons = Buttons::default();
    let button = |ui: &mut egui::Ui, center: egui::Pos2, salt: &str| {
        let rect = egui::Rect::from_center_size(center, egui::vec2(SIZE, SIZE));
        ui.interact(rect, id.with(salt), egui::Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
    };

    if show_restore {
        let resp = button(ui, restore_center, "restore-main")
            .on_hover_text("Bring back the Hyperion main window");
        let fill = if resp.hovered() {
            palette.accent2
        } else {
            palette.accent
        };
        painter.circle_filled(restore_center, SIZE / 2.0, fill);
        // A tiny window glyph.
        let glyph = egui::Rect::from_center_size(restore_center, egui::vec2(9.0, 7.0));
        painter.rect_stroke(
            glyph,
            1.5,
            egui::Stroke::new(1.5, palette.bg_bottom),
            egui::StrokeKind::Middle,
        );
        painter.line_segment(
            [
                glyph.left_top() + egui::vec2(0.0, 2.0),
                glyph.right_top() + egui::vec2(0.0, 2.0),
            ],
            egui::Stroke::new(1.5, palette.bg_bottom),
        );
        buttons.restore = resp.clicked();
    }

    let resp = button(ui, close_center, "close-tile")
        .on_hover_text("Send this tile back to the Hyperion window");
    let (fill, cross) = if resp.hovered() {
        (
            egui::Color32::from_rgb(0xe0, 0x5a, 0x5a),
            egui::Color32::WHITE,
        )
    } else {
        (palette.card_stroke, palette.text)
    };
    painter.circle_filled(close_center, SIZE / 2.0, fill);
    let r = 3.5;
    let stroke = egui::Stroke::new(1.6, cross);
    painter.line_segment(
        [
            close_center + egui::vec2(-r, -r),
            close_center + egui::vec2(r, r),
        ],
        stroke,
    );
    painter.line_segment(
        [
            close_center + egui::vec2(-r, r),
            close_center + egui::vec2(r, -r),
        ],
        stroke,
    );
    buttons.close = resp.clicked();
    buttons
}

/// On Hyprland: floats and positions pop-out windows once they appear, then keeps the
/// saved position/size in sync with any moves made with the compositor's own controls.
pub(super) fn place_windows(
    ctx: &egui::Context,
    detached: &mut BTreeMap<String, DetachedTile>,
    board: &mut TileBoard,
    by_key: &HashMap<String, &Card<'_>>,
    busy: Option<&str>,
) {
    let Some(h) = board.hypr.as_ref() else {
        return;
    };
    if detached.is_empty() {
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
    let mut screens = None;
    for (key, tile) in detached.iter_mut() {
        // Being dragged right now: whoever is dragging it positions it.
        if busy == Some(key.as_str()) {
            continue;
        }
        let Some(c) = by_key.get(key) else { continue };
        let title = window_title(&c.title());
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
        let win = Rect {
            x: pos[0],
            y: pos[1],
            w: tile.size[0],
            h: tile.size[1],
        };
        let screens = screens.get_or_insert_with(|| {
            h.monitors()
                .iter()
                .map(hypr::Monitor::usable)
                .collect::<Vec<_>>()
        });
        let others: Vec<Rect> = clients
            .iter()
            .filter(|cl| {
                cl.pid == own_pid()
                    && cl.address != clients[i].address
                    && (cl.class == MAIN_APP_ID || cl.class == DETACHED_APP_ID)
            })
            .map(hypr::Client::rect)
            .collect();
        let settled = placement::settle(win, screens, &others);
        h.place(&clients[i], settled, tile.size);
        // Later windows in this same pass must see where this one went now.
        clients[i].at = settled.map(|v| v.round() as i32);
        clients[i].size = tile.size.map(|v| v.round() as i32);
        clients[i].floating = true;
        tile.pos = Some(settled);
        state.placed = true;
    }
}
