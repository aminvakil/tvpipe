// SPDX-License-Identifier: LGPL-3.0-or-later
// Test vectors ported from doubletake internal/airplay/{fpsap_test.go,
// fairplay_vectors_test.go,fairplay_key_test.go}
// @ be27776beb05beaa149a7096e4722f5cba60eb37 (LGPL-3.0-or-later).

use super::md5::{self, Mutation};
use super::*;
use crate::util::hex;
use crate::util::vectors::{self, Block};

fn arr<const N: usize>(v: &[u8]) -> [u8; N] {
    v.try_into().expect("vector length")
}

fn h(s: &str) -> Vec<u8> {
    hex::decode(s).unwrap()
}

fn md5_le(state: &[u8; 16], block: &[u8; 64], m: Mutation) -> [u8; 16] {
    let w = md5::compress(md5::words_le(state), block, m);
    let mut out = [0u8; 16];
    for (i, x) in w.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&x.to_le_bytes());
    }
    out
}

/// Test-only inverse of `wrap_key`: what the receiver does.
fn unwrap_key(local: &[u8; 128], remote: &[u8; 128], ekey: &[u8]) -> [u8; 16] {
    let aes = Aes::new(&whitebox::wrapping_key(local, remote)).unwrap();
    let mut key: [u8; 16] = arr(&ekey[56..72]);
    aes.decrypt_block(&mut key);
    for i in 0..16 {
        key[i] ^= ekey[16 + i];
    }
    key
}

fn m4_for(m3: &[u8; 164]) -> Vec<u8> {
    let mut m4 = header(4, 20).to_vec();
    m4.extend_from_slice(&m3[144..]);
    m4
}

// ---------------------------------------------------------------------------
// doubletake's own known answers

/// fpsap_test.go fpsapReferenceLocalSAP.
fn reference_local_sap() -> [u8; 128] {
    arr(&h(
        "0001e4e3dd688293e6fa66b95ba41768e587c65f750218ff1be21543d573cefb\
         087bd36e0c6363c3c8242f4abcfa6d660b801032015405eb4ab04dda7aeff38f\
         fb36f4cfa48f0b5d92ae363f68b45925bbe6413ab6bdc4968f548d21e67d20f\
         1912b6820e53f1013cde29df7350a9b9fa7c51320aea62d2949786c87642e34ba",
    ))
}

/// fairplay_vectors_test.go fairPlayReferenceReceiverSAP.
fn reference_receiver_sap() -> [u8; 128] {
    arr(&h(
        "0001cc342a5e5b1a6773c20e21b8224df862481864ef810aae2e3703c8819c23\
         539de5f5d749bc5b7a266c496283ce7f03937ae1f616de0c15ff338ccaffb09e\
         aabbe40f5d5f558fb97f1731f8f7da60a0ec6579c33ea98312c3b67135a6694f\
         f82305d9ba5c615fa254d2b1834583cee42d4426c835a7a5f6c8421c0da3f1c7",
    ))
}

const CAPTURED_M2: &str = "46504c59030102000000008202034a114c26b77d4e2eec2c8f89fdb653b5b32d\
    3576bc176816d110a14c3f53c08dbb936183bfdfe0a4f3c12e85216003b46f738c40c54da6c436d29d1b\
    342d63c7b314309ae79a33bb1787709ef077cbfe4190117a3423e270fd1a2eac44da1a7934f59dc681d1\
    b70783f228c4d077c2d495f5285c3bf8df586fc2ebfe17fb5b65";

#[test]
fn primitive_known_answers() {
    let block: [u8; 64] = std::array::from_fn(|i| (i * 3 + 1) as u8);
    let key: [u8; 16] = std::array::from_fn(|i| i as u8);
    assert_eq!(
        hex::encode(&md5_le(&key, &block, Mutation::Kdf)),
        "f6f728cb5a4397b675664f9291b859aa"
    );
    assert_eq!(
        hex::encode(&sap::sap_hash(&block)),
        "75498a4e218773030e9cdf04f0c49367"
    );
}

