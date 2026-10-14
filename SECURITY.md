# Security policy

Rooste SSH handles credentials and remote connections, so security reports are taken seriously.

## Reporting a vulnerability

Please **do not** open a public issue. Report it privately through GitHub instead:
[Report a vulnerability](https://github.com/matbcvo/rooste-ssh/security/advisories/new)
(the repository's **Security** tab → **Report a vulnerability**).

Include what you found, how to reproduce it, and which version and platform you used. You will get
an answer within a week. Once a fix is released, the advisory is published and you are credited
unless you prefer otherwise.

## Supported versions

Fixes go into the latest release. Please check that the problem still exists there before
reporting.

## Scope

In scope: the `rooste` desktop app, `rooste-cli` and the crates in this repository, for example
host-key checking, authentication, credential storage, the built-in agent, port, agent and X11
forwarding, and the installers.

Vulnerabilities in dependencies (such as `russh`) are best reported to their projects as well;
let us know too, so a fixed version can be adopted.
