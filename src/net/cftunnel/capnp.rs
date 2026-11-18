//! The Cap'n Proto subset the tunnel's control stream needs: building
//! single-segment messages, reading messages of any shape (far pointers
//! included), the stream framing, and three RPC messages (Bootstrap, Call,
//! Finish; Return in) for `RegistrationServer.registerConnection` and
//! `unregisterConnection`.
//!
//! cloudflared runs the full RPC protocol (zombiezen capnproto2); as the
//! client of one bootstrap capability with calls pipelined on it, this is
//! all that crosses the wire.
//!
//! Sources: capnproto.org `encoding.html` (pointers, framing) and
//! `rpc.capnp`; the field offsets of zombiezen capnproto2
//! `std/capnp/rpc/rpc.capnp.go` and of cloudflared (Apache-2.0)
//! `tunnelrpc/proto/tunnelrpc.capnp{,.go}`, `tunnelrpc/pogs/registration_server.go`.

use crate::error::{Error, Result};

/// Where a builder's struct lives (word indices).
#[derive(Clone, Copy)]
pub struct St {
    data: usize,
    ptrs: usize,
}

impl St {
    pub fn ptr(self, i: usize) -> usize {
        self.ptrs + i
    }
}

/// One segment, the root pointer first.
pub struct Builder {
    buf: Vec<u8>,
}

impl Builder {
    pub fn new() -> Builder {
        Builder { buf: vec![0; 8] }
    }

    fn alloc(&mut self, words: usize) -> usize {
        let at = self.buf.len() / 8;
        self.buf.resize(self.buf.len() + words * 8, 0);
        at
    }

    fn set_ptr(&mut self, at: usize, target: usize, kind: u64, upper: u64) {
        let off = (target as i64 - at as i64 - 1) as i32;
        let p = u64::from((off << 2) as u32) | kind | upper << 32;
        self.buf[at * 8..at * 8 + 8].copy_from_slice(&p.to_le_bytes());
    }

    /// A new struct, pointed to from the pointer slot `at`.
    pub fn new_struct(&mut self, at: usize, data: u16, ptrs: u16) -> St {
        let s = self.alloc(usize::from(data) + usize::from(ptrs));
        self.set_ptr(at, s, 0, u64::from(data) | u64::from(ptrs) << 16);
        St {
            data: s,
            ptrs: s + usize::from(data),
        }
    }

    pub fn root(&mut self, data: u16, ptrs: u16) -> St {
        self.new_struct(0, data, ptrs)
    }

    /// Stores `v` (little-endian) at byte `off` of the data section.
    pub fn set(&mut self, s: St, off: usize, v: &[u8]) {
        let at = s.data * 8 + off;
        self.buf[at..at + v.len()].copy_from_slice(v);
    }

    /// `Data` (or, with `nul`, `Text`) at the pointer slot `at`.
    pub fn bytes(&mut self, at: usize, b: &[u8], nul: bool) {
        let n = b.len() + usize::from(nul);
        let l = self.alloc(n.div_ceil(8));
        self.buf[l * 8..l * 8 + b.len()].copy_from_slice(b);
        self.set_ptr(at, l, 1, 2 | (n as u64) << 3);
    }

    /// A `List(Text)` at the pointer slot `at`.
    pub fn text_list(&mut self, at: usize, items: &[&str]) {
        let l = self.alloc(items.len());
        self.set_ptr(at, l, 1, 6 | (items.len() as u64) << 3);
        for (i, t) in items.iter().enumerate() {
            self.bytes(l + i, t.as_bytes(), true);
        }
    }

    /// The message with its stream framing (one segment).
    pub fn finish(self) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + self.buf.len());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&((self.buf.len() / 8) as u32).to_le_bytes());
        out.extend_from_slice(&self.buf);
        out
    }
}

/// A received message: its segments.
pub struct Message {
    segs: Vec<Vec<u8>>,
}

/// Largest message we take from the edge.
const MAX_MESSAGE: usize = 1 << 20;

fn bad(what: &str) -> Error {
    Error::malformed(format!("capnp: {what}"))
}

