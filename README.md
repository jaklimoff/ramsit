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

## License

[MIT](LICENSE) © Jack Klimov