#[test]
fn sap_hash_corpus() {
    let mut corpus = crate::crypto::ring::Sha256::new();
    let mut state: u64 = 0x6a09e667f3bcc909;
    for _ in 0..64 {
        let mut block = [0u8; 64];
        for b in block.iter_mut() {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            *b = state as u8;
        }
        let d = sap::sap_hash(&block);
        corpus.update(&d);
    }
    assert_eq!(
        hex::encode(&corpus.finish()),
        "36ad2a7920076af59452d9f0c91e3b7d1aebc53f9143bd6819e39119d4535c92"
    );
}

#[test]
fn message_cipher_and_wrapping_key() {
    let cases = [
        (0, "b66a3295ffa6b56e02ed1b3d67fef74b90fe148570de65e6773669126a4905d8405644cae0b2f5ed6109c099c7aea7398dac8d623fbd69b87242b374d98f89502bb5a63e29c46a8ed0e98466966191ec1e6c8675087fde21337db1c8fab4c21db824026335f6fc37e2e5b6f53357d06994bd383d6029a0aff654fb1521bcdde4", "f7dd1ccb9e745f7951a6e325d73a1f5f"),
        (1, "0f95c6ddc8987eda18577da2db074e7c04715af8b3914a73be1b3d6c111953017ee0a39dfcab3e0d57f2f9fbd59c5e18101788c2ab8e3cbb403bcb48b53f3e5bf74f949e79fa5ca679df4bfcb33a69b1442675d03f948fe5bd0c5ffb64b73a5ab58f46d6baae097b599624147c2487991163ecffc4d966240f9526346a10fdb0", "b44ad891396f097aa309bc132f5b8889"),
        (2, "40f18751b44d733e0aa0416401a7d3f40375fad3ce56900602578bca14660909820e6ef3a5e943cafef5370f72c52177d9b82278b414811201a3d99202bedcca26a4d1ad08bc2669f4bae6ca54b8a120d0425edb6082f51f5aecdb547bfdb319099c9ea2729ae6a1c4480827ce9991e273843cf1c7d74ebbebc2657659bcea9f", "d38cd8efecdb20f333273c4312d9b236"),
        (3, "70a3c30edf0e1dfa1785ce4336ed547062672a47f714a0c1f89a83d95691103dfe5cf653d4cb8299793faf33fd0d4482ef5333b41ab094a90e1baf996bcf4989783f6918397fbacddaf00a2b97556dd8099841578bc5eb1444912b47298eaf356fdd6701bb3f64e725a80eb4c6f3556195de35c93e7cc703bdd24351468e9847", "769e2fe4c5ad7fbe6fd6772d00f529f4"),
    ];
    let ct: [u8; 128] = std::array::from_fn(|i| ((i + 16) * 5 + 7) as u8);
    for (mode, pt, aes_key) in cases {
        let got = message::decrypt(mode, &ct);
        assert_eq!(hex::encode(&got), pt, "mode {mode}");
        assert_eq!(message::encrypt(mode, &got), ct, "mode {mode}");
        assert_eq!(
            hex::encode(&whitebox::wrapping_key(&got, &reference_receiver_sap())),
            aes_key,
            "mode {mode}"
        );
    }
}