impl Message {
    /// The framed message at the start of `buf` and its length, or `None`
    /// if more bytes are needed.
    pub fn read(buf: &[u8]) -> Result<Option<(Message, usize)>> {
        let u32_at = |i: usize| {
            buf.get(i * 4..i * 4 + 4)
                .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
        };
        let Some(n) = u32_at(0) else { return Ok(None) };
        let n = n + 1;
        if n > 64 {
            return Err(bad("too many segments"));
        }
        let head = (1 + n).div_ceil(2) * 8;
        let mut sizes = Vec::with_capacity(n);
        for i in 0..n {
            let Some(s) = u32_at(1 + i) else {
                return Ok(None);
            };
            sizes.push(s * 8);
        }
        let total = head + sizes.iter().sum::<usize>();
        if total > MAX_MESSAGE {
            return Err(bad("message too large"));
        }
        if buf.len() < total {
            return Ok(None);
        }
        let mut at = head;
        let segs = sizes
            .iter()
            .map(|&s| {
                at += s;
                buf[at - s..at].to_vec()
            })
            .collect();
        Ok(Some((Message { segs }, total)))
    }

    fn word(&self, seg: usize, i: usize) -> Result<u64> {
        let b = self
            .segs
            .get(seg)
            .and_then(|s| s.get(i * 8..i * 8 + 8))
            .ok_or_else(|| bad("pointer out of bounds"))?;
        Ok(u64::from_le_bytes(b.try_into().unwrap_or_default()))
    }

    /// Follows the pointer at word `at` of `seg`: the target's segment,
    /// its first word and the pointer describing it (its tag).
    fn follow(&self, seg: usize, at: usize) -> Result<Option<(usize, usize, u64)>> {
        let p = self.word(seg, at)?;
        if p == 0 {
            return Ok(None);
        }
        if p & 3 != 2 {
            let off = i64::from((p as u32 as i32) >> 2);
            let t = usize::try_from(at as i64 + 1 + off).map_err(|_| bad("bad offset"))?;
            return Ok(Some((seg, t, p)));
        }
        let pseg = (p >> 32) as usize;
        let pad = (p >> 3 & 0x1fff_ffff) as usize;
        if p & 4 == 0 {
            // Single far: the landing pad is an ordinary pointer.
            let q = self.word(pseg, pad)?;
            if q & 3 == 2 {
                return Err(bad("far pointer to a far pointer"));
            }
            return self.follow(pseg, pad);
        }
        // Double far: a far pointer to the content, then its tag.
        let q = self.word(pseg, pad)?;
        if q & 7 != 2 {
            return Err(bad("bad double-far landing pad"));
        }
        let tag = self.word(pseg, pad + 1)?;
        Ok(Some((
            (q >> 32) as usize,
            (q >> 3 & 0x1fff_ffff) as usize,
            tag,
        )))
    }

    fn struct_at(&self, seg: usize, at: usize) -> Result<Option<Struct<'_>>> {
        let Some((seg, t, tag)) = self.follow(seg, at)? else {
            return Ok(None);
        };
        if tag & 3 != 0 {
            return Err(bad("expected a struct"));
        }
        let (dw, pw) = ((tag >> 32 & 0xffff) as usize, (tag >> 48) as usize);
        if (t + dw + pw) * 8 > self.segs[seg].len() {
            return Err(bad("struct out of bounds"));
        }
        Ok(Some(Struct {
            m: self,
            seg,
            data: t,
            dw,
            ptrs: t + dw,
            pw,
        }))
    }

    pub fn root(&self) -> Result<Struct<'_>> {
        self.struct_at(0, 0)?.ok_or_else(|| bad("no root"))
    }
}

/// A struct in a received message; fields outside it read as defaults.
#[derive(Clone, Copy)]
pub struct Struct<'a> {
    m: &'a Message,
    seg: usize,
    data: usize,
    dw: usize,
    ptrs: usize,
    pw: usize,
}

impl<'a> Struct<'a> {
    fn data(&self, off: usize, n: usize) -> u64 {
        if off + n > self.dw * 8 {
            return 0;
        }
        let at = self.data * 8 + off;
        let mut b = [0u8; 8];
        b[..n].copy_from_slice(&self.m.segs[self.seg][at..at + n]);
        u64::from_le_bytes(b)
    }

    pub fn u16(&self, off: usize) -> u16 {
        self.data(off, 2) as u16
    }

    pub fn u32(&self, off: usize) -> u32 {
        self.data(off, 4) as u32
    }

    pub fn u64(&self, off: usize) -> u64 {
        self.data(off, 8)
    }

    pub fn bit(&self, n: usize) -> bool {
        self.data(n / 8, 1) >> (n % 8) & 1 == 1
    }

