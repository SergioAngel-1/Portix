# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

Portix is a Rust TUI for Linux that lists every listening socket, resolves it to the owning process, and from that process infers **the project it belongs to** (root dir, name, language, framework, package manager, container). The port is the index; the project context is the product.

`PRD.md` (2140 lines, Spanish) is the authoritative spec — architecture, `/proc` parsing traps, cache design, security model, roadmap. Consult the relevant section before implementing anything non-trivial; it documents decisions and their rationale in enough detail to implement without further research. The code is currently an MVP-in-progress and lags the PRD in places.

## Commands

```bash
cargo build                       # debug build
cargo run                         # launch the TUI
cargo run -- --once               # single plain-text frame, no raw mode
cargo run -- --json               # full snapshot as JSON
cargo run -- 3000                 # what's on port 3000; exit 3 if nothing
cargo run -- --no-color           # force the monochrome theme (also honors NO_COLOR)
cargo run -- --proc-root tests/fixtures/proc/<name>   # run against a synthetic /proc
cargo test
cargo test <name>                 # single test by substring, e.g. cargo test parse_ipv4
cargo test -- --nocapture
cargo clippy -- -D warnings
cargo fmt
```

Tests live in `#[cfg(test)] mod tests` blocks inside the module they test (`platform/linux/net.rs`, `detect/scoring.rs`, `ui/format.rs`, `ui/theme.rs`, `core/config.rs`, …). `tests/fixtures/` is reserved for `/proc` trees and project trees driven through `--proc-root`; the top-level `benches/`, `fuzz/`, `detectors/`, `themes/` dirs are still empty placeholders for later phases (theming itself is live in `ui/theme.rs`, not the `themes/` dir; `ui/input.rs` and `ui/widgets/` are commented out in `ui/mod.rs`).

## Architecture

The layering is the load-bearing decision. Dependencies flow one way and never invert:

```
ui ──▶ core ──▶ { detect, cache, platform } ──▶ model
```

- `model/` — pure domain types, `std` + `serde` only. **No I/O.** `Service` is the central entity: the union of a socket, its process, its project, and its framework.
- `platform/` — the *only* door to the OS. Everything goes through the `PlatformProbe` trait and a `ProcRoot`-style injected path (`LinuxProbe::new(proc_root)`), never a hardcoded `/proc`. This injectability is what makes ~all of the discovery code testable without privileges, real processes, or even Linux.
- `detect/` — pure functions from signals to `Project`/`Framework`. Signals are weighted (`detect/signals.rs`), scored (`detect/scoring.rs`), and disambiguated by the registry's `Supersedes`/`Implies` relations (`detect/registry.rs`, e.g. Next.js supersedes React). Must not touch `platform/` or `ui/`.
- `cache/` — process CPU samples keyed by `(pid, starttime)`; L2 project keyed by `inode(cwd)`; L3 framework keyed by `(root_inode, manifest_mtime, disc)`; L4 uid->username (wired: the engine routes every `getpwuid_r` through it, 300 s TTL). **Keys matter**: L1-style identity uses `(pid, starttime)` — never `pid` alone, PIDs get recycled and a stale entry shows a dead process's project. L2 uses `inode(cwd)` — not the path, so a rename doesn't miss.
- `core/` — the only module that knows all the others. `Engine::scan()` runs the canonical pipeline. Must not depend on `ratatui`. `core/config.rs` loads and (theme-only) saves `~/.config/portix/config.toml`.
- `ui/` — Elm-like MVU. `App::handle_key()` mutates state and returns `Vec<Effect>`; `main.rs::run_effect` executes them (kill, xdg-open, clipboard, **persist config**). Kill confirmation is re-resolved from a fresh scan before the modal arms: `x`/`X` emit `Effect::KillRequest(port, signal)`, `run_effect` calls `Engine::service_on_port` and `App::arm_kill` with the *current* owner, so the modal never describes a stale snapshot. Rendering is a pure function of `App`; every view `render(...)` takes `&Theme` and pulls its colors from semantic roles — **never `Color::*` literals** (that discipline is what makes live theme-switching and monochrome work). `ui/` must not import `std::fs` or `platform/`; config I/O lives in `core` and reaches disk only through an `Effect`. `App::new(config, theme, color_enabled)`.

### The canonical pipeline (`core/engine.rs`)

`/proc/net/{tcp,tcp6,udp,udp6}` → socket **inode** → reverse map by readlink-ing every `/proc/<pid>/fd/*` for `socket:[N]` → PID → `/proc/<pid>/{cmdline,cwd,exe,status,stat,cgroup}` → walk up from `cwd` looking for manifests (`package.json`, `Cargo.toml`, `pyproject.toml`, `go.mod`, …) → emit framework signals from cmdline + manifest deps + config files → `Service`.

Note that `/proc/net/*` does **not** contain the PID. The inode→PID inversion is the expensive step (~12k `readlink` on a typical box) and the reason the cache exists.

### UI state, theming, and config

The MVU state and its overlays all live behind `App`:

