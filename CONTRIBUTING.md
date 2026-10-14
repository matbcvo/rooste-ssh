# Contributing to Rooste SSH

Thanks for helping! Bug reports, ideas and pull requests are all welcome. For bigger changes,
please open an issue first so we can agree on the approach.

## Building and running

Requires Rust 1.95 or newer.

```sh
cargo run -p rooste-gui              # desktop app (binary: rooste)
cargo run -p rooste-cli -- user@host # command-line client
```

## Tests

```sh
cargo nextest run --workspace     # or: cargo test --workspace
```

The SSH tests run against an in-process test server (`rooste-testkit`), so they need no setup.
The GUI tests drive the real app with `egui_kittest`. Tests against a real OpenSSH server need the
Docker fixture:

```sh
docker build -t rooste-openssh -f crates/rooste-testkit/docker/openssh/Dockerfile crates/rooste-testkit/docker
docker run -d --rm -p 2222:22 --name rooste-openssh rooste-openssh
cargo nextest run --workspace --run-ignored only
```

The screenshot tests render the app to `target/screenshots/` for a visual check (they need a
wgpu adapter; a software Vulkan driver is enough):

```sh
cargo test -p rooste-gui --test screenshot -- --ignored
```

## Before opening a pull request

CI runs these on Linux and Windows; running them locally first saves a round trip:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace
cargo deny check
```

- Add or update tests with every change: protocol code against the test server, UI changes with a
  GUI test.
- Unsafe code is not allowed (`unsafe_code = "forbid"` for the whole workspace).
- New dependencies must have a license that `deny.toml` allows (MIT, Apache-2.0, BSD and the like).
- Keep commits focused and describe the why in the message.

## Repository layout

| Crate | Purpose |
|---|---|
| `rooste-core` | Session/profile model, config, `ssh_config` and session import |
| `rooste-protocols` | `Transport` trait: SSH, telnet, raw TCP and serial |
| `rooste-terminal` | Terminal emulation (`alacritty_terminal`), input encoding, search |
| `rooste-keys` | Key generation, `.ppk` import and export, the built-in agent |
| `rooste-sftp` | SFTP over an SSH connection: listing, transfers with progress |
| `rooste-gui` | Desktop application (egui) |
| `rooste-cli` | Command-line client, also used in tests and release smoke tests |
| `rooste-testkit` | Shared test fixtures: SSH, telnet and agent test servers |

[docs/ROADMAP.md](docs/ROADMAP.md) describes the architecture, decisions and plans.

## License

By contributing, you agree that your contributions are licensed under the [MIT License](LICENSE).
