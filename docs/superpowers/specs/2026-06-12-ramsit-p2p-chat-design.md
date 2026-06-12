# ramsit — P2P terminal chat via UDP hole punching

**Date:** 2026-06-12
**Status:** Approved

## Goal

A minimal Rust CLI that lets two people chat directly over UDP by punching
through their NATs, using STUN to discover public endpoints and manual
copy-paste to exchange them. No servers to run, no relay.

## Decisions

- **Signaling:** manual copy-paste. Each side prints its public `IP:port`
  ("code"); peers swap codes out-of-band (Signal/SMS) and paste them in.
- **Transport:** UDP (best NAT-traversal success; what STUN is built for).
- **Dependencies:** a couple small crates — `stunclient`, `anyhow`. No async
  runtime; plain `std::net::UdpSocket` + threads.

## The critical invariant

The **same** `UdpSocket` is used for STUN discovery *and* the chat. NATs map
`(local addr) → (public addr)` per source socket; discovering the public port
on one socket and chatting on another would yield a different mapping and the
peer's packets would be dropped. One socket is bound up front and threaded
through STUN, punch, and chat.

## Flow

1. Bind UDP socket to `0.0.0.0:0`. Restricted to IPv4.
2. STUN query (reusing the socket) → learn public `IP:port`; print
   `Your code: 203.0.113.5:54213`. Retry the query up to 3× (UDP requests can
   be lost) before reporting failure.
3. Read peer's code from stdin.
4. Hole punch: both sides repeatedly send small `PUNCH` packets at each
   other's public endpoint until a packet gets through. Early packets are
   dropped before both holes open — expected. Any `Chat` packet seen during
   punch is forwarded to the chat printer, never dropped.
5. Chat: set a read timeout on the socket; `try_clone()` and spawn a receiver
   thread that loops on `recv_from`, prints `Chat` packets (including any that
   arrived during punch), and ignores control packets. A shared `AtomicBool`
   signals shutdown so the receiver unblocks on its next timeout. While idle,
   the sender emits a `KEEPALIVE` every ~15s to refresh the NAT mapping.
   `/quit` sends a `BYE` packet then exits; receiving `BYE` prints
   "peer disconnected" and exits.

## Modules

- `main.rs` — CLI args (optional `--stun <addr>`, default
  `stun.l.google.com:19302`), orchestrates bind → discover → punch → chat.
- `punch.rs` — handshake state machine. Sends `PUNCH`, replies `PUNCH-ACK` on
  receipt, declares connected when an ACK returns; ~60s timeout. Pure helper
  `classify(&[u8]) -> PacketKind` for packet typing.
- `chat.rs` — bidirectional loop: receiver thread + stdin sender + idle
  keepalive; ignores control packets, prints `Chat` packets.

## Protocol

Control packets begin with a reserved sentinel byte (`0x00`) followed by a tag
(`PUNCH`, `PUNCH-ACK`, `KEEPALIVE`, `BYE`). Chat messages are UTF-8 lines and
never start with `0x00`, so user input can never be misclassified as control.
`classify` maps bytes → `Punch | PunchAck | Keepalive | Bye | Chat`. Datagrams
use a 1500-byte recv buffer; chat input lines are capped at 1024 bytes.

Punch handshake (each side, loop every ~500ms, socket read timeout ~500ms):
- Send `PUNCH` to peer.
- On `PUNCH` received → send `PUNCH-ACK`.
- On `PUNCH-ACK` received → connected; send a few more `PUNCH-ACK`s and exit
  the loop.
- After ~60s with no contact → fail (manual copy-paste coordination routinely
  exceeds 20s).

## Error handling

Actionable messages:
- STUN unreachable → check network / try another STUN server.
- Punch timeout → "couldn't punch through — one of you is likely behind a
  symmetric NAT (corporate/cellular); try a different network."
- Malformed peer code → show expected `IP:port` format.

## Known limitation

**Symmetric NATs** assign a different external port per destination, so the
port STUN observes won't match what the peer sees. Works on typical home
routers (cone NAT); may fail on corporate/some cellular networks. Documented
in README; unfixable without a relay (out of scope).

**Same LAN:** if both peers are on the same network, router hairpinning often
fails — the STUN-discovered public endpoints won't loop back. Use different
networks. (No LAN fallback; stays minimal.)

**IPv4 only:** the tool restricts to IPv4 endpoints to keep code parsing and
error messages simple.

## Testing

- Unit: `classify()` for each packet kind, including that a chat line whose
  text is `PUNCH` is classified as `Chat` (sentinel-byte guarantee);
  `SocketAddr` code parse/format round-trip.
- Integration (loopback): bind two sockets, hand each the other's address, run
  the punch on two threads, assert both reach "connected" — exercises the
  handshake without a real NAT.

## Out of scope

Encryption, rendezvous server, file transfer, group chat, TCP fallback,
symmetric-NAT relay.
