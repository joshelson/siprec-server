//! Pluggable storage backends for completed recordings.
//!
//! The [`StorageBackend`] trait abstracts where recordings are stored after
//! finalization. Built-in implementations include:
//!
//! - [`LocalStorage`] — keeps files on the local filesystem (default)
//! - [`S3Storage`] — uploads to Amazon S3 with metadata tagging
//! - [`HttpStorage`] — uploads via HTTP POST/PUT to any webhook or REST API
//! - [`MultiStorage`] — writes to multiple backends simultaneously
//!
//! Custom backends can be implemented by implementing the [`StorageBackend`] trait.

mod http;
mod local;
mod multi;
mod s3;

pub use self::http::{HttpMethod, HttpStorage, HttpStorageConfig};
pub use local::LocalStorage;
pub use multi::MultiStorage;
pub use s3::{S3Storage, S3StorageConfig};

use async_trait::async_trait;
use std::path::Path;

/// Metadata about a completed recording, passed to storage backends.
#[derive(Debug, Clone, Default)]
pub struct RecordingInfo {
    /// SIP Call-ID.
    pub call_id: String,
    /// Caller SIP URI (AOR).
    pub caller: Option<String>,
    /// Callee SIP URI (AOR).
    pub callee: Option<String>,
    /// Recording duration in milliseconds.
    pub duration_ms: u64,
    /// ISO 8601 timestamp when the recording started.
    pub started_at: String,
}

/// Result type for storage operations.
pub type StorageResult = std::result::Result<String, String>;

/// Trait for pluggable recording storage backends.
///
/// Implementations receive a finalized WAV file path and recording metadata,
/// and are responsible for persisting the recording to the desired destination.
#[async_trait]
pub trait StorageBackend: Send + Sync {
    /// Store a completed recording.
    ///
    /// Returns a location identifier (e.g., file path, S3 key, or URL) on
    /// success, or an error description on failure.
    async fn store(&self, file_path: &Path, info: &RecordingInfo) -> StorageResult;

    /// Human-readable name of this storage backend (for logging).
    fn name(&self) -> &str;
}
