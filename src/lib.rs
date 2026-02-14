//! SIPREC (RFC 7865/7866) Recording Server
//!
//! Receives SIPREC INVITEs from an SBC (Session Recording Client),
//! captures dual RTP streams (caller + callee audio), and saves
//! them as stereo WAV files.
//!
//! Storage is pluggable via the [`storage::StorageBackend`] trait.
//! Built-in backends: local filesystem, Amazon S3, HTTP webhook,
//! and multi-backend composite.

pub mod error;
pub mod net;
pub mod pcap;
pub mod recording;
pub mod rtp;
pub mod server;
pub mod session;
pub mod sip;
pub mod siprec;
pub mod storage;
