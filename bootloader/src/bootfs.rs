// bootfs.rs — Simple File System access: pick the config volume (own volume
// first), then flat file/dir readers shared by conf.rs and linuxboot.rs.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::conf::{parse_config, Config};
use uefi::prelude::*;
use uefi::proto::loaded_image::LoadedImage;
use uefi::proto::media::file::{Directory, File, FileAttribute, FileMode, FileType};
use uefi::proto::media::fs::SimpleFileSystem;
use uefi::CString16;

/// Refuse to pool-allocate absurd files (scanners, not real distro kernels).
const MAX_FILE_BYTES: usize = 256 * 1024 * 1024;

fn c16(s: &str) -> Result<CString16, String> {
    CString16::try_from(s).map_err(|_| format!("'{s}' is not representable in UCS-2"))
}

pub fn open_volume(volume: Handle) -> Result<Directory, String> {
    let mut sfs = boot::open_protocol_exclusive::<SimpleFileSystem>(volume)
        .map_err(|e| format!("open SimpleFileSystem: {e}"))?;
    sfs.open_volume()
        .map_err(|e| format!("open volume root: {e}"))
}

/// Read a whole file on the given volume into memory.
pub fn read_file(volume: Handle, path: &str) -> Result<Vec<u8>, String> {
    let mut dir = open_volume(volume)?;
    let p = c16(path)?;
    let handle = dir
        .open(&p, FileMode::Read, FileAttribute::empty())
        .map_err(|e| format!("open {path}: {e}"))?;
    let mut file = match handle.into_type().map_err(|e| format!("{path}: {e}"))? {
        FileType::Regular(f) => f,
        FileType::Dir(_) => return Err(format!("{path}: is a directory")),
    };
    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = file.read(&mut buf).map_err(|e| format!("read {path}: {e}"))?;
        if n == 0 {
            break;
        }
        if out.len() + n > MAX_FILE_BYTES {
            return Err(format!("{path}: exceeds {MAX_FILE_BYTES} byte cap"));
        }
        out.extend_from_slice(&buf[..n]);
    }
    Ok(out)
}

/// List a directory: (name, is_dir), "." and ".." filtered out.
pub fn read_dir(volume: Handle, path: &str) -> Result<Vec<(String, bool)>, String> {
    let mut dir = open_volume(volume)?;
    let p = c16(path)?;
    let handle = dir
        .open(&p, FileMode::Read, FileAttribute::empty())
        .map_err(|e| format!("open {path}: {e}"))?;
    let mut dir = match handle.into_type().map_err(|e| format!("{path}: {e}"))? {
        FileType::Dir(d) => d,
        FileType::Regular(_) => return Err(format!("{path}: is a regular file")),
    };
    let mut out = Vec::new();
    while let Ok(Some(info)) = dir.read_entry_boxed() {
        let name = info.file_name().to_string();
        if name == "." || name == ".." {
            continue;
        }
        let is_dir = info.attribute().contains(FileAttribute::DIRECTORY);
        out.push((name, is_dir));
    }
    Ok(out)
}

/// The volume our own .efi was loaded from (preferred spot for boot.cfg).
fn own_boot_volume() -> Option<Handle> {
    let li = boot::open_protocol_exclusive::<LoadedImage>(boot::image_handle()).ok()?;
    li.device()
}

/// Find and parse \EFI\xios\boot.cfg. Own boot volume wins; otherwise the
/// first volume (firmware enumeration order) with a readable config.
pub fn find_config_volume() -> Option<(Handle, Config)> {
    let own = own_boot_volume();
    let handles = boot::find_handles::<SimpleFileSystem>().ok()?;
    let mut volumes: Vec<Handle> = handles.into_iter().collect();
    volumes.sort_by_key(|h| own != Some(*h));
    for volume in volumes {
        if let Ok(bytes) = read_file(volume, crate::CORE_CFG) {
            if bytes.is_empty() {
                continue;
            }
            let text = String::from_utf8_lossy(&bytes);
            return Some((volume, parse_config(&text)));
        }
    }
    None
}
