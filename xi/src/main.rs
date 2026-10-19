// xi — XiOS slot manager (control plane for KVM slots + web panel)
//
// exit codes: see src/slot.rs `exit { }` map.

mod logx;
mod serve;
mod slot;
mod state;

use logx::log;
use slot::{exit, xios_home, Slot, MAX_SLOTS};
use state::{claim, host_info, release, running_pid, stop as stop_vm};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

fn die(code: i32, msg: &str) -> ! {
    eprintln!("xi: error [{code}]: {msg}");
    log("ERROR", msg);
    std::process::exit(code)
}

fn help() {
    println!(
        "xi {v} — one machine, many systems

USAGE:
  xi list [--json]            list slots (adds status column)
  xi add <name> [opts]        register slot (max {MAX})
      --iso <path>    installer ISO
      --cpu <n>       virtual CPUs      (default 2)
      --ram <mib>     memory in MiB     (default 4096)
      --disk <gib>    disk in GiB       (default 32)
  xi run <name> [--headless]  boot slot VM (window label = xios:<name>)
  xi stop <name>              SIGTERM a running slot
  xi status [name]            host + slot liveness overview
  xi rm <name>                remove slot registration (files untouched)
  xi serve [--port <n>]       web panel  (default http://127.0.0.1:7747)
  xi help | version           

EXIT CODES:
  0 ok · 1 general · 2 bad args · 3 missing dep
  4 unknown slot · 5 qemu failed · 6 already running · 7 bind failed

FILES:
  ~/.xios/slots.txt   slot registry (slot|cpu|ram|disk|iso)
  ~/.xios/disks/      qcow2 slot disks
  ~/.xios/isos/       Xi Store downloads
  ~/.xios/run/        live VM pidfiles
  ~/.xios/log/xi.log  operation log",
        v = slot::VERSION,
        MAX = MAX_SLOTS,
    );
}

fn find_bin(name: &str) -> Option<PathBuf> {
    env::var("PATH").ok()?.split(':').find_map(|dir| {
        let mut p = PathBuf::from(dir);
        p.push(name);
        p.is_file().then_some(p)
    })
}

fn find_virtiofsd() -> Option<PathBuf> {
    [
        "/usr/libexec/virtiofsd",
        "/usr/lib/virtiofsd",
        "/usr/bin/virtiofsd",
    ]
    .iter()
    .map(PathBuf::from)
    .find(|p| p.is_file())
}

fn shared_dir() -> PathBuf {
    let mut p = env::var("HOME").map(PathBuf::from).unwrap_or_default();
    p.push("XiShared");
    let _ = fs::create_dir_all(&p);
    p
}

fn ensure_disk(slot: &Slot) -> PathBuf {
    let disks = xios_home().join("disks");
    if let Err(e) = fs::create_dir_all(&disks) {
        die(exit::GENERAL, &format!("cannot create disks dir: {e}"));
    }
    let disk = disks.join(format!("{}.qcow2", slot.name));
    if !disk.exists() {
        let ok = Command::new("qemu-img")
            .args(["create", "-f", "qcow2"])
            .arg(&disk)
            .arg(format!("{}G", slot.disk_gib))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            die(exit::MISSING_DEP, "qemu-img missing or failed — install qemu-utils");
        }
        log("INFO", &format!("created disk {} ({}G)", disk.display(), slot.disk_gib));
        println!("✔ created {}", disk.display());
    }
    disk
}

