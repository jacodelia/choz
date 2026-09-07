//! choz's interface in a window the host owns.
//!
//! The panels are unchanged: [`choz_ui::embed::Embedded::draw`] draws through a
//! ratatui backend, and [`crate::grid::Grid`] is one. What is left is the part
//! a terminal used to do — turn a grid of cells into pixels, and turn what the
//! user does back into keys — and on Linux that is X11, which choz already
//! speaks: it is how a hosted plugin's own editor gets a window.
//!
//! **A core X font, not a rasteriser.** The grid is monospaced by
//! construction, which is exactly what `-misc-fixed-*` is for, and it costs
//! nothing to link. `image_text16` draws the glyph *and* its background in one
//! request, so a cell is one request and a repaint is no more than the cells
//! that moved.

use anyhow::{Context, Result};
use ratatui::crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::style::Color;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    self, AtomEnum, ChangeGCAux, Char2b, ConfigureWindowAux, ConnectionExt, CreateGCAux,
    CreateWindowAux, EventMask, Gcontext, PropMode, Window as XWindow, WindowClass,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use crate::grid::Grid;

/// The grid a window opens at, in cells. Wide enough for the rack, the FX
/// chain and the panels beside them — the layout choz assumes on a terminal
/// nobody has resized.
pub const COLS: u16 = 160;
pub const ROWS: u16 = 44;

/// The fonts to try, best first. The last one exists on every X server there
/// has ever been, which is the point of naming it.
const FONTS: [&str; 3] = [
    "-misc-fixed-medium-r-normal--13-120-75-75-c-70-iso10646-1",
    "-misc-fixed-medium-r-normal--13-*-*-*-*-*-iso8859-1",
    "fixed",
];

/// What the user did, on its way to the panels.
pub enum Input {
    Key(KeyCode),
    /// Already in cells, which is the only unit the panels know.
    Mouse(MouseEvent),
    /// The window is a different size, in cells.
    Resize(u16, u16),
}

pub struct Window {
    conn: RustConnection,
    window: XWindow,
    gc: Gcontext,
    /// Cell size and baseline, from the font the server gave us.
    cell_w: u16,
    cell_h: u16,
    ascent: i16,
    /// Keycode → keysyms, fetched once. X sends keycodes; the panels want keys.
    keysyms: Vec<Vec<u32>>,
    first_keycode: u8,
    per_keycode: u8,
    /// Whether shift was down, tracked from the events themselves: X reports
    /// the modifier state *before* the event, which is right for us.
    cols: u16,
    rows: u16,
}

impl Window {
    /// Open a window inside the host's, and tell the caller what grid fits.
    pub fn open(parent: Option<XWindow>) -> Result<(Self, u16, u16)> {
        let (conn, screen_num) = x11rb::connect(None).context("no X display")?;
        let screen = &conn.setup().roots[screen_num];
        let parent = parent.unwrap_or(screen.root);

        let font = conn.generate_id()?;
        let metrics = FONTS.iter().find_map(|name| {
            conn.open_font(font, name.as_bytes()).ok()?.check().ok()?;
            let reply = conn.query_font(font).ok()?.reply().ok()?;
            Some(reply)
        });
        let metrics = metrics.context("this X server has no fixed-width font")?;
        // `max_bounds` rather than the per-character widths: the grid is
        // monospaced and a cell is one character wide by definition.
        let cell_w = metrics.max_bounds.character_width.max(1) as u16;
        let cell_h = (metrics.font_ascent + metrics.font_descent).max(1) as u16;
        let ascent = metrics.font_ascent;

        let window = conn.generate_id()?;
        conn.create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            window,
            parent,
            0,
            0,
            COLS * cell_w,
            ROWS * cell_h,
            0,
            WindowClass::INPUT_OUTPUT,
            screen.root_visual,
            &CreateWindowAux::new().event_mask(
                EventMask::EXPOSURE
                    | EventMask::KEY_PRESS
                    | EventMask::STRUCTURE_NOTIFY
                    | EventMask::BUTTON_PRESS
                    | EventMask::BUTTON_RELEASE
                    // Every motion, not only dragged: choz hovers. A window
                    // that reported motion only with a button down would light
                    // up controls solely while something was being dragged.
                    | EventMask::POINTER_MOTION,
            ),
        )?;
        conn.change_property8(
            PropMode::REPLACE,
            window,
            AtomEnum::WM_NAME,
            AtomEnum::STRING,
            b"choz",
        )?;

