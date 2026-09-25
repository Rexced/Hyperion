//! Pure layout maths for the tile grid: no drawing, just rectangles.
//!
//! Each tile covers a block of `[columns, rows]` cells (its span, set by resizing).
//! Tiles are placed in order, left to right and top to bottom, like CSS grid's
//! auto-placement: a tile that doesn't fit in what's left of a row starts the next
//! one, and later tiles never move back into holes left behind, so reading order
//! always matches the tile order the user arranged.

use eframe::egui::{Pos2, Rect, vec2};

/// Gap between tiles, both between columns and between rows.
pub const GAP: f32 = 12.0;
/// A column is added for every this many pixels of width.
const MIN_COLUMN_WIDTH: f32 = 420.0;
pub const MAX_COLUMNS: usize = 4;
/// Tallest a tile can be made, in rows.
pub const MAX_ROWS: usize = 3;

/// `[columns, rows]` a tile covers.
pub type Span = [usize; 2];

pub const ONE_CELL: Span = [1, 1];

/// A saved span, limited to what the grid supports.
pub fn clamp_span(span: [u8; 2]) -> Span {
    [
        usize::from(span[0]).clamp(1, MAX_COLUMNS),
        usize::from(span[1]).clamp(1, MAX_ROWS),
    ]
}

/// How many columns `width` fits, never more than the tiles could fill side by side
/// (`total_columns` is the sum of their column spans).
pub fn columns_for(width: f32, total_columns: usize) -> usize {
    ((width / MIN_COLUMN_WIDTH).floor() as usize)
        .clamp(1, MAX_COLUMNS)
        .min(total_columns.max(1))
}

pub fn column_width(width: f32, columns: usize) -> f32 {
    (width - GAP * (columns as f32 - 1.0)) / columns as f32
}

/// Width of a tile spanning `columns` columns.
pub fn span_width(column_width: f32, columns: usize) -> f32 {
    column_width * columns as f32 + GAP * (columns as f32 - 1.0)
}

