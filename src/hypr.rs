//! Minimal Hyprland IPC client. Wayland doesn't let an app position its own windows,
//! but Hyprland's control socket lets us ask the compositor to float and move them.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::placement::Rect;

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
    /// Id of the monitor whose workspace the window is on.
    #[serde(default)]
    pub monitor: i64,
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
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub scale: f32,
    /// Space taken by bars: `[left, top, right, bottom]`.
    #[serde(default)]
    pub reserved: [i32; 4],
    /// New windows open on the focused monitor.
    #[serde(default)]
    pub focused: bool,
    /// In Hz.
    #[serde(default, rename = "refreshRate")]
    pub refresh_rate: f32,
}

impl Monitor {
    /// The whole monitor, in global logical coordinates.
    pub fn bounds(&self) -> Rect {
        let scale = if self.scale > 0.0 { self.scale } else { 1.0 };
        Rect {
            x: self.x as f32,
            y: self.y as f32,
            w: self.width as f32 / scale,
            h: self.height as f32 / scale,
        }
    }

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

#[derive(Deserialize)]
struct Workspace {
    id: i64,
    name: String,
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

#[derive(Clone, Debug, PartialEq)]
enum Action {
    Float,
    Resize([i32; 2]),
    Move([i32; 2]),
    /// Send to a workspace; `follow` also switches the view to it and focuses.
    ToWorkspace {
        workspace: String,
        follow: bool,
    },
    /// Moves the window onto a monitor (its active workspace). Hyprland only draws a
    /// window on its own workspace's monitor, so a window positioned over another
    /// monitor must be moved there too or it is invisible.
    ToMonitor(String),
    Focus,
}

/// Private special workspace the main window hides on. Special workspaces are only
/// shown when explicitly toggled, and nothing else uses this name.
const HIDDEN_WORKSPACE: &str = "special:hyperion";

fn lua_command(address: &str, actions: &[Action]) -> String {
    let window = format!("address:{address}");
    let calls: Vec<String> = actions
        .iter()
        .map(|a| match a {
            Action::Float => format!(
                "hl.dispatch(hl.dsp.window.float({{ window = \"{window}\", action = \"set\" }}))"
            ),
            &Action::Resize([w, h]) => format!(
                "hl.dispatch(hl.dsp.window.resize({{ window = \"{window}\", x = {w}, y = {h} }}))"
            ),
            &Action::Move([x, y]) => format!(
                "hl.dispatch(hl.dsp.window.move({{ window = \"{window}\", x = {x}, y = {y} }}))"
            ),
            Action::ToWorkspace { workspace, follow } => format!(
                "hl.dispatch(hl.dsp.window.move({{ window = \"{window}\", workspace = {}, follow = {follow} }}))",
                lua_string(workspace)
            ),
            Action::ToMonitor(monitor) => format!(
                "hl.dispatch(hl.dsp.window.move({{ window = \"{window}\", monitor = {}, follow = false }}))",
                lua_string(monitor)
            ),
            Action::Focus => format!("hl.dispatch(hl.dsp.focus({{ window = \"{window}\" }}))"),
        })
        .collect();
    format!("eval {}", calls.join(" "))
}

fn legacy_command(address: &str, actions: &[Action]) -> String {
    let calls: Vec<String> = actions
        .iter()
        .map(|a| match a {
            Action::Float => format!("dispatch setfloating address:{address}"),
            &Action::Resize([w, h]) => {
                format!("dispatch resizewindowpixel exact {w} {h},address:{address}")
            }
            &Action::Move([x, y]) => {
                format!("dispatch movewindowpixel exact {x} {y},address:{address}")
            }
            Action::ToWorkspace {
                workspace,
                follow: true,
            } => {
                format!("dispatch movetoworkspace {workspace},address:{address}")
            }
            Action::ToWorkspace {
                workspace,
                follow: false,
            } => {
                format!("dispatch movetoworkspacesilent {workspace},address:{address}")
            }
            Action::ToMonitor(monitor) => {
                format!("dispatch movewindow mon:{monitor},address:{address}")
            }
            Action::Focus => format!("dispatch focuswindow address:{address}"),
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
/// `pos` is relative to `monitor`, as rule positions are.
fn lua_tile_rule(
    class: &str,
    title: &str,
    placement: Option<(&str, [i32; 2])>,
    size: [i32; 2],
) -> String {
    let name = lua_string(&format!("{class}: {title}"));
    let class = lua_string(&format!("^({})$", regex_escape(class)));
    let title = lua_string(&format!("^({})$", regex_escape(title)));
    let [w, h] = size;
    let placement = placement.map_or(String::new(), |(monitor, [x, y])| {
        format!(", monitor = {}, move = {{ {x}, {y} }}", lua_string(monitor))
    });
    format!(
        "eval hl.window_rule({{ name = {name}, match = {{ class = {class}, title = {title} }}, \
         float = true, no_anim = true, size = {{ {w}, {h} }}{placement} }})"
    )
}

fn legacy_tile_rule(class: &str, title: &str, size: [i32; 2]) -> String {
    let [w, h] = size;
    let title = regex_escape(title);
    format!(
        "[[BATCH]]keyword windowrulev2 float,class:^({class})$;\
         keyword windowrulev2 noanim,class:^({class})$;\
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
    /// Monitor layout, refreshed every few seconds (it rarely changes, and moves
    /// happen every frame during a drag).
    monitors: RefCell<Option<(Instant, Vec<Monitor>)>>,
    /// Monitor each of our windows was last put on, so crossing monitors mid-drag
    /// sends the extra "move to monitor" step only when needed.
    window_monitor: RefCell<HashMap<String, i64>>,
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
                monitors: RefCell::new(None),
                window_monitor: RefCell::new(HashMap::new()),
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
        let fresh = self.query::<Vec<Monitor>>("monitors").unwrap_or_default();
        *self.monitors.borrow_mut() = Some((Instant::now(), fresh.clone()));
        fresh
    }

    fn cached_monitors(&self) -> Vec<Monitor> {
        if let Some((at, list)) = &*self.monitors.borrow()
            && at.elapsed() < Duration::from_secs(3)
        {
            return list.clone();
        }
        self.monitors()
    }

    /// The monitor under the middle of a window at `pos` with `size`.
    fn monitor_for(&self, pos: [f32; 2], size: [f32; 2]) -> Option<Monitor> {
        let center = [pos[0] + size[0] / 2.0, pos[1] + size[1] / 2.0];
        let monitors = self.cached_monitors();
        monitors
            .iter()
            .find(|m| m.bounds().contains(center))
            .or_else(|| monitors.iter().find(|m| m.focused))
            .cloned()
    }

    /// The "move to monitor" step, if the window isn't on `pos`'s monitor yet.
    fn monitor_step(
        &self,
        address: &str,
        current: Option<i64>,
        pos: [f32; 2],
        size: [f32; 2],
    ) -> Option<Action> {
        let target = self.monitor_for(pos, size)?;
        let current = current.or_else(|| self.window_monitor.borrow().get(address).copied());
        self.window_monitor
            .borrow_mut()
            .insert(address.to_owned(), target.id);
        (current != Some(target.id)).then_some(Action::ToMonitor(target.name))
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
        // Rule positions are relative to the rule's monitor while `pos` is global, so
        // pick the monitor under the window and convert. Without this the window first
        // appeared offset (or on the wrong monitor, where Hyprland won't draw it).
        let monitor = pos.and_then(|p| self.monitor_for(p, size));
        let placement = pos.zip(monitor.as_ref()).map(|(p, m)| {
            (
                m.name.clone(),
                [p[0].round() as i32 - m.x, p[1].round() as i32 - m.y],
            )
        });
        let size = size.map(|v| v.round() as i32);
        let lua = lua_tile_rule(
            class,
            title,
            placement.as_ref().map(|(m, p)| (m.as_str(), *p)),
            size,
        );
        let legacy = legacy_tile_rule(class, title, size);
        self.run(|dialect| match dialect {
            Dialect::Lua => lua.clone(),
            Dialect::Legacy => legacy.clone(),
        });
    }

    /// Floats, sizes and positions `client`, sending only the steps it still needs.
    pub fn place(&self, client: &Client, pos: [f32; 2], size: [f32; 2]) {
        let mut actions = Vec::new();
        if !client.floating {
            actions.push(Action::Float);
        }
        let to_monitor = self.monitor_step(&client.address, Some(client.monitor), pos, size);
        let size = size.map(|v| v.round() as i32);
        if client.size != size {
            actions.push(Action::Resize(size));
        }
        let pos = pos.map(|v| v.round() as i32);
        // Moving to another monitor repositions the window, so the exact move follows.
        if let Some(step) = to_monitor {
            actions.push(step);
            actions.push(Action::Move(pos));
        } else if client.at != pos {
            actions.push(Action::Move(pos));
        }
        if !actions.is_empty() {
            self.window_actions(&client.address, &actions);
        }
    }

    /// Address of this process's window with the given class (app id).
    pub fn own_window_address(&self, class: &str) -> Option<String> {
        let pid = i64::from(std::process::id());
        self.clients()
            .into_iter()
            .find(|c| c.pid == pid && c.class == class)
            .map(|c| c.address)
    }

    /// Moves a window onto a hidden workspace without switching the view.
    pub fn hide_window(&self, address: &str) {
        self.window_actions(
            address,
            &[Action::ToWorkspace {
                workspace: HIDDEN_WORKSPACE.to_owned(),
                follow: false,
            }],
        );
    }

    /// Brings a window to the workspace you are looking at and focuses it.
    pub fn show_window(&self, address: &str) {
        let Some(active) = self.query::<Workspace>("activeworkspace") else {
            return;
        };
        // Regular workspaces have positive ids; named ones are addressed by name.
        let workspace = if active.id > 0 {
            active.id.to_string()
        } else {
            format!("name:{}", active.name)
        };
        self.window_actions(
            address,
            &[
                Action::ToWorkspace {
                    workspace,
                    follow: true,
                },
                Action::Focus,
            ],
        );
    }

    /// Moves a window to `pos`, first onto that monitor if it's a different one.
    pub fn move_to(&self, address: &str, pos: [f32; 2], size: [f32; 2]) {
        let mut actions: Vec<Action> = self
            .monitor_step(address, None, pos, size)
            .into_iter()
            .collect();
        actions.push(Action::Move(pos.map(|v| v.round() as i32)));
        self.window_actions(address, &actions);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usable_area_excludes_bars_and_applies_scale() {
        let m = Monitor {
            id: 1,
            name: "DP-3".into(),
            x: 1920,
            y: 0,
            width: 2880,
            height: 1800,
            scale: 2.0,
            reserved: [0, 35, 0, 0],
            focused: false,
            refresh_rate: 60.0,
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
            Some(("DP-1", [8, 43])),
            [460, 190],
        );
        assert_eq!(
            rule,
            r#"eval hl.window_rule({ name = "hyperion-tile: Hyperion — Disk · sd(a)", match = { class = "^(hyperion-tile)$", title = "^(Hyperion — Disk · sd\\(a\\))$" }, float = true, no_anim = true, size = { 460, 190 }, monitor = "DP-1", move = { 8, 43 } })"#
        );
    }

    #[test]
    fn builds_hide_and_show_in_both_dialects() {
        let hide = [Action::ToWorkspace {
            workspace: HIDDEN_WORKSPACE.to_owned(),
            follow: false,
        }];
        assert_eq!(
            lua_command("0xab", &hide),
            r#"eval hl.dispatch(hl.dsp.window.move({ window = "address:0xab", workspace = "special:hyperion", follow = false }))"#
        );
        assert_eq!(
            legacy_command("0xab", &hide),
            "[[BATCH]]dispatch movetoworkspacesilent special:hyperion,address:0xab"
        );
        let show = [
            Action::ToWorkspace {
                workspace: "4".to_owned(),
                follow: true,
            },
            Action::Focus,
        ];
        assert_eq!(
            legacy_command("0xab", &show),
            "[[BATCH]]dispatch movetoworkspace 4,address:0xab;dispatch focuswindow address:0xab"
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
