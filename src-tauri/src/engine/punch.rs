use crate::proto::{classify, would_block, PacketKind, PUNCH, PUNCH_ACK, RECV_BUF};
use anyhow::{bail, Result};
use log::{debug, info, warn};
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

const STEP: Duration = Duration::from_millis(500);
const TIMEOUT: Duration = Duration::from_secs(60);

/// Punch a UDP hole to the peer. `code` is the address the peer advertised via
/// STUN. We knock on the best-known target every `STEP` and listen for the
/// peer's packets.
///
/// For a port-restricted-cone pair (stable port, filters by source IP+port)
/// the advertised port is correct and both sides simply need to be punching at
/// once. As a safety net for a peer whose real port differs from what it
/// advertised (stale code, or a symmetric NAT), we accept control packets from
/// the peer's IP on *any* port and retarget to the real source.
///
/// Returns the confirmed peer address (port possibly re-learned) plus any chat
/// lines that arrived mid-punch (never dropped).
pub fn punch(sock: &UdpSocket, code: SocketAddr) -> Result<(SocketAddr, Vec<String>)> {
    sock.set_read_timeout(Some(STEP))?;
    let deadline = Instant::now() + TIMEOUT;
    let mut buf = [0u8; RECV_BUF];
    let mut early: Vec<String> = Vec::new();

    let peer_ip: IpAddr = code.ip();
    let mut target = code; // where we send; may be re-learned
    let mut sent: u64 = 0;
    let mut recv_total: u64 = 0;
    let mut recv_peer_ip: u64 = 0;

    info!(
        "punch: starting — peer code {code}, local {}, timeout {}s",
        sock.local_addr()
            .map(|a| a.to_string())
            .unwrap_or_else(|_| "?".into()),
        TIMEOUT.as_secs()
    );

    loop {
        if Instant::now() >= deadline {
            warn!(
                "punch: gave up after {}s — sent {sent} PUNCH, received {recv_total} packet(s) \
                 total ({recv_peer_ip} from peer IP {peer_ip})",
                TIMEOUT.as_secs()
            );
            if recv_total == 0 {
                bail!(
                    "couldn't punch through after 60s — received NOTHING back. Most likely: \
                     your bro isn't running it right now, the code is stale (he restarted — \
                     each run prints a NEW code), you didn't both start within ~60s, or a \
                     firewall is dropping UDP. Re-exchange fresh codes and start together."
                );
            }
            bail!(
                "couldn't punch through after 60s — saw {recv_peer_ip} packet(s) from the peer's \
                 IP {peer_ip} but never completed the handshake. Re-run both sides with \
                 RUST_LOG=debug to see the source ports; if they keep changing, that side is \
                 behind a symmetric NAT/CGNAT and needs a relay."
            );
        }

        sock.send_to(PUNCH, target)?;
        sent += 1;
        debug!("punch: -> {target} PUNCH (#{sent})");

        match sock.recv_from(&mut buf) {
            Ok((n, from)) => {
                recv_total += 1;
                let kind = classify(&buf[..n]);
                debug!("punch: <- {from} {kind:?} ({n} bytes)");

                if from.ip() != peer_ip {
                    debug!("punch: ignoring packet from unrelated source {from}");
                    continue;
                }
                recv_peer_ip += 1;

                if from != target {
                    warn!(
                        "punch: peer's real source {from} differs from advertised {target} — \
                         stale code or symmetric NAT; retargeting to {from}"
                    );
                    target = from;
                }

                match kind {
                    PacketKind::Punch => {
                        sock.send_to(PUNCH_ACK, target)?;
                        debug!("punch: -> {target} PUNCH-ACK (acking peer PUNCH)");
                    }
                    PacketKind::PunchAck => {
                        info!("punch: got PUNCH-ACK from {target} — connected (sent {sent}, recv {recv_total})");
                        for _ in 0..5 {
                            sock.send_to(PUNCH_ACK, target)?;
                        }
                        return Ok((target, early));
                    }
                    PacketKind::Chat => {
                        if let Ok(s) = std::str::from_utf8(&buf[..n]) {
                            debug!("punch: buffering early chat from {from}");
                            early.push(s.to_string());
                        }
                    }
                    PacketKind::Keepalive | PacketKind::Bye | PacketKind::Audio => {}
                }
            }
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

        let (a_peer, _) = h1.join().unwrap().expect("a connects");
        let (b_peer, _) = h2.join().unwrap().expect("b connects");

        // Ports match exactly on loopback, so no re-learning happens.
        assert_eq!(a_peer, b_addr);
        assert_eq!(b_peer, a_addr);
    }
}
