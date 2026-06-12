# ramsit P2P Chat Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A minimal Rust CLI (`ramsit`) that lets two people chat directly over UDP by STUN-discovering their public endpoints, swapping them by copy-paste, and punching through their NATs.

**Architecture:** One `UdpSocket` bound at startup is threaded through STUN discovery, hole punching, and chat (the NAT mapping must be preserved). Plain `std::net` + threads, no async. Control packets are sentinel-prefixed (`0x00` + tag) so user text can never collide. Chat runs a receiver thread + idle keepalive thread + a stdin sender on main.

**Tech Stack:** Rust (edition 2021), `stunclient` for STUN, `anyhow` for errors. Tests are `#[cfg(test)]` modules inside source files (binary crate, no lib split needed).

**Module map:**
- `src/proto.rs` — protocol constants, `PacketKind`, `classify()`, `would_block()`, `parse_code()`. Pure, fully unit-tested.
- `src/punch.rs` — `punch(&UdpSocket, SocketAddr) -> Result<Vec<String>>` handshake; returns any chat lines seen during punch so they aren't lost. Loopback unit test.
- `src/chat.rs` — `chat(UdpSocket, SocketAddr, Vec<String>) -> Result<()>` bidirectional loop + keepalive + `encode_chat()` helper (unit-tested).
- `src/main.rs` — arg parsing (`--stun`), STUN discovery with retry, prompts, orchestration.

---

### Task 1: Scaffold the cargo project

**Files:**
- Create: `Cargo.toml`
- Create: `src/main.rs`

- [ ] **Step 1: Create `Cargo.toml`**

```toml
[package]
name = "ramsit"
version = "0.1.0"
edition = "2021"

[dependencies]
anyhow = "1"
stunclient = "0.4"
```

- [ ] **Step 2: Create a placeholder `src/main.rs`**

```rust
fn main() {
    println!("ramsit");
}
```

- [ ] **Step 3: Build to verify the toolchain and deps resolve**

Run: `cargo build`
Expected: compiles successfully, downloads `anyhow` and `stunclient`. `target/` is gitignored.

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock src/main.rs
git commit -m "chore: scaffold ramsit cargo project"
```

---

### Task 2: Protocol module (`proto.rs`)

**Files:**
- Create: `src/proto.rs`
- Modify: `src/main.rs` (add `mod proto;`)

- [ ] **Step 1: Write `src/proto.rs` with the failing tests first**

Write the full module but with `classify`, `parse_code` bodies present so it compiles; we write tests alongside. (For a pure module, write impl + tests together, then run tests.)

```rust
use anyhow::{Context, Result};
use std::net::SocketAddr;

/// Reserved leading byte marking a control packet. UTF-8 chat text never
/// contains a null byte, so user input can never be misclassified.
pub const SENTINEL: u8 = 0x00;

pub const PUNCH: &[u8] = b"\x00PUNCH";
pub const PUNCH_ACK: &[u8] = b"\x00PUNCH-ACK";
pub const KEEPALIVE: &[u8] = b"\x00KEEPALIVE";
pub const BYE: &[u8] = b"\x00BYE";

/// Largest datagram we read; also bounds chat line length upstream.
pub const RECV_BUF: usize = 1500;
/// Max bytes of a chat line put on the wire.
pub const MAX_CHAT_BYTES: usize = 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum PacketKind {
    Punch,
    PunchAck,
    Keepalive,
    Bye,
    Chat,
}

/// Map raw bytes to a packet kind. Only exact control byte-strings are
/// control; everything else is a chat message.
pub fn classify(buf: &[u8]) -> PacketKind {
    match buf {
        PUNCH => PacketKind::Punch,
        PUNCH_ACK => PacketKind::PunchAck,
        KEEPALIVE => PacketKind::Keepalive,
        BYE => PacketKind::Bye,
        _ => PacketKind::Chat,
    }
}

