//! Local filesystem storage backend.
//!
//! Keeps recordings on the local filesystem. This is the default backend
//! and requires no additional configuration beyond the recording directory.

use async_trait::async_trait;
use std::path::Path;
use tracing::info;

use super::{RecordingInfo, StorageBackend, StorageResult};

/// Local filesystem storage — recordings stay where they were written.
#[derive(Debug, Clone)]
pub struct LocalStorage;

#[async_trait]
impl StorageBackend for LocalStorage {
    async fn store(&self, file_path: &Path, info: &RecordingInfo) -> StorageResult {
        // File is already on disk from SessionRecorder; just log and return path.
        info!(
            path = %file_path.display(),
            call_id = %info.call_id,
            duration_ms = info.duration_ms,
            "Recording stored locally"
        );
        Ok(file_path.display().to_string())
    }

    fn name(&self) -> &str {
        "local"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_local_storage_returns_path() {
        let storage = LocalStorage;
        let path = PathBuf::from("/tmp/test-recording.wav");
        let info = RecordingInfo {
            call_id: "call-123".to_string(),
            duration_ms: 5000,
            ..Default::default()
        };

        let result = storage.store(&path, &info).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "/tmp/test-recording.wav");
    }

    #[test]
    fn test_local_storage_name() {
        assert_eq!(LocalStorage.name(), "local");
    }
}
