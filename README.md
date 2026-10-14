# rooste-ssh

A modern, memory-safe SSH client for Linux and Windows, written in Rust — with tabs, split panes,
a session manager, built-in SFTP, key generator and agent.

> **Status:** early development — M2 is done: a desktop app with a working terminal and a
> command-line client. See [docs/ROADMAP.md](docs/ROADMAP.md).

## Building

Requires Rust 1.95 or newer.

```sh
cargo build --release
cargo run -p rooste-gui     # desktop app (binary: rooste)
```

On Linux the desktop app needs the usual X11 or Wayland libraries (present on any desktop).

```sh
cargo run -p rooste-cli -- user@host       # interactive shell
cargo run -p rooste-cli -- user@host ls    # run a command
```

`rooste-cli` tries the SSH agent, then `~/.ssh/id_ed25519`, `id_ecdsa` and `id_rsa` (or keys given
with `-i`), then asks for a password. Unknown host keys are confirmed interactively and saved to
`~/.ssh/known_hosts`. Run `rooste-cli --help` for all options.

## Testing

```sh
cargo nextest run --workspace     # or: cargo test --workspace
```

The SSH tests run against an in-process test server, so they work without any setup. Tests against
a real OpenSSH server need the Docker fixture:

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
