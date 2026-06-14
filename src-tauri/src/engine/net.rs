use crate::audio_engine::AudioEngineHandle;
use crate::proto::{
    classify, would_block, PacketKind, AUDIO_PREFIX, BYE, KEEPALIVE, MAX_CHAT_BYTES, RECV_BUF,
};
use crate::punch;
use anyhow::{anyhow, Result};
use log::{debug, info, warn};
use std::net::{SocketAddr, UdpSocket};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use stunclient::StunClient;

/// Messages from the UI thread to the network worker.
#[derive(Debug)]
pub enum Command {
    PeerCode(SocketAddr),
    Send(String),
    Quit,
}

/// Messages from the network worker to the UI thread. Audio state/levels/errors are
/// emitted by the AudioEngine directly (see bridge), not through this channel.
#[derive(Debug)]
pub enum Event {
    Discovered(SocketAddr),
    Connected(SocketAddr),
    Incoming(String),
    PeerLeft,
    Fatal(String),
}

const POLL: Duration = Duration::from_millis(200);
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);

/// Truncate a chat line to the wire limit on a UTF-8 char boundary, so a
/// multi-byte char straddling the limit is never split (which would make the
/// receiver's `from_utf8` fail and silently drop the message).
pub fn encode_chat(line: &str) -> Vec<u8> {
    let mut end = MAX_CHAT_BYTES.min(line.len());
    while end > 0 && !line.is_char_boundary(end) {
        end -= 1;
    }
    line.as_bytes()[..end].to_vec()
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
pub fn spawn(
    stun: SocketAddr,
    audio: AudioEngineHandle,
) -> (JoinHandle<()>, Sender<Command>, Receiver<Event>) {
    let (cmd_tx, cmd_rx) = channel::<Command>();
    let (evt_tx, evt_rx) = channel::<Event>();
    let handle = thread::spawn(move || worker(stun, audio, cmd_rx, evt_tx));
    (handle, cmd_tx, evt_rx)
}

fn worker(stun: SocketAddr, audio: AudioEngineHandle, cmds: Receiver<Command>, events: Sender<Event>) {
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

    // Start voice via the engine. The net worker owns the socket; the engine gets a
    // clone for sending and emits audio state/levels itself.
    match sock.try_clone() {
        Ok(clone) => audio.start_call(clone, peer),
        Err(e) => warn!("audio: socket clone failed, voice disabled: {e}"),
    }

    session(sock, peer, cmds, events, Some(audio));
}

/// The post-connect bridge loop: forward outgoing `Send`s onto the socket,
/// surface incoming chat/BYE as events, route voice frames to the audio engine,
/// apply audio control commands, and refresh the NAT mapping on a timer.
/// Factored out so a loopback test can drive it without STUN/punch (pass `None`).
pub fn session(
    sock: UdpSocket,
    peer: SocketAddr,
    cmds: Receiver<Command>,
    events: Sender<Event>,
    audio: Option<AudioEngineHandle>,
) {
    let _ = sock.set_read_timeout(Some(POLL));
    let mut buf = [0u8; RECV_BUF];
    let mut last_keepalive = std::time::Instant::now();

    loop {
        // Drain outgoing commands.
        loop {
            match cmds.try_recv() {
                Ok(Command::Send(line)) => {
                    let b = encode_chat(&line);
                    debug!("session: -> {peer} chat ({} bytes)", b.len());
                    let _ = sock.send_to(&b, peer);
                }
                Ok(Command::Quit) => {
                    debug!("session: -> {peer} BYE (local quit)");
                    let _ = sock.send_to(BYE, peer);
                    if let Some(a) = &audio {
                        a.end_call();
                    }
                    return;
                }
                Ok(Command::PeerCode(_)) => {} // already connected; ignore
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    let _ = sock.send_to(BYE, peer);
                    if let Some(a) = &audio {
                        a.end_call();
                    }
                    return;
                }
            }
        }

        // One inbound read (peer IP filter, as in the old chat loop).
        match sock.recv_from(&mut buf) {
            Ok((n, from)) if from.ip() == peer.ip() => {
                let kind = classify(&buf[..n]);
                if kind != PacketKind::Audio {
                    debug!("session: <- {from} {kind:?} ({n} bytes)");
                }
                match kind {
                    PacketKind::Chat => {
                        if let Ok(s) = std::str::from_utf8(&buf[..n]) {
                            if events.send(Event::Incoming(s.to_string())).is_err() {
                                return; // UI gone
                            }
                        }
                    }
                    PacketKind::Audio => {
                        if let Some(a) = &audio {
                            a.play(&buf[AUDIO_PREFIX.len()..n]);
                        }
                    }
                    PacketKind::Bye => {
                        let _ = events.send(Event::PeerLeft);
                        // Peer is gone: stop capturing/sending voice (drops the call
                        // sink). Streams stay open so the Chat-screen meters keep
                        // running; we keep looping so the user can read history.
                        if let Some(a) = &audio {
                            a.end_call();
                        }
                    }
                    _ => {}
                }
            }
            Ok((_, from)) => debug!("session: ignoring packet from unrelated {from}"),
            Err(e) if would_block(&e) => {}
            Err(_) => return,
        }

        if last_keepalive.elapsed() >= KEEPALIVE_INTERVAL {
            let _ = sock.send_to(KEEPALIVE, peer);
            last_keepalive = std::time::Instant::now();
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
        let (b_cmd_tx, b_cmd_rx) = channel();
        let (b_evt_tx, b_evt_rx) = channel();

        let ha = thread::spawn(move || session(a, b_addr, a_cmd_rx, a_evt_tx, None));
        let hb = thread::spawn(move || session(b, a_addr, b_cmd_rx, b_evt_tx, None));

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

    #[test]
    fn session_does_not_surface_audio_as_chat() {
        use crate::proto::AUDIO_PREFIX;
        let b = UdpSocket::bind("127.0.0.1:0").unwrap();
        let b_addr = b.local_addr().unwrap();
        // The session filters inbound packets by IP only, so the spoof socket's
        // random port still passes; port 9 (discard) is just a sentinel peer.
        let peer = "127.0.0.1:9".parse().unwrap();

        let (_b_cmd_tx, b_cmd_rx) = channel();
        let (b_evt_tx, b_evt_rx) = channel();
        let hb = thread::spawn(move || session(b, peer, b_cmd_rx, b_evt_tx, None));

        let spoof = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut pkt = AUDIO_PREFIX.to_vec();
        pkt.extend_from_slice(&[0x10, 0x20, 0x30]);
        spoof.send_to(&pkt, b_addr).unwrap();

        match b_evt_rx.recv_timeout(Duration::from_millis(400)) {
            Err(_) => {} // good: dropped
            Ok(Event::Incoming(s)) => panic!("audio leaked to chat: {s:?}"),
            Ok(other) => panic!("unexpected event: {other:?}"),
        }
        drop(hb); // detached; process ends it
    }
}
