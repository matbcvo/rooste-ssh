# rooste-ssh

A modern, memory-safe SSH client for Linux and Windows, written in Rust — with tabs, split panes,
a session manager, built-in SFTP, key generator and agent.

> **Status:** early development — M5 is done: saved sessions, tabs and split panes, port
> forwarding, jump hosts, an SFTP file panel, a key generator, PuTTY key conversion and a
> built-in SSH agent. See [docs/ROADMAP.md](docs/ROADMAP.md).

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
cargo run -p rooste-cli -- -J bastion -L 5432:db:5432 -N user@host   # jump host + forward
```

`rooste-cli` tries the SSH agent, then `~/.ssh/id_ed25519`, `id_ecdsa` and `id_rsa` (or keys given
with `-i`), then asks for a password. Unknown host keys are confirmed interactively and saved to
`~/.ssh/known_hosts`. Run `rooste-cli --help` for all options.

## Configuration

Settings and saved sessions are stored in `config.toml` in the platform config directory
(`~/.config/rooste-ssh/` on Linux, `%APPDATA%\rooste-ssh\` on Windows). To run portably,
for example from a USB stick, put an empty file named `portable` next to the executable; the
configuration is then kept in `rooste.toml` beside it. Saved passwords go to the system keyring,
never into the configuration file.

## Testing

```sh
cargo nextest run --workspace     # or: cargo test --workspace
```

The SSH tests run against an in-process test server, so they work without any setup. Tests against
a real OpenSSH server need the Docker fixture:

```sh
docker build -t rooste-openssh -f crates/rooste-testkit/docker/openssh/Dockerfile crates/rooste-testkit/docker
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
| `rooste-sftp` | SFTP over an SSH connection: listing, transfers with progress |
| `rooste-gui` | Desktop application (egui) |
| `rooste-cli` | Headless client for testing and scripting |
| `rooste-testkit` | Shared test fixtures |

## License

[MIT](LICENSE)
