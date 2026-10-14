# Changelog

All notable changes are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [0.1.0] - unreleased

The first release.

### Sessions and terminal

- SSH sessions in tabs and split panes, with a searchable session manager (folders, tags) and a
  command palette.
- Duplicate a tab, reconnect automatically when a connection drops, and broadcast input to
  several sessions.
- Telnet, raw TCP and serial connections.
- True color, Unicode and mouse support; search through the scrollback (Ctrl+Shift+F); per-tab
  zoom (Ctrl + / Ctrl − / Ctrl 0); logging sessions to plain-text files.
- Light and dark themes; portable mode.

### Logins and security

- SSH agent (OpenSSH, Pageant on Windows, or the built-in agent), key files in OpenSSH and
  `.ppk` format (including old encrypted PEM keys), passwords in the system keyring, and
  keyboard-interactive login with one-time codes.
- Host keys checked against `~/.ssh/known_hosts`; unknown keys are confirmed first.
- Outdated algorithms only when enabled for a session.

### Forwarding and files

- Local, remote and dynamic (SOCKS) port forwarding with saved tunnels; jump hosts
  (`ProxyJump`); agent and X11 forwarding.
- An SFTP file panel: browse, upload and download files and folders, drag and drop uploads,
  "Download to…".

### Tools

- Key generator, `.ppk` ⇄ OpenSSH key converter and a built-in SSH agent.
- Import from `~/.ssh/config` and from PuTTY saved sessions.
- `rooste-cli`, a command-line client.

### Distribution

- Windows MSI installer and portable zip; Linux `.deb`, `.rpm`, AppImage, `.tar.gz` and Flatpak.
- Optional check for new releases at startup.

[0.1.0]: https://github.com/matbcvo/rooste-ssh/releases/tag/v0.1.0
