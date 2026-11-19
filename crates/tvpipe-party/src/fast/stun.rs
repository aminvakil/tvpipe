//! Just enough STUN (RFC 8489) to route a datagram to its peer: whether
//! it is STUN, and the USERNAME of a binding request (`HOST:PHONE`
//! ufrags, so the host's ufrag names the peer). Never panics, on any
//! input.
//!
//! ```text
//!  0                   1                   2                   3
//! |0 0|  message type (14 bits)   |       message length          |
//! |                     magic cookie 0x2112A442                   |
//! |                transaction id (96 bits)                       |
//! | attribute type                | attribute length              |
//! | value, padded to 4 bytes ...                                  |
//! ```

const MAGIC_COOKIE: [u8; 4] = [0x21, 0x12, 0xa4, 0x42];
const HEADER: usize = 20;
const BINDING_REQUEST: u16 = 0x0001;
const USERNAME: u16 = 0x0006;

/// Whether `data` looks like a STUN message (RFC 7983 demultiplexing:
/// first byte 0..=3, and the magic cookie).
pub(crate) fn is_stun(data: &[u8]) -> bool {
    data.len() >= HEADER && data[0] < 4 && data[4..8] == MAGIC_COOKIE
}

/// The host's ufrag of a binding request (the USERNAME before `:`).
pub(crate) fn request_ufrag(data: &[u8]) -> Option<&str> {
    if !is_stun(data) || u16::from_be_bytes([data[0], data[1]]) != BINDING_REQUEST {
        return None;
    }
    let len = usize::from(u16::from_be_bytes([data[2], data[3]]));
    let body = data.get(HEADER..HEADER.checked_add(len)?)?;
    let mut at = 0usize;
    while let Some(head) = body.get(at..at.checked_add(4)?) {
        let kind = u16::from_be_bytes([head[0], head[1]]);
        let n = usize::from(u16::from_be_bytes([head[2], head[3]]));
        let value = body.get(at + 4..at + 4 + n)?;
        if kind == USERNAME {
            let user = std::str::from_utf8(value).ok()?;
            return user.split_once(':').map(|(host, _)| host);
        }
        at += 4 + n.div_ceil(4) * 4;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A binding request as Chrome sends it (USERNAME, then others).
    fn request(user: &str) -> Vec<u8> {
        let mut attrs = Vec::new();
        let mut attr = |kind: u16, value: &[u8]| {
            attrs.extend_from_slice(&kind.to_be_bytes());
            attrs.extend_from_slice(&(value.len() as u16).to_be_bytes());
            attrs.extend_from_slice(value);
            while attrs.len() % 4 != 0 {
                attrs.push(0);
            }
        };
        attr(0xc057, &[0, 1, 0, 10]); // GOOG-NETWORK-INFO
        attr(USERNAME, user.as_bytes());
        attr(0x0024, &[0x6e, 0x7f, 0x1e, 0xff]); // PRIORITY
        let mut m = vec![0x00, 0x01];
        m.extend_from_slice(&(attrs.len() as u16).to_be_bytes());
        m.extend_from_slice(&MAGIC_COOKIE);
        m.extend_from_slice(&[7; 12]);
        m.extend_from_slice(&attrs);
        m
    }

    #[test]
    fn usernames() {
        assert_eq!(request_ufrag(&request("host:phone")), Some("host"));
        assert_eq!(request_ufrag(&request("abc:")), Some("abc"));
        assert_eq!(request_ufrag(&request("nocolon")), None);
        // Odd lengths are padded.
        assert_eq!(request_ufrag(&request("hostX:p")), Some("hostX"));
        let mut response = request("host:phone");
        response[1] = 0x01; // 0x0101: a success response
        response[0] = 0x01;
        assert!(is_stun(&response));
        assert_eq!(request_ufrag(&response), None);
    }

    #[test]
    fn not_stun() {
        assert!(!is_stun(&[]));
        assert!(!is_stun(&[
            22, 254, 253, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
        ])); // DTLS
        let mut m = request("a:b");
        m[4] = 0;
        assert!(!is_stun(&m));
    }

    #[test]
    fn truncated_and_lying_lengths() {
        let m = request("host:phone");
        for n in 0..m.len() {
            let _ = request_ufrag(&m[..n]);
        }
        let mut long = m.clone();
        long[2] = 0xff;
        assert_eq!(request_ufrag(&long), None);
        let mut attr = m.clone();
        attr[22] = 0xff; // the first attribute's length
        assert_eq!(request_ufrag(&attr), None);
    }

    #[test]
    fn fuzz_smoke() {
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let base = request("host:phone");
        for _ in 0..20_000 {
            let mut m = base.clone();
            for _ in 0..(next() % 6) {
                let i = (next() as usize) % m.len();
                m[i] = next() as u8;
            }
            m.truncate((next() as usize) % (m.len() + 1));
            let _ = request_ufrag(&m);
        }
    }
}
