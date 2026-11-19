use super::*;
use crate::error::ErrorKind;
use crate::util::vectors;

#[test]
fn crc32_known_answers() {
    assert_eq!(crc32(b""), 0);
    assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    assert_eq!(
        crc32(b"The quick brown fox jumps over the lazy dog"),
        0x414f_a339
    );
}

#[test]
fn reads_python_zipfile() {
    let entries = read(&vectors::fixture("zip/bundle.zip"), Limits::default()).unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "game/tvpipe.json",
            "game/h.html",
            "game/img.bin",
            "game/\u{fc}ml\u{e4}ut.txt"
        ]
    );
    assert!(entries[0].data.starts_with(b"{\"name\":\"x\""));
    assert_eq!(entries[1].data.len(), 31 + 12 * 50);
    assert_eq!(entries[2].data[..4], [0, 1, 2, 3]);
    assert_eq!(entries[3].data, "gr\u{fc}\u{df}e".as_bytes());
}

#[test]
fn data_descriptors() {
    let entries = read(&vectors::fixture("zip/descriptor.zip"), Limits::default()).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].data, b"streamed ".repeat(100));
    assert_eq!(entries[1].data, b"second");
}

#[test]
fn refusals() {
    let e = read(&vectors::fixture("zip/bzip2.zip"), Limits::default()).unwrap_err();
    assert_eq!(e.kind(), ErrorKind::Unsupported, "{e}");
    assert!(read(b"not a zip at all", Limits::default()).is_err());
    assert!(read(&[], Limits::default()).is_err());
    // A bomb: 4 MiB of zeros in 4 KiB.
    let bomb = vectors::fixture("zip/bomb.zip");
    assert!(read(&bomb, Limits::default()).is_ok());
    let tight = Limits {
        ratio: 100,
        ..Limits::default()
    };
    assert!(read(&bomb, tight)
        .unwrap_err()
        .to_string()
        .contains("ratio"));
    let small = Limits {
        total: 1 << 20,
        ..Limits::default()
    };
    assert!(read(&bomb, small).is_err());
    let few = Limits {
        entries: 3,
        ..Limits::default()
    };
    assert!(read(&vectors::fixture("zip/bundle.zip"), few).is_err());
}

#[test]
fn corruption_is_an_error_never_a_panic() {
    let good = vectors::fixture("zip/bundle.zip");
    // A flipped byte inside the stored entry's data: the CRC catches it.
    let at = good
        .windows(4)
        .position(|w| w == [0, 1, 2, 3])
        .expect("stored data");
    let mut d = good.clone();
    d[at + 100] ^= 0xff;
    assert!(read(&d, Limits::default())
        .unwrap_err()
        .to_string()
        .contains("CRC"));
    // The encryption flag.
    let mut d = good.clone();
    let cd = d
        .windows(4)
        .position(|w| w == CENTRAL_SIG.to_le_bytes())
        .unwrap();
    d[cd + 8] |= 1;
    assert_eq!(
        read(&d, Limits::default()).unwrap_err().kind(),
        ErrorKind::Unsupported
    );
    for n in 0..good.len() {
        let _ = read(&good[..n], Limits::default());
    }
    let mut rng = 0x9e37_79b9u32;
    for _ in 0..3000 {
        let mut d = good.clone();
        rng ^= rng << 13;
        rng ^= rng >> 17;
        rng ^= rng << 5;
        let i = rng as usize % d.len();
        d[i] = (rng >> 24) as u8;
        let _ = read(&d, Limits::default());
    }
}
