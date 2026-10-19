// main.rs — xi UEFI boot manager (see SPEC.md §4: boot firmware → XiOS base OS).
//
// Boot flow:
//   1. locate the volume that hosts /EFI/xios/boot.cfg (own volume first)
//   2. parse boot.cfg + entries/*
//   3. show branded picker (timeout, arrows, enter)
//   4. boot kernel via its EFI-stub (cmdline + initrd as load options),
//      or chain-load any other EFI binary
//   5. control returns here when a started image exits -> back to menu
//
// If no config is found we return to firmware so the next boot option runs.

#![no_main]
#![no_std]

#[macro_use]
extern crate uefi;

extern crate alloc;

mod bootfs;
mod brand;
mod conf;
mod linuxboot;
mod menu;

use uefi::prelude::*;

/// Directory conventions replicated by `xi boot-menu sync` (planned) and docs.
pub const CORE_CFG: &str = "\\EFI\\xios\\boot.cfg";
pub const ENTRY_DIR: &str = "\\EFI\\xios\\entries";

fn xi_boot() {
    let Some((volume, mut c)) = bootfs::find_config_volume() else {
        println!("xi: no {CORE_CFG} on any disk found — continuing boot order");
        return;
    };
    log::info!("xi: config volume ok");

    let entries = conf::load_entries(&volume, ENTRY_DIR, &mut c);
    if entries.is_empty() {
        println!("xi: no entries in \\EFI\\xios\\entries — continuing boot order");
        return;
    }
    // Builtin cleanup entry (? not): plain firmware return exists below via ESC.
    let default_idx = match conf::pick_default(&entries, c.default_title) {
        Some(i) => i,
        None => 0,
    };

    loop {
        match menu::choose(&entries, default_idx, c.timeout_secs) {
            Some(i) => {
                let e = &entries[i];
                let tag = linuxboot::start_boot_entry(&volume, e);
                if let Err(msg) = tag {
                    println!("xi: boot '{}' failed: {msg}", e.title);
                    menu::pause("\npress any key to return to menu ... ");
                }
            }
            None => {
                println!("xi: → back to firmware, boot next disk");
                return;
            }
        }
    }
}

#[entry]
fn main() -> Status {
    uefi::helpers::init().unwrap();
    // Menus and downloads can outlive the firmware's 5-minute boot watchdog.
    let _ = boot::set_watchdog_timer(0, 0, None);
    brand::clear_screen();

    // Panics are reported by uefi's own panic handler; keep booting onward.
    xi_boot();
    Status::SUCCESS
}
