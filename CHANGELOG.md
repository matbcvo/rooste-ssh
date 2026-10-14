# Changelog

All notable changes are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [0.2.0] - 2026-10-14

### Added

- Experimental macOS builds (`.dmg` for Apple Silicon and Intel), not yet notarized. Shortcuts use
  ⌘ instead of Ctrl, ⌘C and ⌘V copy and paste, and settings are kept in
  `~/Library/Application Support/rooste-ssh`.

## [0.1.1] - 2026-10-14

### Changed

- The project moved to its own organization:
  [github.com/rooste-ssh/rooste-ssh](https://github.com/rooste-ssh/rooste-ssh). Old links
  redirect; the update check now asks the new address.
- The Linux app ID is now `ee.prica.rooste-ssh` (it was `io.github.matbcvo.rooste-ssh`) and no
  longer depends on where the repository lives. If you installed the 0.1.0 Flatpak, uninstall it
  and install this one; other packages and the Windows installer upgrade as usual. Settings,
  saved sessions and saved passwords are kept.

## [0.1.0] - 2026-10-14

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

[0.2.0]: https://github.com/rooste-ssh/rooste-ssh/releases/tag/v0.2.0
[0.1.1]: https://github.com/rooste-ssh/rooste-ssh/releases/tag/v0.1.1
[0.1.0]: https://github.com/rooste-ssh/rooste-ssh/releases/tag/v0.1.0
