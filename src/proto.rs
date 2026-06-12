use anyhow::{Context, Result};
use std::net::SocketAddr;

/// Reserved leading byte marking a control packet. UTF-8 chat text never
/// contains a null byte, so user input can never be misclassified.
#[allow(dead_code)]
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
    let addr: SocketAddr = s.parse().with_context(|| {
        format!("invalid peer code '{s}' — expected form like 203.0.113.5:54213")
    })?;
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
