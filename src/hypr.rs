//! Minimal Hyprland IPC client. Wayland doesn't let an app position its own windows,
//! but Hyprland's control socket lets us ask the compositor to float and move them.

use std::cell::Cell;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;
use serde::de::DeserializeOwned;

/// How close (logical px) a dropped window must be to an edge before it snaps to it.
pub const SNAP_DISTANCE: f32 = 24.0;
/// Space kept between a snapped window and the edge or neighbour it snapped to.
pub const SNAP_GAP: f32 = 8.0;

#[derive(Clone, Debug, Deserialize)]
pub struct Client {
    pub address: String,
    pub pid: i64,
    pub class: String,
    pub title: String,
    pub at: [i32; 2],
    pub size: [i32; 2],
    #[serde(default)]
    pub floating: bool,
}

impl Client {
    pub fn rect(&self) -> Rect {
        Rect {
            x: self.at[0] as f32,
            y: self.at[1] as f32,
            w: self.size[0] as f32,
            h: self.size[1] as f32,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Monitor {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub scale: f32,
    /// Space taken by bars: `[left, top, right, bottom]`.
    #[serde(default)]
    pub reserved: [i32; 4],
}

impl Monitor {
    /// The part of the monitor not covered by bars, in global logical coordinates.
    pub fn usable(&self) -> Rect {
        let scale = if self.scale > 0.0 { self.scale } else { 1.0 };
        let [left, top, right, bottom] = self.reserved.map(|v| v as f32);
        Rect {
            x: self.x as f32 + left,
            y: self.y as f32 + top,
            w: self.width as f32 / scale - left - right,
            h: self.height as f32 / scale - top - bottom,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn contains(&self, p: [f32; 2]) -> bool {
        p[0] >= self.x && p[0] < self.x + self.w && p[1] >= self.y && p[1] < self.y + self.h
    }

    fn center(&self) -> [f32; 2] {
        [self.x + self.w / 2.0, self.y + self.h / 2.0]
    }
}

#[derive(Deserialize)]
struct CursorPos {
    x: i32,
    y: i32,
}

/// Hyprland 0.53+ takes dispatchers as Lua (`hl.dsp.window.move{...}`); older releases,
/// still shipped by some distros, use the `movewindowpixel`-style text commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dialect {
    Lua,
    Legacy,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Action {
    Float,
    Resize([i32; 2]),
    Move([i32; 2]),
}

fn lua_command(address: &str, actions: &[Action]) -> String {
    let window = format!("address:{address}");
    let calls: Vec<String> = actions
        .iter()
        .map(|a| match *a {
            Action::Float => format!(
                "hl.dispatch(hl.dsp.window.float({{ window = \"{window}\", action = \"set\" }}))"
            ),
            Action::Resize([w, h]) => format!(
                "hl.dispatch(hl.dsp.window.resize({{ window = \"{window}\", x = {w}, y = {h} }}))"
            ),
            Action::Move([x, y]) => format!(
                "hl.dispatch(hl.dsp.window.move({{ window = \"{window}\", x = {x}, y = {y} }}))"
            ),
        })
        .collect();
    format!("eval {}", calls.join(" "))
}

fn legacy_command(address: &str, actions: &[Action]) -> String {
    let calls: Vec<String> = actions
        .iter()
        .map(|a| match *a {
            Action::Float => format!("dispatch setfloating address:{address}"),
            Action::Resize([w, h]) => {
                format!("dispatch resizewindowpixel exact {w} {h},address:{address}")
            }
            Action::Move([x, y]) => {
                format!("dispatch movewindowpixel exact {x} {y},address:{address}")
            }
        })
        .collect();
    format!("[[BATCH]]{}", calls.join(";"))
}

fn regex_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if "\\.+*?()|[]{}^$".contains(ch) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

fn lua_string(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// A named rule (re-registering the same name just updates it) so the window maps
/// already floating at its final size and position.
fn lua_tile_rule(class: &str, title: &str, pos: Option<[i32; 2]>, size: [i32; 2]) -> String {
    let name = lua_string(&format!("{class}: {title}"));
    let class = lua_string(&format!("^({})$", regex_escape(class)));
    let title = lua_string(&format!("^({})$", regex_escape(title)));
    let [w, h] = size;
    let placement = pos.map_or(String::new(), |[x, y]| format!(", move = {{ {x}, {y} }}"));
    format!(
        "eval hl.window_rule({{ name = {name}, match = {{ class = {class}, title = {title} }}, \
         float = true, size = {{ {w}, {h} }}{placement} }})"
    )
}

fn legacy_tile_rule(class: &str, title: &str, size: [i32; 2]) -> String {
    let [w, h] = size;
    let title = regex_escape(title);
    format!(
        "[[BATCH]]keyword windowrulev2 float,class:^({class})$;\
         keyword windowrulev2 size {w} {h},title:^({title})$"
    )
}

/// Every command in a batch answers "ok"; anything else is an error message.
fn all_ok(reply: &str) -> bool {
    let mut parts = reply.split_whitespace().peekable();
    parts.peek().is_some() && parts.all(|p| p == "ok")
}

pub struct Hypr {
    socket: PathBuf,
    dialect: Cell<Option<Dialect>>,
}

impl Hypr {
    /// `None` when not running under Hyprland.
    pub fn detect() -> Option<Self> {
        let sig = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?;
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
        runtime
            .map(|dir| dir.join("hypr").join(&sig).join(".socket.sock"))
            .into_iter()
            // Hyprland before 0.40 kept its sockets in /tmp.
            .chain(std::iter::once(
                PathBuf::from("/tmp/hypr").join(&sig).join(".socket.sock"),
            ))
            .find(|p| p.exists())
            .map(|socket| Self {
                socket,
                dialect: Cell::new(None),
            })
    }

    fn request(&self, command: &str) -> io::Result<String> {
        let mut stream = UnixStream::connect(&self.socket)?;
        stream.set_read_timeout(Some(Duration::from_millis(250)))?;
        stream.set_write_timeout(Some(Duration::from_millis(250)))?;
        stream.write_all(command.as_bytes())?;
        let mut reply = String::new();
        stream.read_to_string(&mut reply)?;
        Ok(reply)
    }

    fn query<T: DeserializeOwned>(&self, what: &str) -> Option<T> {
        serde_json::from_str(&self.request(&format!("j/{what}")).ok()?).ok()
    }

    pub fn cursor_pos(&self) -> Option<[f32; 2]> {
        let c: CursorPos = self.query("cursorpos")?;
        Some([c.x as f32, c.y as f32])
    }

    pub fn clients(&self) -> Vec<Client> {
        self.query("clients").unwrap_or_default()
    }

    pub fn monitors(&self) -> Vec<Monitor> {
        self.query("monitors").unwrap_or_default()
    }

    /// Makes the next window with this class and title map floating at `size` (and at
    /// `pos`, where given), instead of tiled and then resized — egui's GL surface
    /// doesn't always follow a resize that lands right after the window first maps.
    pub fn register_tile_rule(
        &self,
        class: &str,
        title: &str,
        pos: Option<[f32; 2]>,
        size: [f32; 2],
    ) {
        let pos = pos.map(|p| p.map(|v| v.round() as i32));
        let size = size.map(|v| v.round() as i32);
        let lua = lua_tile_rule(class, title, pos, size);
        let legacy = legacy_tile_rule(class, title, size);
        self.run(|dialect| match dialect {
            Dialect::Lua => lua.clone(),
            Dialect::Legacy => legacy.clone(),
        });
    }

    /// Floats, sizes and positions `client`, sending only the steps it still needs.
    pub fn place(&self, client: &Client, pos: [f32; 2], size: [f32; 2]) {
        let pos = pos.map(|v| v.round() as i32);
        let size = size.map(|v| v.round() as i32);
        let mut actions = Vec::new();
        if !client.floating {
            actions.push(Action::Float);
        }
        if client.size != size {
            actions.push(Action::Resize(size));
        }
        if client.at != pos {
            actions.push(Action::Move(pos));
        }
        if !actions.is_empty() {
            self.window_actions(&client.address, &actions);
        }
    }

    pub fn move_to(&self, address: &str, pos: [f32; 2]) {
        self.window_actions(address, &[Action::Move(pos.map(|v| v.round() as i32))]);
    }

    fn window_actions(&self, address: &str, actions: &[Action]) {
        self.run(|dialect| match dialect {
            Dialect::Lua => lua_command(address, actions),
            Dialect::Legacy => legacy_command(address, actions),
        });
    }

    /// Sends the command in the dialect this Hyprland speaks, finding out on first use.
    fn run(&self, command: impl Fn(Dialect) -> String) {
        let send = |dialect| {
            self.request(&command(dialect))
                .is_ok_and(|reply| all_ok(&reply))
        };
        if let Some(known) = self.dialect.get() {
            send(known);
            return;
        }
        for dialect in [Dialect::Lua, Dialect::Legacy] {
            if send(dialect) {
                self.dialect.set(Some(dialect));
                return;
            }
        }
    }
}

/// Nudges `win` onto a nearby screen edge or neighbouring window, but only when it is
/// already within [`SNAP_DISTANCE`]; anything further away is left exactly where it is.
pub fn snap(win: Rect, monitors: &[Rect], others: &[Rect]) -> [f32; 2] {
    let center = win.center();
    let screen = monitors
        .iter()
        .find(|m| m.contains(center))
        .or_else(|| monitors.first());

    let mut xs = Vec::new();
    let mut ys = Vec::new();
    if let Some(s) = screen {
        xs.extend([s.x + SNAP_GAP, s.x + s.w - win.w - SNAP_GAP]);
        ys.extend([s.y + SNAP_GAP, s.y + s.h - win.h - SNAP_GAP]);
    }
    // Only neighbours that are actually close count: a window on another monitor must
    // not pull this one's top edge into line with it.
    let reach = SNAP_DISTANCE + SNAP_GAP;
    for o in others {
        let h_gap = (o.x - (win.x + win.w)).max(win.x - (o.x + o.w)).max(0.0);
        let v_gap = (o.y - (win.y + win.h)).max(win.y - (o.y + o.h)).max(0.0);
        if v_gap <= SNAP_DISTANCE && h_gap <= reach {
            // Side by side: sit next to it, and line up top or bottom edges.
            xs.extend([o.x + o.w + SNAP_GAP, o.x - win.w - SNAP_GAP]);
            ys.extend([o.y, o.y + o.h - win.h]);
        }
        if h_gap <= SNAP_DISTANCE && v_gap <= reach {
            // Stacked: sit above/below it, and line up left or right edges.
            ys.extend([o.y + o.h + SNAP_GAP, o.y - win.h - SNAP_GAP]);
            xs.extend([o.x, o.x + o.w - win.w]);
        }
    }

    let nearest = |value: f32, candidates: &[f32]| {
        candidates
            .iter()
            .copied()
            .filter(|c| (c - value).abs() <= SNAP_DISTANCE)
            .min_by(|a, b| (a - value).abs().total_cmp(&(b - value).abs()))
            .unwrap_or(value)
    };
    [nearest(win.x, &xs), nearest(win.y, &ys)]
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect {
        x: 0.0,
        y: 35.0,
        w: 1920.0,
        h: 1045.0,
    };

    fn win(x: f32, y: f32) -> Rect {
        Rect {
            x,
            y,
            w: 400.0,
            h: 200.0,
        }
    }

    #[test]
    fn far_from_edges_stays_put() {
        assert_eq!(snap(win(500.0, 400.0), &[SCREEN], &[]), [500.0, 400.0]);
    }

    #[test]
    fn near_screen_corner_snaps_with_gap() {
        assert_eq!(snap(win(15.0, 50.0), &[SCREEN], &[]), [8.0, 43.0]);
        let right = 1920.0 - 400.0 - SNAP_GAP;
        assert_eq!(
            snap(win(right - 10.0, 400.0), &[SCREEN], &[]),
            [right, 400.0]
        );
    }

    #[test]
    fn snaps_beside_a_neighbour_and_aligns_tops() {
        let other = Rect {
            x: 100.0,
            y: 300.0,
            w: 400.0,
            h: 200.0,
        };
        let dropped = win(100.0 + 400.0 + 20.0, 310.0);
        assert_eq!(snap(dropped, &[SCREEN], &[other]), [508.0, 300.0]);
    }

    #[test]
    fn snaps_below_a_neighbour_and_aligns_left() {
        let other = Rect {
            x: 700.0,
            y: 100.0,
            w: 400.0,
            h: 200.0,
        };
        let dropped = win(690.0, 100.0 + 200.0 + 15.0);
        assert_eq!(snap(dropped, &[SCREEN], &[other]), [700.0, 308.0]);
    }

    #[test]
    fn distant_window_does_not_pull_edges_into_line() {
        // Regression: a pop-out on the other monitor used to drag this one's top edge.
        let far = Rect {
            x: 1935.0,
            y: 110.0,
            w: 460.0,
            h: 230.0,
        };
        assert_eq!(snap(win(490.0, 120.0), &[SCREEN], &[far]), [490.0, 120.0]);
    }

    #[test]
    fn usable_area_excludes_bars_and_applies_scale() {
        let m = Monitor {
            x: 1920,
            y: 0,
            width: 2880,
            height: 1800,
            scale: 2.0,
            reserved: [0, 35, 0, 0],
        };
        assert_eq!(
            m.usable(),
            Rect {
                x: 1920.0,
                y: 35.0,
                w: 1440.0,
                h: 865.0
            }
        );
    }

    #[test]
    fn builds_lua_dispatch_for_hyprland_053_and_later() {
        let cmd = lua_command("0xab", &[Action::Float, Action::Move([8, 43])]);
        assert_eq!(
            cmd,
            "eval hl.dispatch(hl.dsp.window.float({ window = \"address:0xab\", action = \"set\" })) \
             hl.dispatch(hl.dsp.window.move({ window = \"address:0xab\", x = 8, y = 43 }))"
        );
    }

    #[test]
    fn builds_legacy_batch_for_older_hyprland() {
        let cmd = legacy_command("0xab", &[Action::Resize([460, 190]), Action::Move([8, 43])]);
        assert_eq!(
            cmd,
            "[[BATCH]]dispatch resizewindowpixel exact 460 190,address:0xab;\
             dispatch movewindowpixel exact 8 43,address:0xab"
        );
    }

    #[test]
    fn tile_rule_escapes_title_regex_and_places_window() {
        let rule = lua_tile_rule(
            "hyperion-tile",
            "Hyperion — Disk · sd(a)",
            Some([8, 43]),
            [460, 190],
        );
        assert_eq!(
            rule,
            r#"eval hl.window_rule({ name = "hyperion-tile: Hyperion — Disk · sd(a)", match = { class = "^(hyperion-tile)$", title = "^(Hyperion — Disk · sd\\(a\\))$" }, float = true, size = { 460, 190 }, move = { 8, 43 } })"#
        );
    }

    #[test]
    fn recognises_success_and_error_replies() {
        assert!(all_ok("ok"));
        assert!(all_ok("ok\n\nok\n\nok"));
        assert!(!all_ok(""));
        assert!(!all_ok("error: [string ...]: ')' expected near 'exact'"));
    }

    #[test]
    fn parses_hyprland_client_json() {
        let json = r#"[{"address":"0xabc","pid":42,"class":"hyperion-tile","title":"Hyperion — CPU",
            "at":[10,20],"size":[440,210],"floating":true,"workspace":{"id":1}}]"#;
        let clients: Vec<Client> = serde_json::from_str(json).unwrap();
        assert_eq!(
            clients[0].rect(),
            Rect {
                x: 10.0,
                y: 20.0,
                w: 440.0,
                h: 210.0
            }
        );
    }
}
