//! Screen and viewport.
//!
//! The terminal height is decremented in exactly one place — [`Screen::viewport`]
//! — and the result has its own type so it cannot be confused with the full
//! screen. Layout and every piece of scroll arithmetic take a [`Viewport`];
//! only the top-level draw function ever sees a [`Screen`].

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Screen {
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Viewport {
    pub rows: u16,
    pub cols: u16,
}

impl Screen {
    pub fn new(cols: u16, rows: u16) -> Screen {
        Screen { rows, cols }
    }

    /// The content region: everything except the bottom row, which belongs to
    /// the status line or a toast.
    pub fn viewport(&self) -> Viewport {
        Viewport {
            rows: self.rows.saturating_sub(1),
            cols: self.cols,
        }
    }

    pub fn status_row(&self) -> u16 {
        self.rows.saturating_sub(1)
    }
}

impl Viewport {
    pub fn new(cols: u16, rows: u16) -> Viewport {
        Viewport { rows, cols }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_viewport_is_one_row_shorter_than_the_screen() {
        let screen = Screen::new(80, 24);
        assert_eq!(screen.viewport().rows, 23);
        assert_eq!(screen.viewport().cols, 80);
        assert_eq!(screen.status_row(), 23);
    }

    #[test]
    fn a_zero_height_screen_does_not_underflow() {
        let screen = Screen::new(80, 0);
        assert_eq!(screen.viewport().rows, 0);
        assert_eq!(screen.status_row(), 0);
    }
}
