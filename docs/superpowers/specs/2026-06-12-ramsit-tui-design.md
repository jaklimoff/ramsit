# ramsit — ratatui TUI chat experience

**Date:** 2026-06-12
**Status:** Approved

## Goal

Replace the line-based chat UI with a full-screen ratatui terminal UI covering
the whole session: STUN discovery, code exchange, punching, and a scrollable
chat with a status bar and input box. The existing STUN/punch/protocol code is
preserved and moved behind a network worker thread.

## Decisions

- **Layout:** status bar (peer + connection state) on top, scrollable message
  history in the middle, input box pinned at the bottom.
- **Logging:** to a file `ramsit.log` (still `RUST_LOG`-controlled), since the
  TUI owns the terminal and stderr would corrupt the display.
- **Scope of TUI:** the entire flow runs inside the TUI from launch (discovery,
  code exchange, punching, chat, errors).

## Architecture

Two threads communicating over `mpsc` channels:

- **UI thread (main):** owns the terminal via `ratatui`, draws frames, reads
  keyboard events. Never touches the socket.
- **Network worker thread:** owns the `UdpSocket`. Runs STUN discovery, the
  punch handshake, and the chat send/recv + keepalive loop. Reports progress as
  events instead of printing.

Channels:

- **UI → worker** `Command`: `PeerCode(SocketAddr)`, `Send(String)`, `Quit`.
- **worker → UI** `Event`: `Discovered(SocketAddr)`, `Status(Status)`,
  `Incoming(String)`, `PeerLeft`, `Fatal(String)` where
  `Status = Connecting | Punching | Connected { peer } | Disconnected`.

The proven STUN/punch code is unchanged; it runs on the worker and emits events.

## State machine

`App.screen` advances on worker events; keystrokes edit the current screen:

```
Discovering
  → Exchange { my_code, input, error: Option<String> }
  → Punching { peer }
  → Chat { peer, messages: Vec<String>, input, scroll, status }
  → Fatal { msg }
```

Worker sequence: bind socket → `discover()` → emit `Discovered(my_code)` → wait
for `PeerCode` command → `punch()` → emit `Connected{peer}` (or `Fatal`) →
forward any early chat lines as `Incoming` → run the session bridge loop.

**Quit semantics.** `discover()` and `punch()` are blocking (punch up to 60s),
so the worker cannot read `Quit` while inside them. We do **not** plumb a cancel
flag. Instead, `Quit` sets `app.should_quit` and the UI exits immediately
(`ratatui::restore()` runs), abandoning the worker thread — the OS reaps it on
process exit. `BYE` is best-effort: it is sent only when `Quit` arrives during
the session loop (where the worker is actively draining commands). This matches
the existing `chat.rs` `process::exit(0)` pragmatism.

## Modules

- `proto.rs` — unchanged (protocol constants, `classify`, `parse_code`).
- `punch.rs` — unchanged (`punch()` handshake, called by the worker).
- `net.rs` — the network worker. `spawn(stun_addr) -> (JoinHandle,
  Sender<Command>, Receiver<Event>)` runs `discover → punch → session`. Replaces
  `chat.rs`; the send/recv/keepalive logic and `encode_chat` move here, bridged
  to channels. The post-connect loop is factored as a separate
  `fn session(sock, peer, early, cmds: Receiver<Command>, events: Sender<Event>)`
  so the loopback test can drive two sessions on localhost without STUN/punch.
  The session loop sets a short socket read timeout and on each tick: drains
  pending `Command`s, does one `recv_from` (keeping the `from.ip() == peer.ip()`
  filter from today's `chat.rs`), and fires a keepalive on its ~15s timer.
- `app.rs` — `App` state struct, `apply(Event)`, and
  `on_key(KeyEvent) -> Option<Command>`. Pure logic, no terminal; unit-tested.
- `ui.rs` — `draw(frame, &app)` rendering with ratatui widgets; pure
  presentation, matches on `App.screen`.
- `main.rs` — parse `--stun`, init the file logger, `ratatui::init()`, spawn the
  worker, run the event loop, `ratatui::restore()` on exit.

## Screens

1. **Discovering** — centered "Discovering your public address via STUN…".
2. **Exchange** — "Your code: `X` (share it)" + a `Peer code:` input. Invalid
   input shows an inline error and stays on the screen (no crash).
3. **Punching** — "Punching through to `peer`… (up to 60s)".
4. **Chat** — status bar (peer address + colored ● for connection state),
   scrollable history, input box. `scroll` is an offset in lines from the
   bottom (0 = pinned). New messages auto-scroll only when already pinned
   (`scroll == 0`), so reading history isn't interrupted; PageUp/PageDown adjust
   the offset, clamped to `[0, max]`.
5. **Fatal** — red error message (STUN/punch failure text) + "press q to quit".

## Keys

- Printable char → append to input.
- Backspace → delete last char.
- Enter → send non-empty input (echoes `you> …` locally, emits `Send`).
- PageUp / PageDown → scroll history.
- Esc or Ctrl-C → emit `Quit`; UI restores the terminal and exits immediately
  (see Quit semantics above). Ctrl-C is delivered as a `KeyEvent` because raw
  mode disables `ISIG`; there is no SIGINT.

No mid-line cursor editing in v1 (append/backspace only).

`encode_chat` truncates on a **UTF-8 char boundary**, not a raw byte cut, so a
multi-byte char straddling the 1024-byte limit isn't split. (Today's raw
`truncate(MAX_CHAT_BYTES)` can yield invalid UTF-8 that the receiver's
`from_utf8` rejects, silently dropping the whole message — a latent bug this
fixes.)

