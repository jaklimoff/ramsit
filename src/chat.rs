use crate::proto::{classify, would_block, PacketKind, BYE, KEEPALIVE, MAX_CHAT_BYTES, RECV_BUF};
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
