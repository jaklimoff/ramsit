# ramsit

Tiny P2P desktop chat. Two people, direct UDP connection, NAT hole-punched
with STUN. No server, no relay, no account.

## Build

Voice links the system Opus library, so install it first:

- macOS: `brew install opus pkg-config`
- Debian/Ubuntu: `sudo apt install libopus-dev pkg-config`
- Fedora: `sudo dnf install opus-devel pkgconf-pkg-config`

Then install JS deps and run the desktop app in dev:

    pnpm install
    pnpm tauri dev

Build a release bundle with:

    pnpm tauri build

## Use

On **both** machines run `pnpm tauri dev` (or launch the built app). A window
opens and shows `Your code: 203.0.113.5:54213`. Click **copy**, send your code to
the other person (Signal/SMS/whatever), paste theirs into the **Peer code** field
and click **Connect**. Both sides should connect within ~60s of each other. Once
connected, type messages and press Enter to send.

## Voice

Voice goes live automatically once two peers connect (Opus, 48 kHz mono, using
your system **default** input and output devices). Use the **Mute** button and
the **Mic**/**Speaker** sliders (0–200%) in the chat screen. The status shows
`[LIVE]`/`[MUTED]`/`[no voice]`. If no audio device is available the call still
works as text chat and shows `[no voice]`.

## Same local network (LAN)

The code the app shows is your *public* endpoint. When both machines sit behind
the same router it usually won't loop back (hairpinning), so on a shared LAN you
exchange **local** addresses instead. Hole punching isn't needed here — there's
no NAT between you — so you just point each side straight at the other.

On each machine:

1. Find its LAN IP:
   - macOS: `ipconfig getifaddr en0`
   - Linux: `hostname -I | awk '{print $1}'`
2. Find the UDP port `ramsit` bound (it's logged to `ramsit.log`):

       grep "socket: bound" ramsit.log
       # socket: bound to 0.0.0.0:54321   -> your local port is 54321

Your LAN code is then `<LAN-IP>:<port>`, e.g. `192.168.1.42:54321`. Send that to
the other person and paste *their* LAN code into the **Peer code** field —
instead of the public codes the app displays. Everything after connecting works
the same.

Note: `ramsit` still queries STUN at startup before it accepts a peer code, so
both machines need to reach the internet even for a LAN-only chat. Only the
codes you exchange change.

## Debugging a failed connection

`ramsit` writes logs to `ramsit.log`. By default it logs `info` (STUN result,
connect, and — on failure — a summary of what arrived). For a full
packet-by-packet trace, set `RUST_LOG=debug` before launching:

    RUST_LOG=debug pnpm tauri dev
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
- **Same LAN needs local codes** — router hairpinning drops the looped-back
  public endpoints, so on a shared network exchange LAN addresses instead (see
  [Same local network](#same-local-network-lan)). Across different networks the
  public codes work as usual.
- **IPv4 only.**
- Messages are plaintext UDP: no encryption, no delivery guarantee, no history.
- **Voice** is plaintext too, uses the system default devices only, and has no
  packet-loss concealment — lossy links will sound choppy.

## License

[MIT](LICENSE) © Jack Klimov