#[test]
fn descriptor_and_tail_known_answers() {
    let decrypt3 = |payload: [u8; 128]| message::decrypt(3, &payload);
    let local = reference_local_sap();
    let mut one = [0u8; 128];
    one[0] = 1;
    for (payload, want) in [
        ([0u8; 128], "7e38958ffe4ed433743919fe7eb16376afa4eb9e"),
        (one, "ea46797d726c6a9be43ffa72385ff97ce1c54f1b"),
    ] {
        assert_eq!(
            hex::encode(&whitebox::descriptor(&local, &decrypt3(payload))),
            want
        );
    }

    let sparse = |i: usize| {
        let mut p = [0u8; 128];
        p[i] = 0x42;
        p
    };
    let captured: [u8; 128] = arr(&h(CAPTURED_M2)[14..142]);
    for (payload, want) in [
        ([0u8; 128], "6f627565f3e77f5b5ede91beee7baf92e4241e0b"),
        ([0xff; 128], "dc2cc74f2ed55484f59f95b96082f0f5c017dd17"),
        (captured, "4b911e48af23d8406368aeafbb61bfcd569e3e55"),
        (sparse(0), "9bfb9556b8659c2ac94b7ef9e587d71e159ea624"),
        (sparse(63), "150d9fa4eb456e73ba48de5779c5c996b16b3b23"),
        (sparse(64), "a167db30424ff8890d085c0f1c92b2c5cc06fc45"),
        (sparse(127), "d246ec5e7adc8118994b8df77146529486ac7caf"),
    ] {
        assert_eq!(
            hex::encode(&whitebox::m3_tail(&local, &decrypt3(payload))),
            want
        );
    }
}

/// fairplay_key_test.go TestFairPlayKeyDecryptVector and
/// TestFairPlayAlternateMACVector: captured m3/ekey records.
#[test]
fn captured_ekey_vectors() {
    let m3 = h("46504c590301030000000098018f1a9c7d0af257b31f21f5c2d2bc814c032d457835ad0b06250574bbc7ab4a58cca6eead2c911d7f3e1e7ed4c058955dff3d5ceef014387a985bdb34995015e3dfbdacc56047cb926e093b13e9fdb5e1eee317c018bbc87fc5453c7671647da686da3d564875d03f8aea9d60092de06110bc7be0c16f391c369c75344ae47f33acfcf10e63a9b58bfce215e96001c49e4be967c5067f2a");
    let ekey = h("46504c59010201000000003c0000000088e4f82c8178c18b4751ac24b27c0c2a00000010c899dc6965c1081de6a9d966e2ba3e34548cdbc651c322db18dc22f58fe154a60aecee18");
    let local = message::decrypt(m3[12] as usize, &arr(&m3[16..144]));
    assert_eq!(
        hex::encode(&unwrap_key(&local, &reference_receiver_sap(), &ekey)),
        "8e1214398d46d72e7b1b8e32f80c8bf0"
    );

    let m2 = h("46504c5903010200000000820201cf32a25714b2524f8aa0ad7af164e37bcf4424e200047efc0ad67afcd95ded1c2730bb591b962ed63a9c4ded88ba8fc78de64d91ccfd5c7b56da88e31f5cceafc7431995a01665a54e1939d25b94db64b9e45d8d063e1e6af07e9656162b0efa404275ea5a44d9591c7256b9fbe6513898b80227721988571650942ad946688a");
    let m3 = h("46504c590301030000000098018f1a9c5b9228300aafe0b41f28b66a62a6cd62bf84eb623273dead10b1f034a8d568126faa133f6ad5ab91acda3839817b4d9530b679fee43ac9e950f6e7aaf1381bd2d3d5198a03bf5648890d19234270a3583e4651893be09c6c75463c42e544fec9abc9f7722a2cc254364365ef91ded76b8c00f9674b08920fb9401e4be6d52a33f2f9ed6fadb672be45c3cde5ad94f3fea5b32ee4");
    let remote = message::decrypt(m2[13] as usize, &arr(&m2[14..142]));
    assert_eq!(remote, reference_receiver_sap());
    let local = message::decrypt(m3[12] as usize, &arr(&m3[16..144]));
    assert_eq!(
        hex::encode(&whitebox::descriptor(&local, &remote)),
        "2fd95dc2c23122bc77c57b983a9188c4760db322"
    );
    // A session holding exactly that state reproduces the MAC'd prefix.
    let session = FpSession {
        local_sap: local,
        exchanged: Some(Exchanged {
            remote_sap: remote,
            m3: arr(&m3),
        }),
    };
    let ekey = session
        .wrap_key_with_mask(
            &arr(&h("000102030405060708090a0b0c0d0e0f")),
            &arr(&h("c853e777b9b65e7652d768d97c974f15")),
        )
        .unwrap();
    assert_eq!(
        hex::encode(&ekey[..56]),
        "46504c59010201000000003c00000000c853e777b9b65e7652d768d97c974f15\
         000000102a53a0008888fe26bfb1e1f825f38f50d6730059"
    );
}