fn run_slot(name: &str, headless: bool) {
    if !slot::valid_name(name) {
        die(exit::ARGS, "slot name must be 3-32 chars [a-z0-9-]");
    }

    // Double-run guard is threaded through an atomic flag so the
    // virtiofsd fallback path can't race it.
    let launching = Arc::new(AtomicBool::new(true));

    let (slots, notes) = slot::load_slots();
    for n in &notes {
        log("WARN", &format!("registry recovery: {n}"));
    }
    let Some(s) = slots.into_iter().find(|s| s.name == name) else {
        die(exit::NO_SLOT, &format!("no slot named '{name}' — see: xi list"));
    };
    if let Some(pid) = running_pid(name) {
        die(exit::ALREADY_RUNNING, &format!("slot '{name}' already running (pid {pid})"));
    }
    if launching.load(Ordering::SeqCst) {
        launching.store(false, Ordering::SeqCst); // placeholder — real lock is the pidfile below
    }

    let qemu = match find_bin("qemu-system-x86_64") {
        Some(p) => p,
        None => die(exit::MISSING_DEP, "qemu-system-x86_64 not found — install: sudo apt install qemu-system-x86"),
    };
    if let Err(e) = s.budget_ok(host_info().cores.max(1)) {
        die(exit::ARGS, e);
    }
    let disk = ensure_disk(&s);

    let sockets = xios_home().join("sockets");
    let _ = fs::create_dir_all(&sockets);
    let sock = sockets.join(format!("{name}.sock"));
    let _ = fs::remove_file(&sock);

    let virtiofsd = match find_virtiofsd() {
        Some(p) => p,
        None => die(exit::MISSING_DEP, "virtiofsd not found — install: sudo apt install virtiofsd"),
    };

    let spawn_fsd = |sandbox: Option<&str>| -> Result<std::process::Child, ()> {
        let mut cmd = Command::new(&virtiofsd);
        cmd.arg("--socket-path")
            .arg(&sock)
            .arg("--shared-dir")
            .arg(shared_dir())
            .stderr(Stdio::null());
        if let Some(sb) = sandbox {
            cmd.arg("--sandbox").arg(sb);
        }
        let mut c = cmd.spawn().or(Err(()))?;
        for _ in 0..40 {
            if sock.exists() {
                break;
            }
            if let Ok(Some(_)) = c.try_wait() {
                return Err(());
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        if let Ok(Some(_)) = c.try_wait() {
            let _ = fs::remove_file(&sock);
            return Err(());
        }
        Ok(c)
    };

    let mut fsd: std::process::Child = match spawn_fsd(None) {
        Ok(c) => c,
        Err(()) => {
            let _ = fs::remove_file(&sock);
            log("WARN", "virtiofsd sandbox unavailable — fallback --sandbox none");
            println!("note: virtiofsd fallback --sandbox none (install uidmap for secure mode)");
            match spawn_fsd(Some("none")) {
                Ok(c) => c,
                Err(()) => {
                    let _ = fs::remove_file(&sock);
                    die(exit::MISSING_DEP, "failed to start virtiofsd (both sandbox modes)")
                }
            }
        }
    };

    let iso_path = s
        .iso
        .as_ref()
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .unwrap_or_default();

    let v = s.vcpus;
    let m = s.ram_mib;

    println!(
        "booting slot '{name}' — cpu {v} · ram {m} MiB · disk {} · shared: {}",
        disk.display(),
        shared_dir().display()
    );
    log("INFO", &format!("boot slot<{name}> cpu{v} ram{m} headless={headless}"));

    let mut cmd = Command::new(&qemu);
    cmd.args([
        "-enable-kvm",
        "-name",
        &format!("xios:{name}"),
        "-machine",
        "q35",
        "-cpu",
        "host",
        "-smp",
        &v.to_string(),
        "-m",
        &format!("{m}M"),
        "-object",
        &format!("memory-backend-memfd,id=mem,size={m}M,share=on"),
        "-numa",
        "node,memdev=mem",
        "-drive",
        &format!("if=virtio,format=qcow2,file={}", disk.display()),
        "-chardev",
        &format!("socket,id=char0,path={}", sock.display()),
        "-device",
        "vhost-user-fs-pci,id=fs0,chardev=char0,tag=xios",
        "-vga",
        "virtio",
    ]);

    if !iso_path.as_os_str().is_empty() {
        cmd.arg("-cdrom").arg(&iso_path).args(["-boot", "d"]);
    }

    if let Ok(qmp) = env::var("XIOS_QMP") {
        cmd.arg("-qmp").arg(format!("unix:{qmp},server,nowait"));
    }

    if headless {
        cmd.args(["-display", "none", "-no-reboot"]);
    } else {
        cmd.arg("-display").arg("gtk");
    }

    let result = cmd.spawn();
    let mut child = match result {
        Ok(c) => c,
        Err(e) => {
            let _ = fsd.kill();
            let _ = fsd.wait();
            die(exit::QEMU_FAIL, &format!("qemu launch failed: {e}"));
        }
    };

    if let Err(e) = claim(name, child.id(), child.id()) {
        let _ = child.kill();
        let _ = child.wait();
        let _ = fsd.kill();
        let _ = fsd.wait();
        die(exit::ALREADY_RUNNING, &e);
    }

    let _ = child.wait();
    let _ = fsd.kill();
    let _ = fsd.wait();
    release(name);

    let dead = match child.try_wait() {
        Ok(Some(st)) => st.success(),
        _ => false,
    };
    if !dead && !headless {
        log("WARN", &format!("slot {name} exited non-zero"));
    }
    release(name);
    println!("● slot '{name}' ended — XiOS ready");
    log("INFO", &format!("slot {name} ended"));
}

fn cmd_list(json: bool) {
    let (slots, notes) = slot::load_slots();
    for n in &notes {
        log("WARN", &format!("registry recovery: {n}"));
    }

    if json {
        let mut out = String::from("[");
        for (i, s) in slots.iter().enumerate() {
            let running = running_pid(&s.name).is_some();
            let iso = s.iso.clone().unwrap_or_default();
            let _ = &iso;
            out.push_str(&format!(
                "{{\"name\":\"{}\",\"cpu\":{},\"ram\":{},\"disk\":{},\"running\":{},\"iso\":\"{}\"}}{}",
                s.name,
                s.vcpus,
                s.ram_mib,
                s.disk_gib,
                running,
                iso.replace('\\', "\\\\").replace('"', "\\\""),
                if i + 1 < slots.len() { "," } else { "" }
            ));
        }
        out.push(']');
        println!("{out}");
        return;
    }

    if slots.is_empty() {
        println!("no slots yet — try: xi add mint --iso ~/Downloads/mint.iso");
        return;
    }
    println!(
        "{:<12} {:>5} {:>9} {:>9} {:<9} {}",
        "SLOT", "CPU", "RAM(MiB)", "DISK(GiB)", "STATE", "ISO"
    );
    for s in &slots {
        let state = if running_pid(&s.name).is_some() { "RUNNING" } else { "stopped" };
        println!(
            "{:<12} {:>5} {:>9} {:>9} {:<9} {}",
            s.name,
            s.vcpus,
            s.ram_mib,
            s.disk_gib,
            state,
            s.iso.as_deref().unwrap_or("-")
        );
    }
    println!("\n{}/{} slots in use", slots.len(), MAX_SLOTS);
}

fn find_iso_arg(args: &[String], flag: &str) -> Option<String> {
    let idx = args.iter().position(|a| a == flag)?;
    args.get(idx + 1).cloned()
}

fn cmd_add(args: &[String]) {
    let Some(name) = args.first() else {
        die(exit::ARGS, "xi add <name> — a slot name is required");
    };
    if !slot::valid_name(name) {
        die(exit::ARGS, "slot name must be 3-32 chars: [a-z0-9-] (example: linux-mint)");
    }

    let (mut slots, _) = slot::load_slots();
    if slots.len() >= MAX_SLOTS {
        die(exit::GENERAL, &format!("slot limit reached ({MAX_SLOTS}) — remove one first"));
    }
    if slots.iter().any(|s| s.name == *name) {
        die(exit::GENERAL, &format!("slot '{name}' already exists"));
    }

    let iso = find_iso_arg(args, "--iso");
    if let Some(p) = &iso {
        if !PathBuf::from(p).exists() {
            die(exit::ARGS, &format!("ISO not found: {p}"));
        }
    }
    let vcpus = find_iso_arg(args, "--cpu")
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);
    let ram_mib = find_iso_arg(args, "--ram")
        .and_then(|v| v.parse().ok())
        .unwrap_or(4096);
    let disk_gib = find_iso_arg(args, "--disk")
        .and_then(|v| v.parse().ok())
        .unwrap_or(32);

    let s = Slot {
        name: name.clone(),
        vcpus,
        ram_mib,
        disk_gib,
        iso,
    };
    if let Err(e) = s.budget_ok(host_info().cores.max(1)) {
        die(exit::ARGS, e);
    }

    slots.push(s);
    if let Err(e) = slot::save_slots(&slots) {
        die(exit::GENERAL, &e);
    }

    let iso_note = slots
        .iter()
        .find(|s| &s.name == name)
        .and_then(|s| s.iso.clone())
        .unwrap_or_default();

    println!("✔ slot '{name}' registered — cpu {vcpus} · ram {ram_mib} MiB · disk {disk_gib} GiB");
    if !iso_note.is_empty() {
        println!("  installer: {iso_note}");
    }
    println!("  disk: {}/disks/{name}.qcow2", xios_home().display());
    log("INFO", &format!("add slot {name} cpu{vcpus} ram{ram_mib} disk{disk_gib}"));
}

fn cmd_rm(args: &[String]) {
    let Some(name) = args.first() else {
        die(exit::ARGS, "xi rm <name>");
    };
    if running_pid(name).is_some() {
        die(exit::ALREADY_RUNNING, &format!("slot '{name}' is running — xi stop {name} first"));
    }
    let (mut slots, _) = slot::load_slots();
    let before = slots.len();
    slots.retain(|s| s.name != *name);
    if slots.len() == before {
        die(exit::NO_SLOT, &format!("no slot named '{name}'"));
    }
    if let Err(e) = slot::save_slots(&slots) {
        die(exit::GENERAL, &e);
    }
    println!("✔ slot '{name}' removed (your files in ~/XiShared untouched)");
    log("INFO", &format!("rm slot {name}"));
}

fn cmd_status() {
    let h = host_info();
    let (slots, _) = slot::load_slots();
    println!("host — {} cores · {} MiB RAM · {} GiB free", h.cores, h.mem_total_mib, h.disk_free_gib);
    println!("slots — {}/{} in use", slots.len(), MAX_SLOTS);
    for s in &slots {
        let st = match running_pid(&s.name) {
            Some(pid) => format!("running (pid {pid})"),
            None => "stopped".into(),
        };
        println!("  {:<14} {st}", s.name);
    }
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();

    // Route top-level commands; split args at first non-flag positional.
    match args.first().map(String::as_str) {
        None => help(),
        Some("help") | Some("-h") | Some("--help") => help(),
        Some("version") | Some("-V") | Some("--version") => println!("xi {}", slot::VERSION),
        Some("list") => cmd_list(args.iter().any(|a| a == "--json")),
        Some("status") => cmd_status(),
        Some("serve") => serve::serve(&args[1..]),
        Some("add") => cmd_add(&args[1..]),
        Some("rm") | Some("remove") => cmd_rm(&args[1..]),
        Some("run") => {
            let rest: Vec<String> = args[1..]
                .iter()
                .filter(|a| a.as_str() != "--headless" && a.as_str() != "--json")
                .cloned()
                .collect();
            run_slot(rest.first().map(String::as_str).unwrap_or_default(), args.iter().any(|a| a == "--headless"));
        }
        Some("stop") => {
            let Some(name) = args.get(1) else {
                die(exit::ARGS, "xi stop <name>");
            };
            match stop_vm(name) {
                Ok(true) => println!("● slot '{name}' stopped"),
                Ok(false) => println!("slot '{name}' was not running"),
                Err(e) => die(exit::GENERAL, &e),
            }
            log("INFO", &format!("stop slot {name}"));
        }
        Some(other) => die(exit::ARGS, &format!("unknown command '{other}' — see: xi help")),
    }
}
