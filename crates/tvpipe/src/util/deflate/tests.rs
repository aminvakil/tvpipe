use super::*;
use crate::util::hex;
use crate::util::inflate::inflate;

/// A fixed pseudo-random byte stream (xorshift).
fn noise(n: usize, seed: u64) -> Vec<u8> {
    let mut x = seed | 1;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

/// Text that compresses like a web page: words from a small vocabulary.
fn text(n: usize) -> Vec<u8> {
    let words = [
        "<div class=\"player\">",
        "</div>",
        "function",
        "const ",
        "return ",
        "party.screen.set({ phase: ",
        "\n",
        "  ",
        "import { PartyClient } from \"/_tvpipe/lib/client.js\";",
        "the ",
        "drawing ",
        "{",
        "}",
    ];
    let r = noise(n, 7);
    let mut out = Vec::new();
    let mut i = 0;
    while out.len() < n {
        out.extend_from_slice(words[usize::from(r[i % r.len()]) % words.len()].as_bytes());
        i += 1;
    }
    out.truncate(n);
    out
}

fn round_trip(data: &[u8]) -> Vec<u8> {
    let z = deflate(data);
    let back = inflate(&z, data.len()).unwrap_or_else(|e| panic!("{} bytes: {e}", data.len()));
    assert_eq!(back, data);
    z
}

#[test]
fn round_trips() {
    for n in [0, 1, 2, 3, 4, 5, 100, 1000] {
        round_trip(&noise(n, 1));
        round_trip(&text(n));
        round_trip(&vec![b'a'; n]);
    }
    // Every byte value, runs longer than 258, matches 32 KiB back.
    let all: Vec<u8> = (0..=255u8).collect();
    round_trip(&all);
    round_trip(&vec![0u8; 100_000]);
    let mut far = noise(40_000, 3);
    far.extend_from_within(..2000);
    round_trip(&far);
    // Several blocks of each kind.
    let mut mixed = text(200_000);
    mixed.extend(noise(150_000, 9));
    mixed.extend(text(50_000));
    round_trip(&mixed);
}

#[test]
fn compresses() {
    let t = text(100_000);
    let z = round_trip(&t);
    assert!(z.len() < t.len() / 4, "{} of {}", z.len(), t.len());
    // Incompressible data costs a few bytes per 64 KiB (stored blocks).
    let r = noise(200_000, 5);
    let z = round_trip(&r);
    assert!(z.len() <= r.len() + r.len() / 1000 + 16, "{}", z.len());
    let z = round_trip(&vec![7u8; 1 << 20]);
    assert!(z.len() < 3000, "{}", z.len());
}

#[test]
fn deterministic_and_known_answers() {
    let t = text(300_000);
    assert_eq!(deflate(&t), deflate(&t));
    // Checked with Python: zlib.decompress(bytes.fromhex(...), -15).
    assert_eq!(hex::encode(&deflate(b"")), "0300");
    assert_eq!(hex::encode(&deflate(b"a")), "4b0400");
    assert_eq!(
        hex::encode(&deflate(b"hello hello hello hello")),
        "cb48cdc9c957c02001"
    );
}

#[test]
fn huffman_lengths_are_limited_and_complete() {
    // Fibonacci frequencies make the deepest unlimited tree.
    let mut f = vec![0u32; 40];
    let (mut a, mut b) = (1u32, 1u32);
    for x in f.iter_mut() {
        *x = a;
        (a, b) = (b, a.saturating_add(b));
    }
    for limit in [7u8, 15] {
        let l = huffman_lengths(&f, limit);
        assert!(l.iter().all(|&x| x >= 1 && x <= limit), "{l:?}");
        let kraft: u64 = l.iter().map(|&x| 1u64 << (limit - x)).sum();
        assert_eq!(kraft, 1 << limit, "{l:?}");
    }
    // One symbol, or none: two codes of one bit.
    assert_eq!(huffman_lengths(&[0, 5, 0], 15), vec![1, 1, 0]);
    assert_eq!(huffman_lengths(&[0, 0, 0], 15), vec![1, 1, 0]);
}
