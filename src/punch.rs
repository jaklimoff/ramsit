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