    pub fn ptr_struct(&self, i: usize) -> Result<Option<Struct<'a>>> {
        if i >= self.pw {
            return Ok(None);
        }
        self.m.struct_at(self.seg, self.ptrs + i)
    }

    /// `Data` or `Text` (with its NUL) at pointer `i`; empty if null.
    pub fn bytes(&self, i: usize) -> Result<&'a [u8]> {
        if i >= self.pw {
            return Ok(&[]);
        }
        let Some((seg, t, tag)) = self.m.follow(self.seg, self.ptrs + i)? else {
            return Ok(&[]);
        };
        if tag & 3 != 1 || tag >> 32 & 7 != 2 {
            return Err(bad("expected a byte list"));
        }
        let n = (tag >> 35) as usize;
        self.m.segs[seg]
            .get(t * 8..t * 8 + n)
            .ok_or_else(|| bad("list out of bounds"))
    }

    pub fn text(&self, i: usize) -> Result<String> {
        let b = self.bytes(i)?;
        let b = b.strip_suffix(&[0]).unwrap_or(b);
        Ok(String::from_utf8_lossy(b).into_owned())
    }
}

// ---------------------------------------------------------------------------
// RPC (rpc.capnp): message union discriminants and the messages we send.

const CALL: u16 = 2;
pub const RETURN: u16 = 3;
const FINISH: u16 = 4;
const BOOTSTRAP: u16 = 8;

pub fn bootstrap(question: u32) -> Vec<u8> {
    let mut b = Builder::new();
    let m = b.root(1, 1);
    b.set(m, 0, &BOOTSTRAP.to_le_bytes());
    let bs = b.new_struct(m.ptr(0), 1, 1);
    b.set(bs, 0, &question.to_le_bytes());
    b.finish()
}

/// Finish (releasing the result's capabilities, the default).
pub fn finish(question: u32) -> Vec<u8> {
    let mut b = Builder::new();
    let m = b.root(1, 1);
    b.set(m, 0, &FINISH.to_le_bytes());
    let f = b.new_struct(m.ptr(0), 1, 0);
    b.set(f, 0, &question.to_le_bytes());
    b.finish()
}

/// A call on the capability the answer to `on` will be (promise
/// pipelining); `params` builds the parameter struct at the slot it gets.
pub fn call(
    question: u32,
    on: u32,
    interface: u64,
    method: u16,
    params: impl FnOnce(&mut Builder, usize),
) -> Vec<u8> {
    let mut b = Builder::new();
    let m = b.root(1, 1);
    b.set(m, 0, &CALL.to_le_bytes());
    let c = b.new_struct(m.ptr(0), 3, 3);
    b.set(c, 0, &question.to_le_bytes());
    b.set(c, 4, &method.to_le_bytes());
    b.set(c, 8, &interface.to_le_bytes());
    let target = b.new_struct(c.ptr(0), 1, 1);
    b.set(target, 4, &1u16.to_le_bytes()); // promisedAnswer
    let promised = b.new_struct(target.ptr(0), 1, 1);
    b.set(promised, 0, &on.to_le_bytes());
    let payload = b.new_struct(c.ptr(1), 0, 2);
    params(&mut b, payload.ptr(0));
    b.finish()
}

/// What a Return says: the result's content struct, or an exception.
pub enum Returned<'a> {
    Results(Option<Struct<'a>>),
    Exception(String),
    Other(u16),
}

/// The message's kind and, for a Return, its answer ID and outcome.
pub fn parse(m: &Message) -> Result<(u16, Option<(u32, Returned<'_>)>)> {
    let root = m.root()?;
    let which = root.u16(0);
    if which != RETURN {
        return Ok((which, None));
    }
    let r = root.ptr_struct(0)?.ok_or_else(|| bad("empty return"))?;
    let out = match r.u16(6) {
        0 => {
            let payload = r.ptr_struct(0)?.ok_or_else(|| bad("no payload"))?;
            // The bootstrap's content is a capability, not a struct.
            Returned::Results(payload.ptr_struct(0).ok().flatten())
        }
        1 => Returned::Exception(match r.ptr_struct(0)? {
            Some(e) => e.text(0)?,
            None => String::new(),
        }),
        w => Returned::Other(w),
    };
    Ok((which, Some((r.u32(0), out))))
}

// ---------------------------------------------------------------------------
// cloudflared's tunnelrpc.capnp

/// `RegistrationServer` (cloudflared calls it through `TunnelServer`, which
/// extends it; the method's interface ID is this one).
pub const REGISTRATION_SERVER: u64 = 0xf716_95ec_7fe8_5497;
pub const REGISTER_CONNECTION: u16 = 0;
pub const UNREGISTER_CONNECTION: u16 = 1;

pub struct Registration<'a> {
    pub account_tag: &'a str,
    pub secret: &'a [u8],
    pub tunnel_id: &'a [u8; 16],
    pub client_id: &'a [u8; 16],
    pub conn_index: u8,
    pub previous_attempts: u8,
    pub version: &'a str,
    pub arch: &'a str,
}

