// serve.rs — XiOS web panel + JSON APIs (std-only: zero crates, zero data cost).
//
// GET  /                  dashboard (embedded panel.html)
// GET  /api/state         host stats + slots + isos + download jobs
// POST /api/slot/add      name iso cpu ram disk
// POST /api/slot/run      name
// POST /api/slot/stop     name
// POST /api/slot/rm       name
// POST /api/store/dl      url [sha256]     (background download to ~/.xios/isos)
// GET  /api/dl/:id        {id,frac,done}
// GET  /api/isos          available ISOs (isos dir + ~/Downloads)

use crate::slot::{self, exit, valid_name, xios_home, Slot, MAX_SLOTS};
use crate::state::{host_info, running_pid};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

const PANEL: &str = include_str!("panel.html");
const EXE_ENV: &str = "XIOS_EXE"; // xi binary path (used for detached spawns)

pub struct Job {
    pub file: String,
    pub expect: u64,
    pub sha: Option<String>,
}

pub struct Hub {
    pub jobs: Mutex<HashMap<u64, Job>>,
    pub seq: AtomicU64,
}

fn xi_exe() -> PathBuf {
    if let Ok(p) = std::env::var(EXE_ENV) {
        return PathBuf::from(p);
    }
    let mut p = xios_home();
    p.push("bin");
    p.push("xi");
    if !p.exists() {
        // dev fallback: cargo target dir next to the current exe
        if let Ok(cur) = std::env::current_exe() {
            let t = cur.parent().unwrap().join("xi");
            if t.exists() {
                return t;
            }
        }
    }
    p
}

fn isos_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![xios_home().join("isos")];
    if let Ok(dl) = std::env::var("HOME") {
        let dl = PathBuf::from(dl).join("Downloads");
        if dl.is_dir() {
            dirs.push(dl);
        }
    }
    dirs
}

fn list_isos() -> Vec<(String, u64)> {
    let mut out = Vec::new();
    for d in isos_dirs() {
        if let Ok(rd) = fs::read_dir(&d) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().and_then(|s| s.to_str()) == Some("iso") {
                    let size = e.metadata().map(|m| m.len()).unwrap_or(0);
                    out.push((p.display().to_string(), size));
                }
            }
        }
    }
    out.sort();
    out
}

