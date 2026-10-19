// logx.rs — append-only operation log with rotate (syncro-style state log).

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::slot::xios_home;

pub fn log_path() -> PathBuf {
    let mut p = xios_home();
    p.push("log");
    let _ = fs::create_dir_all(&p);
    p.push("xi.log");
    p
}

/// 2026-10-18T01:22:33Z LEVEL message
pub fn log(level: &str, msg: &str) {
    let line = format!(
        "{bare} {LEVEL} {msg}\n",
        LEVEL = level,
        msg = msg,
        bare = iso_now()
    );

    let path = log_path();
    // rotate at ~512 KiB (xi.log -> xi.log.1, single level)
    if let Ok(md) = fs::metadata(&path) {
        if md.len() > 512 * 1024 {
            let _ = fs::rename(&path, path.with_extension("log.1"));
        }
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = f.write_all(line.as_bytes());
    }
}

/// Real UTC ISO-8601 timestamp without external crates.
fn iso_now() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let days = secs / 86400;
    let time = secs % 86400;
    let (h, m, s) = (time / 3600, time % 3600 / 60, time % 60);

    // civil-from-days (Howard Hinnant's algorithm), correct for all dates.
    let z = days as i64 + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };

    format!("{year:04}-{month:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}
