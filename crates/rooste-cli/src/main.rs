//! Headless SSH client: `rooste-cli [user@]host [command...]`.

mod auth;
mod forwards;
mod interactive;

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use clap::{Parser, ValueEnum};
use futures::future::BoxFuture;
use rooste_proto::ssh::{
    AgentSource, HostKeyPolicy, HostKeyPrompt, PtyRequest, SshClient, SshConfig, SshError,
    UnknownHostKey, connect, connect_via,
};

use rooste_core::Destination;

/// Exit code for local and connection errors, as OpenSSH uses.
const EXIT_ERROR: u8 = 255;

#[derive(Debug, Parser)]
#[command(version, about = "Headless rooste-ssh client")]
struct Args {
    #[arg(help = "Server to connect to: [user@]host[:port]")]
    destination: String,

    /// Port (overrides a port given in the destination)
    #[arg(short, long)]
    port: Option<u16>,

    /// User name (overrides a user given in the destination)
    #[arg(short = 'l', long)]
    user: Option<String>,

    /// Private key file; may be repeated. Defaults to ~/.ssh/id_ed25519,
    /// id_ecdsa and id_rsa
    #[arg(short = 'i', long = "identity", value_name = "FILE")]
    identities: Vec<PathBuf>,

    /// Do not try keys from the SSH agent
    #[arg(long)]
    no_agent: bool,

    /// known_hosts file (default: ~/.ssh/known_hosts)
    #[arg(long, value_name = "FILE")]
    known_hosts: Option<PathBuf>,

    /// What to do with hosts not yet in known_hosts
    #[arg(long, value_enum, default_value_t = HostKeyMode::Ask)]
    host_key: HostKeyMode,

    /// Also offer outdated algorithms (SHA-1, CBC) for old devices
    #[arg(long)]
    allow_legacy: bool,

    /// Connect through these jump hosts: [user@]host[:port][,...]
    #[arg(short = 'J', long = "jump", value_name = "HOSTS")]
    jump: Option<String>,

    /// Local forward [bind:]port:host:hostport; may be repeated
    #[arg(short = 'L', value_name = "SPEC")]
    local: Vec<String>,

    /// Remote forward [bind:]port:host:hostport; may be repeated
    #[arg(short = 'R', value_name = "SPEC")]
    remote: Vec<String>,

    /// SOCKS proxy on [bind:]port; may be repeated
    #[arg(short = 'D', value_name = "SPEC")]
    dynamic: Vec<String>,

    /// Forward the SSH agent to the server
    #[arg(short = 'A')]
    forward_agent: bool,

    /// Only forward ports; no shell or command
    #[arg(short = 'N')]
    no_command: bool,

    /// Run this command instead of an interactive shell
    #[arg(trailing_var_arg = true)]
    command: Vec<String>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum HostKeyMode {
    Ask,
    AcceptNew,
    Strict,
}

#[tokio::main]
async fn main() {
    let code = match run(Args::parse()).await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("rooste-cli: {e:#}");
            if let Some(SshError::NoCommonAlgorithm {
                legacy_would_help: true,
                ..
            }) = e.downcast_ref()
            {
                eprintln!(
                    "hint: this server only supports outdated algorithms; \
                     rerun with --allow-legacy if you trust the network"
                );
            }
            EXIT_ERROR
        }
    };
    let _ = std::io::stdout().flush();
    // Exit right away: the runtime would otherwise wait for the blocking
    // stdin reader of an interactive session until the next key press.
    std::process::exit(code.into());
}

