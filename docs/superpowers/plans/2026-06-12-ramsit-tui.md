# ramsit ratatui TUI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace ramsit's line-based chat with a full-screen ratatui TUI covering discovery → code exchange → punching → scrollable chat, with the network running on a worker thread that talks to the UI over channels.

**Architecture:** UI thread (main) owns the terminal and draws/reads keys; a network worker thread owns the `UdpSocket` and runs STUN discovery, `punch()`, and the chat send/recv/keepalive loop. They communicate over two `mpsc` channels (`Command` UI→worker, `Event` worker→UI). `proto.rs` and `punch.rs` are untouched.

**Tech Stack:** Rust 2021, `ratatui` 0.29 (re-exports `crossterm`), `stunclient`, `anyhow`, `log` + `env_logger` (logs to `ramsit.log`).

**Module map (after this plan):**
- `proto.rs` — unchanged.
- `punch.rs` — unchanged.
- `net.rs` — NEW. `Command`/`Event` enums, `discover()` (moved from main), `encode_chat()` (char-boundary truncation), `session()` (testable bridge), `worker()`, `spawn()`. Replaces `chat.rs`.
- `app.rs` — NEW. `App`, `Screen`, `apply(Event)`, `on_key(KeyEvent) -> Option<Command>`, `connection_lost()`. Pure logic, unit-tested.
- `ui.rs` — NEW. `draw(&mut Frame, &App)`. Rendered in tests via `TestBackend`.
- `main.rs` — rewritten. File logger, `ratatui::init/restore`, spawn worker, event loop.
- `chat.rs` — DELETED in the final task.

