//! Valve's protobuf definitions for Deadlock, generated at build time from the files in
//! `protos/` (copied from SteamDatabase/Protobufs at the commit named in `PIN`).
//!
//! The .proto files declare no `package`, so every message and enum lands in this crate's
//! root. Compiled with `protox` (pure Rust); no `protoc` is needed.

pub use prost;

include!(concat!(env!("OUT_DIR"), "/haste_protos.rs"));
