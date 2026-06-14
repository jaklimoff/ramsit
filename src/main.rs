mod app;
mod audio;
mod net;
mod proto;
mod punch;
mod ui;

use anyhow::{Context, Result};
use app::App;
use ratatui::crossterm::event::{self, Event as CEvent, KeyEventKind};
use ratatui::DefaultTerminal;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::Duration;

const DEFAULT_STUN: &str = "stun.l.google.com:19302";

fn main() -> Result<()> {
    init_logger()?;

    let stun = stun_arg().unwrap_or_else(|| DEFAULT_STUN.to_string());
    let stun_addr = resolve_stun(&stun)?;
    log::info!("stun: using server {stun} ({stun_addr})");

    let (_handle, cmd_tx, evt_rx) = net::spawn(stun_addr);

    let mut terminal = ratatui::init();
    let mut app = App::new();
    let result = run(&mut terminal, &mut app, &cmd_tx, &evt_rx);
    ratatui::restore();
    // On quit we abandon the worker, but give it a beat to flush a best-effort
    // BYE first — it's the only way the peer learns we left (no liveness timer),
    // and the session loop drains commands within one ~200ms poll.
    std::thread::sleep(Duration::from_millis(300));
    result
}

fn run(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    cmd_tx: &Sender<net::Command>,
    evt_rx: &Receiver<net::Event>,
) -> Result<()> {
    loop {
        terminal.draw(|f| ui::draw(f, app))?;

        if event::poll(Duration::from_millis(100))? {
            if let CEvent::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    if let Some(cmd) = app.on_key(key) {
                        let _ = cmd_tx.send(cmd);
                    }
                }
            }
        }

        loop {
            match evt_rx.try_recv() {
                Ok(ev) => {
                    log::debug!("ui: event {ev:?}");
                    app.apply(ev);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    app.connection_lost();
                    break;
                }
            }
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}

/// Logs go to a file so they never corrupt the TUI. Default level `info`; set
/// RUST_LOG=debug for a per-packet trace. Watch with `tail -f ramsit.log`.
fn init_logger() -> Result<()> {
    let file = std::fs::File::create("ramsit.log").context("create ramsit.log")?;
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .target(env_logger::Target::Pipe(Box::new(file)))
        .format_timestamp_millis()
        .format_target(false)
        .init();
    Ok(())
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