**Note on intermediate warnings:** Tasks 2–4 add modules before `main.rs` wires them, so `cargo build`/`cargo test` may emit `dead_code` warnings (warnings don't fail those commands). Task 5 wires everything and is the one that must pass `cargo clippy -- -D warnings` cleanly.

---

### Task 1: Add the ratatui dependency

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: Add `ratatui` to `[dependencies]`**

Edit `Cargo.toml` so the dependencies block reads:

```toml
[dependencies]
anyhow = "1"
env_logger = "0.11"
log = "0.4"
ratatui = "0.29"
stunclient = "0.4"
```

- [ ] **Step 2: Build to fetch and confirm it resolves**

Run: `cargo build`
Expected: compiles; downloads `ratatui` and `crossterm`. Existing tests/code still build.

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: add ratatui dependency"
```

---

### Task 2: Network worker module (`net.rs`)

**Files:**
- Create: `src/net.rs`
- Modify: `src/main.rs` (add `mod net;`, switch to `net::discover`, drop the local `discover`)

- [ ] **Step 1: Create `src/net.rs`**

```rust
use crate::proto::{classify, would_block, PacketKind, BYE, KEEPALIVE, MAX_CHAT_BYTES, RECV_BUF};
use crate::punch;
use anyhow::{anyhow, Result};
use log::{info, warn};
use std::net::{SocketAddr, UdpSocket};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use stunclient::StunClient;

/// Messages from the UI thread to the network worker.
pub enum Command {
    PeerCode(SocketAddr),
    Send(String),
    Quit,
}

/// Messages from the network worker to the UI thread.
pub enum Event {
    Discovered(SocketAddr),
    Connected(SocketAddr),
    Incoming(String),
    PeerLeft,
    Fatal(String),
}

const POLL: Duration = Duration::from_millis(200);
const KEEPALIVE_TICKS: u32 = 75; // 75 * 200ms = 15s

/// Truncate a chat line to the wire limit on a UTF-8 char boundary, so a
/// multi-byte char straddling the limit is never split (which would make the
/// receiver's `from_utf8` fail and silently drop the message).
pub fn encode_chat(line: &str) -> Vec<u8> {
    let mut end = MAX_CHAT_BYTES.min(line.len());
    while end > 0 && !line.is_char_boundary(end) {
        end -= 1;
    }
    line[..end].as_bytes().to_vec()
}

/// Query our public endpoint via STUN, retrying up to 3× (UDP can drop the
/// request) before giving up.
pub fn discover(sock: &UdpSocket, stun: SocketAddr) -> Result<SocketAddr> {
    let client = StunClient::new(stun);
    let mut last = None;
    for attempt in 1..=3 {
        match client.query_external_address(sock) {
            Ok(addr) => return Ok(addr),
            Err(e) => {
                warn!("stun: attempt {attempt}/3 failed: {e}");
                last = Some(e.to_string());
            }
        }
    }
    Err(anyhow!(
        "STUN query failed after 3 tries ({}) — check your network or try \
         another server with --stun <host:port>",
        last.unwrap_or_default()
    ))
}

/// Spawn the network worker. Returns its join handle plus the channel ends the
/// UI uses to command it and receive its events.
pub fn spawn(stun: SocketAddr) -> (JoinHandle<()>, Sender<Command>, Receiver<Event>) {
    let (cmd_tx, cmd_rx) = channel::<Command>();
    let (evt_tx, evt_rx) = channel::<Event>();
    let handle = thread::spawn(move || worker(stun, cmd_rx, evt_tx));
    (handle, cmd_tx, evt_rx)
}

fn worker(stun: SocketAddr, cmds: Receiver<Command>, events: Sender<Event>) {
    let sock = match UdpSocket::bind("0.0.0.0:0") {
        Ok(s) => s,
        Err(e) => {
            let _ = events.send(Event::Fatal(format!("failed to bind a UDP socket: {e}")));
            return;
        }
    };
    info!(
        "socket: bound to {}",
        sock.local_addr()
            .map(|a| a.to_string())
            .unwrap_or_else(|_| "?".into())
    );

    let my = match discover(&sock, stun) {
        Ok(a) => a,
        Err(e) => {
            let _ = events.send(Event::Fatal(e.to_string()));
            return;
        }
    };
    info!("stun: discovered public endpoint {my}");
    let _ = events.send(Event::Discovered(my));

    // Wait for the peer code (or an early quit / UI gone).
    let code = loop {
        match cmds.recv() {
            Ok(Command::PeerCode(c)) => break c,
            Ok(Command::Quit) | Err(_) => return,
            Ok(_) => {} // ignore Send before we're connected
        }
    };

    let (peer, early) = match punch::punch(&sock, code) {
        Ok(v) => v,
        Err(e) => {
            let _ = events.send(Event::Fatal(e.to_string()));
            return;
        }
    };
    info!("connected to {peer}");
    let _ = events.send(Event::Connected(peer));
    for m in early {
        let _ = events.send(Event::Incoming(m));
    }

    session(sock, peer, cmds, events);
}

/// The post-connect bridge loop: forward outgoing `Send`s onto the socket,
/// surface incoming chat/BYE as events, and refresh the NAT mapping on a timer.
/// Factored out so a loopback test can drive it without STUN/punch.
pub fn session(sock: UdpSocket, peer: SocketAddr, cmds: Receiver<Command>, events: Sender<Event>) {
    let _ = sock.set_read_timeout(Some(POLL));
    let mut buf = [0u8; RECV_BUF];
    let mut ticks = 0u32;

    loop {
        // Drain outgoing commands.
        loop {
            match cmds.try_recv() {
                Ok(Command::Send(line)) => {
                    let _ = sock.send_to(&encode_chat(&line), peer);
                }
                Ok(Command::Quit) => {
                    let _ = sock.send_to(BYE, peer);
                    return;
                }
                Ok(Command::PeerCode(_)) => {} // already connected; ignore
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    let _ = sock.send_to(BYE, peer);
                    return;
                }
            }
        }

        // One inbound read (peer IP filter, as in the old chat loop).
        match sock.recv_from(&mut buf) {
            Ok((n, from)) if from.ip() == peer.ip() => match classify(&buf[..n]) {
                PacketKind::Chat => {
                    if let Ok(s) = std::str::from_utf8(&buf[..n]) {
                        if events.send(Event::Incoming(s.to_string())).is_err() {
                            return; // UI gone
                        }
                    }
                }
                PacketKind::Bye => {
                    let _ = events.send(Event::PeerLeft);
                    // Keep running so the user can read history and quit cleanly.
                }
                _ => {}
            },
            Ok(_) => {}
            Err(e) if would_block(&e) => {}
            Err(_) => return,
        }

        ticks += 1;
        if ticks >= KEEPALIVE_TICKS {
            let _ = sock.send_to(KEEPALIVE, peer);
            ticks = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_chat_truncates_on_char_boundary() {
        // 600 × 'é' (2 bytes each) = 1200 bytes, over the 1024 limit.
        let line = "é".repeat(600);
        let bytes = encode_chat(&line);
        assert!(bytes.len() <= MAX_CHAT_BYTES);
        // Must still be valid UTF-8 — never split a multi-byte char.
        assert!(std::str::from_utf8(&bytes).is_ok());
        // 1024 is odd vs 2-byte chars, so it lands on 1024-1 = 1023? No: the
        // last whole 'é' ends at an even offset ≤ 1024, i.e. 1024.
        assert_eq!(bytes.len() % 2, 0);
    }

    #[test]
    fn encode_chat_passes_short_lines_through() {
        assert_eq!(encode_chat("hi bro"), b"hi bro");
    }

    #[test]
    fn session_delivers_message_over_loopback() {
        let a = UdpSocket::bind("127.0.0.1:0").unwrap();
        let b = UdpSocket::bind("127.0.0.1:0").unwrap();
        let a_addr = a.local_addr().unwrap();
        let b_addr = b.local_addr().unwrap();

        let (a_cmd_tx, a_cmd_rx) = channel();
        let (a_evt_tx, _a_evt_rx) = channel();
        let (b_cmd_tx, _b_cmd_rx) = channel();
        let (b_evt_tx, b_evt_rx) = channel();

        let ha = thread::spawn(move || session(a, b_addr, a_cmd_rx, a_evt_tx));
        let hb = thread::spawn(move || session(b, a_addr, b_cmd_rx, b_evt_tx));

        a_cmd_tx.send(Command::Send("hello bro".into())).unwrap();
        match b_evt_rx.recv_timeout(Duration::from_secs(2)).unwrap() {
            Event::Incoming(s) => assert_eq!(s, "hello bro"),
            _ => panic!("expected Incoming"),
        }

        a_cmd_tx.send(Command::Quit).unwrap();
        b_cmd_tx.send(Command::Quit).unwrap();
        let _ = ha.join();
        let _ = hb.join();
    }
}
```

- [ ] **Step 2: Wire `net` into `main.rs` and use `net::discover`**

In `src/main.rs`, add the module declaration with the others:

```rust
mod chat;
mod net;
mod proto;
mod punch;
```

Then delete the local `discover` function (the whole `fn discover(...) { ... }` block) and change its one call site in `main()` from:

```rust
    let my_addr = discover(&sock, stun_addr)?;
```

to:

```rust
    let my_addr = net::discover(&sock, stun_addr)?;
```

Remove the now-unused `use stunclient::StunClient;` and `use anyhow::anyhow;` from `main.rs` if the compiler flags them (they moved to `net.rs`).

- [ ] **Step 3: Run tests**

Run: `cargo test net`
Expected: `encode_chat_truncates_on_char_boundary`, `encode_chat_passes_short_lines_through`, and `session_delivers_message_over_loopback` pass. (`dead_code` warnings for `spawn`/`Event` variants are expected — main doesn't use them yet.)

- [ ] **Step 4: Confirm the whole suite still builds and passes**

Run: `cargo test`
Expected: all existing tests plus the 3 new ones pass.

- [ ] **Step 5: Commit**

```bash
git add src/net.rs src/main.rs
git commit -m "feat: network worker module with testable session bridge"
```

---

### Task 3: UI state machine (`app.rs`)

**Files:**
- Create: `src/app.rs`
- Modify: `src/main.rs` (add `mod app;`)

- [ ] **Step 1: Create `src/app.rs`**

```rust
use crate::net::{Command, Event};
use crate::proto::parse_code;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

const PAGE: usize = 5;

/// Which screen is showing. Carries that screen's state.
pub enum Screen {
    Discovering,
    Exchange {
        my_code: std::net::SocketAddr,
        input: String,
        error: Option<String>,
    },
    Punching {
        peer: std::net::SocketAddr,
    },
    Chat {
        peer: std::net::SocketAddr,
        messages: Vec<String>,
        input: String,
        scroll: usize, // lines from the bottom; 0 = pinned
        connected: bool,
    },
    Fatal {
        msg: String,
    },
}

pub struct App {
    pub screen: Screen,
    pub should_quit: bool,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    pub fn new() -> Self {
        App {
            screen: Screen::Discovering,
            should_quit: false,
        }
    }

    /// Apply an event from the network worker.
    pub fn apply(&mut self, ev: Event) {
        match ev {
            Event::Discovered(code) => {
                if matches!(self.screen, Screen::Discovering) {
                    self.screen = Screen::Exchange {
                        my_code: code,
                        input: String::new(),
                        error: None,
                    };
                }
            }
            Event::Connected(peer) => {
                self.screen = Screen::Chat {
                    peer,
                    messages: Vec::new(),
                    input: String::new(),
                    scroll: 0,
                    connected: true,
                };
            }
            Event::Incoming(s) => {
                if let Screen::Chat { messages, scroll, .. } = &mut self.screen {
                    messages.push(format!("peer> {s}"));
                    if *scroll > 0 {
                        let max = messages.len().saturating_sub(1);
                        *scroll = (*scroll + 1).min(max);
                    }
                }
            }
            Event::PeerLeft => {
                if let Screen::Chat {
                    messages, connected, ..
                } = &mut self.screen
                {
                    *connected = false;
                    messages.push("* peer disconnected *".to_string());
                }
            }
            Event::Fatal(msg) => {
                self.screen = Screen::Fatal { msg };
            }
        }
    }

    /// The worker's event channel disconnected unexpectedly.
    pub fn connection_lost(&mut self) {
        if !matches!(self.screen, Screen::Fatal { .. }) {
            self.screen = Screen::Fatal {
                msg: "connection lost — the network thread stopped".to_string(),
            };
        }
    }

    /// Handle a keypress. May mutate screen state and/or return a command to
    /// send to the worker.
    pub fn on_key(&mut self, key: KeyEvent) -> Option<Command> {
        // Global quit: Esc or Ctrl-C from any screen.
        let ctrl_c = key.code == KeyCode::Char('c')
            && key.modifiers.contains(KeyModifiers::CONTROL);
        if key.code == KeyCode::Esc || ctrl_c {
            self.should_quit = true;
            return Some(Command::Quit);
        }

        let mut transition: Option<Screen> = None;
        let mut cmd: Option<Command> = None;
        let mut quit = false;

        match &mut self.screen {
            Screen::Exchange { input, error, .. } => match key.code {
                KeyCode::Char(c) => {
                    input.push(c);
                    *error = None;
                }
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Enter => match parse_code(input) {
                    Ok(addr) => {
                        transition = Some(Screen::Punching { peer: addr });
                        cmd = Some(Command::PeerCode(addr));
                    }
                    Err(e) => *error = Some(e.to_string()),
                },
                _ => {}
            },
            Screen::Chat {
                messages,
                input,
                scroll,
                ..
            } => match key.code {
                KeyCode::Char(c) => input.push(c),
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Enter => {
                    if !input.is_empty() {
                        let line = std::mem::take(input);
                        messages.push(format!("you> {line}"));
                        *scroll = 0;
                        cmd = Some(Command::Send(line));
                    }
                }
                KeyCode::PageUp => {
                    let max = messages.len().saturating_sub(1);
                    *scroll = (*scroll + PAGE).min(max);
                }
                KeyCode::PageDown => {
                    *scroll = scroll.saturating_sub(PAGE);
                }
                _ => {}
            },
            Screen::Fatal { .. } => {
                if key.code == KeyCode::Char('q') {
                    quit = true;
                }
            }
            _ => {} // Discovering, Punching: nothing but the global quit
        }

        if let Some(s) = transition {
            self.screen = s;
        }
        if quit {
            self.should_quit = true;
        }
        cmd
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn addr() -> std::net::SocketAddr {
        "203.0.113.5:54213".parse().unwrap()
    }

    fn chat_app() -> App {
        App {
            screen: Screen::Chat {
                peer: addr(),
                messages: Vec::new(),
                input: String::new(),
                scroll: 0,
                connected: true,
            },
            should_quit: false,
        }
    }

    #[test]
    fn discovered_moves_to_exchange() {
        let mut app = App::new();
        app.apply(Event::Discovered(addr()));
        assert!(matches!(app.screen, Screen::Exchange { .. }));
    }

    #[test]
    fn incoming_appends_message_in_chat() {
        let mut app = chat_app();
        app.apply(Event::Incoming("yo".into()));
        if let Screen::Chat { messages, .. } = &app.screen {
            assert_eq!(messages, &vec!["peer> yo".to_string()]);
        } else {
            panic!("expected Chat");
        }
    }

    #[test]
    fn submit_chat_line_emits_send_and_echoes() {
        let mut app = chat_app();
        if let Screen::Chat { input, .. } = &mut app.screen {
            *input = "hello".to_string();
        }
        let cmd = app.on_key(key(KeyCode::Enter));
        assert!(matches!(cmd, Some(Command::Send(ref s)) if s == "hello"));
        if let Screen::Chat { messages, input, .. } = &app.screen {
            assert_eq!(messages, &vec!["you> hello".to_string()]);
            assert!(input.is_empty());
        } else {
            panic!("expected Chat");
        }
    }

    #[test]
    fn scroll_clamps_at_both_bounds() {
        let mut app = chat_app();
        if let Screen::Chat { messages, .. } = &mut app.screen {
            *messages = vec!["a".into(), "b".into(), "c".into()]; // max scroll = 2
        }
        app.on_key(key(KeyCode::PageUp)); // +5, clamped to 2
        if let Screen::Chat { scroll, .. } = &app.screen {
            assert_eq!(*scroll, 2);
        }
        app.on_key(key(KeyCode::PageDown)); // -5, clamped to 0
        if let Screen::Chat { scroll, .. } = &app.screen {
            assert_eq!(*scroll, 0);
        }
    }

    #[test]
    fn bad_peer_code_shows_inline_error() {
        let mut app = App::new();
        app.apply(Event::Discovered(addr()));
        if let Screen::Exchange { input, .. } = &mut app.screen {
            *input = "garbage".to_string();
        }
        let cmd = app.on_key(key(KeyCode::Enter));
        assert!(cmd.is_none());
        match &app.screen {
            Screen::Exchange { error, .. } => assert!(error.is_some()),
            _ => panic!("should stay on Exchange"),
        }
    }

    #[test]
    fn esc_sets_quit_and_returns_quit_command() {
        let mut app = chat_app();
        let cmd = app.on_key(key(KeyCode::Esc));
        assert!(app.should_quit);
        assert!(matches!(cmd, Some(Command::Quit)));
    }

    #[test]
    fn fatal_event_sets_fatal_screen() {
        let mut app = App::new();
        app.apply(Event::Fatal("boom".into()));
        assert!(matches!(app.screen, Screen::Fatal { .. }));
    }
}
```

- [ ] **Step 2: Register the module in `src/main.rs`**

Add to the module list:

```rust
mod app;
mod chat;
mod net;
mod proto;
mod punch;
```

- [ ] **Step 3: Run the app tests**

Run: `cargo test app`
Expected: all 7 tests pass. (`dead_code` warnings expected — `main.rs` doesn't use `App` yet.)

- [ ] **Step 4: Commit**

```bash
git add src/app.rs src/main.rs
git commit -m "feat: App state machine with apply/on_key and unit tests"
```

---

### Task 4: Rendering (`ui.rs`)

**Files:**
- Create: `src/ui.rs`
- Modify: `src/main.rs` (add `mod ui;`)

- [ ] **Step 1: Create `src/ui.rs`**

```rust
use crate::app::{App, Screen};
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

/// Render the whole UI for the current screen.
pub fn draw(f: &mut Frame, app: &App) {
    match &app.screen {
        Screen::Discovering => {
            centered(f, "Discovering your public address via STUN…", Color::Gray);
        }
        Screen::Exchange {
            my_code,
            input,
            error,
        } => draw_exchange(f, &my_code.to_string(), input, error.as_deref()),
        Screen::Punching { peer } => {
            centered(
                f,
                &format!("Punching through to {peer}…  (up to 60s)"),
                Color::Yellow,
            );
        }
        Screen::Chat {
            peer,
            messages,
            input,
            scroll,
            connected,
        } => draw_chat(f, &peer.to_string(), messages, input, *scroll, *connected),
        Screen::Fatal { msg } => centered(f, msg, Color::Red),
    }
}

fn centered(f: &mut Frame, text: &str, color: Color) {
    let p = Paragraph::new(text)
        .style(Style::default().fg(color))
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL).title(" ramsit "));
    f.render_widget(p, f.area());
}

fn draw_exchange(f: &mut Frame, my_code: &str, input: &str, error: Option<&str>) {
    let lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::raw("Your code: "),
            Span::styled(my_code, Style::default().fg(Color::Green).bold()),
        ]),
        Line::from("Share it with your bro, then paste theirs below."),
        Line::from(""),
        Line::from(vec![Span::raw("Peer code: "), Span::raw(input), Span::raw("_")]),
        Line::from(""),
        Line::from(match error {
            Some(e) => Span::styled(e, Style::default().fg(Color::Red)),
            None => Span::raw("Press Enter to connect · Esc to quit"),
        }),
    ];
    let p = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" ramsit "))
        .wrap(Wrap { trim: true });
    f.render_widget(p, f.area());
}

