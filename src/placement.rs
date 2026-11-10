//! Where to put floating tile windows: gentle snapping and overlap avoidance.
//! Platform-neutral maths in global logical pixels; each platform supplies the
//! screen areas and window rectangles (Hyprland via IPC, others via the window system).

/// How close (logical px) a dropped window must be to an edge before it snaps to it.
pub const SNAP_DISTANCE: f32 = 24.0;
/// Space kept between a snapped window and the edge or neighbour it snapped to.
pub const SNAP_GAP: f32 = 8.0;

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

    pub fn center(&self) -> [f32; 2] {
        [self.x + self.w / 2.0, self.y + self.h / 2.0]
    }
}

impl Rect {
    pub fn overlaps(&self, o: &Rect) -> bool {
        self.x < o.x + o.w && o.x < self.x + self.w && self.y < o.y + o.h && o.y < self.y + self.h
    }

    fn inside(&self, area: &Rect) -> bool {
        self.x >= area.x
            && self.y >= area.y
            && self.x + self.w <= area.x + area.w
            && self.y + self.h <= area.y + area.h
    }

    fn at(&self, pos: [f32; 2]) -> Rect {
        Rect {
            x: pos[0],
            y: pos[1],
            ..*self
        }
    }
}

/// The screen areas a window may be placed in, and how much we trust their edges.
///
/// Some desktops tell us the *usable* area — what is left once panels and bars are
/// subtracted — and some will only tell us the size of the whole display. The
/// difference decides what we are entitled to do with it. Keeping a window inside
/// these bounds is worth doing either way, because overlapping a panel is a far
/// smaller sin than sitting entirely off the screen. Snapping a window flush against
/// an edge is not: do that to the bottom of the whole display on a desktop with a
/// bottom panel and the window is tucked behind it every single time, by construction.
#[derive(Clone, Copy)]
pub struct Screens<'a> {
    areas: &'a [Rect],
    /// Whether `areas` has panels and bars already excluded.
    exact: bool,
}

impl<'a> Screens<'a> {
    /// Areas with panels and bars excluded, as Hyprland reports them.
    pub fn usable(areas: &'a [Rect]) -> Self {
        Self { areas, exact: true }
    }

    /// Whole displays, panels included — all some desktops will report. Bounds a
    /// window, but never used as something to snap against.
    pub fn whole_display(areas: &'a [Rect]) -> Self {
        Self {
            areas,
            exact: false,
        }
    }
}

