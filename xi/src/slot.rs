// slot.rs — Slot type + registry (load/save/corruption-recovery) + unit tests.

use std::env;
use std::fs;
use std::path::PathBuf;

pub const VERSION: &str = "0.1.0";
pub const MAX_SLOTS: usize = 5;

/// POSIX exit code map (also printed in `xi help`):
/// 0 ok · 1 general error · 2 bad args · 3 missing dependency
/// 4 unknown slot · 5 qemu launch failure · 6 slot already running · 7 bind failure
pub mod exit {
    pub const GENERAL: i32 = 1;
    pub const ARGS: i32 = 2;
    pub const MISSING_DEP: i32 = 3;
    pub const NO_SLOT: i32 = 4;
    pub const QEMU_FAIL: i32 = 5;
    pub const ALREADY_RUNNING: i32 = 6;
    pub const BIND_FAIL: i32 = 7;
}

#[derive(Clone, Debug)]
pub struct Slot {
    pub name: String,
    pub vcpus: u32,
    pub ram_mib: u32,
    pub disk_gib: u32,
    pub iso: Option<String>,
}

pub fn xios_home() -> PathBuf {
    let base = env::var("XIOS_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let mut p = env::var("HOME").map(PathBuf::from).unwrap_or_default();
            p.push(".xios");
            p
        });
    let _ = fs::create_dir_all(&base);
    base
}

pub fn registry_path() -> PathBuf {
    let mut p = xios_home();
    p.push("slots.txt");
    p
}

/// Slot names: lowercase slug, 3..32 chars, [a-z0-9-], no leading/trailing '-'.
pub fn valid_name(name: &str) -> bool {
    let s: &str = name;
    (3..=32).contains(&s.len())
        && s.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && !s.starts_with('-')
        && !s.ends_with('-')
        && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

impl Slot {
    pub fn to_line(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}",
            self.name,
            self.vcpus,
            self.ram_mib,
            self.disk_gib,
            self.iso.clone().unwrap_or_default()
        )
    }

    pub fn try_from_line(line: &str) -> Option<Slot> {
        let f: Vec<&str> = line.split('|').collect();
        if f.len() != 5 {
            return None;
        }
        let name = f[0].trim().to_string();
        if !valid_name(&name) {
            return None;
        }
        Some(Slot {
            name,
            vcpus: f[1].trim().parse().ok()?,
            ram_mib: f[2].trim().parse().ok()?,
            disk_gib: f[3].trim().parse().ok()?,
            iso: {
                let s = f[4].trim().to_string();
                if s.is_empty() { None } else { Some(s) }
            },
        })
    }

    /// Sanity bounds so a typo can't ask the host for 4096 TiB.
    pub fn budget_ok(&self, host_vcpus: u32) -> Result<(), &'static str> {
        if self.vcpus == 0 || self.vcpus > host_vcpus.max(1) {
            return Err("vcpus out of range (1..=host cores)");
        }
        if !(64..=131072).contains(&self.ram_mib) {
            return Err("ram must be 64..131072 MiB");
        }
        if !(1..=4096).contains(&self.disk_gib) {
            return Err("disk must be 1..4096 GiB");
        }
        Ok(())
    }
}

/// Load the registry. Corrupt lines are skipped and reported via the
/// returned Vec<String> of recovery notes (production: never crash on data).
pub fn load_slots() -> (Vec<Slot>, Vec<String>) {
    let mut notes: Vec<String> = Vec::new();
    let path = registry_path();
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return (Vec::new(), notes),
    };
    let mut slots = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match Slot::try_from_line(line) {
            Some(s) => {
                if slots.iter().any(|x: &Slot| x.name == s.name) {
                    notes.push(format!("line {}: duplicate slot '{}' skipped", n + 1, s.name));
                } else {
                    slots.push(s);
                }
            }
            None => notes.push(format!("line {}: corrupt registry entry skipped", n + 1)),
        }
    }
    (slots, notes)
}

/// Atomic save (tmp + rename) — storage-level crash safety.
pub fn save_slots(slots: &[Slot]) -> Result<(), String> {
    let mut data: String = slots.iter().map(|s| s.to_line()).collect::<Vec<_>>().join("\n");
    if !data.is_empty() {
        data.push('\n');
    }
    let path = registry_path();
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, data).map_err(|e| format!("write failed: {e}"))?;
    fs::rename(&tmp, &path).map_err(|e| format!("atomic rename failed: {e}"))
}

/* --------------------------------- tests --------------------------------- */

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let slots = vec![
            Slot { name: "mint".into(), vcpus: 4, ram_mib: 8192, disk_gib: 64, iso: Some("/x/mint.iso".into()) },
            Slot { name: "a1x".into(), vcpus: 1, ram_mib: 1024, disk_gib: 8, iso: None },
        ];
        let text = slots.iter().map(|s| s.to_line()).collect::<Vec<_>>().join("\n");
        let back: Vec<Slot> = text.lines().filter_map(Slot::try_from_line).collect();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].name, "mint");
        assert_eq!(back[0].ram_mib, 8192);
        assert!(back[1].iso.is_none());
    }

    #[test]
    fn corrupt_lines_never_panic() {
        for bad in [
            "",
            "no-pipes-here",
            "a|b|c|d|e", // name invalid
            "badname|2|1024|10", // missing 5th
            "|2|1024|10|",
            "mint|zz|1024|10|",
            "mint|2|zz|10|",
            "mint|2|1024|zz|",
        ] {
            assert!(Slot::try_from_line(bad).is_none(), "should reject: {bad:?}");
        }
    }

    #[test]
    fn name_rules() {
        assert!(valid_name("mint"));
        assert!(valid_name("android-x86"));
        assert!(valid_name("a1x"));
        assert!(!valid_name("a"));          // too short
        assert!(!valid_name("UPPER"));
        assert!(!valid_name("-lead"));
        assert!(!valid_name("trail-"));
        assert!(!valid_name("has space"));
        assert!(!valid_name("has_underscore"));
    }

    #[test]
    fn budget_bounds() {
        let ok = Slot { name: "a1x".into(), vcpus: 4, ram_mib: 4096, disk_gib: 32, iso: None };
        assert!(ok.budget_ok(8).is_ok());
        assert!(ok.budget_ok(2).is_err()); // demands more cores than host
        assert!(Slot { name: "a1x".into(), vcpus: 2, ram_mib: 16, disk_gib: 32, iso: None }.budget_ok(8).is_err());
    }

    #[test]
    fn dedupe_and_recovery() {
        // feed a registry with good, bad and duplicate lines
        let lines = "a1x|2|1024|8|\nbroken-line\na1x|4|2048|16|\nb1y|2|1024|8|";
        let mut slots = Vec::new();
        let mut notes = 0;
        for line in lines.lines() {
            match Slot::try_from_line(line) {
                Some(s) => {
                    if slots.iter().any(|x: &Slot| x.name == s.name) {
                        notes += 1;
                    } else {
                        slots.push(s);
                    }
                }
                None => notes += 1,
            }
        }
        assert_eq!(slots.len(), 2);
        assert_eq!(notes, 2);
    }
}
