// brand.rs — the "xi" visual identity on the UEFI console (colors, header,
// padded row painter shared with menu.rs).

use alloc::format;

use uefi::CString16;
use uefi::proto::console::text::{Color, Output};
use uefi::system;

/// Brand palette.
pub const ACCENT: Color = Color::LightBlue;
pub const INK: Color = Color::White;
pub const SURFACE: Color = Color::Black;
pub const MUTED: Color = Color::LightGray;
pub const FAINT: Color = Color::DarkGray;
/// Highlight row: white text on the brand accent.
pub const SELECT_FG: Color = Color::White;
pub const SELECT_BG: Color = Color::Blue;

/// Screen row where menu entries start (below the 3-line header).
pub const ROW_BASE: usize = 4;

pub fn set(fg: Color, bg: Color) {
    system::with_stdout(|o| {
        let _ = o.set_color(fg, bg);
    });
}

pub fn clear_screen() {
    system::with_stdout(|o| {
        let _ = o.set_color(INK, SURFACE);
        let _ = o.clear();
        let _ = o.set_cursor_position(0, 0);
    });
}

/// Screen width of the active text mode (classic 80 fallback).
pub fn width() -> usize {
    let w = system::with_stdout(|o| {
        o.current_mode()
            .ok()
            .flatten()
            .map(|m| m.columns())
            .unwrap_or(80)
    });
    w.min(200)
}

/// Write exactly one full-width line at `row` — the space padding repaints
/// the background when rows change, so menus redraw in place without ghosts.
pub fn put_at(o: &mut Output, cols: usize, row: usize, s: &str) {
    let _ = o.set_cursor_position(0, row);
    let width = cols.saturating_sub(1);
    let padded = format!("{s:<w$}", w = width);
    if let Ok(c) = CString16::try_from(padded.as_str()) {
        let _ = o.output_string(&c);
    }
}

/// The 3 header rows above ROW_BASE.
pub fn draw_header(o: &mut Output, cols: usize) {
    let _ = o.set_color(ACCENT, SURFACE);
    put_at(o, cols, 0, "  xi  ·  XiOS boot manager");
    let _ = o.set_color(MUTED, SURFACE);
    put_at(o, cols, 1, "  one machine, many systems");
    let _ = o.set_color(FAINT, SURFACE);
    put_at(o, cols, 2, "");
    let _ = o.set_color(INK, SURFACE);
}
