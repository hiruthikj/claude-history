//! Geometry of list mode: where the search bar, rows and status bar sit, how
//! many rows fit, which row a screen line belongs to, and where the visible
//! window starts for a given selection.
//!
//! `ui::render_list_mode` and `App::handle_list_click` both consume this, so
//! a click can never disagree with what was drawn (view mode has the same
//! arrangement in `ui::view_layout_rects`).

use ratatui::prelude::*;

/// Rows kept visible above and below the selection while scrolling, when the
/// viewport is tall enough to afford them.
pub const SCROLLOFF: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListLayout {
    pub search_bar: Rect,
    pub list: Rect,
    /// Absent when the terminal is too short to show a bottom bar.
    pub status_bar: Option<Rect>,
    pub lines_per_item: usize,
}

/// Blank columns left and right of everything in list mode.
pub const SIDE_MARGIN: u16 = 2;
/// Terminals shorter than this lose the breathing lines around the search
/// bar and above the status bar.
const AIRY_MIN_HEIGHT: u16 = 12;

impl ListLayout {
    /// Split the whole frame, borderless: a side margin, a blank line, the
    /// two-line search bar (query and a rule), another blank line, the row
    /// area, a blank line and the one-line status bar. Short terminals drop
    /// the blank lines first, then the status bar.
    pub fn new(frame_area: Rect, lines_per_item: usize) -> Self {
        let margin = if frame_area.width >= 40 {
            SIDE_MARGIN
        } else {
            0
        };
        let x = frame_area.x.saturating_add(margin);
        let width = frame_area.width.saturating_sub(margin * 2);
        let air = u16::from(frame_area.height >= AIRY_MIN_HEIGHT);
        let top = frame_area.y.saturating_add(air);
        let bottom = frame_area.y.saturating_add(frame_area.height);

        let search_bar = Rect {
            x,
            y: top,
            width,
            height: 2.min(bottom.saturating_sub(top)),
        };
        let list_y = top.saturating_add(2 + air).min(bottom);
        let status_bar = (frame_area.height >= 6).then(|| Rect {
            x,
            y: bottom - 1,
            width,
            height: 1,
        });
        let list_end = match status_bar {
            Some(bar) => bar.y.saturating_sub(air),
            None => bottom,
        };
        let list = Rect {
            x,
            y: list_y,
            width,
            height: list_end.saturating_sub(list_y),
        };
        Self {
            search_bar,
            list,
            status_bar,
            lines_per_item: lines_per_item.max(1),
        }
    }

    /// Whole rows that fit in the row area (may be zero).
    pub fn rows_per_page(&self) -> usize {
        rows_per_page(self.list.height, self.lines_per_item)
    }

    /// First visible row, given the offset the previous frame settled on.
    pub fn scroll_offset(&self, anchor: usize, selected: Option<usize>, len: usize) -> usize {
        scroll_offset(anchor, selected, self.rows_per_page(), len)
    }

    /// List index of the row drawn at screen line `y`, if `y` is inside the
    /// row area and the row exists.
    pub fn row_at(&self, offset: usize, y: u16, len: usize) -> Option<usize> {
        if self.list.height == 0
            || y < self.list.y
            || y >= self.list.y.saturating_add(self.list.height)
        {
            return None;
        }
        if self.rows_per_page() == 0 {
            return None;
        }
        let relative = usize::from(y - self.list.y) / self.lines_per_item;
        let index = offset.checked_add(relative)?;
        (index < len).then_some(index)
    }
}

pub fn rows_per_page(list_height: u16, lines_per_item: usize) -> usize {
    usize::from(list_height) / lines_per_item.max(1)
}

