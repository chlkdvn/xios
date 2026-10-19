# XiOS

_Brand: **xi** — Xi Hub Operating System_

> **One machine. Many systems.**

XiOS is a **container-style OS hub for laptops and desktops**. Your computer always
boots into **XiOS** — a lightweight base OS — and XiOS manages up to **5 standalone
operating systems** (Linux Mint, Android-x86, Fedora, anything that exists as an
installer ISO) in isolated KVM virtual machines, each with its own virtual disk and
a defined resource budget.

**Your files are yours:** Documents, Downloads, Pictures, Music, Videos live with
XiOS itself and are **shared into every slot** via virtiofs. Switch from Mint to
Android — same files. Like Docker volumes, but for whole operating systems.

**Mental model: XiOS is to laptops what Docker is to servers.**

## The launch video

[![XiOS launch video](docs/brag/brag.jpg)](https://github.com/chlkdvn/xios/raw/main/docs/brag/brag.mp4)

_*(click the image to watch `docs/brag/brag.mp4` — ~4 MB)*_

> **XiOS: one machine, many systems.** Five OS slots with their own CPU/RAM/disk,
> their own UEFI boot manager, and a web panel that looks like neofetch.
> It boots. It runs. It is yours.

---

## What's in this repo

```
XiOS/
  SPEC.md      ← the full project spec / design document
  xi/          ← `xi`, the slot manager CLI (Rust): KVM slots + web panel
  bootloader/  ← Xi boot manager (Rust, x86_64 UEFI) built for the ESP
  docs/        ← screenshots & extras
```

## Installation

### Prerequisites

- `rustup` / Rust toolchain (stable)
- `qemu`, `python3`
- For the UEFI boot manager: `nightly` Rust + `x86_64-unknown-uefi` target

### Build the `xi` CLI

```bash
cd xi
cargo build --release
```

The `xi` CLI manages your OS slots:

```bash
xi list [--json]            # list slots (with status column)
xi add <name> [opts]        # register a slot (max 5)
      --iso <path>          # installer ISO path
      --cpu <n>             # virtual CPUs (default 2)
      --ram <mib>           # memory in MiB (default 4096)
      --disk <gib>          # disk in GiB (default 32)
xi run <name> [--headless]  # boot slot VM (window label = xios:<name>)
xi stop <name>              # stop a running slot
xi status [name]            # host + slot liveness overview
xi rm <name>                # remove slot registration (files untouched)
xi serve [--port <n>]       # web panel — http://127.0.0.1:7747
xi help | version
```

State lives under `~/.xios/`:

- `slots.txt` — slot registry (`slot|cpu|ram|disk|iso`)
- `disks/` — qcow2 slot disks
- `isos/` — Xi Store downloads
- `run/` — live VM pidfiles
- `log/xi.log` — operation log

### Build the UEFI boot manager

```bash
cd bootloader
rustup switch bootmanager +nightly
rustup target add x86_64-unknown-uefi
cargo build --release
```

The resulting `.efi` file is dropped into the ESP by `mkimage.sh`.

---

## How an OS gets in (two ways, identical result)

### A. Flash drive (the familiar way)

1. Plug a USB stick containing a distro installer ISO.
2. XiOS detects it and shows the **slot wizard**: name + resource budget.
3. XiOS creates a fresh virtual disk and boots the installer in a VM with the
   USB passed through — the installer looks *exactly* like a normal install.
4. **The one critical difference:** it installs onto the slot's virtual disk.
   Your SSD is never touched. Nothing is ever wiped.

### B. Xi Hub (downloads inside XiOS)

Same wizard, but XiOS fetches the official ISO itself, verifies the checksum,
and starts the installer. No USB needed.

After install, the slot appears in the picker:

```
xi run mint   →  your whole screen is Linux Mint
```

Exit the VM → you fall back into XiOS.

## Resource model

Every slot declares a budget:

```
mint:     cpu 4   ram 8G   disk 64G
android:  cpu 2   ram 4G   disk 16G
fedora:   cpu 2   ram 4G   disk 24G
```

- The base OS + idle slots cost almost nothing (~300 MB RAM total).
- Running slots strictly honor their caps — a runaway OS can't starve the host.
- The **sharedFolders** rule is invariant: every slot sees the same `/home/xi/*`.
- Deleting a slot deletes that slot's *system disk only* — never your files.

## Architecture

```
                  boot firmware (UEFI)
                          │
                   XiOS base OS  ◄── small, fast, quiet
                          │
   ┌──────────────────────┼──────────────────────┐
   │                      │                      │
xi CLI / Picker      Slot Manager           Xi Hub (store)
(TUI, later GUI)     (libvirt + KVM)        (ISO library)
   │                      │
   │             ┌────────┴─────────┐
   │             │ virtiofs bridge  │  shared: /home/xi/*
   │             └────────┬─────────┘
   └── slot VM 1 ────────┴── slot VM 2 ── ⋯ slot VM 5
    (Mint)               (Android-x86)
```

## Why Rust for the tooling?

`xi` is a *control-plane* program: it deletes disk images, launches VMs, runs
concurrent tasks, and handles untrusted ISO data. Rust's memory- and
thread-safety guarantees make that safe by construction. The heavyweight work
(actual virtualization) happens in KVM/QEMU — `xi` just commands it.

## Roadmap

| #  | Milestone                            | You get                                                     |
|----|--------------------------------------|-------------------------------------------------------------|
| M1 | Slot engine on your machine          | `xi` CLI + first slot VM + shared folders working           |
| M2 | Slot wizard + Xi Hub                 | USB-in detection, ISO library, checksums, enforced budgets  |
| M3 | Picker boot UX                       | Full-screen picker at VM exit, slot usage stats             |
| M4 | XiOS ISO                             | Bootable Debian-based installer — any machine becomes XiOS  |
| M5 | Polish                               | Recovery mode, slot snapshots ("snapshot before risky")     |

## Non-goals (v1)

- No nested graphic managers, no cloud sync, no Apple Silicon support yet.
- Max slots: 5 — deliberate simplicity, not a technical limit.
- No proprietary driver blobs bundled into the XiOS ISO.

---

See [SPEC.md](SPEC.md) for the full design document.
