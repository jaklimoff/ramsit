mod app;
mod chat;
mod net;
mod proto;
mod punch;
mod ui;

use anyhow::Context;
use anyhow::Result;
use log::info;
use proto::parse_code;
use std::io::{BufRead, Write};
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};

const DEFAULT_STUN: &str = "stun.l.google.com:19302";

fn main() -> Result<()> {
    // Logs go to stderr (chat UI stays on stdout). Default level `info`; set
    // RUST_LOG=debug for per-packet tracing when diagnosing a failed punch.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .format_target(false)
        .init();

    let stun = stun_arg().unwrap_or_else(|| DEFAULT_STUN.to_string());
    let stun_addr = resolve_stun(&stun)?;
    info!("stun: using server {stun} ({stun_addr})");

    let sock = UdpSocket::bind("0.0.0.0:0").context("failed to bind a UDP socket")?;
    info!("socket: bound to {}", sock.local_addr()?);
    let my_addr = net::discover(&sock, stun_addr)?;
    info!("stun: discovered public endpoint {my_addr}");

    println!("Your code: {my_addr}");
    println!("Send that to your bro, then paste theirs below.\n");

    print!("Peer code: ");
    std::io::stdout().flush()?;
    let code = read_peer_code()?;
    info!("peer: advertised code {code}");

    println!("\nPunching through… (this can take up to a minute)");
    let (peer, early) = punch::punch(&sock, code)?;
    info!("connected to {peer}");
    println!("Connected! Type messages. /quit to leave.\n");

    chat::chat(sock, peer, early)
}

/// Parse an optional `--stun <addr>` flag.
fn stun_arg() -> Option<String> {
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--stun" {
            return args.next();
        }
    }
    None
}

/// Resolve a STUN server host:port to its first IPv4 socket address.
fn resolve_stun(s: &str) -> Result<SocketAddr> {
    s.to_socket_addrs()
        .with_context(|| format!("could not resolve STUN server '{s}'"))?
        .find(|a| a.is_ipv4())
        .with_context(|| format!("no IPv4 address for STUN server '{s}'"))
}

/// Read and parse the peer's code from one line of stdin.
fn read_peer_code() -> Result<SocketAddr> {
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .context("failed to read peer code from stdin")?;
    parse_code(&line)
}