async fn run(args: Args) -> anyhow::Result<u8> {
    let dest = Destination::parse(&args.destination)?;
    let host = dest.host;
    let port = args.port.or(dest.port).unwrap_or(22);
    let user = args
        .user
        .or(dest.user)
        .or_else(local_user)
        .context("no user name given; use user@host or -l")?;

    let policy = match args.host_key {
        HostKeyMode::Ask => HostKeyPolicy::Ask(Arc::new(TerminalPrompt)),
        HostKeyMode::AcceptNew => HostKeyPolicy::AcceptNew,
        HostKeyMode::Strict => HostKeyPolicy::Strict,
    };
    let config_for = |host: &str, port: u16| {
        let mut config = SshConfig::new(host, port);
        if let Some(path) = &args.known_hosts {
            config.known_hosts = path.clone();
        }
        config.allow_legacy_algorithms = args.allow_legacy;
        config.host_key_policy = policy.clone();
        config
    };
    let methods = auth::Methods {
        agent: !args.no_agent,
        identities: args.identities.clone(),
    };
    let specs = forwards::Specs::parse(&args.local, &args.remote, &args.dynamic)?;

    // Each jump host is reached through the previous one.
    let mut previous: Option<SshClient> = None;
    let hops = match &args.jump {
        Some(jump) => Destination::parse_list(jump)?,
        None => Vec::new(),
    };
    for hop in hops {
        let hop_user = hop.user.clone().unwrap_or_else(|| user.clone());
        let config = config_for(&hop.host, hop.port.unwrap_or(22));
        let mut client = match previous.take() {
            None => connect(&config).await?,
            Some(via) => connect_via(via, &config).await?,
        };
        auth::authenticate(&mut client, &hop_user, &hop.host, methods.clone()).await?;
        previous = Some(client);
    }
    let config = config_for(&host, port);
    let mut client = match previous {
        None => connect(&config).await?,
        Some(via) => connect_via(via, &config).await?,
    };
    auth::authenticate(&mut client, &user, &host, methods).await?;

    let _forwards = specs.start(&client).await?;
    let agent = args.forward_agent.then_some(AgentSource::Default);
    if args.no_command {
        eprintln!("Forwarding; press Ctrl+C to stop.");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = async { while !client.is_closed() { tokio::time::sleep(std::time::Duration::from_millis(500)).await } } => {
                anyhow::bail!("connection closed");
            }
        }
        let _ = client.disconnect().await;
        return Ok(0);
    }

    if !args.command.is_empty() {
        let output = client.exec_with(&args.command.join(" "), agent).await?;
        std::io::stdout().write_all(&output.stdout)?;
        std::io::stderr().write_all(&output.stderr)?;
        return Ok(exit_code(output.exit_status));
    }

    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let pty = PtyRequest {
        term: std::env::var("TERM").unwrap_or_else(|_| "xterm-256color".into()),
        cols,
        rows,
    };
    let mut shell = client.open_shell_with(&pty, agent).await?;
    interactive::run(&mut shell).await?;
    let status = shell.exit_status();
    let _ = client.disconnect().await;
    Ok(exit_code(status))
}

/// Remote exit status as a process exit code; 255 if the server sent none.
fn exit_code(status: Option<u32>) -> u8 {
    status.map_or(EXIT_ERROR, |s| u8::try_from(s).unwrap_or(EXIT_ERROR))
}

fn local_user() -> Option<String> {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .ok()
}

/// Asks on the terminal before trusting an unknown host key.
struct TerminalPrompt;

impl HostKeyPrompt for TerminalPrompt {
    fn confirm_unknown(&self, key: UnknownHostKey) -> BoxFuture<'static, bool> {
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let host = if key.port == 22 {
                    key.host.clone()
                } else {
                    format!("[{}]:{}", key.host, key.port)
                };
                eprintln!("The authenticity of host '{host}' can't be established.");
                eprintln!("{} key fingerprint is {}.", key.algorithm, key.fingerprint);
                eprint!("Are you sure you want to continue connecting (yes/no)? ");
                let mut answer = String::new();
                std::io::stdin().lock().read_line(&mut answer).ok();
                matches!(answer.trim().to_lowercase().as_str(), "yes" | "y")
            })
            .await
            .unwrap_or(false)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes() {
        assert_eq!(exit_code(Some(0)), 0);
        assert_eq!(exit_code(Some(3)), 3);
        assert_eq!(exit_code(Some(300)), 255);
        assert_eq!(exit_code(None), 255);
    }

    #[test]
    fn args_parse() {
        let args = Args::try_parse_from([
            "rooste-cli",
            "-p",
            "2222",
            "-i",
            "key",
            "--host-key",
            "accept-new",
            "me@host",
            "ls",
            "-la",
        ])
        .unwrap();
        assert_eq!(args.port, Some(2222));
        assert_eq!(args.identities, [PathBuf::from("key")]);
        assert!(matches!(args.host_key, HostKeyMode::AcceptNew));
        assert_eq!(args.destination, "me@host");
        assert_eq!(args.command, ["ls", "-la"]);
    }
}