fn draw_chat(
    f: &mut Frame,
    peer: &str,
    messages: &[String],
    input: &str,
    scroll: usize,
    connected: bool,
) {
    let areas = Layout::vertical([Constraint::Min(1), Constraint::Length(3)]).split(f.area());
    let (history_area, input_area) = (areas[0], areas[1]);

    // Status bar = title of the history block.
    let (dot, label, color) = if connected {
        ("●", "connected", Color::Green)
    } else {
        ("●", "disconnected", Color::Red)
    };
    let title = Line::from(vec![
        Span::raw(format!(" ramsit — peer {peer} ")),
        Span::styled(dot, Style::default().fg(color)),
        Span::raw(format!(" {label} ")),
    ]);

    let inner_h = history_area.height.saturating_sub(2) as usize; // minus borders
    let lines = visible_lines(messages, inner_h, scroll);
    let history = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(title))
        .wrap(Wrap { trim: false });
    f.render_widget(history, history_area);

    let prompt = format!("> {input}");
    let input_p = Paragraph::new(prompt.as_str())
        .block(Block::default().borders(Borders::ALL).title(" message "));
    f.render_widget(input_p, input_area);
    place_cursor(f, input_area, input.chars().count());
}

/// Pick the slice of messages to show: bottom-anchored, offset up by `scroll`.
fn visible_lines(messages: &[String], height: usize, scroll: usize) -> Vec<Line<'_>> {
    if height == 0 || messages.is_empty() {
        return Vec::new();
    }
    let end = messages.len().saturating_sub(scroll);
    let start = end.saturating_sub(height);
    messages[start..end].iter().map(|m| Line::from(m.as_str())).collect()
}

