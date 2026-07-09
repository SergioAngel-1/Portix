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
 3000  tcp    my-shop              Next.js            next-server       48213   0.3%   214M    2h14m
 5173  tcp    docs-site            Vite               node              51002   0.0%    98M      12m
 8000  tcp    api                  FastAPI            python3.13         7781   1.2%   120M      44m
 5432  tcp    —                    —                  postgres           1234   0.1%    45M       3d
```

## What it does

- **Every listening socket** — TCP and UDP, IPv4 and IPv6. Dual-stack servers
  (the ones that bind both `0.0.0.0` and `::`) show up once, not twice.
- **The owning process** — command, PID, live CPU %, memory, and uptime.
- **The project behind it** — walks up from the process's working directory to
  the nearest manifest (`package.json`, `Cargo.toml`, `pyproject.toml`,
  `go.mod`, …) and reads the project name, package manager, and workspace.
- **The framework** — from the command, the manifest dependencies, and config
  files. Recognizes Next.js, Vite, Astro, Nuxt, Remix, NestJS, Express,
  FastAPI, Django, Flask, Uvicorn, Gunicorn, Go and Rust.
- **Containers** — flags processes running under Docker, Podman, or containerd.
- **Actions on the spot** — open the URL in your browser, copy it, or kill the
  process, right from the list.

It does all this **without root**: no `sudo`, no capabilities, no network calls,
ever. It reads `/proc`, and nothing else leaves your machine.

## Install

### From the AUR (Arch, CachyOS, …)

```bash
# with an AUR helper
paru -S portix-git      # or: yay -S portix-git
```

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
| `Enter` / `l`  | Open the detail panel for a service      |
| `h` / `Esc`    | Close the detail panel                   |
| `/`            | Search (filters as you type)             |
| `o`            | Open the service URL in your browser     |
| `y`            | Copy the URL to the clipboard            |
| `x`            | Kill the process (SIGTERM)               |
| `X`            | Force-kill the process (SIGKILL)         |
| `r`            | Refresh now                              |
| `?`            | Help                                     |
| `q`            | Quit                                     |

Killing asks for confirmation first, and re-checks the process identity the
instant before signaling — so a PID that got recycled between the list and your
keypress is never the one that gets killed.

## Using it from the command line

Portix is also a normal CLI, handy in scripts and one-liners.

```bash
# What's on port 3000? (accepts 3000 or :3000)
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
| `--no-color`         | Disable colors                                      |
| `--completions <SH>` | Print shell completions (`bash`, `zsh`, `fish`, …)  |

### Shell completions

```bash
# bash
portix --completions bash | sudo tee /usr/share/bash-completion/completions/portix
# zsh
portix --completions zsh  > ~/.zfunc/_portix
# fish
portix --completions fish > ~/.config/fish/completions/portix.fish
```

(The AUR package installs these for you.)

## Privacy & safety

Portix is built to be safe to run against your own machine:

- **No network. Ever.** No telemetry, no update checks. The only thing it opens
  outward is your browser, on a `localhost` URL, when you press `o`.
- **No elevated privileges.** Ports owned by other users simply show a partial
  row instead of erroring. It never asks for `sudo`.
- **It never reads process environments** (`/proc/<pid>/environ`), where secrets
  like `DATABASE_URL` live.
- **Everything drawn is sanitized**, so a process with a booby-trapped name can't
  hijack your terminal.

## License

Dual-licensed under **MIT** or **Apache-2.0** — your choice. See
[LICENSE-MIT](LICENSE-MIT) and [LICENSE-APACHE](LICENSE-APACHE).