/// `registerConnection(auth :TunnelAuth, tunnelId :Data, connIndex :UInt8,
/// options :ConnectionOptions)`'s parameters at slot `at`.
pub fn register_params(r: &Registration, b: &mut Builder, at: usize) {
    let p = b.new_struct(at, 1, 3);
    let auth = b.new_struct(p.ptr(0), 0, 2);
    b.bytes(auth.ptr(0), r.account_tag.as_bytes(), true);
    b.bytes(auth.ptr(1), r.secret, false);
    b.bytes(p.ptr(1), r.tunnel_id, false);
    b.set(p, 0, &[r.conn_index]);
    let opts = b.new_struct(p.ptr(2), 1, 2);
    b.set(opts, 2, &[r.previous_attempts]);
    let client = b.new_struct(opts.ptr(0), 0, 4);
    b.bytes(client.ptr(0), r.client_id, false);
    b.text_list(client.ptr(1), &[]);
    b.bytes(client.ptr(2), r.version.as_bytes(), true);
    b.bytes(client.ptr(3), r.arch.as_bytes(), true);
}

/// The edge's answer to `registerConnection`: the location (an airport
/// code) or the error with whether to retry.
pub fn registered(results: Option<Struct>) -> Result<std::result::Result<String, (String, bool)>> {
    let response = results
        .and_then(|r| r.ptr_struct(0).transpose())
        .transpose()?
        .ok_or_else(|| bad("no ConnectionResponse"))?;
    let detail = response.ptr_struct(0)?.ok_or_else(|| bad("empty result"))?;
    Ok(match response.u16(0) {
        1 => Ok(detail.text(1)?),
        0 => Err((detail.text(0)?, detail.bit(64))),
        w => return Err(bad(&format!("unknown result {w}"))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::hex;

    #[test]
    fn framing_and_layout() {
        // Bootstrap{questionId: 0}, checked by hand against encoding.html:
        // segment table, root struct pointer (offset 0, 1 data word, 1
        // pointer), discriminant 8, pointer to the Bootstrap struct.
        assert_eq!(
            hex::encode(&bootstrap(0)),
            concat!(
                "0000000005000000",
                "0000000001000100",
                "0800000000000000",
                "0000000001000100",
                "0000000000000000",
                "0000000000000000",
            )
        );
        let (m, n) = Message::read(&finish(7)).unwrap().unwrap();
        assert_eq!(n, finish(7).len());
        let (which, ret) = parse(&m).unwrap();
        assert_eq!((which, ret.is_none()), (FINISH, true));
        let f = m.root().unwrap().ptr_struct(0).unwrap().unwrap();
        assert_eq!((f.u32(0), f.bit(32)), (7, false));
        // Incomplete input asks for more.
        assert!(Message::read(&finish(7)[..20]).unwrap().is_none());
        assert!(Message::read(&[1, 0]).unwrap().is_none());
    }

    #[test]
    fn register_call_round_trips() {
        let r = Registration {
            account_tag: "acct",
            secret: &[1, 2, 3],
            tunnel_id: &[9; 16],
            client_id: &[5; 16],
            conn_index: 0,
            previous_attempts: 2,
            version: "tvpipe/1",
            arch: "macos_aarch64",
        };
        let bytes = call(1, 0, REGISTRATION_SERVER, REGISTER_CONNECTION, |b, at| {
            register_params(&r, b, at)
        });
        let (m, _) = Message::read(&bytes).unwrap().unwrap();
        let c = m.root().unwrap().ptr_struct(0).unwrap().unwrap();
        assert_eq!((c.u32(0), c.u16(4), c.u64(8)), (1, 0, REGISTRATION_SERVER));
        let target = c.ptr_struct(0).unwrap().unwrap();
        assert_eq!(target.u16(4), 1);
        assert_eq!(target.ptr_struct(0).unwrap().unwrap().u32(0), 0);
        let params = c.ptr_struct(1).unwrap().unwrap().ptr_struct(0);
        let params = params.unwrap().unwrap();
        let auth = params.ptr_struct(0).unwrap().unwrap();
        assert_eq!(auth.text(0).unwrap(), "acct");
        assert_eq!(auth.bytes(1).unwrap(), &[1, 2, 3]);
        assert_eq!(params.bytes(1).unwrap(), &[9; 16]);
        let opts = params.ptr_struct(2).unwrap().unwrap();
        assert_eq!(opts.u16(2) & 0xff, 2);
        let client = opts.ptr_struct(0).unwrap().unwrap();
        assert_eq!(client.text(2).unwrap(), "tvpipe/1");
        assert_eq!(client.text(3).unwrap(), "macos_aarch64");
    }

    /// A Return as the edge sends it, in two segments with a far pointer
    /// (single and double) to show both are followed.
    #[test]
    fn reads_returns_across_segments() {
        // Build ConnectionResponse{connectionDetails{locationName "vie06"}}
        // as a Return in one segment, then move the Return struct to a
        // second segment behind a far pointer.
        let mut b = Builder::new();
        let m = b.root(1, 1);
        b.set(m, 0, &RETURN.to_le_bytes());
        let r = b.new_struct(m.ptr(0), 2, 1);
        b.set(r, 0, &1u32.to_le_bytes());
        let payload = b.new_struct(r.ptr(0), 0, 2);
        let results = b.new_struct(payload.ptr(0), 0, 1);
        let resp = b.new_struct(results.ptr(0), 1, 1);
        b.set(resp, 0, &1u16.to_le_bytes());
        let details = b.new_struct(resp.ptr(0), 1, 2);
        b.bytes(details.ptr(1), b"vie06", true);
        let one = b.finish();
        let check = |bytes: &[u8]| {
            let (m, n) = Message::read(bytes).unwrap().unwrap();
            assert_eq!(n, bytes.len());
            let (which, ret) = parse(&m).unwrap();
            assert_eq!(which, RETURN);
            let Some((1, Returned::Results(content))) = ret else {
                panic!("not a return")
            };
            assert_eq!(registered(content).unwrap(), Ok("vie06".to_owned()));
        };
        check(&one);
        // Segment 0: root struct (1 data word, 1 pointer) whose pointer is
        // far; segment 1: the landing pad then everything else.
        let seg = &one[8..];
        let words = seg.len() / 8;
        let word = |i: usize| u64::from_le_bytes(seg[i * 8..i * 8 + 8].try_into().unwrap());
        // In `one`, word 2 points to the Return at word 3; relocate words
        // 3.. to segment 1 at word 1 (after a pad): offsets inside stay
        // valid, the pad points to word 1 (offset 0).
        let ret_ptr = word(2) & !0xffff_fffc; // offset 0
        let mut s0 = seg[..16].to_vec();
        let mut s1 = ret_ptr.to_le_bytes().to_vec();
        s1.extend_from_slice(&seg[24..]);
        // Single far: landing pad at word 0 of segment 1.
        s0.extend_from_slice(&(2u64 | 1 << 32).to_le_bytes());
        let frame = |s0: &[u8], s1: &[u8]| {
            let mut f = Vec::new();
            for v in [1u32, (s0.len() / 8) as u32, (s1.len() / 8) as u32, 0] {
                f.extend_from_slice(&v.to_le_bytes());
            }
            f.extend_from_slice(s0);
            f.extend_from_slice(s1);
            f
        };
        check(&frame(&s0, &s1));
        // Double far: a pad in segment 0 (words 3, 4) naming segment 1,
        // word 1, with the struct's tag.
        s0.truncate(16);
        s0.extend_from_slice(&(6u64 | 3 << 3).to_le_bytes()); // double far to seg 0 word 3
        s0.extend_from_slice(&(2u64 | 1 << 3 | 1 << 32).to_le_bytes());
        s0.extend_from_slice(&ret_ptr.to_le_bytes());
        check(&frame(&s0, &s1));
        assert_eq!(words, one.len() / 8 - 1);
        // Corrupt pointers are errors, not panics.
        let mut bad = one.clone();
        bad[8 + 16 + 4] = 0xff; // the Return pointer's struct size
        let (m, _) = Message::read(&bad).unwrap().unwrap();
        assert!(parse(&m).is_err());
        let huge = [0xffu8, 0, 0, 0];
        assert!(Message::read(&huge).is_err());
    }

    #[test]
    fn registration_error() {
        let mut b = Builder::new();
        let results = b.root(0, 1);
        let resp = b.new_struct(results.ptr(0), 1, 1);
        let err = b.new_struct(resp.ptr(0), 2, 1);
        b.bytes(err.ptr(0), b"nope", true);
        b.set(err, 8, &[1]);
        let bytes = b.finish();
        let (m, _) = Message::read(&bytes).unwrap().unwrap();
        let got = registered(Some(m.root().unwrap())).unwrap();
        assert_eq!(got, Err(("nope".to_owned(), true)));
    }
}