// ---------------------------------------------------------------------------
// Oracle-generated vectors (tests/vectors/fairplay)

#[test]
fn primitive_vectors() {
    let blocks = vectors::load("fairplay/primitives.txt");
    let mut seen = [0; 3];
    for b in &blocks {
        let name = b.name();
        if b.get("hash").is_some() {
            assert_eq!(
                sap::sap_hash(&arr(&b.hex("block"))).to_vec(),
                b.hex("hash"),
                "{name}"
            );
            seen[0] += 1;
        } else if b.get("mutation").is_some() {
            let m = match b.parse::<u8>("mutation") {
                0 => Mutation::Swap,
                1 => Mutation::Cycle,
                _ => Mutation::Kdf,
            };
            let out = md5_le(&arr(&b.hex("state")), &arr(&b.hex("block")), m);
            assert_eq!(out.to_vec(), b.hex("out"), "{name}");
            seen[1] += 1;
        } else {
            let mode = b.parse::<usize>("mode");
            let pt: [u8; 128] = arr(&b.hex("pt"));
            let ct: [u8; 128] = arr(&b.hex("ct"));
            assert_eq!(message::encrypt(mode, &pt), ct, "{name}");
            assert_eq!(message::decrypt(mode, &ct), pt, "{name}");
            seen[2] += 1;
        }
    }
    assert!(seen.iter().all(|&n| n > 0), "{seen:?}");
}

fn check_session(b: &Block) {
    let name = b.name();
    let mut s = FpSession::with_entropy(&arr(&b.hex("entropy")));
    assert_eq!(s.m1().to_vec(), b.hex("m1"), "{name}");
    let m3 = s.m3(&b.hex("m2")).unwrap();
    assert_eq!(m3.to_vec(), b.hex("m3"), "{name}");
    s.check_m4(&m4_for(&m3)).unwrap();
    let key: [u8; 16] = arr(&b.hex("key"));
    let ekey = s.wrap_key_with_mask(&key, &arr(&b.hex("mask"))).unwrap();
    assert_eq!(ekey.to_vec(), b.hex("ekey"), "{name}");
    let remote = &s.exchanged.as_ref().unwrap().remote_sap;
    assert_eq!(unwrap_key(&s.local_sap, remote, &ekey), key, "{name}");
}

#[test]
fn session_vectors() {
    let blocks = vectors::load("fairplay/session.txt");
    assert!(blocks.len() >= 13);
    for b in &blocks {
        check_session(b);
    }
}

#[test]
fn stream_key_vectors() {
    for b in &vectors::load("fairplay/stream_keys.txt") {
        let master = stream_master_key(&arr(&b.hex("key")), &b.hex("secret"));
        assert_eq!(master.to_vec(), b.hex("master"), "{}", b.name());
        let (k, iv) = stream_key_iv(&master, b.parse("stream_connection_id"));
        assert_eq!(k.to_vec(), b.hex("stream_key"), "{}", b.name());
        assert_eq!(iv.to_vec(), b.hex("stream_iv"), "{}", b.name());
    }
    let key = [7u8; 16];
    assert_eq!(stream_master_key(&key, &[]), key);
}

// ---------------------------------------------------------------------------
// State and malformed input

fn valid_m2(mode: u8) -> Vec<u8> {
    let mut m2 = header(2, 130).to_vec();
    m2.extend_from_slice(&[2, mode]);
    m2.extend_from_slice(&[0x5a; 128]);
    m2
}

