// opts-echo.rs — diagnostic: print this image's LoadOptions (and device path)
// so the loader bypass paths can be verified. Printed to the console, which
// under OVMF mirrors to the serial log.

#![no_main]
#![no_std]

#[macro_use]
extern crate uefi;

extern crate alloc;

use uefi::prelude::*;
use uefi::proto::loaded_image::LoadedImage;

#[entry]
fn main() -> Status {
    uefi::helpers::init().unwrap();
    let _ = boot::set_watchdog_timer(0, 0, None);

    println!("opts-echo: hola from the probe image");

    if let Ok(li) = boot::open_protocol_exclusive::<LoadedImage>(boot::image_handle()) {
        println!("opts-echo: device present: {}", li.device().is_some());
        match li.load_options_as_cstr16() {
            Ok(opts) => println!("opts-echo: options = \"{opts}\""),
            Err(e) => println!("opts-echo: options error = {e:?}"),
        }
    } else {
        println!("opts-echo: could not open LoadedImage on self");
    }

    println!("opts-echo: done — returning to loader");
    Status::SUCCESS
}
