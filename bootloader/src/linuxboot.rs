// linuxboot.rs — start a boot entry: Linux kernel via its EFI stub
// (cmdline + initrd= passed as load options), or chain-load any EFI binary
// from its real device path so the guest resolves its own resources.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use uefi::prelude::*;
use uefi::proto::device_path::build::{self, DevicePathBuilder};
use uefi::proto::device_path::DevicePath;
use uefi::proto::loaded_image::LoadedImage;
use uefi::proto::BootPolicy;
use uefi::boot::LoadImageSource;
use uefi::CString16;

use crate::conf::Entry;

pub fn start_boot_entry(volume: &Handle, e: &Entry) -> Result<(), String> {
    if let Some(chain) = e.chain.as_deref() {
        let opts = if e.cmdline.is_empty() {
            None
        } else {
            Some(e.cmdline.as_str())
        };
        boot_image(volume, chain, opts)
    } else if let Some(kernel) = e.kernel.as_deref() {
        // EFI stub convention: full cmdline, plus `initrd=<path>` pointing at
        // the initramfs on the same volume as the kernel image.
        let mut opts = String::new();
        if !e.cmdline.is_empty() {
            opts.push_str(e.cmdline.trim());
        }
        if let Some(initrd) = e.initrd.as_deref() {
            if !opts.is_empty() {
                opts.push(' ');
            }
            opts.push_str("initrd=");
            opts.push_str(initrd);
        }
        let opts = if opts.is_empty() { None } else { Some(opts.as_str()) };

        println!("xi: booting '{}' — kernel {kernel}", e.title);
        boot_image(volume, kernel, opts)
    } else {
        Err("entry has neither `kernel` nor `chain`".to_string())
    }
}

/// UCS-2 (LE) bytes for the load options, NUL-terminated. The returned Vec
/// must stay alive until the image has been started (the firmware only
/// records the pointer, it does not copy).
fn opts_bytes(s: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity((s.len() + 1) * 2);
    for ch in s.chars() {
        let u = u16::try_from(ch as u32).map_err(|_| format!("'{s}' not UCS-2"))?;
        out.extend_from_slice(&u.to_le_bytes());
    }
    out.extend_from_slice(&0u16.to_le_bytes());
    Ok(out)
}

/// Load `path` (relative to `volume`) as an EFI image **by device path** —
/// the image keeps a real DeviceHandle, so guests can open files next to
/// themselves (initrd=, shim configs, …) — then set load options and start.
fn boot_image(volume: &Handle, path: &str, opts: Option<&str>) -> Result<(), String> {
    // Build: <volume device path> + FilePath("<path>").
    let mut dp_vec = Vec::with_capacity(512);
    let device_path: &DevicePath = {
        let vol_dp = boot::open_protocol_exclusive::<DevicePath>(*volume)
            .map_err(|err| format!("open DevicePath: {err}"))?;
        let mut builder = DevicePathBuilder::with_vec(&mut dp_vec);
        for node in vol_dp.node_iter() {
            builder = builder.push(&node).map_err(|err| format!("device path: {err}"))?;
        }
        let px = CString16::try_from(path).map_err(|_| format!("'{path}' not UCS-2"))?;
        let builder = builder
            .push(&build::media::FilePath { path_name: &px })
            .map_err(|err| format!("device path: {err}"))?;
        builder.finalize().map_err(|err| format!("device path: {err}"))?
    };

    let image = boot::load_image(
        boot::image_handle(),
        LoadImageSource::FromDevicePath {
            device_path,
            boot_policy: BootPolicy::ExactMatch,
        },
    )
    .map_err(|err| format!("load_image: {err}"))?;

    let opts_buf = opts.map(opts_bytes).transpose()?;
    if let Some(bytes) = &opts_buf {
        let mut img = boot::open_protocol_exclusive::<LoadedImage>(image)
            .map_err(|err| format!("open LoadedImage: {err}"))?;
        unsafe { img.set_load_options(bytes.as_ptr(), bytes.len() as u32) };
    }

    // The kernel stub exits boot services itself; StartImage only returns
    // control here when the image comes back (normally: never — the machine
    // is now running the slot OS).
    boot::start_image(image).map_err(|err| format!("boot: {err}"))
}
