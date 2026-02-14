//! SIPREC (RFC 7865/7866) protocol handling.

pub mod metadata;
pub mod multipart;

pub use metadata::{Participant, RecordingMetadata, StreamAssociation};
pub use multipart::{BodyPart, MultipartBody};
