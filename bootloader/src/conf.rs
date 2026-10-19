// conf.rs — boot.cfg + per-entry settings parsing ( \@EFI\xios\boot.cfg + entries\*.cfg ).

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::bootfs::{read_dir, read_file};
use uefi::Handle;

pub const DEFAULT_TIMEOUT: u64 = 10;

/// Core settings from boot.cfg.
#[derive(Default)]
pub struct Config {
    pub default_title: Option<String>,
    pub timeout_secs: u64,
}

/// One bootable item (Linux EFI-stub kernel or a chain-loaded .efi).
#[derive(Clone)]
pub struct Entry {
    /// File stem of this entry's .cfg (matches `default=` in boot.cfg).
    pub id: String,
    pub title: String,
    pub kernel: Option<String>,
    pub initrd: Option<String>,
    pub cmdline: String,
    pub chain: Option<String>,
}

fn kv(line: &str) -> Option<(&str, &str)> {
    let (k, v) = line.split_once('=')?;
    Some((k.trim(), v.trim()))
}

fn set_str(dst: &mut Option<String>, v: &str) {
    if !v.is_empty() {
        *dst = Some(v.to_string());
    }
}

pub fn parse_config(text: &str) -> Config {
    let mut c = Config::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match kv(line) {
            Some(("default", v)) => set_str(&mut c.default_title, v),
            Some(("timeout", v)) => {
                c.timeout_secs = v.parse::<u64>().unwrap_or(DEFAULT_TIMEOUT).clamp(0, 3600);
            }
            _ => {}
        }
    }
    c
}

pub fn parse_entry(text: &str, fallback_title: &str) -> Option<Entry> {
    let mut e = Entry {
        id: fallback_title.trim().replace(" ", "-").to_lowercase(),
        title: fallback_title.to_string(),
        kernel: None,
        initrd: None,
        cmdline: String::new(),
        chain: None,
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match kv(line) {
            Some((k, v)) if k == "title" => {
                e.title = v.to_string();
            }
            Some((k, v)) if k == "kernel" => set_str(&mut e.kernel, v),
            Some((k, v)) if k == "initrd" => set_str(&mut e.initrd, v),
            Some((k, v)) if k == "cmdline" => e.cmdline = v.to_string(),
            Some((k, v)) if k == "chain" => set_str(&mut e.chain, v),
            _ => {}
        }
    }
    if e.kernel.is_none() && e.chain.is_none() {
        return None;
    }
    Some(e)
}

/// Load every valid \*.cfg in `base_dir` (a volume-relative path). Order is
/// deterministic: sorted by file name. `cfg` currently only reviewed, not
/// mutated — kept as a param so future per-OS defaults can slot in cleanly.
pub fn load_entries(volume: &Handle, base_dir: &str, _cfg: &mut Config) -> Vec<Entry> {
    let mut entries = Vec::new();
    let Ok(items) = read_dir(*volume, base_dir) else {
        return entries;
    };
    let mut names: Vec<String> = items
        .into_iter()
        .filter(|(_, is_dir)| !is_dir)
        .map(|(name, _)| name)
        .filter(|name| name.to_lowercase().ends_with(".cfg"))
        .collect();
    names.sort_by_key(|a| a.to_lowercase());
    for name in names {
        let path = format!("{base_dir}\\{name}");
        let fallback = name
            .strip_suffix(".cfg")
            .unwrap_or(name.as_str())
            .replace('_', " ");
        if let Ok(text) = read_file(*volume, &path) {
            if let Some(e) = parse_entry(&String::from_utf8_lossy(&text), &fallback) {
                entries.push(e);
            }
        }
    }
    entries
}

pub fn pick_default(entries: &[Entry], want: Option<String>) -> Option<usize> {
    let want = want?;
    let want_norm = want.trim().to_lowercase();
    if want_norm.is_empty() {
        return None;
    }
    entries.iter().position(|e| e.id == want_norm)
}
