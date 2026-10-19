// menu.rs — branded picker: arrows change selection, enter boots, esc returns
// to firmware; the countdown auto-boots the default entry.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use uefi::prelude::*;
use uefi::proto::console::text::{Key, Output, ScanCode};
use uefi::system;

use crate::brand::{self, FAINT, INK, MUTED, ROW_BASE, SELECT_BG, SELECT_FG, SURFACE};
use crate::conf::Entry;

const KEY_ENTER: u16 = 0x000d;
const KEY_SPACE: u16 = 0x0020;

fn confirm(k: &Key) -> bool {
    match k {
        Key::Printable(c) => {
            let v = u16::from(*c);
            v == KEY_ENTER || v == KEY_SPACE
        }
        Key::Special(_) => false,
    }
}

fn entry_line(e: &Entry, i: usize, sel: usize) -> String {
    let cursor = if i == sel { " >" } else { "  " };
    format!("{cursor} {:02}  {}", i + 1, e.title)
}

fn draw_row(o: &mut Output, entries: &[Entry], i: usize, sel: usize, cols: usize) {
    let (fg, bg) = if i == sel {
        (SELECT_FG, SELECT_BG)
    } else {
        (INK, SURFACE)
    };
    let _ = o.set_color(fg, bg);
    brand::put_at(o, cols, ROW_BASE + i, &entry_line(&entries[i], i, sel));
}

fn footer_text(entries: &[Entry], sel: usize, left: u64) -> String {
    let name = entries.get(sel).map(|e| e.title.as_str()).unwrap_or("?");
    let countdown = if left > 0 {
        format!("auto-boot in {left}s")
    } else {
        "now".to_string()
    };
    format!("  '{name}' - {countdown} - [UP]/[DOWN] move  [ENTER] boot  [ESC] firmware")
}

fn draw_footer(o: &mut Output, entries: &[Entry], sel: usize, left: u64, cols: usize) {
    let row = ROW_BASE + entries.len() + 1;
    let _ = o.set_color(FAINT, SURFACE);
    brand::put_at(o, cols, row, &footer_text(entries, sel, left));
    let _ = o.set_color(INK, SURFACE);
}

fn draw_screen(entries: &[Entry], sel: usize, left: u64) {
    let cols = brand::width();
    system::with_stdout(|o| {
        let _ = o.set_color(INK, SURFACE);
        let _ = o.clear();
        let _ = o.set_cursor_position(0, 0);
        brand::draw_header(o, cols);
        for i in 0..entries.len() {
            draw_row(o, entries, i, sel, cols);
        }
        draw_footer(o, entries, sel, left, cols);
    });
}

fn refresh(entries: &[Entry], sel: usize, left: u64) {
    let cols = brand::width();
    system::with_stdout(|o| {
        for i in 0..entries.len() {
            draw_row(o, entries, i, sel, cols);
        }
        draw_footer(o, entries, sel, left, cols);
    });
}

/// Blocks until a key is available (no busy-waiting).
pub fn pause(msg: &str) {
    brand::set(MUTED, SURFACE);
    println!();
    println!("{msg}");
    let Ok(ev) = system::with_stdin(|i| i.wait_for_key_event()) else {
        return;
    };
    let events = Vec::from([ev]);
    let _ = boot::wait_for_event(&events);
    let _ = system::with_stdin(|i| i.read_key());
    brand::set(INK, SURFACE);
}

/// Show the picker; `Some(idx)` = boot choice, `None` = back to firmware.
pub fn choose(entries: &[Entry], default_idx: usize, timeout_secs: u64) -> Option<usize> {
    // Menus are idle; the firmware's 5-min watchdog would otherwise reboot us.
    let _ = boot::set_watchdog_timer(0, 0, None);
    if entries.is_empty() {
        return None;
    }
    let mut sel = default_idx.min(entries.len() - 1);

    let key_evt = system::with_stdin(|i| i.wait_for_key_event()).ok()?;
    let timer_evt =
        unsafe { boot::create_event(boot::EventType::TIMER, boot::Tpl::APPLICATION, None, None) }
            .ok()?;
    let events = Vec::from([key_evt, timer_evt]);

    let mut left = timeout_secs;
    draw_screen(entries, sel, left);
    if timeout_secs == 0 {
        return Some(sel);
    }

    loop {
        let _ = boot::set_timer(
            &events[1],
            boot::TimerTrigger::Relative(core::time::Duration::from_secs(1)),
        );
        match boot::wait_for_event(&events) {
            Ok(0) => match system::with_stdin(|i| i.read_key()) {
                Ok(Some(Key::Special(ScanCode::UP))) => {
                    if sel > 0 {
                        sel -= 1;
                        left = 0;
                        refresh(entries, sel, left);
                    }
                }
                Ok(Some(Key::Special(ScanCode::DOWN))) => {
                    if sel + 1 < entries.len() {
                        sel += 1;
                        left = 0;
                        refresh(entries, sel, left);
                    }
                }
                Ok(Some(Key::Special(ScanCode::ESCAPE))) => {
                    brand::clear_screen();
                    return None;
                }
                Ok(Some(k)) if confirm(&k) => {
                    brand::clear_screen();
                    return Some(sel);
                }
                _ => {}
            },
            Ok(_) => {
                if left > 0 {
                    left -= 1;
                    refresh(entries, sel, left);
                    if left == 0 {
                        brand::clear_screen();
                        return Some(sel);
                    }
                }
            }
            Err(_) => {
                boot::stall(core::time::Duration::from_secs(1));
                if left > 0 {
                    left -= 1;
                    refresh(entries, sel, left);
                    if left == 0 {
                        brand::clear_screen();
                        return Some(sel);
                    }
                }
            }
        }
    }
}