#[test]
fn rejects_malformed_m2() {
    let mut s = FpSession::with_entropy(&[1; 126]);
    assert!(s.m3(&valid_m2(3)).is_ok());
    let bad = |f: &dyn Fn(&mut Vec<u8>)| {
        let mut m2 = valid_m2(3);
        f(&mut m2);
        m2
    };
    for (m2, kind) in [
        (vec![], ErrorKind::Malformed),
        (bad(&|m| m.truncate(141)), ErrorKind::Malformed),
        (bad(&|m| m.push(0)), ErrorKind::Malformed),
        (bad(&|m| m[0] = b'X'), ErrorKind::Malformed),
        (bad(&|m| m[6] = 4), ErrorKind::Malformed),
        (bad(&|m| m[11] = 0x81), ErrorKind::Malformed),
        (bad(&|m| m[12] = 1), ErrorKind::Malformed),
        (bad(&|m| m[13] = 4), ErrorKind::Unsupported),
        (bad(&|m| m[13] = 0xff), ErrorKind::Unsupported),
    ] {
        let err = FpSession::with_entropy(&[1; 126]).m3(&m2).unwrap_err();
        assert_eq!(err.kind(), kind, "{err}");
    }
}

use crate::error::ErrorKind;

#[test]
fn m4_and_wrap_need_m3() {
    let mut s = FpSession::with_entropy(&[2; 126]);
    assert_eq!(
        s.check_m4(&[0; 32]).unwrap_err().kind(),
        ErrorKind::Protocol
    );
    assert_eq!(
        s.wrap_key(&[0; 16]).unwrap_err().kind(),
        ErrorKind::Protocol
    );
    let m3 = s.m3(&valid_m2(0)).unwrap();
    let m4 = m4_for(&m3);
    s.check_m4(&m4).unwrap();
    let mut wrong = m4.clone();
    wrong[31] ^= 1;
    assert_eq!(s.check_m4(&wrong).unwrap_err().kind(), ErrorKind::Crypto);
    assert_eq!(
        s.check_m4(&m4[..31]).unwrap_err().kind(),
        ErrorKind::Malformed
    );
    let mut bad_type = m4.clone();
    bad_type[6] = 3;
    assert_eq!(
        s.check_m4(&bad_type).unwrap_err().kind(),
        ErrorKind::Malformed
    );
    // Random masks: two wraps of one key differ but both unwrap.
    let a = s.wrap_key(&[9; 16]).unwrap();
    let b = s.wrap_key(&[9; 16]).unwrap();
    assert_ne!(a, b);
    let remote = &s.exchanged.as_ref().unwrap().remote_sap;
    assert_eq!(unwrap_key(&s.local_sap, remote, &a), [9; 16]);
    assert_eq!(unwrap_key(&s.local_sap, remote, &b), [9; 16]);
}

#[test]
fn fresh_sessions_differ() {
    let a = FpSession::new().unwrap();
    let b = FpSession::new().unwrap();
    assert_ne!(a.local_sap, b.local_sap);
    assert_eq!(&a.local_sap[..2], &[0, 1]);
}

/// Arbitrary bytes as m2/m4 never panic.
#[test]
fn fuzz_no_panic() {
    let mut x: u64 = 0x243f6a8885a308d3;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    for _ in 0..2000 {
        let len = (next() % 200) as usize;
        let mut buf: Vec<u8> = (0..len).map(|_| next() as u8).collect();
        if next() % 2 == 0 && len >= 142 {
            buf.truncate(142);
            buf[..12].copy_from_slice(&header(2, 130));
            buf[12] = 2;
        }
        let mut s = FpSession::with_entropy(&[3; 126]);
        if let Ok(m3) = s.m3(&buf) {
            let _ = s.check_m4(&buf);
            let _ = s.check_m4(&m4_for(&m3));
        }
    }
}

// ---------------------------------------------------------------------------
// Differential fuzzing and hardware (run by hand)

