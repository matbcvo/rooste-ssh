# Scripting

Rooste SSH can run small scripts that drive a session for you: log in to a device step by step,
run a series of commands, or wait for a message and react to it. Scripts are written in
[Rhai](https://rhai.rs), a simple language that looks a lot like JavaScript or Rust.

## Running scripts

Scripts are `.rhai` files in the **scripts folder**, next to the configuration file:

| System | Scripts folder |
|---|---|
| Linux | `~/.config/rooste-ssh/scripts/` |
| Windows | `%APPDATA%\rooste-ssh\scripts\` |
| macOS | `~/Library/Application Support/rooste-ssh/scripts/` |
| Portable | `scripts\` next to `rooste.exe` |

The easiest way to get there is **Open scripts folder**, in a tab's right-click menu (under **Run
script**) or in the command palette (Ctrl+Shift+P). The first time, Rooste creates the folder with
an `example.rhai` to start from.

To run a script:

- right-click a tab → **Run script** → the script, or
- open the command palette and choose **Run script: …**, or
- in a saved session, set **Run on connect** to a script: it runs every time that session connects.

While a script runs, a bar above the terminal shows its name with a **Stop** button. Messages from
the script, and whether it finished or failed, appear in the status bar at the bottom.

## Functions

| Function | What it does |
|---|---|
| `send(text)` | Types `text` into the session, as if you typed it. |
| `send_line(text)` | Types `text` and presses Enter. |
| `wait_for(text)` | Waits until `text` appears in the output; `true` when it does, `false` after 30 seconds. |
| `wait_for(text, seconds)` | The same with your own time limit, e.g. `wait_for("$ ", 5)` or `wait_for("ok", 0.5)`. |
| `sleep(seconds)` | Pauses, e.g. `sleep(2)` or `sleep(0.25)`. |
| `notify(message)` | Shows `message` in the status bar. |
| `ask(question)` | Asks you for some text and returns it. Cancelling stops the script. |
| `ask_password(question)` | The same, with the answer hidden as you type. |

And these values describe the session:

| Value | Example |
|---|---|
| `SESSION_NAME` | `"web-1"` |
| `HOST` | `"web1.example.com"` |
| `USER` | `"deploy"` |

`wait_for` looks at the output as plain text (colors and other terminal codes removed) that
arrived since the script started or since the previous `wait_for` matched. Everything up to and
including the match is used up, so waiting for the same prompt twice waits for it to appear twice.

## Examples

Enable mode on a network switch, asking for the password instead of storing it:

```rhai
send_line("enable");
if wait_for("Password:", 10) {
    send_line(ask_password("Enable password"));
}
if wait_for("#", 10) {
    send_line("terminal length 0");
    send_line("show running-config");
} else {
    notify("Did not get the enable prompt");
}
```

Run commands one after another, each after the shell prompt returns:

```rhai
let prompt = `${USER}@`;
for command in ["sudo apt update", "apt list --upgradable"] {
    wait_for(prompt, 30);
    send_line(command);
}
notify("Done");
```

Answer a question about which environment to deploy:

```rhai
let target = ask("Deploy which branch?");
send_line(`./deploy.sh ${target}`);
if wait_for("Deployed", 600) {
    notify(`${target} is deployed on ${SESSION_NAME}`);
} else {
    notify("Deploy did not finish within 10 minutes");
}
```

Rhai has the usual building blocks: `let` variables, `if`/`else`, `while`, `loop`, `for … in`,
functions with `fn`, strings with `${…}` inside backticks, arrays and maps. See the
[Rhai book](https://rhai.rs/book/) for the whole language.

## Safety

Scripts can only use the functions above. They can't read or write files, start programs or open
network connections. A script that loops forever can always be stopped with **Stop**, and closing
the tab stops it too.

Scripts don't see what you type in the terminal, and `ask_password` answers are only passed to
the script. Still, only run scripts you trust: a script can type anything into your session.
