# Portix

**See what's running on your machine's ports — and which project it belongs to.**

Portix is a terminal app for Linux that lists every listening port, finds the
process behind it, and figures out the *project* that process is part of: its
folder, name, language, framework, package manager, and whether it's running in
a container.

`lsof -i` and `ss` tell you *pid 48213 is on port 3000*. Portix tells you
*that's your `my-shop` Next.js dev server*.

```
PORT   PROTO  PROJECT              FRAMEWORK          COMMAND            PID     CPU     MEM   UPTIME
 3000  tcp*   my-shop              Next.js            next-server       48213   0.3%   214M    2h14m
 5173  tcp    docs-site            Vite               node              51002   0.0%    98M      12m
 8000  tcp    api                  FastAPI · Uvicorn   python3.13         7781   1.2%   120M      44m
 5432  tcp    —                    —                  postgres           1234   0.1%    45M       3d
```

(`tcp*` means one service listening on both IPv4 and IPv6.)

## What it does

- **Every listening socket** — TCP and UDP, IPv4 and IPv6. Dual-stack servers
  (the ones that bind both `0.0.0.0` and `::`) show up once, not twice.
- **The owning process** — command, PID, live CPU %, memory, and uptime.
- **The project behind it** — walks up from the process's working directory to
  the nearest manifest (`package.json`, `Cargo.toml`, `pyproject.toml`,
  `go.mod`, …) and reads the project name, package manager, and workspace.
- **The framework** — from the command, the manifest dependencies, and config
  files. Recognizes Next.js, Vite, Astro, Nuxt, Remix, NestJS, Express, React,
  FastAPI, Django, Flask, Uvicorn, Gunicorn, Go and Rust.
- **Containers** — flags processes running under Docker, Podman, containerd,
  or systemd-nspawn. A port published through Docker's userland proxy is
  attributed to the destination container (showing its IP) instead of the
  `docker-proxy` process.
- **Actions on the spot** — open the URL in your browser, copy it, or kill the
  process, right from the list.

It does all this **without root**: no `sudo`, no capabilities, no network calls,
ever. It reads `/proc`, and nothing else leaves your machine.

## Install

### From the AUR (Arch, CachyOS, …)

No package is published yet — install from source below. (When a `portix-git`
PKGBUILD exists, it will be linked here.)

### From source

Needs Rust 1.75+.

```bash
git clone https://github.com/SergioAngel-1/Portix.git
cd Portix
cargo build --release
./target/release/portix
```

The binary is self-contained; copy `target/release/portix` anywhere on your
`PATH`.

### Update / reinstall

Installing is a **local** operation — the CI pipeline only verifies the code,
nothing deploys from GitHub. To update the binary on this machine, rebuild
and overwrite it:

```bash
./scripts/install.sh                     # release build -> ~/.local/bin/portix
./scripts/install.sh --sudo              # -> /usr/local/bin (prompts for password)
./scripts/install.sh --bin-dir "$HOME/.cargo/bin"
./scripts/install.sh --profile dev       # debug build (faster compile)
BIN_DIR=/opt/portix ./scripts/install.sh # same settings as env vars
```

The script always overwrites the destination, so re-running it after a
`git pull` is exactly "update portix". Equivalently, Cargo's own installer:

```bash
cargo install --path . --force           # -> ~/.cargo/bin/portix
```

## Using the app (TUI)

Just run it:

```bash
portix
```

You get a live, auto-refreshing table. Move around and act with single keys:

| Key            | Action                                   |
|----------------|------------------------------------------|
| `j` / `↓`      | Move down                                |
| `k` / `↑`      | Move up                                  |
| `g` / `G`      | Jump to top / bottom                     |
| `i` / `Enter` / `l` | Open the detail panel for a service  |
| `h` / `Esc`    | Close the detail panel                   |
| `/`            | Search (filters as you type)             |
| `o`            | Open the service URL in your browser     |
| `y`            | Copy the URL to the clipboard            |
| `x`            | Kill the process (SIGTERM)               |
| `X`            | Force-kill the process (SIGKILL)         |
| `r`            | Refresh now                              |
| `t`            | Theme picker (live preview, `Enter` saves) |
| `?`            | Help                                     |
| `q`            | Quit                                     |