/// Move `anchor` the least amount that keeps `selected` visible with
/// [`SCROLLOFF`] rows of context, clamped so the window never runs past the
/// end of the list. Pure, so the renderer and the click handler compute the
/// same window from the same inputs.
pub fn scroll_offset(anchor: usize, selected: Option<usize>, rows: usize, len: usize) -> usize {
    let rows = rows.max(1);
    let max_offset = len.saturating_sub(rows);
    let anchor = anchor.min(max_offset);
    let Some(selected) = selected else {
        return anchor;
    };
    let selected = selected.min(len.saturating_sub(1));
    let scrolloff = SCROLLOFF.min((rows - 1) / 2);
    let offset = if selected < anchor.saturating_add(scrolloff) {
        selected.saturating_sub(scrolloff)
    } else if selected + scrolloff >= anchor + rows {
        (selected + scrolloff + 1).saturating_sub(rows)
    } else {
        anchor
    };
    offset.min(max_offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_frame_into_search_rows_and_status_bar() {
        let layout = ListLayout::new(Rect::new(0, 0, 80, 20), 3);
        assert_eq!(layout.search_bar, Rect::new(2, 1, 76, 2));
        assert_eq!(layout.list, Rect::new(2, 4, 76, 14));
        assert_eq!(layout.status_bar, Some(Rect::new(2, 19, 76, 1)));
        assert_eq!(layout.rows_per_page(), 4);
    }

    #[test]
    fn short_terminal_drops_the_blank_lines_then_the_status_bar() {
        let layout = ListLayout::new(Rect::new(0, 0, 80, 8), 3);
        assert_eq!(layout.search_bar, Rect::new(2, 0, 76, 2));
        assert_eq!(layout.list, Rect::new(2, 2, 76, 5));
        assert_eq!(layout.status_bar, Some(Rect::new(2, 7, 76, 1)));

        let layout = ListLayout::new(Rect::new(0, 0, 30, 5), 3);
        assert_eq!(layout.status_bar, None);
        assert_eq!(layout.list, Rect::new(0, 2, 30, 3));
        assert_eq!(layout.rows_per_page(), 1);
    }

    #[test]
    fn row_at_maps_screen_lines_to_visible_rows() {
        let layout = ListLayout::new(Rect::new(0, 0, 80, 20), 3);
        assert_eq!(layout.row_at(0, 4, 10), Some(0));
        assert_eq!(layout.row_at(0, 6, 10), Some(0));
        assert_eq!(layout.row_at(0, 7, 10), Some(1));
        assert_eq!(layout.row_at(4, 7, 10), Some(5));
        assert_eq!(layout.row_at(0, 2, 10), None, "search bar");
        assert_eq!(layout.row_at(0, 19, 10), None, "status bar");
        assert_eq!(layout.row_at(0, 10, 2), None, "past the end of the list");
    }

    #[test]
    fn scroll_offset_keeps_selection_inside_window_with_context() {
        // 10 rows, 100 items, scrolloff 2
        assert_eq!(scroll_offset(0, Some(0), 10, 100), 0);
        assert_eq!(scroll_offset(0, Some(7), 10, 100), 0);
        assert_eq!(
            scroll_offset(0, Some(8), 10, 100),
            1,
            "one row before the bottom margin"
        );
        assert_eq!(scroll_offset(1, Some(9), 10, 100), 2);
        assert_eq!(
            scroll_offset(50, Some(51), 10, 100),
            49,
            "scrolling back up"
        );
        assert_eq!(scroll_offset(50, Some(20), 10, 100), 18, "jump far up");
        assert_eq!(
            scroll_offset(0, Some(99), 10, 100),
            90,
            "End clamps to the last page"
        );
        assert_eq!(scroll_offset(90, Some(0), 10, 100), 0);
    }

    #[test]
    fn scroll_offset_degrades_for_short_windows_and_empty_lists() {
        assert_eq!(
            scroll_offset(0, Some(3), 1, 10),
            3,
            "no scrolloff with one row"
        );
        assert_eq!(
            scroll_offset(0, Some(3), 2, 10),
            2,
            "selection on the bottom row"
        );
        assert_eq!(
            scroll_offset(0, Some(3), 3, 10),
            2,
            "scrolloff 1 with three rows"
        );
        assert_eq!(scroll_offset(7, None, 10, 100), 7);
        assert_eq!(
            scroll_offset(7, None, 10, 5),
            0,
            "anchor clamped when the list shrinks"
        );
        assert_eq!(scroll_offset(0, Some(0), 10, 0), 0);
        assert_eq!(
            scroll_offset(3, Some(9), 0, 10),
            9,
            "zero rows behaves like one"
        );
    }
}
