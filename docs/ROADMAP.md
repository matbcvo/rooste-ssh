# rooste-ssh roadmap

## Goal

A memory-safe, cross-platform (Linux and Windows) **GUI** SSH client written in Rust, with
everything needed for daily remote work in one app.

### Key features

- Tabs and split panes
- Searchable session manager with folders and tags; plain TOML config with a portable mode
- Works with existing OpenSSH setups: reads `~/.ssh/config` and `known_hosts`
- Built-in SFTP browser, key generator and SSH agent
- Port-forwarding UI with saved tunnels, ProxyJump
- Modern rendering: true color, HiDPI, font fallback, optional ligatures
- Keepalive, auto-reconnect, duplicate session
- Broadcast input to several sessions, command palette

### Interoperability

For people coming from other clients: import saved sessions from PuTTY (Windows registry,
`~/.putty/sessions`), import `.ppk` keys, and serve existing Pageant clients from the built-in
agent on Windows.

## Decisions

| Topic | Decision |
|---|---|
| App type | GUI desktop app (egui). `rooste-cli` doubles as a test client and later a scriptable command-line client |
| Protocols | SSH first. Serial, telnet and raw TCP in M5b, behind the `Transport` trait that exists from day one. rlogin is not planned |
| Legacy algorithms | Modern defaults; a small legacy set can be enabled **per session only** (see [Security](#security)) |
| Platforms | Linux and Windows. macOS may come for free later |
| License | MIT |
| MSRV | Rust 1.89 (edition 2024; required by russh) |

## Technology

| Concern | Choice | Why |
|---|---|---|
| SSH | `russh` with the `ring` crypto backend | Pure Rust, async; `ring` builds easily on Windows |
| Async runtime | `tokio` | Required by russh |
| Terminal emulation | `alacritty_terminal` | Proven VT parser and grid |
| GUI | `egui`/`eframe` + custom terminal widget | Fast to build, identical on both OSes, GPU via wgpu |
| SFTP | `russh-sftp` | Pairs with russh |
| Keys | `ssh-key` + own `.ppk` parser | Generate/convert keys, import `.ppk` keys |
| Agent | `ssh-agent-lib` | Unix socket on Linux; OpenSSH named pipe on Windows |
| Secrets | `keyring` | Windows Credential Manager, Linux Secret Service |
| Serial | `serialport` | |
| Config | `serde` + `toml`, `directories` | |

## Architecture

```
crates/
├─ rooste-core/     session/profile model, config, ssh_config and session import
├─ rooste-proto/    Transport trait: SSH (russh), later serial, telnet, raw
├─ rooste-term/     alacritty_terminal wrapper: input encoding, selection, scrollback
├─ rooste-keys/     keygen, .ppk import/export, agent client/server
├─ rooste-sftp/     SFTP operations, transfer queue
├─ rooste-gui/      egui app (binary `rooste`): tabs, splits, session manager, settings
├─ rooste-cli/      headless client
└─ rooste-testkit/  shared test fixtures
```

- The GUI talks to sessions through message channels, so a slow network never freezes the UI.
- The terminal widget, tabs, logging and broadcast only use `Transport`
  ([crates/rooste-proto/src/lib.rs](../crates/rooste-proto/src/lib.rs)).
- Every session profile carries a `protocol` field (`ssh`, `serial`, `telnet`, `raw`).

## Milestones

Each milestone is done only when its tests pass in CI on **both Linux and Windows**.

### M0 — Foundations ✅
- Cargo workspace, rustfmt, clippy, `cargo-deny`, MIT license, README.
- `ci.yml`: lint, Linux + Windows × stable + MSRV test matrix, OpenSSH Docker integration job,
  cargo-deny, coverage.
- `rooste-testkit` skeleton and OpenSSH Docker fixture; Dependabot.

### M1 — Headless SSH client (`rooste-cli`) ✅
- Password, key-file and agent authentication.
- Host-key verification against `known_hosts`.
- Interactive PTY in a raw-mode terminal, resize.
- Audit which algorithms russh supports (modern and legacy) — see [Security](#security).
- In-process `russh` test server in `rooste-testkit`; Windows OpenSSH Server CI job.
- **Tests required:** password, key and agent login; host-key mismatch; resize — on both OSes.

### M2 — Terminal widget (hardest part)
- Render the `alacritty_terminal` grid in egui.
- Keyboard mapping including Windows AltGr and IME; mouse reporting; selection and clipboard;
  scrollback; resize propagation.
- **Tests required:** `insta` snapshot tests of the grid; regression suite from `vttest`/`esctest`
  cases; manual check with `htop`, `vim`, `tmux`.

### M3 — First usable app (MVP)
- Session manager with saved profiles.
- Import from `~/.ssh/config` and other clients (see [Interoperability](#interoperability)).
- Tabs, settings dialog, host-key confirmation dialog.
- **Tests required:** `egui_kittest` tests for every new screen and dialog.

### M4 — Power features
- Split panes; local, remote and dynamic (SOCKS) port forwarding; ProxyJump; agent forwarding;
  auto-reconnect; broadcast input.
- Per-session "Allow legacy algorithms" option, ⚠ indicator on the tab, and an error message that
  offers to enable it when a server only supports legacy algorithms.

### M5 — Built-in tools
- SFTP browser with drag and drop.
- Key generator, `.ppk` import/export.
- Built-in SSH agent.

### M5b — Extra protocols
- **Raw TCP:** plain `tokio` TCP stream.
- **Telnet:** small built-in implementation (ECHO, SGA, NAWS window size, TTYPE terminal type).
- **Serial:** `serialport`; baud rate, data bits, parity, stop bits, flow control; port picker
  (`COMx` on Windows, `/dev/ttyUSB*` and `/dev/ttyS*` on Linux).

### M6 — Distribution
- Windows: MSI (`cargo-wix`), winget, portable zip, code signing.
- Linux: `.deb`, `.rpm`, AppImage, Flatpak.
- `release.yml` with smoke tests of the packaged binaries; update check.

### Later
Session logging and recording, FIDO2 (`sk-ed25519`) keys, X11 forwarding, scripting or plugins,
macOS.

## Testing strategy

| Layer | Covers | Tools |
|---|---|---|
| Unit | Config parsing, imports, `.ppk`, key-to-escape-sequence encoding | `cargo test` via `cargo-nextest` |
| Property | Parsers never panic; import → export → import round-trips | `proptest` |
| Snapshot | Terminal grid after escape sequences | `insta` |
| Integration (SSH) | Auth methods, host keys, PTY, resize, forwarding, ProxyJump, SFTP, agent | In-process `russh` server + OpenSSH in Docker |
| Interop | OpenSSH (several versions), Dropbear, legacy-algorithm server | Docker images |
| GUI | Session manager, tabs, dialogs, terminal widget | `egui_kittest` |
| End-to-end | Scripted session through `rooste-cli` | `expectrl` |
| Fuzzing | Terminal input, `.ppk`, config, SFTP protocol | `cargo-fuzz` |
| Benchmarks | Terminal throughput, frame time | `criterion` |

### Protocol tests

| Protocol | Linux CI | Windows CI |
|---|---|---|
| SSH | In-process server + OpenSSH Docker | In-process server + Windows OpenSSH Server |
| Raw | In-process TCP echo server | Same |
| Telnet | In-process server + `busybox telnetd` Docker | In-process server |
| Serial | `socat` virtual port pairs | Mock transport |

Every transport also runs one shared conformance suite (connect, send/receive, resize,
disconnect, error handling).

### Windows caveat
GitHub Windows runners cannot run Linux containers. There the in-process server covers
integration tests, a separate job tests against the built-in Windows OpenSSH Server, and
Windows-only code (named-pipe agent, registry import) gets Windows-only tests.

### GitHub workflows

- **`ci.yml`** (every push and pull request, required for merging): lint (fmt, clippy, docs with
  warnings as errors), test matrix, integration, `cargo-deny`, coverage (`cargo-llvm-cov` →
  Codecov, informational for now).
- **`nightly.yml`** (scheduled, from M1/M2): fuzzing ~10 min per target with crash artifacts, full
  interop matrix, beta and nightly toolchains, benchmarks compared with the previous run. The
  legacy-server check must **fail** with defaults and **succeed** with legacy enabled.
- **`release.yml`** (on `v*` tags, M6): build, install the packaged MSI/`.deb`, run
  `rooste --version` and a test connection, sign, publish.
- Dependabot for Cargo, GitHub Actions and Docker; branch protection on `main` requiring CI.

## Security

| Tier | Algorithms | Behavior |
|---|---|---|
| Default | mlkem768x25519 (post-quantum hybrid), curve25519, ECDH NIST P-256/384/521, DH group 14–18 and group-exchange with SHA-2 KEX; Ed25519, ECDSA, RSA-SHA2 keys; ChaCha20-Poly1305, AES-GCM, AES-CTR; HMAC-SHA2 (incl. ETM) | Always on |
| Legacy (opt-in per session) | DH group 14, group 1 and group-exchange with SHA-1; `ssh-rsa` SHA-1 signatures; AES-CBC; HMAC-SHA1 (incl. ETM) | Off by default; never a global setting; ⚠ on the tab |
| Never | SSH-1, DSA, DES/3DES, RC4 | Not implemented |

- `unsafe_code = "forbid"` workspace-wide; crates that truly need unsafe opt out explicitly.
- Key material held in zeroized memory.
- The policy lives in [crates/rooste-proto/src/ssh/algorithms.rs](../crates/rooste-proto/src/ssh/algorithms.rs).
  russh's own default also offers `ssh-rsa` (SHA-1) host keys; rooste-ssh moves it to the legacy
  tier. Never-supported algorithms are excluded at build time (the russh `dsa` and `des` features
  are off).
- Audit (russh 0.64): every algorithm in the default and legacy tiers is implemented with the
  `ring` backend; `aws-lc-rs` offers the same set.

## Risks

- Terminal correctness and Windows keyboard input — budget extra time for M2.
- russh gaps: GSSAPI/Kerberos needs an external implementation of russh's `GssapiAuthenticator`;
  keyboard-interactive login is not wired up yet (password auth covers most servers).
- Font rendering (fallback, CJK, emoji) in egui.
