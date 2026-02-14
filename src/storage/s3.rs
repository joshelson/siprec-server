//! Amazon S3 storage backend.
//!
//! Uploads completed recordings to an S3 bucket with metadata
//! (call ID, participants, timestamps) as object metadata headers.

use async_trait::async_trait;
use aws_sdk_s3::Client as S3Client;
use aws_sdk_s3::primitives::ByteStream;
use std::path::Path;
use tracing::{error, info};

use super::{RecordingInfo, StorageBackend, StorageResult};

/// S3 storage backend configuration.
#[derive(Debug, Clone)]
pub struct S3StorageConfig {
    /// S3 bucket name.
    pub bucket: String,
    /// Key prefix (e.g., "recordings/").
    pub key_prefix: String,
    /// Whether to delete the local file after successful upload.
    pub delete_local: bool,
}

/// Amazon S3 storage backend.
pub struct S3Storage {
    client: S3Client,
    config: S3StorageConfig,
}

impl S3Storage {
    /// Create a new S3 storage backend with the given client and config.
    pub fn new(client: S3Client, config: S3StorageConfig) -> Self {
        Self { client, config }
    }

    /// Create a new S3 storage backend using the default AWS config.
    pub async fn from_defaults(config: S3StorageConfig) -> Self {
        let aws_config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let client = S3Client::new(&aws_config);
        Self::new(client, config)
    }

    /// Build the S3 object key from config prefix and filename.
    pub fn build_key(&self, file_path: &Path) -> String {
        let filename = file_path
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or("unknown.wav");
        format!("{}{}", self.config.key_prefix, filename)
    }
}

#[async_trait]
impl StorageBackend for S3Storage {
    async fn store(&self, file_path: &Path, info: &RecordingInfo) -> StorageResult {
        let key = self.build_key(file_path);

        let body = ByteStream::from_path(file_path)
            .await
            .map_err(|e| format!("Failed to read file: {}", e))?;

        let mut req = self
            .client
            .put_object()
            .bucket(&self.config.bucket)
            .key(&key)
            .content_type("audio/wav")
            .body(body)
            .metadata("call-id", &info.call_id)
            .metadata("duration-ms", info.duration_ms.to_string())
            .metadata("started-at", &info.started_at);

        if let Some(ref caller) = info.caller {
            req = req.metadata("caller", caller);
        }
        if let Some(ref callee) = info.callee {
            req = req.metadata("callee", callee);
        }

        match req.send().await {
            Ok(_) => {
                info!(
                    bucket = %self.config.bucket,
                    key = %key,
                    call_id = %info.call_id,
                    duration_ms = info.duration_ms,
                    "Recording uploaded to S3"
                );

                // Delete local file if configured
                if self.config.delete_local
                    && let Err(e) = std::fs::remove_file(file_path)
                {
                    error!(
                        error = %e,
                        path = %file_path.display(),
                        "Failed to remove local recording after S3 upload"
                    );
                }

                Ok(key)
            }
            Err(e) => {
                error!(
                    error = %e,
                    bucket = %self.config.bucket,
                    key = %key,
                    "Failed to upload recording to S3"
                );
                Err(format!("S3 upload failed: {}", e))
            }
        }
    }

    fn name(&self) -> &str {
        "s3"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_s3_storage_config() {
        let config = S3StorageConfig {
            bucket: "my-bucket".to_string(),
            key_prefix: "recordings/".to_string(),
            delete_local: false,
        };
        assert_eq!(config.bucket, "my-bucket");
        assert_eq!(config.key_prefix, "recordings/");
        assert!(!config.delete_local);
    }

    #[test]
    fn test_recording_info_default() {
        let info = RecordingInfo::default();
        assert!(info.call_id.is_empty());
        assert!(info.caller.is_none());
        assert!(info.callee.is_none());
        assert_eq!(info.duration_ms, 0);
        assert!(info.started_at.is_empty());
    }

    #[test]
    fn test_recording_info_with_values() {
        let info = RecordingInfo {
            call_id: "call-123".to_string(),
            caller: Some("sip:alice@example.com".to_string()),
            callee: Some("sip:bob@example.com".to_string()),
            duration_ms: 30000,
            started_at: "2024-01-15T10:30:00Z".to_string(),
        };
        assert_eq!(info.call_id, "call-123");
        assert_eq!(info.caller.as_deref(), Some("sip:alice@example.com"));
        assert_eq!(info.duration_ms, 30000);
    }
}