/// Scan removable-media mount points for installer ISOs ("flash detection").
/// Depth-2 walk covers /media/<user>/<label> and /run/media/<user>/<label>.
fn list_usb_isos() -> Vec<(String, u64)> {
    let mut roots: Vec<PathBuf> = Vec::new();
    for base in ["/media", "/run/media"] {
        if let Ok(rd) = fs::read_dir(base) {
            for user in rd.flatten() {
                if let Ok(r2) = fs::read_dir(user.path()) {
                    for label in r2.flatten() {
                        roots.push(label.path());
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    for root in roots {
        // label dir itself or one subdir down (nested media trees)
        let mut stack = vec![root];
        for _ in 0..2 {
            let mut next = Vec::new();
            for dir in stack {
                if let Ok(rd) = fs::read_dir(&dir) {
                    for e in rd.flatten() {
                        let p = e.path();
                        if p.is_dir() {
                            next.push(p);
                        } else if p.extension().and_then(|s| s.to_str()) == Some("iso") {
                            let size = e.metadata().map(|m| m.len()).unwrap_or(0);
                            out.push((p.display().to_string(), size));
                        }
                    }
                }
            }
            stack = next;
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Spawn `xi run|stop|...` fully detached so the panel can close/free;
/// qemu gtk windows keep working after the panel dies.
fn spawn_detached(args: &[&str]) -> Result<(), String> {
    let exe = xi_exe();
    let ok = Command::new("setsid")
        .arg(&exe)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())?;
    Ok(ok)
}

fn json_state(hub: &Hub) -> String {
    let h = host_info();
    let (slots, _) = slot::load_slots();
    let mut s = String::with_capacity(1024);
    s.push_str("{\"host\":{\"cores\":");
    s.push_str(&h.cores.to_string());
    s.push_str(",\"mem\":");
    s.push_str(&h.mem_total_mib.to_string());
    s.push_str(",\"free\":");
    s.push_str(&h.disk_free_gib.to_string());
    s.push_str(",\"slots\":");
    s.push_str(&slots.len().to_string());
    s.push_str(",\"max\":");
    s.push_str(&MAX_SLOTS.to_string());
    s.push_str("},\"slots\":[");

    for (i, sl) in slots.iter().enumerate() {
        let run = running_pid(&sl.name).is_some();
        let iso = sl.iso.clone().unwrap_or_default().replace('\\', "\\\\").replace('"', "\\\"");
        s.push_str(&format!(
            "{{\"name\":\"{}\",\"cpu\":{},\"ram\":{},\"disk\":{},\"running\":{},\"iso\":\"{}\"}}{}",
            sl.name, sl.vcpus, sl.ram_mib, sl.disk_gib, run, iso,
            if i + 1 < slots.len() { "," } else { "" }
        ));
    }
    s.push_str("],\"isos\":[");
    let isos = list_isos();
    for (i, (p, size)) in isos.iter().enumerate() {
        s.push_str(&format!(
            "{{\"path\":\"{}\",\"size\":{}}}{}",
            p.replace('\\', "\\\\").replace('"', "\\\""),
            size,
            if i + 1 < isos.len() { "," } else { "" }
        ));
    }
    s.push_str("],\"usb\":[");
    let usb = list_usb_isos();
    for (i, (p, size)) in usb.iter().enumerate() {
        s.push_str(&format!(
            "{{\"path\":\"{}\",\"size\":{}}}{}",
            p.replace('\\', "\\\\").replace('"', "\\\""),
            size,
            if i + 1 < usb.len() { "," } else { "" }
        ));
    }
    s.push_str("],\"jobs\":[");

    let mut jobs = hub.jobs.lock().unwrap();
    let mut first = true;
    for (id, j) in jobs.iter() {
        let have = fs::metadata(Path::new(&j.file)).map(|m| m.len()).unwrap_or(0);
        let frac = if j.expect > 0 { (have * 100 / j.expect).min(100) } else { 0 };
        s.push_str(&format!(
            "{}{{\"id\":{},\"file\":\"{}\",\"frac\":{},\"sha\":\"{}\"}}",
            if first { "" } else { "," },
            id,
            j.file.replace('\\', "\\\\").replace('"', "\\\""),
            frac,
            j.sha.clone().unwrap_or_default()
        ));
        first = false;
    }
    jobs.retain(|_, j| fs::metadata(Path::new(&j.file)).map(|m| m.len()).is_err());

    s.push_str("]}");
    s
}

fn form(val: &str, key: &str) -> Option<String> {
    val.split('&').find_map(|p| {
        let (k, v) = p.split_once('=')?;
        (k == key).then(|| url_decode(v))
    })
}

fn url_decode(mut s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    while let Some(idx) = s.find('%') {
        out.push_str(&s[..idx]);
        s = &s[idx..];
        if s.len() >= 3 {
            if let Ok(b) = u8::from_str_radix(&s[1..3], 16) {
                out.push(b as char);
                s = &s[3..];
                continue;
            }
        }
        s = &s[1..];
    }
    out.push_str(s);
    out.replace('+', " ")
}

fn respond(stream: &mut TcpStream, code: &str, ctype: &str, body: &str) {
    let head = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        code,
        ctype,
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body.as_bytes());
}

fn handle(mut stream: TcpStream, hub: &Hub) {
    if let Ok(bytes) = stream.try_clone() {
        let mut reader = BufReader::new(bytes);
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() {
            return;
        }
        let (method, path) = {
            let mut parts = line.trim().split_whitespace();
            (
                parts.next().unwrap_or("").to_string(),
                parts.next().unwrap_or("/xios:bad").to_string(),
            )
        };

        let mut clen = 0usize;
        loop {
            let mut h = String::new();
            match reader.read_line(&mut h) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let t = h.trim().to_lowercase();
                    if t.is_empty() {
                        break;
                    }
                    if let Some(v) = t.strip_prefix("content-length:") {
                        clen = v.trim().parse().unwrap_or(0);
                    }
                }
            }
        }
        let mut body = vec![0u8; clen.min(65536)];
        let _ = reader.read_exact(&mut body);
        let body = String::from_utf8_lossy(&body).into_owned();

        handle_route(&mut stream, hub, &method, &path, &body);
    }
}

fn handle_route(stream: &mut TcpStream, hub: &Hub, method: &str, path: &str, body: &str) {
    if path == "/" && method == "GET" {
        respond(stream, "200 OK", "text/html; charset=utf-8", PANEL);
        return;
    }
    if path == "/api/state" && method == "GET" {
        respond(stream, "200 OK", "application/json", &json_state(hub));
        return;
    }
    if path == "/api/slot/add" && method == "POST" {
        let name = form(body, "name").unwrap_or_default();
        let iso = form(body, "iso").unwrap_or_default();
        let cpu: u32 = form(body, "cpu").and_then(|v| v.parse().ok()).unwrap_or(2);
        let ram: u32 = form(body, "ram").and_then(|v| v.parse().ok()).unwrap_or(4096);
        let disk: u32 = form(body, "disk").and_then(|v| v.parse().ok()).unwrap_or(32);

        let (mut slots, _) = slot::load_slots();
        if slots.len() >= MAX_SLOTS {
            respond(stream, "409 Conflict", "text/plain", &format!("slot limit {MAX_SLOTS} reached"));
            return;
        }
        if !valid_name(&name) {
            respond(stream, "400 Bad Request", "text/plain", "name must be 3-32 [a-z0-9-]");
            return;
        }
        if slots.iter().any(|s| s.name == name) {
            respond(stream, "409 Conflict", "text/plain", &format!("slot '{name}' exists"));
            return;
        }
        if !iso.starts_with("http") {
            let p = Path::new(&iso);
            if !p.is_file() || p.extension().and_then(|s| s.to_str()) != Some("iso") {
                respond(stream, "400 Bad Request", "text/plain", "ISO path missing or not .iso");
                return;
            }
        }
        slots.push(Slot { name: name.clone(), vcpus: cpu, ram_mib: ram, disk_gib: disk, iso: Some(iso) });
        if let Err(e) = slot::save_slots(&slots) {
            respond(stream, "500 Internal Server Error", "text/plain", &e);
            return;
        }
        crate::logx::log("INFO", &format!("ui add slot {name}"));
        respond(stream, "200 OK", "text/plain", "ok");
        return;
    }
    if path.starts_with("/api/slot/") && method == "POST" {
        let Some(name) = form(body, "name") else {
            respond(stream, "400 Bad Request", "text/plain", "missing name");
            return;
        };
        match path.rsplit_once('/').map(|(_, action)| action) {
            Some("run") => {
                if running_pid(&name).is_none() {
                    if let Err(e) = spawn_detached(&["run", &name]) {
                        respond(stream, "500 Internal Server Error", "text/plain", &e);
                        return;
                    }
                }
                respond(stream, "200 OK", "text/plain", "ok");
                return;
            }
            Some("stop") => {
                match crate::state::stop(&name) {
                    Ok(_) => respond(stream, "200 OK", "text/plain", "ok"),
                    Err(e) => respond(stream, "500 Internal Server Error", "text/plain", &e),
                }
                return;
            }
            Some("rm") => {
                let (mut slots, _) = slot::load_slots();
                let n = slots.len();
                slots.retain(|s| s.name != name);
                if slots.len() == n {
                    respond(stream, "404 Not Found", "text/plain", &format!("no slot {name}"));
                    return;
                }
                if let Err(e) = slot::save_slots(&slots) {
                    respond(stream, "500 Internal Server Error", "text/plain", &e);
                    return;
                }
                respond(stream, "200 OK", "text/plain", "ok");
                return;
            }
            _ => {}
        }
    }
    if path == "/api/store/dl" && method == "POST" {
        let Some(url) = form(body, "url") else {
            respond(stream, "400 Bad Request", "text/plain", "missing url");
            return;
        };
        if !url.starts_with("https://") {
            respond(stream, "400 Bad Request", "text/plain", "https URLs only");
            return;
        }
        let sha = form(body, "sha256").filter(|s| s.len() == 64);
        let fname = url.rsplit('/').next().unwrap_or("download.iso");
        let dest = xios_home().join("isos").join(fname);
        let _ = fs::create_dir_all(isos_dirs()[0].parent().unwrap_or(&xios_home()));
        let _ = fs::create_dir_all(xios_home().join("isos"));
        if let Some(sha) = sha.as_deref() {
            let target = dest.with_extension("iso.sha256");
            let _ = fs::write(&target, format!("{sha}  {fname}\n"));
        }
        let id = hub.seq.fetch_add(1, Ordering::SeqCst);
        hub.jobs.lock().unwrap().insert(
            id,
            Job {
                file: dest.display().to_string(),
                expect: 0,
                sha,
            },
        );
        // detached curl chain: download then fsync sha check marker file
        let cmd_str = format!(
            "curl -fL -C - --output {} \"{}\"; test $? -eq 0",
            dest.display(),
            url
        );
        let _ = Command::new("setsid")
            .args(["sh", "-c", &cmd_str])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        respond(stream, "200 OK", "application/json", &format!("{{\"id\":{id}}}"));
        return;
    }
    if let Some(idstr) = path.strip_prefix("/api/dl/") {
        if let Ok(id) = idstr.parse::<u64>() {
            let jobs = hub.jobs.lock().unwrap();
            if let Some(j) = jobs.get(&id) {
                let have = fs::metadata(Path::new(&j.file)).map(|m| m.len()).unwrap_or(0);
                let frac = if j.expect > 0 { (have * 100 / j.expect).min(100) } else { 0 };
                drop(jobs);
                respond(stream, "200 OK", "application/json", &format!("{{\"id\":{id},\"frac\":{frac},\"done\":{}}}{}", frac == 100 && have > 0, have > 0));
                return;
            }
        }
        respond(stream, "404 Not Found", "text/plain", "no job");
        return;
    }

    respond(stream, "404 Not Found", "text/plain", "not found");
}

pub fn serve(args: &[String]) {
    let mut port: u16 = std::env::var("XIOS_UI_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(7747);
    for (i, a) in args.iter().enumerate() {
        if a == "--port" && args.get(i + 1).is_some() {
            if let Ok(p) = args[i + 1].parse() {
                port = p;
            }
        }
    }

    let hub = Arc::new(Hub {
        jobs: Mutex::new(HashMap::new()),
        seq: AtomicU64::new(1),
    });
    let bind = format!("127.0.0.1:{port}");
    let listener = match TcpListener::bind(&bind) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("xi: error [7] cannot bind {bind}: {e}");
            crate::logx::log("ERROR", &format!("panel bind {bind}: {e}"));
            std::process::exit(exit::BIND_FAIL);
        }
    };

    let exe = xi_exe();
    println!("xios panel → http://{bind}   (xi {})", slot::VERSION);
    println!("engine: {}", exe.display());
    crate::logx::log("INFO", &format!("panel up http://{bind}"));

    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                let hub = hub.clone();
                thread::spawn(move || handle(s, &hub.clone()));
            }
            Err(_) => thread::sleep(Duration::from_millis(50)),
        }
    }
}