fn place_cursor(f: &mut Frame, area: Rect, input_chars: usize) {
    // "> " prefix (2) + chars, inside the left border (+1).
    let x = area.x + 1 + 2 + input_chars as u16;
    let y = area.y + 1;
    f.set_cursor_position((x.min(area.x + area.width.saturating_sub(2)), y));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn render(app: &App, w: u16, h: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        buf.content().iter().map(|c| c.symbol()).collect()
    }

    #[test]
    fn discovering_screen_shows_prompt() {
        let s = render(&App::new(), 60, 10);
        assert!(s.contains("Discovering"));
    }

    #[test]
    fn chat_screen_shows_messages_and_status() {
        use crate::app::Screen;
        let app = App {
            screen: Screen::Chat {
                peer: "203.0.113.5:54213".parse().unwrap(),
                messages: vec!["peer> yo".into(), "you> hey".into()],
                input: "typing".into(),
                scroll: 0,
                connected: true,
            },
            should_quit: false,
        };
        let s = render(&app, 60, 10);
        assert!(s.contains("peer> yo"));
        assert!(s.contains("you> hey"));
        assert!(s.contains("connected"));
        assert!(s.contains("> typing"));
    }
}
```

- [ ] **Step 2: Register the module in `src/main.rs`**

```rust
mod app;
mod chat;
mod net;
mod proto;
mod punch;
mod ui;
```

- [ ] **Step 3: Run the UI render tests**

Run: `cargo test ui`
Expected: `discovering_screen_shows_prompt` and `chat_screen_shows_messages_and_status` pass (rendered via `TestBackend`).

- [ ] **Step 4: Commit**

```bash
git add src/ui.rs src/main.rs
git commit -m "feat: ratatui rendering with TestBackend render tests"
```

---

### Task 5: Wire the event loop in `main.rs` and remove `chat.rs`

**Files:**
- Modify: `src/main.rs` (full rewrite of the runtime; keep `stun_arg`/`resolve_stun`)
- Delete: `src/chat.rs`

- [ ] **Step 1: Rewrite `src/main.rs`**

Replace the entire file with:

```rust
mod app;
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
                Ok(ev) => app.apply(ev),
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
```

- [ ] **Step 2: Delete the obsolete line-based chat module**

```bash
git rm src/chat.rs
```

- [ ] **Step 3: Build and run the full test suite**

Run: `cargo test`
Expected: all tests pass — proto (4), punch (1), net (3), app (7), ui (2). No `chat::` tests remain.

- [ ] **Step 4: Lint and format clean (this is the task that enforces it)**

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: no warnings, no diffs. If `cargo fmt --check` reports diffs, run `cargo fmt` and re-run. If clippy flags an unused import in `main.rs` (e.g. a leftover from the old flow), remove it.

- [ ] **Step 5: Manual smoke test (single instance — confirms the TUI runs)**

Run: `cargo run`
Expected: the alternate screen opens, shows "Discovering…", then the Exchange screen with `Your code: <ip:port>`. Type a bogus code and press Enter → an inline red error appears, you stay on the screen. Press Esc → the terminal restores cleanly to your shell prompt (no leftover raw mode). Check `ramsit.log` was written.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: ratatui TUI event loop; remove line-based chat"
```

