# rooste-ssh

A modern, memory-safe SSH client for Linux and Windows, written in Rust — with tabs, split panes,
a session manager, built-in SFTP, key generator and agent.

> **Status:** early development (milestone M0). See [docs/ROADMAP.md](docs/ROADMAP.md).

## Building

Requires Rust 1.85 or newer.

```sh
cargo build --release
cargo run -p rooste-gui     # desktop app (binary: rooste)
cargo run -p rooste-cli     # headless client
```

## Testing

```sh
cargo nextest run --workspace     # or: cargo test --workspace
```

Integration tests need the OpenSSH Docker fixture:

```sh
docker build -t rooste-openssh crates/rooste-testkit/docker/openssh
docker run -d --rm -p 2222:22 --name rooste-openssh rooste-openssh
cargo nextest run --workspace --run-ignored only
```

## Repository layout

| Crate | Purpose |
|---|---|
| `rooste-core` | Session/profile model, config, `ssh_config` and session import |
| `rooste-proto` | `Transport` trait: SSH, later serial, telnet, raw |
| `rooste-term` | Terminal emulation (`alacritty_terminal`) |
| `rooste-keys` | Key generation, `.ppk` import, agent |
| `rooste-sftp` | SFTP operations and transfer queue |
| `rooste-gui` | Desktop application (egui) |
| `rooste-cli` | Headless client for testing and scripting |
| `rooste-testkit` | Shared test fixtures |

## License

[MIT](LICENSE)