/// Nudges `win` onto a nearby screen edge or neighbouring window, but only when it is
/// already within [`SNAP_DISTANCE`]; anything further away is left exactly where it is.
pub fn snap(win: Rect, monitors: Screens<'_>, others: &[Rect]) -> [f32; 2] {
    let center = win.center();
    let screen = monitors
        .exact
        .then(|| {
            monitors
                .areas
                .iter()
                .find(|m| m.contains(center))
                .or_else(|| monitors.areas.first())
        })
        .flatten();

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

/// If `win` was dropped on top of other windows, moves it to the nearest side (left,
/// right, above or below) of one of them where it fits on a screen and overlaps
/// nothing. The windows underneath never move. Returns `win`'s position unchanged
/// when there is no overlap or nowhere free to go.
pub fn avoid_overlap(win: Rect, screens: Screens<'_>, others: &[Rect]) -> [f32; 2] {
    let here = [win.x, win.y];
    if !others.iter().any(|o| win.overlaps(o)) {
        return here;
    }
    // A candidate has to land on a screen we actually know about. Treating "no screens
    // known" as "anywhere is fine" meant that on desktops where we can't read the
    // monitor layout, each overlapping window pushed the candidate further right with
    // nothing to stop it — a tile could settle wholly off the side of the display,
    // taking its close button with it and leaving no way to get it back. Unknown
    // bounds now mean nothing fits, so the window simply stays where it was dropped.
    let fits = |r: &Rect| {
        screens.areas.iter().any(|s| r.inside(s)) && !others.iter().any(|o| r.overlaps(o))
    };
    let distance = |p: &[f32; 2]| (p[0] - here[0]).hypot(p[1] - here[1]);
    others
        .iter()
        .filter(|o| win.overlaps(o))
        .flat_map(|o| {
            [
                [o.x - win.w - SNAP_GAP, win.y],
                [o.x + o.w + SNAP_GAP, win.y],
                [win.x, o.y - win.h - SNAP_GAP],
                [win.x, o.y + o.h + SNAP_GAP],
            ]
        })
        .filter(|p| fits(&win.at(*p)))
        .min_by(|a, b| distance(a).total_cmp(&distance(b)))
        .unwrap_or(here)
}

/// Where a released window ends up: off any window it landed on, then snapped to a
/// nearby edge (unless snapping would put it back on top of something).
pub fn settle(win: Rect, screens: Screens<'_>, others: &[Rect]) -> [f32; 2] {
    let free = win.at(avoid_overlap(win, screens, others));
    let snapped = free.at(snap(free, screens, others));
    if others.iter().any(|o| snapped.overlaps(o)) {
        [free.x, free.y]
    } else {
        [snapped.x, snapped.y]
    }
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
        assert_eq!(
            snap(win(500.0, 400.0), Screens::usable(&[SCREEN]), &[]),
            [500.0, 400.0]
        );
    }

    #[test]
    fn near_screen_corner_snaps_with_gap() {
        assert_eq!(
            snap(win(15.0, 50.0), Screens::usable(&[SCREEN]), &[]),
            [8.0, 43.0]
        );
        let right = 1920.0 - 400.0 - SNAP_GAP;
        assert_eq!(
            snap(win(right - 10.0, 400.0), Screens::usable(&[SCREEN]), &[]),
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
        assert_eq!(
            snap(dropped, Screens::usable(&[SCREEN]), &[other]),
            [508.0, 300.0]
        );
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
        assert_eq!(
            snap(dropped, Screens::usable(&[SCREEN]), &[other]),
            [700.0, 308.0]
        );
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
        assert_eq!(
            snap(win(490.0, 120.0), Screens::usable(&[SCREEN]), &[far]),
            [490.0, 120.0]
        );
    }

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    #[test]
    fn no_overlap_means_no_move() {
        let other = rect(1000.0, 400.0, 400.0, 200.0);
        assert_eq!(
            avoid_overlap(win(100.0, 100.0), Screens::usable(&[SCREEN]), &[other]),
            [100.0, 100.0]
        );
    }

    #[test]
    fn dropped_mostly_right_of_a_window_moves_to_its_right() {
        let other = rect(500.0, 400.0, 400.0, 200.0);
        // Overlaps the right part of `other`: the right side is closest.
        let pos = avoid_overlap(win(800.0, 420.0), Screens::usable(&[SCREEN]), &[other]);
        assert_eq!(pos, [908.0, 420.0]);
    }

    #[test]
    fn dropped_mostly_below_moves_below() {
        let other = rect(500.0, 400.0, 400.0, 200.0);
        let pos = avoid_overlap(win(520.0, 560.0), Screens::usable(&[SCREEN]), &[other]);
        assert_eq!(pos, [520.0, 608.0]);
    }

    #[test]
    fn skips_sides_that_leave_the_screen_or_hit_a_third_window() {
        // Near the top-right corner: right and above of `other` are off-screen, and
        // below it is taken by `third`, so the nearest free spot is to the left.
        let other = rect(1500.0, 60.0, 400.0, 200.0);
        let third = rect(1480.0, 260.0, 440.0, 300.0);
        let pos = avoid_overlap(
            win(1510.0, 110.0),
            Screens::usable(&[SCREEN]),
            &[other, third],
        );
        assert_eq!(pos, [1072.0, 110.0]);
    }

    #[test]
    fn nowhere_free_leaves_it_where_dropped() {
        let screen = rect(0.0, 0.0, 420.0, 220.0);
        let other = rect(0.0, 0.0, 420.0, 220.0);
        assert_eq!(
            avoid_overlap(win(10.0, 10.0), Screens::usable(&[screen]), &[other]),
            [10.0, 10.0]
        );
    }

    #[test]
    fn a_whole_display_bounds_a_window_but_is_never_snapped_against() {
        // Regression: on desktops that only report the size of the display, snapping to
        // its bottom edge tucked the window behind the panel there — every time, by
        // construction. Cinnamon reserves 40px at the bottom of a 768px screen, so a
        // 182-tall tile snapped to 578 has its last 32px hidden. Approximate bounds may
        // keep a window on screen; they may not pull it flush against an edge.
        let display = Rect {
            x: 0.0,
            y: 0.0,
            w: 1360.0,
            h: 768.0,
        };
        let near_bottom = Rect {
            x: 400.0,
            y: 575.0,
            w: 528.0,
            h: 182.0,
        };

        // Usable bounds: snapping to the bottom edge is meaningful, so it happens.
        assert_eq!(
            snap(near_bottom, Screens::usable(&[display]), &[])[1],
            768.0 - 182.0 - SNAP_GAP
        );
        // Whole-display bounds: left exactly where it was.
        assert_eq!(
            snap(near_bottom, Screens::whole_display(&[display]), &[])[1],
            near_bottom.y
        );

        // But they still bound it: a window shoved off another one can't leave them.
        let main = Rect {
            x: 0.0,
            y: 0.0,
            w: 1360.0,
            h: 700.0,
        };
        let dropped = Rect {
            x: 400.0,
            y: 300.0,
            w: 528.0,
            h: 300.0,
        };
        let moved = dropped.at(avoid_overlap(
            dropped,
            Screens::whole_display(&[display]),
            &[main],
        ));
        assert!(moved.inside(&display), "left the display: {moved:?}");
    }

    #[test]
    fn unknown_screens_leave_a_dropped_window_alone_rather_than_pushing_it_away() {
        // Regression, found on Mint/X11 where the monitor layout isn't readable: with
        // no known screens every candidate position counted as fitting, so each
        // overlapping window shoved the next one further right with nothing to stop it.
        // Tiles ended up past the edge of a 1360px-wide display — close button and all,
        // so there was no way to get them back.
        let screen = Rect {
            x: 0.0,
            y: 0.0,
            w: 1360.0,
            h: 768.0,
        };
        let main = Rect {
            x: 0.0,
            y: 0.0,
            w: 1360.0,
            h: 700.0,
        };
        let dropped = Rect {
            x: 400.0,
            y: 300.0,
            w: 528.0,
            h: 300.0,
        };

        // Knowing the screen, it may move, but never off the side of it.
        let with_screen = dropped.at(avoid_overlap(dropped, Screens::usable(&[screen]), &[main]));
        assert!(
            with_screen.x + with_screen.w <= screen.w,
            "ran off a screen it knew about: {with_screen:?}"
        );

        // Not knowing it, it must stay put rather than guess.
        assert_eq!(
            avoid_overlap(dropped, Screens::usable(&[]), &[main]),
            [dropped.x, dropped.y]
        );
    }

    #[test]
    fn settle_avoids_overlap_then_snaps() {
        let other = rect(500.0, 400.0, 400.0, 200.0);
        // Lands overlapping, gets pushed right, then snaps its top to `other`'s top.
        assert_eq!(
            settle(win(800.0, 410.0), Screens::usable(&[SCREEN]), &[other]),
            [908.0, 400.0]
        );
    }
}
