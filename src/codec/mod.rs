//! Wire formats: JSON, binary plist, TLV8, CastMessage protobuf.
//!
//! Owner: W2. Planned files: `json.rs`, `bplist.rs`, `tlv8.rs`,
//! `protobuf.rs` (PLAN §3).

pub mod bplist;
mod fuzz;
pub mod json;
pub mod protobuf;
pub mod tlv8;