        let gc = conn.generate_id()?;
        conn.create_gc(gc, window, &CreateGCAux::new().font(font))?;
        conn.map_window(window)?;
        conn.flush()?;

        let mapping = conn
            .get_keyboard_mapping(
                conn.setup().min_keycode,
                conn.setup().max_keycode - conn.setup().min_keycode + 1,
            )?
            .reply()?;
        let per = mapping.keysyms_per_keycode.max(1);
        let keysyms = mapping
            .keysyms
            .chunks(per as usize)
            .map(<[u32]>::to_vec)
            .collect();
        let first_keycode = conn.setup().min_keycode;

        Ok((
            Self {
                conn,
                window,
                gc,
                cell_w,
                cell_h,
                ascent,
                keysyms,
                first_keycode,
                per_keycode: per,
                cols: COLS,
                rows: ROWS,
            },
            COLS,
            ROWS,
        ))
    }

    /// The window's size in pixels, which is what the host asks for.
    pub fn pixel_size(&self) -> (u32, u32) {
        (
            self.cols as u32 * self.cell_w as u32,
            self.rows as u32 * self.cell_h as u32,
        )
    }

    /// How many cells fit in a window of this many pixels.
    pub fn cells_for(&self, width: u32, height: u32) -> (u16, u16) {
        (
            (width / self.cell_w.max(1) as u32).max(1) as u16,
            (height / self.cell_h.max(1) as u32).max(1) as u16,
        )
    }

    /// Ask X for this many pixels, for a host that sizes the window itself.
    pub fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        let _ = self
            .conn
            .configure_window(
                self.window,
                &ConfigureWindowAux::new().width(width).height(height),
            )?
            .check();
        let (cols, rows) = self.cells_for(width, height);
        self.cols = cols;
        self.rows = rows;
        self.conn.flush()?;
        Ok(())
    }

    pub fn show(&self) -> Result<()> {
        self.conn.map_window(self.window)?;
        self.conn.flush()?;
        Ok(())
    }

    pub fn hide(&self) -> Result<()> {
        self.conn.unmap_window(self.window)?;
        self.conn.flush()?;
        Ok(())
    }

    /// Everything X has to say since the last look. Never blocks: this runs on
    /// the host's main thread, between two of its own callbacks.
    pub fn pump(&mut self, grid: &mut Grid) -> Vec<Input> {
        let mut out = Vec::new();
        while let Ok(Some(event)) = self.conn.poll_for_event() {
            match event {
                Event::Expose(_) => grid.dirty_all(),
                Event::ConfigureNotify(e) => {
                    let (cols, rows) = self.cells_for(e.width as u32, e.height as u32);
                    if (cols, rows) != (self.cols, self.rows) {
                        self.cols = cols;
                        self.rows = rows;
                        out.push(Input::Resize(cols, rows));
                    }
                }
                Event::ButtonPress(e) => {
                    if let Some(kind) = button_down(e.detail) {
                        out.push(Input::Mouse(self.mouse_at(kind, e.event_x, e.event_y)));
                    }
                }
                Event::ButtonRelease(e) => {
                    if let Some(button) = button_of(e.detail) {
                        let kind = MouseEventKind::Up(button);
                        out.push(Input::Mouse(self.mouse_at(kind, e.event_x, e.event_y)));
                    }
                }
                Event::MotionNotify(e) => {
                    // Which button is held comes off the state mask, because a
                    // drag and a hover are different gestures to a fader.
                    let kind = match held(e.state) {
                        Some(button) => MouseEventKind::Drag(button),
                        None => MouseEventKind::Moved,
                    };
                    out.push(Input::Mouse(self.mouse_at(kind, e.event_x, e.event_y)));
                }
                Event::KeyPress(e) => {
                    // Bit 0 of the state is Shift, which is the only modifier
                    // the keysym table needs to pick a column.
                    let shifted = e.state.contains(xproto::KeyButMask::SHIFT);
                    if let Some(code) = self.key_of(e.detail, shifted) {
                        out.push(Input::Key(code));
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// A pointer position in pixels as the cell it is over.
    fn mouse_at(&self, kind: MouseEventKind, x: i16, y: i16) -> MouseEvent {
        MouseEvent {
            kind,
            column: (x.max(0) as u16 / self.cell_w.max(1)).min(self.cols.saturating_sub(1)),
            row: (y.max(0) as u16 / self.cell_h.max(1)).min(self.rows.saturating_sub(1)),
            modifiers: KeyModifiers::NONE,
        }
    }

    /// One X keycode as the key the panels handle.
    fn key_of(&self, keycode: u8, shifted: bool) -> Option<KeyCode> {
        let index = keycode.checked_sub(self.first_keycode)? as usize;
        let group = self.keysyms.get(index)?;
        // Column 1 is the shifted key, when the keyboard has one there.
        let keysym = match shifted && self.per_keycode > 1 && group.get(1).is_some_and(|s| *s != 0)
        {
            true => group[1],
            false => *group.first()?,
        };
        keysym_to_key(keysym)
    }

    /// Paint what moved. One request per cell, which is what `image_text16`
    /// buys: it fills the background and draws the glyph together, so a cell
    /// never flickers between the two.
    ///
    /// ponytail: per cell, not per run of same-coloured cells. ratatui hands us
    /// a diff, so a normal frame is a handful of them; batch by run if a full
    /// repaint on resize ever shows.
    pub fn paint(&mut self, grid: &mut Grid) -> Result<()> {
        for index in std::mem::take(&mut grid.dirty) {
            let Some(cell) = grid.cells.get(index) else {
                continue;
            };
            let x = (index % grid.width as usize) as i16 * self.cell_w as i16;
            let y = (index / grid.width as usize) as i16 * self.cell_h as i16;
            self.conn.change_gc(
                self.gc,
                &ChangeGCAux::new()
                    .foreground(pixel(cell.fg, true))
                    .background(pixel(cell.bg, false)),
            )?;
            let glyph = cell.symbol.chars().next().unwrap_or(' ');
            // A core font is indexed by two bytes, so anything past the basic
            // plane is drawn as the box a terminal would show for it.
            let point = u32::from(glyph);
            let point = if point > 0xFFFF { 0xFFFD } else { point };
            let ch = Char2b {
                byte1: (point >> 8) as u8,
                byte2: (point & 0xFF) as u8,
            };
            self.conn
                .image_text16(self.window, self.gc, x, y + self.ascent, &[ch])?;
        }
        self.conn.flush()?;
        Ok(())
    }
}

/// X numbers its buttons; the panels name three of them and read the wheel as
/// a scroll.
fn button_of(detail: u8) -> Option<MouseButton> {
    Some(match detail {
        1 => MouseButton::Left,
        2 => MouseButton::Middle,
        3 => MouseButton::Right,
        _ => return None,
    })
}

/// A press, which on X includes the wheel: buttons 4 and 5 are one notch up
/// and one down, and they arrive as presses with no meaningful release.
fn button_down(detail: u8) -> Option<MouseEventKind> {
    Some(match detail {
        4 => MouseEventKind::ScrollUp,
        5 => MouseEventKind::ScrollDown,
        6 => MouseEventKind::ScrollLeft,
        7 => MouseEventKind::ScrollRight,
        other => MouseEventKind::Down(button_of(other)?),
    })
}

/// The button held during a motion, from the state mask X reports with it.
fn held(state: xproto::KeyButMask) -> Option<MouseButton> {
    if state.contains(xproto::KeyButMask::BUTTON1) {
        return Some(MouseButton::Left);
    }
    if state.contains(xproto::KeyButMask::BUTTON2) {
        return Some(MouseButton::Middle);
    }
    if state.contains(xproto::KeyButMask::BUTTON3) {
        return Some(MouseButton::Right);
    }
    None
}

/// A ratatui colour as an X pixel on a TrueColor visual, which is every visual
/// worth drawing on since about 2005.
///
/// `Reset` is the terminal's own default, and a window has none — so it is
/// choz's: near-black behind, bone white in front.
fn pixel(color: Color, foreground: bool) -> u32 {
    match color {
        Color::Reset => match foreground {
            true => 0x00D0D0D0,
            false => 0x00101010,
        },
        Color::Black => 0x00000000,
        Color::Red => 0x00CC0000,
        Color::Green => 0x0000CC00,
        Color::Yellow => 0x00CCCC00,
        Color::Blue => 0x000000CC,
        Color::Magenta => 0x00CC00CC,
        Color::Cyan => 0x0000CCCC,
        Color::Gray => 0x00AAAAAA,
        Color::DarkGray => 0x00555555,
        Color::LightRed => 0x00FF5555,
        Color::LightGreen => 0x0055FF55,
        Color::LightYellow => 0x00FFFF55,
        Color::LightBlue => 0x005555FF,
        Color::LightMagenta => 0x00FF55FF,
        Color::LightCyan => 0x0055FFFF,
        Color::White => 0x00FFFFFF,
        Color::Rgb(r, g, b) => u32::from_be_bytes([0, r, g, b]),
        // The 256-colour cube, the same arithmetic every terminal uses.
        Color::Indexed(i) => indexed(i),
    }
}

/// One of the 256 palette entries as RGB: sixteen named, a 6×6×6 cube, then
/// twenty-four greys.
fn indexed(i: u8) -> u32 {
    const STEPS: [u32; 6] = [0, 95, 135, 175, 215, 255];
    match i {
        0..=15 => {
            let bright = i >= 8;
            let n = i & 7;
            let level = if bright { 255 } else { 205 };
            let (r, g, b) = (
                (n & 1 != 0) as u32 * level,
                (n & 2 != 0) as u32 * level,
                (n & 4 != 0) as u32 * level,
            );
            match (n, bright) {
                (0, true) => 0x00808080,
                (0, false) => 0,
                _ => (r << 16) | (g << 8) | b,
            }
        }
        16..=231 => {
            let n = (i - 16) as u32;
            (STEPS[(n / 36) as usize] << 16)
                | (STEPS[((n / 6) % 6) as usize] << 8)
                | STEPS[(n % 6) as usize]
        }
        232..=255 => {
            let grey = 8 + (i as u32 - 232) * 10;
            (grey << 16) | (grey << 8) | grey
        }
    }
}

/// An X keysym as the key choz's handlers take. The ones a panel reads; a
/// keysym with no counterpart is not a key here.
fn keysym_to_key(keysym: u32) -> Option<KeyCode> {
    Some(match keysym {
        0xFF08 => KeyCode::Backspace,
        0xFF09 => KeyCode::Tab,
        0xFF0D | 0xFF8D => KeyCode::Enter,
        0xFF1B => KeyCode::Esc,
        0xFF50 => KeyCode::Home,
        0xFF51 => KeyCode::Left,
        0xFF52 => KeyCode::Up,
        0xFF53 => KeyCode::Right,
        0xFF54 => KeyCode::Down,
        0xFF55 => KeyCode::PageUp,
        0xFF56 => KeyCode::PageDown,
        0xFF57 => KeyCode::End,
        0xFF63 => KeyCode::Insert,
        0xFFFF => KeyCode::Delete,
        0xFFBE..=0xFFC9 => KeyCode::F((keysym - 0xFFBD) as u8),
        // Latin-1 sits at its own code points, and the Unicode keysyms are the
        // code point with a flag on top.
        0x20..=0xFF => KeyCode::Char(char::from_u32(keysym)?),
        0x1000000..=0x110FFFF => KeyCode::Char(char::from_u32(keysym - 0x1000000)?),
        _ => return None,
    })
}

impl Window {
    /// Move the window inside the host's, which is the whole of what "embedded"
    /// means on X11: the host hands over the XID of the box it has made room
    /// for, and ours becomes a child of it.
    pub fn reparent(&mut self, parent: XWindow) -> Result<()> {
        self.conn.reparent_window(self.window, parent, 0, 0)?;
        self.conn.map_window(self.window)?;
        self.conn.flush()?;
        Ok(())
    }

    /// The pixels a grid of this many cells needs — the inverse of
    /// [`Self::cells_for`], for a host asking what size it may make the window.
    pub fn pixels_for(&self, cols: u16, rows: u16) -> (u32, u32) {
        (
            cols as u32 * self.cell_w as u32,
            rows as u32 * self.cell_h as u32,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The keys a panel actually reads, coming off X as the codes choz's
    /// handlers match on. Enter, Escape and the arrows are the whole of moving
    /// around the rack; a letter is what picks a plugin out of a list.
    #[test]
    fn the_keys_the_panels_read_survive_the_trip_from_x() {
        assert_eq!(keysym_to_key(0xFF0D), Some(KeyCode::Enter));
        assert_eq!(keysym_to_key(0xFF8D), Some(KeyCode::Enter), "the keypad's");
        assert_eq!(keysym_to_key(0xFF1B), Some(KeyCode::Esc));
        assert_eq!(keysym_to_key(0xFF51), Some(KeyCode::Left));
        assert_eq!(keysym_to_key(0xFF54), Some(KeyCode::Down));
        assert_eq!(keysym_to_key(0xFF09), Some(KeyCode::Tab));
        assert_eq!(keysym_to_key(0x72), Some(KeyCode::Char('r')), "rescan");
        assert_eq!(keysym_to_key(0x20), Some(KeyCode::Char(' ')));
        assert_eq!(keysym_to_key(0xFFBE), Some(KeyCode::F(1)));
        assert_eq!(keysym_to_key(0xFFC9), Some(KeyCode::F(12)));
        // A Unicode keysym is the code point with a flag on it.
        assert_eq!(keysym_to_key(0x1000101), Some(KeyCode::Char('ā')));
        // Modifiers are not keys: Shift on its own must not press anything.
        assert_eq!(keysym_to_key(0xFFE1), None);
    }

    /// The palette a panel is painted in. choz styles with the 256-colour
    /// indexes as often as with RGB, and a window drawing those wrong is a
    /// window whose meters are the wrong colour.
    #[test]
    fn colours_reach_the_window_as_the_pixels_they_are() {
        assert_eq!(pixel(Color::Rgb(0x12, 0x34, 0x56), true), 0x00123456);
        assert_eq!(pixel(Color::Black, true), 0);
        assert_eq!(pixel(Color::White, true), 0x00FFFFFF);
        // The cube: 16 is its black corner, 231 its white one.
        assert_eq!(pixel(Color::Indexed(16), true), 0x00000000);
        assert_eq!(pixel(Color::Indexed(231), true), 0x00FFFFFF);
        // The grey ramp runs 8..238, evenly.
        assert_eq!(pixel(Color::Indexed(232), true), 0x00080808);
        assert_eq!(pixel(Color::Indexed(255), true), 0x00EEEEEE);
        // Reset is the terminal's own default, and a window has none of its
        // own — so the two must not come out the same, or text is invisible.
        assert_ne!(pixel(Color::Reset, true), pixel(Color::Reset, false));
    }

    /// The pointer, which is half of choz's interface: the faders, the rack
    /// buttons and every drawer row are clicked, and the wheel is how a knob
    /// moves. X numbers its buttons and puts the wheel among them.
    #[test]
    fn the_pointer_and_the_wheel_come_through_as_the_panels_expect() {
        assert_eq!(
            button_down(1),
            Some(MouseEventKind::Down(MouseButton::Left))
        );
        assert_eq!(
            button_down(3),
            Some(MouseEventKind::Down(MouseButton::Right))
        );
        // The wheel is a button press on X and a scroll everywhere else.
        assert_eq!(button_down(4), Some(MouseEventKind::ScrollUp));
        assert_eq!(button_down(5), Some(MouseEventKind::ScrollDown));
        assert_eq!(button_of(4), None, "the wheel has no button to release");
        assert_eq!(button_down(9), None, "and a thumb button is not a click");

        // A motion with a button down is a drag — a fader being moved — and one
        // with none is a hover, which lights a control without changing it.
        let mut state = xproto::KeyButMask::default();
        assert_eq!(held(state), None);
        state |= xproto::KeyButMask::BUTTON1;
        assert_eq!(held(state), Some(MouseButton::Left));
    }
}