---

### Task 6: Update the README for the TUI

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Update the "Use" section**

In `README.md`, replace the paragraph describing the line-based prompts with a TUI description. Change the section that currently reads:

```markdown
Each side prints a code like `Your code: 203.0.113.5:54213`. Send your code to
the other person (Signal/SMS/whatever) and paste theirs at the `Peer code:`
prompt. Both sides should paste within ~60s of each other. Once it says
`Connected!`, type away. `/quit` to leave.
```

with:

```markdown
A full-screen TUI opens. It discovers your public code and shows it as
`Your code: 203.0.113.5:54213`. Send your code to the other person
(Signal/SMS/whatever) and paste theirs into the `Peer code:` field, then press
Enter. Both sides should connect within ~60s of each other. Once connected, type
messages and press Enter to send; PageUp/PageDown scroll the history; Esc (or
Ctrl-C) quits.
```

- [ ] **Step 2: Point the debugging section at the log file**

In the "Debugging a failed connection" section, replace the line:

```markdown
`ramsit` logs to stderr. By default it prints `info` (STUN result, connect,
```

with:

```markdown
`ramsit` writes logs to `ramsit.log` (the TUI owns the terminal, so logs can't
go to the screen). By default it logs `info` (STUN result, connect,
```

and replace the example block:

