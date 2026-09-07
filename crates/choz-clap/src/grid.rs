//! A ratatui backend that draws nowhere: it keeps the cells.
//!
//! choz's whole interface is [`choz_ui::embed::Embedded::draw`], and that draws
//! through any backend — the terminal's, a test's, or this one. A plugin window
//! is not a terminal, so instead of escape sequences this keeps the grid the
//! panels asked for, and [`crate::gui`] paints it into an X11 window.
//!
//! It is the smallest thing that can stand where `CrosstermBackend` stands: no
//! scrollback, no alternate screen, no cursor to speak of. A plugin window has
//! none of those to give.

use std::io;

use ratatui::backend::{Backend, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};

/// One cell as the window needs it: the character, and the two colours it is
/// drawn in. Everything else a `Cell` carries (underline, italics) has no
/// counterpart in a core X font and is dropped rather than half-drawn.
#[derive(Clone, PartialEq)]
pub struct GridCell {
    pub symbol: String,
    pub fg: ratatui::style::Color,
    pub bg: ratatui::style::Color,
    pub bold: bool,
}

impl Default for GridCell {
    fn default() -> Self {
        Self {
            symbol: " ".to_string(),
            fg: ratatui::style::Color::Reset,
            bg: ratatui::style::Color::Reset,
            bold: false,
        }
    }
}

/// The grid the panels drew, and how big it is.
pub struct Grid {
    pub cells: Vec<GridCell>,
    pub width: u16,
    pub height: u16,
    /// Cells changed since the window last painted, as flat indices. The
    /// window drains it: repainting a 200×60 grid every frame is a megabyte of
    /// X traffic per frame, and only a handful of cells move.
    pub dirty: Vec<usize>,
    cursor: Position,
}

impl Grid {
    pub fn new(width: u16, height: u16) -> Self {
        let mut grid = Self {
            cells: Vec::new(),
            width: 0,
            height: 0,
            dirty: Vec::new(),
            cursor: Position::new(0, 0),
        };
        grid.resize(width, height);
        grid
    }

    /// Give it a new shape. Everything on it is lost, which is what a resized
    /// window means: the panels lay themselves out again for the new size.
    pub fn resize(&mut self, width: u16, height: u16) {
        let (width, height) = (width.max(1), height.max(1));
        if (width, height) == (self.width, self.height) {
            return;
        }
        self.width = width;
        self.height = height;
        self.cells = vec![GridCell::default(); width as usize * height as usize];
        self.dirty = (0..self.cells.len()).collect();
    }

    /// Mark the whole grid for repainting — after an expose, when the window
    /// has been handed back a surface with nothing on it.
    pub fn dirty_all(&mut self) {
        self.dirty = (0..self.cells.len()).collect();
    }
}

impl Backend for Grid {
    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        for (x, y, cell) in content {
            if x >= self.width || y >= self.height {
                continue;
            }
            let index = y as usize * self.width as usize + x as usize;
            let next = GridCell {
                symbol: cell.symbol().to_string(),
                fg: cell.fg,
                bg: cell.bg,
                bold: cell.modifier.contains(ratatui::style::Modifier::BOLD),
            };
            // Only what actually moved: ratatui already hands us its own diff,
            // but a redraw after a resize hands us everything.
            if self.cells[index] != next {
                self.cells[index] = next;
                self.dirty.push(index);
            }
        }
        Ok(())
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        Ok(self.cursor)
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.cursor = position.into();
        Ok(())
    }

    fn clear(&mut self) -> io::Result<()> {
        self.cells.fill(GridCell::default());
        self.dirty_all();
        Ok(())
    }

    fn size(&self) -> io::Result<Size> {
        Ok(Size::new(self.width, self.height))
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        Ok(WindowSize {
            columns_rows: Size::new(self.width, self.height),
            // In pixels, which the window knows and the grid does not. Nothing
            // in choz's panels reads it except the wallpaper, and a plugin
            // window has no terminal to composite one.
            pixels: Size::new(0, 0),
        })
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
