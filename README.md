# ramsit

Tiny P2P terminal chat. Two people, direct UDP connection, NAT hole-punched
with STUN. No server, no relay, no account.

## Build

    cargo build --release

## Use

On **both** machines run:

    cargo run

A full-screen TUI opens. It discovers your public code and shows it as
`Your code: 203.0.113.5:54213`. Send your code to the other person
(Signal/SMS/whatever) and paste theirs into the `Peer code:` field, then press
Enter. Both sides should connect within ~60s of each other. Once connected, type
messages and press Enter to send; PageUp/PageDown scroll the history; Esc (or
Ctrl-C) quits.

Pick a different STUN server with `--stun host:port` (default
`stun.l.google.com:19302`).

## Debugging a failed connection

`ramsit` writes logs to `ramsit.log` (the TUI owns the terminal, so logs can't
go to the screen). By default it logs `info` (STUN result, connect,
and — on failure — a summary of what arrived). For a full packet-by-packet
trace, set `RUST_LOG=debug`:

    RUST_LOG=debug cargo run
    # in another terminal:
    tail -f ramsit.log

Have **both** sides run with `RUST_LOG=debug` and start within ~60s of each
other. What the logs tell you when a punch fails:

- **`received 0 packet(s)`** — nothing came back. The other side isn't running,
  the code is stale (restarting prints a *new* code), you didn't start together,
  or a local firewall is dropping UDP.
- **`peer's real source <ip:port> differs from advertised`** — the peer's
  packets arrive from a different port than their code. If it's stable, `ramsit`
  retargets automatically; if the port keeps changing, that side is behind a
  symmetric NAT and needs a relay.
- **packets flowing both ways but no `Connected!`** — usually a timing issue;
  re-exchange fresh codes and start simultaneously.

## Limitations

- **Symmetric NATs** (many corporate / some cellular networks) assign a
  different port per destination, so hole punching can't work without a relay.
  If it times out, try a different network.
- **Same LAN won't work** — router hairpinning usually drops the looped-back
  public endpoints. Use different networks.
- **IPv4 only.**
- Messages are plaintext UDP: no encryption, no delivery guarantee, no history.

## License

[MIT](LICENSE) © Jack Klimov
