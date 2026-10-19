// state.rs — VM lifecycle: pidfiles, liveness detection, stop, host probing.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::slot::xios_home;

fn run_dir() -> PathBuf {
    let mut p = xios_home();
    p.push("run");
    let _ = fs::create_dir_all(&p);
    p
}

pub fn pidfile(name: &str) -> PathBuf {
    let mut p = run_dir();
    p.push(format!("{name}.pid"));
    p
}

/// Write pid atomically; returns Err if another VM already claims the slot.
pub fn claim(name: &str, pid: u32, qemu_pid: u32) -> Result<(), String> {
    let pf = pidfile(name);
    if pf.exists() {
        if let Ok(old) = fs::read_to_string(&pf) {
            if let Some(oldp) = parse_pid(&old) {
                if is_alive(oldp) {
                    return Err(format!("slot '{name}' already running (pid {oldp})"));
                }
            }
        }
    }
    let tmp = pf.with_extension("tmp");
    fs::write(&tmp, format!("{pid}\n{qemu_pid}\n")).map_err(|e| e.to_string())?;
    fs::rename(&tmp, &pf).map_err(|e| e.to_string())
}

pub fn release(name: &str) {
    let _ = fs::remove_file(pidfile(name));
}

fn parse_pid(text: &str) -> Option<u32> {
    text.lines().nth(1)?.trim().parse().ok()
}

/// 1 = alive, 0 = dead, -1 = can't tell (zombie/short window)
fn is_alive(pid: u32) -> bool {
    let st = Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    matches!(st, Ok(s) if s.success())
}

/// Returns running pid (qemu) if the slot has a live VM.
pub fn running_pid(name: &str) -> Option<u32> {
    let pf = pidfile(name);
    let text = fs::read_to_string(&pf).ok()?;
    let pid = parse_pid(&text)?;
    if is_alive(pid) {
        Some(pid)
    } else {
        let _ = fs::remove_file(&pf); // stale — self-heal
        None
    }
}

/// SIGTERM the VM's qemu and wait for it to die (up to ~5s), then release.
pub fn stop(name: &str) -> Result<bool, String> {
    let Some(pid) = running_pid(name) else {
        return Ok(false);
    };
    let _ = Command::new("kill")
        .arg(pid.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    for _ in 0..50 {
        if !is_alive(pid) {
            release(name);
            return Ok(true);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    release(name);
    Err(format!("slot '{name}' did not exit after SIGTERM (pid {pid}) — force with: kill {pid}"))
}

/* ----------------------------- host probing ------------------------------ */

pub struct Host { pub cores: u32, pub mem_total_mib: u64, pub disk_free_gib: u64 }

pub fn host_info() -> Host {
    let cores = fs::read_to_string("/sys/devices/system/cpu/present")
        .map(|t| {
            t.trim()
                .rsplit('-')
                .next()
                .and_then(|s| s.parse::<u32>().ok())
                .map(|last| last + 1)
                .unwrap_or(1)
        })
        .unwrap_or(1);

    let mem_total_mib = fs::read_to_string("/proc/meminfo")
        .map(|t| {
            t.lines()
                .find(|l| l.starts_with("MemTotal:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|n| n.parse::<u64>().ok())
                .unwrap_or(0)
        })
        .unwrap_or(0);

    let disk_free_gib = Command::new("df")
        .arg("-BG")
        .arg("/home")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|out| {
            out.lines().nth(1).and_then(|l| {
                l.split_whitespace().nth(3)?.trim_end_matches('G').parse::<u64>().ok()
            })
        })
        .unwrap_or(0);

    Host { cores, mem_total_mib, disk_free_gib }
}