/// True for the timeout error returned by a socket read deadline. Unix maps
/// it to `WouldBlock`, Windows to `TimedOut`.
pub fn would_block(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

/// Parse a peer "code" (`IPv4:port`) with a friendly error.
pub fn parse_code(s: &str) -> Result<SocketAddr> {
    let s = s.trim();
    let addr: SocketAddr = s
        .parse()
        .with_context(|| format!("invalid peer code '{s}' — expected form like 203.0.113.5:54213"))?;
    if !addr.is_ipv4() {
        anyhow::bail!("peer code '{s}' must be IPv4");
    }
    Ok(addr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_control_packets() {
        assert_eq!(classify(PUNCH), PacketKind::Punch);
        assert_eq!(classify(PUNCH_ACK), PacketKind::PunchAck);
        assert_eq!(classify(KEEPALIVE), PacketKind::Keepalive);
        assert_eq!(classify(BYE), PacketKind::Bye);
    }

    #[test]
    fn typed_text_is_always_chat() {
        // A user literally typing "PUNCH" has no sentinel byte → Chat.
        assert_eq!(classify(b"PUNCH"), PacketKind::Chat);
        assert_eq!(classify(b"hello bro"), PacketKind::Chat);
        assert_eq!(classify(b""), PacketKind::Chat);
    }

    #[test]
    fn parses_valid_code_round_trip() {
        let a = parse_code("203.0.113.5:54213").unwrap();
        assert_eq!(a.to_string(), "203.0.113.5:54213");
    }

    #[test]
    fn rejects_garbage_and_ipv6() {
        assert!(parse_code("not-an-addr").is_err());
        assert!(parse_code("[::1]:80").is_err());
    }
}
```

- [ ] **Step 2: Register the module in `src/main.rs`**

Replace `src/main.rs` with:

```rust
mod proto;

fn main() {
    println!("ramsit");
}
```

- [ ] **Step 3: Run the tests, expect them to pass**

Run: `cargo test proto`
Expected: 4 tests pass. (`mod proto` is reachable from the binary crate; an unused-warning on `main` is fine.)

- [ ] **Step 4: Commit**

```bash
git add src/proto.rs src/main.rs
git commit -m "feat: protocol constants, classify, parse_code with tests"
```

---

### Task 3: Hole-punch module (`punch.rs`)

**Files:**
- Create: `src/punch.rs`
- Modify: `src/main.rs` (add `mod punch;`)

- [ ] **Step 1: Write `src/punch.rs` with implementation and a loopback test**

```rust
use crate::proto::{classify, would_block, PacketKind, PUNCH, PUNCH_ACK, RECV_BUF};
use anyhow::{bail, Result};
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

const STEP: Duration = Duration::from_millis(500);
const TIMEOUT: Duration = Duration::from_secs(60);

/// Punch a UDP hole to `peer` using `sock`. Returns once we have confirmation
/// the peer received our packets (we got a PUNCH-ACK). Any chat lines that
/// arrive during punching are returned so the caller can print them — they are
/// never dropped.
pub fn punch(sock: &UdpSocket, peer: SocketAddr) -> Result<Vec<String>> {
    sock.set_read_timeout(Some(STEP))?;
    let deadline = Instant::now() + TIMEOUT;
    let mut buf = [0u8; RECV_BUF];
    let mut early: Vec<String> = Vec::new();

    loop {
        if Instant::now() >= deadline {
            bail!(
                "couldn't punch through after 60s — one of you is likely behind a \
                 symmetric NAT (corporate/cellular); try a different network"
            );
        }

        // Keep knocking every iteration until we're confirmed connected.
        sock.send_to(PUNCH, peer)?;

        match sock.recv_from(&mut buf) {
            Ok((n, from)) if from == peer => match classify(&buf[..n]) {
                PacketKind::Punch => {
                    sock.send_to(PUNCH_ACK, peer)?;
                }
                PacketKind::PunchAck => {
                    // Peer received our PUNCH. Confirm a few more times, done.
                    for _ in 0..5 {
                        sock.send_to(PUNCH_ACK, peer)?;
                    }
                    return Ok(early);
                }
                PacketKind::Chat => {
                    if let Ok(s) = std::str::from_utf8(&buf[..n]) {
                        early.push(s.to_string());
                    }
                }
                PacketKind::Keepalive | PacketKind::Bye => {}
            },
            Ok(_) => {} // packet from someone other than the peer; ignore
            Err(e) if would_block(&e) => {} // read timeout, loop and re-send
            Err(e) => return Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_peers_punch_on_loopback() {
        let a = UdpSocket::bind("127.0.0.1:0").unwrap();
        let b = UdpSocket::bind("127.0.0.1:0").unwrap();
        let a_addr = a.local_addr().unwrap();
        let b_addr = b.local_addr().unwrap();

        let h1 = std::thread::spawn(move || punch(&a, b_addr));
        let h2 = std::thread::spawn(move || punch(&b, a_addr));

        assert!(h1.join().unwrap().is_ok());
        assert!(h2.join().unwrap().is_ok());
    }
}
```

- [ ] **Step 2: Register the module in `src/main.rs`**

```rust
mod proto;
mod punch;

fn main() {
    println!("ramsit");
}
```

- [ ] **Step 3: Run the loopback test, expect it to pass**

Run: `cargo test punch`
Expected: `two_peers_punch_on_loopback` passes within ~1s (both threads converge quickly on loopback).

- [ ] **Step 4: Commit**

```bash
git add src/punch.rs src/main.rs
git commit -m "feat: UDP hole-punch handshake with loopback test"
```

---

### Task 4: Chat module (`chat.rs`)

**Files:**
- Create: `src/chat.rs`
- Modify: `src/main.rs` (add `mod chat;`)

- [ ] **Step 1: Write `src/chat.rs` with implementation and an `encode_chat` test**

```rust
use crate::proto::{
    classify, would_block, PacketKind, BYE, KEEPALIVE, MAX_CHAT_BYTES, RECV_BUF,
};
use anyhow::Result;
use std::io::BufRead;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

const POLL: Duration = Duration::from_millis(500);
const KEEPALIVE_TICKS: u32 = 30; // 30 * 500ms = 15s

/// Turn a stdin line into wire bytes, truncated to the max chat size. Trailing
/// bytes past the limit are dropped (humans won't paste novels).
pub fn encode_chat(line: &str) -> Vec<u8> {
    let mut bytes = line.as_bytes().to_vec();
    bytes.truncate(MAX_CHAT_BYTES);
    bytes
}

/// Run the chat session until the user types `/quit` or the peer sends BYE.
pub fn chat(sock: UdpSocket, peer: SocketAddr, early: Vec<String>) -> Result<()> {
    sock.set_read_timeout(Some(POLL))?;
    let running = Arc::new(AtomicBool::new(true));

    for m in early {
        println!("peer> {m}");
    }

    // Receiver thread: print chat, handle peer BYE.
    let rsock = sock.try_clone()?;
    let rpeer = peer;
    let rrunning = running.clone();
    let receiver = thread::spawn(move || {
        let mut buf = [0u8; RECV_BUF];
        while rrunning.load(Ordering::Relaxed) {
            match rsock.recv_from(&mut buf) {
                Ok((n, from)) if from == rpeer => match classify(&buf[..n]) {
                    PacketKind::Chat => {
                        if let Ok(s) = std::str::from_utf8(&buf[..n]) {
                            println!("peer> {s}");
                        }
                    }
                    PacketKind::Bye => {
                        println!("peer disconnected");
                        std::process::exit(0);
                    }
                    _ => {} // punch/ack/keepalive: ignore
                },
                Ok(_) => {}
                Err(e) if would_block(&e) => {}
                Err(_) => break,
            }
        }
    });

    // Keepalive thread: refresh the NAT mapping every ~15s.
    let ksock = sock.try_clone()?;
    let kpeer = peer;
    let krunning = running.clone();
    let keepalive = thread::spawn(move || {
        let mut ticks = 0u32;
        while krunning.load(Ordering::Relaxed) {
            thread::sleep(POLL);
            ticks += 1;
            if ticks >= KEEPALIVE_TICKS {
                let _ = ksock.send_to(KEEPALIVE, kpeer);
                ticks = 0;
            }
        }
    });

    // Main: read stdin, send each line. `/quit` sends BYE and exits.
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let line = line?;
        if line == "/quit" {
            let _ = sock.send_to(BYE, peer);
            break;
        }
        sock.send_to(&encode_chat(&line), peer)?;
    }

    running.store(false, Ordering::Relaxed);
    let _ = receiver.join();
    let _ = keepalive.join();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_and_truncates() {
        assert_eq!(encode_chat("hi"), b"hi");
        let long = "x".repeat(MAX_CHAT_BYTES + 50);
        assert_eq!(encode_chat(&long).len(), MAX_CHAT_BYTES);
    }
}
```

- [ ] **Step 2: Register the module in `src/main.rs`**

```rust
mod chat;
mod proto;
mod punch;

fn main() {
    println!("ramsit");
}
```

- [ ] **Step 3: Run the test, expect it to pass**

Run: `cargo test chat`
Expected: `encodes_and_truncates` passes.

- [ ] **Step 4: Commit**

```bash
git add src/chat.rs src/main.rs
git commit -m "feat: chat loop with keepalive, BYE, and encode_chat test"
```

---

### Task 5: Wire it all together (`main.rs`)

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Replace `src/main.rs` with the full orchestration**

```rust
mod chat;
mod proto;
mod punch;

use anyhow::{anyhow, Context, Result};
use proto::parse_code;
use std::io::{BufRead, Write};
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use stunclient::StunClient;

const DEFAULT_STUN: &str = "stun.l.google.com:19302";

fn main() -> Result<()> {
    let stun = stun_arg().unwrap_or_else(|| DEFAULT_STUN.to_string());
    let stun_addr = resolve_stun(&stun)?;

    let sock = UdpSocket::bind("0.0.0.0:0").context("failed to bind a UDP socket")?;
    let my_addr = discover(&sock, stun_addr)?;

    println!("Your code: {my_addr}");
    println!("Send that to your bro, then paste theirs below.\n");

    print!("Peer code: ");
    std::io::stdout().flush()?;
    let peer = read_peer_code()?;

    println!("\nPunching through… (this can take up to a minute)");
    let early = punch::punch(&sock, peer)?;
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

/// Query our public endpoint via STUN, retrying up to 3× (UDP can drop the
/// request) before giving up.
fn discover(sock: &UdpSocket, stun: SocketAddr) -> Result<SocketAddr> {
    let client = StunClient::new(stun);
    let mut last = None;
    for _ in 0..3 {
        match client.query_external_address(sock) {
            Ok(addr) => return Ok(addr),
            Err(e) => last = Some(e.to_string()),
        }
    }
    Err(anyhow!(
        "STUN query failed after 3 tries ({}) — check your network or try \
         another server with --stun <host:port>",
        last.unwrap_or_default()
    ))
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
```

- [ ] **Step 2: Build and run the whole test suite**

Run: `cargo test`
Expected: all tests pass (proto: 4, punch: 1, chat: 1).

- [ ] **Step 3: Build the release binary and confirm STUN discovery works live**

Run: `cargo run` (requires internet for the STUN query)
Expected: prints `Your code: <your-public-ip>:<port>` then `Peer code: `. Type `/quit`-able after — but with no peer it'll sit in "Punching…". Press Ctrl-C to abort. Confirms STUN + arg flow work end to end. (Full two-peer test is the manual smoke test in Task 6.)

- [ ] **Step 4: Commit**

```bash
git add src/main.rs
git commit -m "feat: wire STUN discovery, prompts, punch, and chat in main"
```

---

### Task 6: README, lint, and manual smoke test

**Files:**
- Create: `README.md`

- [ ] **Step 1: Write `README.md`**

```markdown
# ramsit

Tiny P2P terminal chat. Two people, direct UDP connection, NAT hole-punched
with STUN. No server, no relay, no account.

## Build

    cargo build --release

## Use

On **both** machines run:

    cargo run

Each side prints a code like `Your code: 203.0.113.5:54213`. Send your code to
the other person (Signal/SMS/whatever) and paste theirs at the `Peer code:`
prompt. Both sides should paste within ~60s of each other. Once it says
`Connected!`, type away. `/quit` to leave.

Pick a different STUN server with `--stun host:port` (default
`stun.l.google.com:19302`).

## Limitations

- **Symmetric NATs** (many corporate / some cellular networks) assign a
  different port per destination, so hole punching can't work without a relay.
  If it times out, try a different network.
- **Same LAN won't work** — router hairpinning usually drops the looped-back
  public endpoints. Use different networks.
- **IPv4 only.**
- Messages are plaintext UDP: no encryption, no delivery guarantee, no history.
```

- [ ] **Step 2: Run clippy and formatting checks**

Run: `cargo clippy -- -D warnings && cargo fmt --check`
Expected: no warnings, no formatting diffs. (If `cargo fmt --check` reports diffs, run `cargo fmt` and re-commit.)

- [ ] **Step 3: Manual two-peer smoke test**

The automated loopback test covers the handshake, but verify a real run across two NATs (or two machines / a phone hotspot + home wifi):

1. On machine A: `cargo run` → copy "Your code".
2. On machine B: `cargo run` → copy "Your code".
3. Paste B's code into A's `Peer code:` prompt, and A's code into B's.
4. Both should print `Connected!`. Type on A → appears as `peer>` on B and vice-versa.
5. Wait >30s idle, then send a message — it should still arrive (keepalive kept the mapping open).
6. Type `/quit` on A → B prints `peer disconnected` and exits.

Record the result. If it times out, note which networks were used (the symmetric-NAT limitation is expected on some).

- [ ] **Step 4: Commit**

```bash
git add README.md
git commit -m "docs: README with usage and limitations"
```

---

## Notes on design decisions baked into this plan

- **`proto.rs` instead of `classify` living in `punch.rs`** (spec said the latter): both `punch` and `chat` classify packets and use `would_block`, so a shared pure module is the cleaner boundary. Same behavior, better tested.
- **Keepalive is unconditional every 15s** rather than "only when idle." Simpler, and one tiny packet every 15s is harmless — it always refreshes the mapping. Honors the spec's intent (prevent mapping expiry) with less state.
- **Peer BYE calls `process::exit(0)`** from the receiver thread rather than signaling main (which is blocked on stdin). This is the architect-sanctioned simple shutdown; the local `/quit` path joins threads cleanly.
- **Binary crate, tests inline** (`#[cfg(test)]`): avoids a lib/bin split. The loopback punch test runs two real sockets on two threads — genuine handshake coverage.