## Event loop

UI thread, each iteration:

1. `terminal.draw(|f| ui::draw(f, &app))`.
2. `event::poll(~100ms)`; if a key is ready, `app.on_key(key)` → maybe send a
   `Command`.
3. Drain `Event`s with `try_recv`: on `Ok(e)` → `app.apply(e)`; on
   `Err(Disconnected)` (worker died/finished) → `app.screen = Fatal{"connection
   lost"}`.
4. Break when `app.should_quit`.

Worker thread, post-connect, each iteration: drain `Command`s (`try_recv`) — a
`Quit`, or `Disconnected` (UI gone), ends the loop after a best-effort `BYE`;
one `recv_from` with a short timeout (emit `Incoming`/`PeerLeft`); keepalive on
timer.

## Logging

`env_logger` retargeted to a file:

```rust
let file = File::create("ramsit.log")?;
env_logger::Builder::from_env(Env::default().default_filter_or("info"))
    .target(env_logger::Target::Pipe(Box::new(file)))
    .init();
```

`RUST_LOG=debug` still controls verbosity. `tail -f ramsit.log` to watch.

## Error handling

- STUN failure / punch timeout → worker emits `Fatal(msg)` → Fatal screen.
- Worker channel disconnect (panic/finish) → UI shows `Fatal{"connection lost"}`.
- Bad peer code → validated in the UI with `proto::parse_code`; inline error,
  no worker round trip.
- Terminal restoration: `ratatui::init()` installs a panic hook and
  `ratatui::restore()` runs on every normal and panic exit path. Caveat:
  SIGTERM/SIGHUP (`kill`, closing the window) bypass restore and leave the
  terminal in raw mode — out of scope for v1; `reset` recovers it.

`Status::Disconnected` is set only when the peer sends `BYE` (→ `PeerLeft`).
There is no peer-liveness timeout; silence from the peer is not detected (same
as today).

## Testing

- `app.rs` unit tests: `Discovered` → Exchange; `Incoming` appends a message;
  submitting a Chat line yields a `Send` command and a `you>` echo; scroll
  offset clamps at both bounds; `Fatal` → Fatal screen.
- `net.rs` integration test (loopback): two `session(sock, peer, …)` bridges on
  localhost (no STUN/punch), each given the other's address; a `Send` command on
  one produces an `Incoming` event on the other. Also assert char-boundary
  truncation in `encode_chat` (a multi-byte char at the limit isn't split).
- `punch.rs` / `proto.rs` tests unchanged.
- Manual smoke: real two-peer run over different networks.

## Dependencies

Add `ratatui` (re-exports `crossterm`). This grows the dependency footprint
beyond the original "minimal crates" goal — the accepted cost of the richer UX.

## Out of scope

Mid-line input editing, message timestamps, themes/config, mouse support,
multiple peers, file transfer, encryption.