/// Compares random sessions against the doubletake oracle harness:
///
/// ```text
/// cargo test fairplay::tests::differential -- --ignored
/// ```
///
/// Uses `TVPIPE_FP_ORACLE` or `~/.cache/streamlib-oracle/fp/fporacle`;
/// skips when neither exists.
#[test]
#[ignore]
fn differential_oracle() {
    use std::io::{BufRead, BufReader, Write as _};
    use std::process::{Command, Stdio};

    let path = std::env::var_os("TVPIPE_FP_ORACLE")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|h| std::path::Path::new(&h).join(".cache/streamlib-oracle/fp/fporacle"))
        });
    let Some(path) = path.filter(|p| p.exists()) else {
        eprintln!("no oracle; skipping");
        return;
    };
    let mut child = Command::new(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut ask = |cmd: String| {
        writeln!(stdin, "{cmd}").unwrap();
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        line.trim().to_owned()
    };
    let rand = |n: usize| {
        let mut v = vec![0u8; n];
        random_fill(&mut v).unwrap();
        v
    };
    let n: usize = std::env::var("TVPIPE_FP_ROUNDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(500);
    for i in 0..n {
        let block: [u8; 64] = arr(&rand(64));
        let got = sap::sap_hash(&block);
        assert_eq!(
            hex::encode(&got),
            ask(format!("sap-hash {}", hex::encode(&block))),
            "sap-hash {}",
            hex::encode(&block)
        );

        let entropy = rand(126);
        let mut m2 = valid_m2((i % 4) as u8);
        m2[14..].copy_from_slice(&rand(128));
        let key = rand(16);
        let mask = rand(16);
        let want = ask(format!(
            "session {} {} {} {}",
            hex::encode(&entropy),
            hex::encode(&m2),
            hex::encode(&key),
            hex::encode(&mask)
        ));
        let mut s = FpSession::with_entropy(&arr(&entropy));
        let m3 = s.m3(&m2).unwrap();
        let ekey = s.wrap_key_with_mask(&arr(&key), &arr(&mask)).unwrap();
        let got = format!(
            "{} {} {}",
            hex::encode(&s.m1()),
            hex::encode(&m3),
            hex::encode(&ekey)
        );
        assert_eq!(got, want, "session m2={}", hex::encode(&m2));
    }
    drop(stdin);
    let _ = child.wait();
    eprintln!("{n} rounds match the oracle");
}

/// The full exchange against a real receiver over the verified control
/// channel (uses the real credential store, like the CLI):
///
/// ```text
/// TVPIPE_ATV=192.168.1.34 cargo test hardware_fp_setup -- --ignored --nocapture
/// ```
#[test]
#[ignore]
fn hardware_fp_setup() {
    use crate::airplay::credentials::Store;
    use crate::airplay::pair;

    let Ok(target) = std::env::var("TVPIPE_ATV") else {
        eprintln!("TVPIPE_ATV not set; skipping");
        return;
    };
    let target = pair::resolve(&target).unwrap();
    let store = Store::open(&crate::storage::Storage::cli()).unwrap();
    let mut control = pair::connect(&target, &store).unwrap();
    eprintln!(
        "verified with {} ({:?})",
        control.addr,
        match &control.auth {
            pair::Auth::Verified(r) => r.name.as_str(),
            pair::Auth::Transient => "transient",
        }
    );

    let mut s = FpSession::new().unwrap();
    let m2 = post(&mut control.client, &s.m1(), "m1").unwrap();
    eprintln!(
        "m2: {} bytes, mode {}",
        m2.len(),
        m2.get(13).copied().unwrap_or(0xff)
    );
    let m3 = s.m3(&m2).unwrap();
    let m4 = post(&mut control.client, &m3, "m3").unwrap();
    eprintln!("m4: {} bytes", m4.len());
    s.check_m4(&m4).unwrap();
    let ekey = s.wrap_key(&random_key()).unwrap();
    eprintln!("m4 confirms m3; ekey {} bytes", ekey.len());

    // The connection is still usable afterwards.
    let resp = control.client.request("GET", "/info", &[], None).unwrap();
    eprintln!("GET /info after fp-setup: {}", resp.status);

    // And the one-call helper on a fresh connection.
    let mut control = pair::connect(&target, &store).unwrap();
    fp_setup(&mut control.client).unwrap();
    eprintln!("fp_setup ok");
}

fn random_key() -> [u8; 16] {
    let mut k = [0u8; 16];
    random_fill(&mut k).unwrap();
    k
}
