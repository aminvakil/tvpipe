# W2 Codecs

Status: in progress. Owns `src/codec/**`, `tests/vectors/codec/**`,
`tests/fixtures/codec/**`.

- [x] `json.rs`: `Value` (Null, Bool, Number as f64 + preserved i64 when
      integral, String, Array, Object as ordered Vec<(String, Value)>),
      parser (RFC 8259, full string escapes incl. surrogate pairs, depth
      limit), compact serializer, convenience getters (`get`, `as_str`,
      `as_u64`, ...). Builders for constructing Cast messages ergonomically.
- [ ] `bplist.rs`: bplist00 reader and writer: null/bool, int (1/2/4/8/16
      byte), real (4/8), date, data, ASCII and UTF-16 strings, arrays, dicts
      (sets/UIDs not needed). Offsets/refs of any width, bounds-checked, cycle
      and depth limits. Value type shared with nothing else (`Plist`).
      Fixtures produced by `plutil -convert binary1` and Python `plistlib`
      (generator script in `scripts/vectors/`), committed as binary files.
- [x] `tlv8.rs`: HAP TLV8 encode/decode with fragmentation of values > 255
      bytes and merging of consecutive same-type fragments; HAP type
      constants (Method, Identifier, Salt, PublicKey, Proof, EncryptedData,
      State, Error, RetryDelay, Certificate, Signature, Permissions,
      FragmentData, FragmentLast, Flags, Separator).
- [ ] `protobuf.rs`: varint + length-delimited wire format, `CastMessage`
      (protocol_version=1 enum, source_id=2, destination_id=3, namespace=4,
      payload_type=5 enum, payload_utf8=6, payload_binary=7) encode/decode.
      Unknown fields skipped. `protoc` is not installed; build fixtures by
      hand from the wire-format spec and cross-check with Python if
      `google.protobuf` is available, else document the bytes.

Every parser: never panics on arbitrary input. Include a small
deterministic fuzz smoke test (xorshift PRNG, mutate fixtures, 10⁴
iterations) per format.

Sources allowed: RFC 8259, Apple's public CF bplist format description
(CFBinaryPList.c is APSL — describe, don't copy), HAP spec, protobuf
encoding docs, openscreen/pychromecast for `cast_channel.proto` field
numbers.