```markdown
    RUST_LOG=debug cargo run
```

with:

```markdown
    RUST_LOG=debug cargo run
    # in another terminal:
    tail -f ramsit.log
```

- [ ] **Step 3: Verify the build is still clean and commit**

Run: `cargo build`
Expected: compiles.

```bash
git add README.md
git commit -m "docs: README for the TUI and ramsit.log logging"
```

---

## Notes on design decisions baked into this plan

- **`Command`/`Event` live in `net.rs`** (the thread-boundary contract); `app.rs` imports the two enums as pure data. Avoids an extra module while keeping `app.rs` free of any socket/terminal code.
- **No `Status` event** (spec mentioned one): the status indicator is derived from `Screen` + the `Chat.connected` flag, set by `Connected`/`PeerLeft`. Simpler, fewer message types.
- **`session()` is public and STUN-free** so the loopback test drives two bridges directly — the same seam shape as the old `chat::chat`.
- **Quit during punch:** `on_key` sets `should_quit` immediately and the UI exits (`ratatui::restore()`), abandoning the worker (it dies on process exit). `BYE` is sent only when `Quit` reaches the session loop. Matches the spec's accepted pragmatism.
- **`ui.rs` is testable** via `TestBackend`, so it isn't dead code in its own task and we get real render assertions.
- **Char-boundary `encode_chat`** fixes the latent UTF-8 truncation drop the architect flagged.
```