Killing asks for confirmation first, and re-checks the process identity the
instant before signaling — so a PID that got recycled between the list and your
keypress is never the one that gets killed, and a port that changed owners while
you were deciding is refused instead of killing the new process.

When a scan is degraded (unreadable processes, sockets without an owner,
container ports that may be hidden), the footer shows a `⚠N` count; `?` unfolds
the details.

## Using it from the command line

Portix is also a normal CLI, handy in scripts and one-liners.

```bash
# What's on port 3000? (accepts 3000, :3000, or --port 3000)
portix 3000
# → :3000 my-shop Next.js pid 48213
# Prints nothing and exits with code 3 if the port is free — so you can do:
portix :3000 || pnpm dev

# One plain-text snapshot, no interactive UI
portix --once

# The full snapshot as JSON (for jq, dashboards, etc.)
portix --json
portix --json 3000        # just the one service

# Free up a port
portix --kill 3000        # asks to confirm, sends SIGTERM
portix --kill 3000 --force --yes   # SIGKILL, no prompt
```

Exit codes: `0` success · `1` error · `2` bad usage (clap) · `3` nothing
listening on that port. JSON output is wrapped in `schema_version` (currently
`1`) so consumers can detect breaking changes.

### Useful flags

| Flag                 | What it does                                        |
|----------------------|-----------------------------------------------------|
| `--once`             | Print one text frame and exit (no TUI)              |
| `--json`             | Emit the snapshot as JSON                           |
| `--kill <PORT>`      | Kill whatever listens on a port                     |
| `--force`            | Use SIGKILL instead of SIGTERM                      |
| `--yes`              | Skip the kill confirmation                          |
| `--interval <MS>`    | Refresh interval for the TUI                        |
| `--system-ports`     | Include ports below 1024                            |
| `--no-color`         | Disable colors (the `NO_COLOR` env var does too)    |
| `--completions <SH>` | Print shell completions (`bash`, `zsh`, `fish`, …)  |
| `--proc-root <PATH>` | Read `/proc` from an alternative root (tests, containers) |

### Shell completions

```bash
# bash
portix --completions bash | sudo tee /usr/share/bash-completion/completions/portix
# zsh
portix --completions zsh  > ~/.zfunc/_portix
# fish
portix --completions fish > ~/.config/fish/completions/portix.fish
```

## Configuration

Portix reads `~/.config/portix/config.toml` (XDG). All keys are optional:

```toml
theme = "default-dark"      # default-dark | mono | auto (pick it live with `t`)
refresh_ms = 1000           # TUI refresh interval (clamped to >= 200)
show_udp = true             # show UDP listeners
show_system_ports = false   # include ports below 1024
show_loopback_only = false  # only services bound to localhost
cpu_scale = "system"        # "system" (0-100%) or "core" (per-core, like top)
kill_confirm = true         # ask before killing (x/X)
```

Unknown keys are ignored (v1 features can appear without breaking parsing),
and the file is created on first theme save. Under `sudo portix`, the
invoking user's config is read instead of root's.

## Privacy & safety

Portix is built to be safe to run against your own machine:

- **No network. Ever.** No telemetry, no update checks. The only thing it opens
  outward is your browser, on a `localhost` URL (or the exact address a service
  is bound to), when you press `o`.
- **No elevated privileges.** Ports owned by other users simply show a partial
  row instead of erroring. It never asks for `sudo`.
- **It never reads process environments** (`/proc/<pid>/environ`), where secrets
  like `DATABASE_URL` live.
- **Everything drawn is sanitized**, so a process with a booby-trapped name can't
  hijack your terminal.

## License

Dual-licensed under **MIT** or **Apache-2.0** — your choice. See
[LICENSE-MIT](LICENSE-MIT) and [LICENSE-APACHE](LICENSE-APACHE).