/// The span a tile gets when its bottom-right corner is dragged to `corner`, from
/// its top-left `anchor`. Steps are one cell plus a gap, measured when the resize
/// started so the grid reflowing underneath doesn't feed back into the result. The
/// corner snaps to whichever cell boundary is nearest.
pub fn span_for(anchor: Pos2, corner: Pos2, step: [f32; 2], columns: usize) -> Span {
    let cells = |length: f32, step: f32, max: usize| {
        (((length + GAP) / step).round().max(1.0) as usize).min(max)
    };
    [
        cells(corner.x - anchor.x, step[0], columns),
        cells(corner.y - anchor.y, step[1], MAX_ROWS),
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Placed {
    row: usize,
    col: usize,
    span: Span,
}

/// Auto-placement: each tile goes at the first free spot at or after where the
/// previous one ended.
fn place(columns: usize, spans: &[Span]) -> Vec<Placed> {
    let mut taken: Vec<Vec<bool>> = Vec::new();
    let free = |taken: &Vec<Vec<bool>>, row: usize, col: usize, [w, h]: Span| {
        (row..row + h).all(|r| {
            taken
                .get(r)
                .is_none_or(|cells| !cells[col..col + w].contains(&true))
        })
    };
    let (mut row, mut col) = (0, 0);
    spans
        .iter()
        .map(|&span| {
            let [w, h] = span;
            loop {
                if col + w > columns {
                    row += 1;
                    col = 0;
                } else if free(&taken, row, col, span) {
                    break;
                } else {
                    col += 1;
                }
            }
            if taken.len() < row + h {
                taken.resize(row + h, vec![false; columns]);
            }
            for cells in &mut taken[row..row + h] {
                cells[col..col + w].fill(true);
            }
            let placed = Placed { row, col, span };
            col += w;
            placed
        })
        .collect()
}

/// Rows split the available height evenly, but never get shorter than their tiles
/// need; the grid then grows past the available height and the caller scrolls.
pub struct Grid {
    origin: Pos2,
    columns: usize,
    column_width: f32,
    /// `(top, height)` of each row, `top` relative to `origin`.
    rows: Vec<(f32, f32)>,
    placed: Vec<Placed>,
}

impl Grid {
    /// `tiles`: each tile's span and minimum height.
    pub fn new(origin: Pos2, width: f32, height: f32, tiles: &[(Span, f32)]) -> Self {
        let columns = columns_for(width, tiles.iter().map(|(s, _)| s[0]).sum());
        let spans: Vec<Span> = tiles
            .iter()
            .map(|(s, _)| [s[0].min(columns), s[1]])
            .collect();
        let placed = place(columns, &spans);

        let row_count = placed.iter().map(|p| p.row + p.span[1]).max().unwrap_or(0);
        let even = if row_count == 0 {
            0.0
        } else {
            ((height - GAP * (row_count as f32 - 1.0)) / row_count as f32).max(0.0)
        };
        let mut heights = vec![even; row_count];
        // One-row tiles grow their own row.
        let mut sized = vec![false; row_count];
        for (p, &(_, min)) in placed.iter().zip(tiles) {
            if p.span[1] == 1 {
                heights[p.row] = heights[p.row].max(min);
                sized[p.row] = true;
            }
        }
        // A row covered only by taller tiles gets the height a one-row tile needs, so
        // "two rows" is never shorter than two normal tiles when the grid overflows.
        let normal = placed
            .iter()
            .zip(tiles)
            .filter(|(p, _)| p.span[1] == 1)
            .map(|(_, &(_, min))| min)
            .fold(even, f32::max);
        for (h, sized) in heights.iter_mut().zip(sized) {
            if !sized {
                *h = normal;
            }
        }
        // Then taller tiles make up any remaining shortfall in their last row.
        for (p, &(_, min)) in placed.iter().zip(tiles) {
            let [_, h] = p.span;
            if h > 1 {
                let spanned = p.row..p.row + h;
                let have = heights[spanned].iter().sum::<f32>() + GAP * (h as f32 - 1.0);
                if have < min {
                    heights[p.row + h - 1] += min - have;
                }
            }
        }
        let mut rows = Vec::with_capacity(row_count);
        let mut top = 0.0;
        for h in heights {
            rows.push((top, h));
            top += h + GAP;
        }
        Self {
            origin,
            columns,
            column_width: column_width(width, columns),
            rows,
            placed,
        }
    }

    pub fn columns(&self) -> usize {
        self.columns
    }

    /// One column plus a gap, and one row (of tile `index`) plus a gap: how far a
    /// tile's corner moves per cell when resizing it.
    pub fn step(&self, index: usize) -> [f32; 2] {
        let row = self.rows[self.placed[index].row].1;
        [self.column_width + GAP, row + GAP]
    }

    /// Total height of all rows, for the scroll area.
    pub fn height(&self) -> f32 {
        self.rows.last().map_or(0.0, |&(top, h)| top + h)
    }

    pub fn cell(&self, index: usize) -> Rect {
        let Placed { row, col, span } = self.placed[index];
        let (top, _) = self.rows[row];
        let (last_top, last_h) = self.rows[row + span[1] - 1];
        Rect::from_min_size(
            self.origin + vec2(col as f32 * (self.column_width + GAP), top),
            vec2(
                span_width(self.column_width, span[0]),
                last_top + last_h - top,
            ),
        )
    }

    /// The slot index under `p`. A tile's slot includes the gaps to its right and
    /// below it. Anywhere else (a hole, the empty end of a row, below the grid) means
    /// the first tile after that spot in reading order, or the end, so a tile can
    /// always be dropped last.
    pub fn slot_at(&self, p: Pos2) -> usize {
        let count = self.placed.len();
        if count == 0 {
            return 0;
        }
        let hit = (0..count).find(|&i| {
            let r = self.cell(i);
            Rect::from_min_max(r.min, r.max + vec2(GAP, GAP)).contains(p)
        });
        if let Some(i) = hit {
            return i;
        }
        let rel = p - self.origin;
        let col =
            ((rel.x / (self.column_width + GAP)).floor().max(0.0) as usize).min(self.columns - 1);
        let row = self
            .rows
            .iter()
            .position(|&(top, h)| rel.y < top + h + GAP)
            .unwrap_or(self.rows.len());
        self.placed
            .iter()
            .position(|q| (q.row, q.col) > (row, col))
            .unwrap_or(count - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::pos2;

    fn grid(width: f32, height: f32, mins: &[f32]) -> Grid {
        let tiles: Vec<(Span, f32)> = mins.iter().map(|&m| (ONE_CELL, m)).collect();
        Grid::new(Pos2::ZERO, width, height, &tiles)
    }

    fn spanned(width: f32, height: f32, spans: &[Span]) -> Grid {
        let tiles: Vec<(Span, f32)> = spans.iter().map(|&s| (s, 0.0)).collect();
        Grid::new(Pos2::ZERO, width, height, &tiles)
    }

    #[test]
    fn rows_share_the_available_height() {
        // 1000px wide -> 2 columns; 4 tiles -> 2 rows splitting 800px minus one gap.
        let g = grid(1000.0, 800.0, &[100.0; 4]);
        assert_eq!(
            g.cell(0),
            Rect::from_min_size(Pos2::ZERO, vec2(494.0, 394.0))
        );
        assert_eq!(g.cell(3).min, pos2(506.0, 406.0));
        assert_eq!(g.height(), 800.0);
    }

    #[test]
    fn a_tall_tile_grows_its_row_and_the_grid_scrolls() {
        let g = grid(1000.0, 400.0, &[100.0, 350.0, 100.0, 100.0]);
        assert_eq!(g.cell(0).height(), 350.0);
        assert_eq!(g.cell(2).height(), 194.0);
        assert!(g.height() > 400.0);
    }

    #[test]
    fn never_more_columns_than_tiles_can_fill() {
        assert_eq!(columns_for(1900.0, 2), 2);
        assert_eq!(columns_for(1900.0, 9), 4);
        assert_eq!(columns_for(300.0, 9), 1);
        let g = grid(1900.0, 600.0, &[100.0; 2]);
        assert_eq!(g.cell(0).width(), (1900.0 - GAP) / 2.0);
        // One tile spanning two columns can use two.
        assert_eq!(spanned(1900.0, 600.0, &[[2, 1]]).columns(), 2);
    }

    #[test]
    fn slot_at_maps_pointer_to_cells_and_end() {
        // 2 columns, 3 tiles: row 0 = [0, 1], row 1 = [2, (empty)].
        let g = grid(1000.0, 800.0, &[100.0; 3]);
        assert_eq!(g.slot_at(pos2(10.0, 10.0)), 0);
        assert_eq!(g.slot_at(pos2(600.0, 10.0)), 1);
        assert_eq!(g.slot_at(pos2(10.0, 500.0)), 2);
        // Empty end of the last row and far below both mean "last slot".
        assert_eq!(g.slot_at(pos2(600.0, 500.0)), 2);
        assert_eq!(g.slot_at(pos2(10.0, 5000.0)), 2);
        // The gap between columns belongs to the left cell.
        assert_eq!(g.slot_at(pos2(500.0, 10.0)), 0);
    }

    #[test]
    fn a_wide_tile_takes_the_row_and_pushes_the_rest_down() {
        let g = spanned(1000.0, 800.0, &[[2, 1], ONE_CELL, ONE_CELL]);
        assert_eq!(
            g.cell(0),
            Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 394.0))
        );
        assert_eq!(g.cell(1).min, pos2(0.0, 406.0));
        assert_eq!(g.cell(2).min, pos2(506.0, 406.0));
    }

    #[test]
    fn a_wide_tile_that_does_not_fit_starts_the_next_row_leaving_a_hole() {
        // Row 0 = [0, hole], row 1 = [1 1], row 2 = [2].
        let g = spanned(1000.0, 900.0, &[ONE_CELL, [2, 1], ONE_CELL]);
        assert_eq!(g.cell(1).min.x, 0.0);
        assert_eq!(g.cell(1).width(), 1000.0);
        assert!(g.cell(2).min.y > g.cell(1).min.y);
        // Later tiles never jump back into the hole.
        assert_eq!(g.cell(2).min.x, 0.0);
        // The hole means "before the next tile".
        assert_eq!(g.slot_at(pos2(700.0, 50.0)), 1);
    }

    #[test]
    fn a_tall_tile_spans_rows_and_others_flow_beside_it() {
        // Row 0 = [0, 1], row 1 = [0, 2], row 2 = [3].
        let g = spanned(1000.0, 900.0, &[[1, 2], ONE_CELL, ONE_CELL, ONE_CELL]);
        assert_eq!(g.cell(0).height(), g.cell(1).height() * 2.0 + GAP);
        assert_eq!(g.cell(2).min, pos2(506.0, g.cell(1).max.y + GAP));
        assert_eq!(g.cell(3).min.x, 0.0);
        assert_eq!(g.cell(3).min.y, g.cell(0).max.y + GAP);
    }

    #[test]
    fn a_tall_tile_that_needs_more_room_grows_its_last_row() {
        let tiles = [([1, 2], 700.0), (ONE_CELL, 100.0), (ONE_CELL, 100.0)];
        let g = Grid::new(Pos2::ZERO, 1000.0, 400.0, &tiles);
        assert_eq!(g.cell(0).height(), 700.0);
    }

    #[test]
    fn a_two_row_tile_is_never_shorter_than_two_normal_tiles_when_overflowing() {
        // Regression: rows covered only by the tall, full-width tile kept the tiny
        // even share of an overflowing grid while the others grew to fit their tiles.
        let mut tiles = vec![([2, 2], 0.0)];
        tiles.extend([(ONE_CELL, 200.0); 8]);
        let g = Grid::new(Pos2::ZERO, 1000.0, 600.0, &tiles);
        assert_eq!(g.cell(0).height(), 200.0 * 2.0 + GAP);
        assert_eq!(g.cell(1).height(), 200.0);
    }

    #[test]
    fn spans_wider_than_the_grid_are_clipped() {
        let g = spanned(1000.0, 800.0, &[[4, 1], ONE_CELL]);
        assert_eq!(g.cell(0).width(), 1000.0);
        assert_eq!(clamp_span([9, 0]), [MAX_COLUMNS, 1]);
    }

    #[test]
    fn resizing_snaps_the_corner_to_the_nearest_cell() {
        let step = [506.0, 406.0];
        let at = |x: f32, y: f32| span_for(Pos2::ZERO, pos2(x, y), step, 2);
        assert_eq!(at(494.0, 394.0), [1, 1]);
        // Past halfway into the next column/row: it grows.
        assert_eq!(at(780.0, 650.0), [2, 2]);
        // Never below one cell, never beyond the columns or MAX_ROWS.
        assert_eq!(at(-50.0, -50.0), [1, 1]);
        assert_eq!(at(5000.0, 5000.0), [2, MAX_ROWS]);
    }
}