- **Theme** (`ui/theme.rs`) — a semantic palette (`accent`, `ok`, `warn`, `danger`, `container`, `label`, `muted`, `text`). `Theme::select(name, use_color)` maps the `theme` config value (`default-dark` | `mono` | `auto`) to a palette; `--no-color`/`NO_COLOR` (i.e. `use_color == false`) force monochrome over any name. `App` resolves the `Theme` once at startup and threads `&app.theme` into every render. When adding a view, take `&Theme` and read colors from its roles.
- **Theme picker** (`t` → `Mode::ThemePicker`, `ui/views/theme_picker.rs`) — `j`/`k`/`t` preview live (the whole UI recolors because rendering is pure), `Enter` saves, `Esc` reverts to the pre-open theme. Saving is `Effect::PersistConfig` → `Config::save_theme`, which rewrites **only** the `theme` line of `config.toml`, preserving comments and other keys.
- **Info panel** (`i` / `Enter` / `l` → `ui/views/detail_panel.rs`) — a centered modal with the full recoverable detail for the selected service. Like every other view, every system-derived string it draws passes through `format::cell`.
- **Degradations** (`Snapshot.degradations`) — `SocketsWithoutPid`, `ProcessesPartialRead`, `ScanFailed`, `ContainerPortsHidden`. Rendered as a `⚠N` count in the footer and unfolded to full messages in the help modal (`?`). `ProcessesPartialRead` means a PID was found but its `/proc` metadata was unreadable — distinct from a socket with no PID at all. `ContainerPortsHidden` means a container runtime appears active, but its userland-proxy is disabled and published ports may be hidden from `/proc/net`.

### Container awareness

- **Per-socket attribution** (`core/engine.rs` + `platform/linux/cgroup.rs`) — container membership is resolved per PID from `/proc/<pid>/cgroup` and cached per process during a scan.
- **Docker proxy rewrite** (`core/docker_proxy.rs`) — when the owning process is `docker-proxy`, the service row is rewritten into the destination container using parsed `-container-ip` / `-container-port` flags. The original host process/cmdline are preserved so the UI still shows what really runs on the host. Known limitation: the synthesized `Container.id` holds the container **IP** (from `-container-ip`), not a short ID, and `name`/`compose_project` stay `None` — the Docker API is only consulted in the v1.5 phase.
- **Container warning** (`platform/linux/container_warning.rs`, reached via `PlatformProbe::container_warning`) — if active runtimes are detected but no `docker-proxy` sibling is found, a `Degradation::ContainerPortsHidden` is emitted so the user knows some published ports may not appear. The inverse is deliberate: with `docker-proxy` running, the userland-proxy is enabled, ports **are** visible, and no warning is shown. Runtime detection reuses `platform/linux/cgroup.rs::parse_cgroup_line` (structured parsing, never substrings), so a host that merely runs `dockerd` (its own `docker.service` unit) does not trip the warning. The engine recomputes it at most every 10 s (`Engine::container_warning_cached`).

## Non-negotiable constraints

These come from PRD §7, §13 and exist because each has bitten a real tool:

- **Read all four `/proc/net` files.** Modern Node servers bind `::` and never appear in `/proc/net/tcp`. Then dedupe dual-stack pairs by `(pid, port, family)` — merging only IPv4+IPv6 pairs; two sockets of the same IP version on one port (SO_REUSEPORT, or 127.0.0.1 + 0.0.0.0 in one process) are distinct listeners and stay separate — or `next dev` shows up twice and the tool looks broken.
- **Hex address parsing is host-order, not network-order.** `0100007F` is `127.0.0.1`, not `1.0.0.127`. IPv6 is four `u32`s, each byte-reversed independently, groups keeping their order.
- **Parse `/proc/<pid>/stat` from the last `)`.** `comm` is attacker-controlled and can contain `) 0 0 0`.
- **Everything from the system is sanitized before it is drawn.** `ui::format::sanitize_for_terminal` is the single mandatory chokepoint; a process with `argv[0] = "\x1b]0;pwned\x07"` otherwise hijacks the terminal. No exceptions, no bypasses.
- **Re-verify `(pid, starttime)` immediately before sending a signal.** TOCTOU: the PID may have been recycled between the snapshot and the keypress.
- **Never `sh -c`.** Always `Command::new(prog).arg(x)` with separated args.
- **`/proc/<pid>/environ` is never read.** It holds `DATABASE_URL` and tokens. Detector `env_var` signals may test for a key's *existence* only.
- **No network. Ever.** No telemetry, no update checks. The only outbound action is `xdg-open` on a localhost URL.
- **No setuid, no capabilities, no `sudo`/`pkexec` invocation.** Unreadable `/proc` entries (`EACCES`) degrade gracefully into a partial row; they are not errors.
- Target `#![forbid(unsafe_code)]` everywhere outside `platform/linux/`, and no `unwrap()`/`expect()` in `platform/` or `detect/` — a process can vanish between `readdir` and `open`, and that is the normal case, not an exception.

## Performance budget

First frame < 50 ms, hot refresh < 15 ms, RSS < 15 MB. Redraw on events, never in an idle loop — CPU at rest between snapshots must be indistinguishable from zero. Note on the current state: enrichment (project + framework) runs **synchronously inside `Engine::scan()`** today, so a scan pays detection cost before the frame is drawn; the off-critical-path design of PRD §10.3 (rayon pool + `Event::Enriched`) is not yet implemented. The container warning lives in `platform/linux/container_warning.rs` (the platform layer is the only door to `/proc`) and is recomputed **at most once per 10 s** (`Engine::container_warning_cached`) because it walks all of `/proc`; it is a system-level scan, not a per-refresh one.
