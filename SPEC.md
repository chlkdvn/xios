# XiOS — Full Project Explanation

**Xi Hub Operating System** — brand: **xi**

> One machine. Many systems.

---

## 1. The problem

Normal laptops run **one** OS at a time. Want Mint today and
Android-x86 tomorrow? Historically you'd dual-boot (reboot roulette,
risk of wiping everything) or juggle live-USBs (nothing persists).

## 2. The idea: XiOS

XiOS is a **container-style OS hub**:

- Your machine boots **always** into XiOS — a lightweight base OS.
- XiOS manages up to **5 standalone OS slots** (Linux Mint, Android-x86,
  Fedora, anything that exists as an installer ISO).
- Each slot is a full isolated OS with **its own virtual disk**, booted
  inside a KVM virtual machine with a **defined resource budget**
  (CPUs, RAM, disk caps per slot).
- **Your files are yours**: real files (Documents, Downloads, Pictures,
  Music, Videos…) live with XiOS itself and are **shared into every
  slot** by virtiofs. Switch from Mint to Android — same files.
  Like Docker volumes, but for whole operating systems.

Mental model: **XiOS is to laptops what Docker is to servers.**

## 3. How a new OS gets in (two ways, identical result)

### A. Flash drive (the familiar way)
1. Plug a USB stick that contains a distro installer ISO.
2. XiOS detects it and shows the **slot wizard**: name, resource budget.
3. XiOS creates a fresh virtual disk and boots the installer in a VM
   with the USB passed through.
4. The installer screen looks *exactly* like installing on a normal
   laptop — because the VM sees a real laptop.
5. **The one critical difference**: it installs onto the slot's
   virtual disk, never touching your SSD. Nothing is ever wiped.

### B. Xi Hub (downloads inside XiOS)
Same wizard, but XiOS fetches the official ISO itself, verifies the
checksum and starts the installer. No USB needed.

After install the slot appears in the picker:
`xi run mint` → the whole screen becomes Linux Mint.
Exit the VM → you fall back into XiOS.

## 4. System architecture

```
                      boot firmware (UEFI)
                              │
                       XiOS base OS  ◄── small, fast, quiet (~350 MB RAM idle)
                              │
        ┌─────────────────────┼──────────────────────┐
        │                     │                      │
   xi CLI / Picker      Slot Manager           Xi Hub (store)
   (TUI, later GUI)     (libvirt + KVM)        (ISO library)
        │                     │
        │            ┌────────┴─────────┐
        │            │ virtiofs bridge  │   shared: /home/xi/*
        │            └────────┬─────────┘
        └────── slot VM 1 ────┴── slot VM 2 ──── ⋯ slot VM 5
         (Mint)               (Android-x86)
```

### Components

| Piece            | What it is                                     | Tech |
|------------------|------------------------------------------------|------|
| `xi` CLI         | The manager: list/add/run/stop/remove slots    | Rust |
| Slot runtime     | Creates VMs, resources caps, USB passthrough   | KVM + QEMU + libvirt |
| Xi Hub           | Catalog → ISO download → checksum verify       | Rust (same binary, subcommand) |
| Shared storage   | Real user folders mounted into every slot      | virtiofs |
| Slot disks       | One qcow2 image per slot, up to 5              | qcow2 (sparse, thin) |
| Base OS image    | Bootable XiOS ISO (final milestone)           | Debian live-build |

## 5. The resource model (the "Docker" feeling)

Every slot declares a budget. Example defaults:

```
mint:     cpu 4   ram 8G   disk 64G   allow-screen? full
android:  cpu 2   ram 4G   disk 16G
fedora:   cpu 2   ram 4G   disk 24G
```

- The base OS + idle slots cost almost nothing (~300 MB RAM total).
- Running slots strictly honor their caps — a runaway OS can't starve
  the host.
- The **sharedFolders** rule is invariant: every slot sees the same
  `/home/xi/*` content. Deleting a slot deletes that slot's *system*
  disk only — never your files.

## 6. What ships when (milestones)

| # | Milestone | You get |
|---|-----------|---------|
| M1 | Slot engine on your current machine | `xi` CLI + first slot VM + shared folders working |
| M2 | Slot wizard + Xi Hub | USB-in detection, ISO library, checksums, budgets enforced |
| M3 | Picker boot UX | Full-screen picker at VM exit, slot usage stats |
| M4 | XiOS ISO | A bootable Debian-based installer that turns any machine into an XiOS host |
| M5 | Polish | Recovery mode, slot snapshots ("snapshot before risky install") |

## 7. C vs Rust — the honest comparison

| Factor | C | Rust |
|--------|---|------|
| Result size / speed | Tiny (KBs), native | Small (MBs), native. Fast gap invisible here — `xi` mostly orchestrates subprocesses |
| Development speed | Slower to get right | Faster (compiler catches your mistakes) |
| Safety | Manual memory management; a `xi rm`-style bug could corrupt qcow2/paths | Memory + thread safety by design — big deal for a tool that *manages disks and VMs* |
| Concurrency (5 slots + downloads at once) | Doable, risky | Allowed, cheap |
| Ecosystem for this job | libvirt C API | `libvirt` crate wraps the same API, plus superb CLI crates (clap, tokio) |
| Your situation | Fine but more time-per-feature | Toolchain just installed, fast iteration |

**Decision: Rust.** Not because C is bad — because `xi` is a
*control-plane* program: it deletes images, launches VMs, handles
concurrent tasks and untrusted ISO data. Rust's guarantees make that
safe by construction. The heavyweight work (virtualization) happens in
KVM/QEMU itself, written in C by the experts — `xi` just commands it.

## 8. Repo layout (M1)

```
XiOS/
  SPEC.md            ← this file
  xi/                ← the manager CLI (Rust workspace)
    src/main.rs      ← x++++++++++++++++++ subcommands
  docs/              ← design notes per component
```

## 9. Non-goals (v1)

- No nested graphic managers, no cloud sync, no Apple Silicon yet.
- Max 5 slots — deliberate simplicity, not a technical limit.
- No proprietary driver blobs bundled into XiOS ISO.
