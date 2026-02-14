//! HTTP webhook storage backend.
//!
//! Uploads completed recordings to any HTTP endpoint via multipart POST
//! or PUT. This is the most flexible backend — it works with any REST API,
//! webhook receiver, or custom recording management system.

use async_trait::async_trait;
use reqwest::multipart;
use std::path::Path;
use tracing::{error, info};

use super::{RecordingInfo, StorageBackend, StorageResult};

/// HTTP upload method.
#[derive(Debug, Clone)]
pub enum HttpMethod {
    /// Multipart POST (most common for file uploads).
    Post,
    /// PUT with raw body (S3-compatible pre-signed URLs, etc.).
    Put,
}

/// HTTP webhook storage configuration.
#[derive(Debug, Clone)]
pub struct HttpStorageConfig {
    /// The endpoint URL to upload recordings to.
    pub url: String,
    /// HTTP method to use.
    pub method: HttpMethod,
    /// Optional authorization header value (e.g., `"Bearer <token>"`).
    pub auth_header: Option<String>,
    /// Whether to delete the local file after successful upload.
    pub delete_local: bool,
}

/// HTTP webhook storage backend.
pub struct HttpStorage {
    client: reqwest::Client,
    config: HttpStorageConfig,
}

impl HttpStorage {
    /// Create a new HTTP storage backend.
    pub fn new(config: HttpStorageConfig) -> Self {
        let client = reqwest::Client::new();
        Self { client, config }
    }
}

#[async_trait]
impl StorageBackend for HttpStorage {
    async fn store(&self, file_path: &Path, info: &RecordingInfo) -> StorageResult {
        let file_bytes = tokio::fs::read(file_path)
            .await
            .map_err(|e| format!("Failed to read file: {}", e))?;

        let filename = file_path
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or("recording.wav")
            .to_string();

        let result = match self.config.method {
            HttpMethod::Post => {
                let file_part = multipart::Part::bytes(file_bytes)
                    .file_name(filename.clone())
                    .mime_str("audio/wav")
                    .map_err(|e| format!("Invalid MIME type: {}", e))?;

                let form = multipart::Form::new()
                    .text("call_id", info.call_id.clone())
                    .text("duration_ms", info.duration_ms.to_string())
                    .text("started_at", info.started_at.clone())
                    .text("caller", info.caller.clone().unwrap_or_default())
                    .text("callee", info.callee.clone().unwrap_or_default())
                    .part("file", file_part);

                let mut req = self.client.post(&self.config.url).multipart(form);

                if let Some(ref auth) = self.config.auth_header {
                    req = req.header("Authorization", auth);
                }

                req.send().await
            }
            HttpMethod::Put => {
                let mut req = self
                    .client
                    .put(&self.config.url)
                    .header("Content-Type", "audio/wav")
                    .header("X-Call-ID", &info.call_id)
                    .header("X-Duration-Ms", info.duration_ms.to_string())
                    .header("X-Started-At", &info.started_at)
                    .body(file_bytes);

                if let Some(ref caller) = info.caller {
                    req = req.header("X-Caller", caller);
                }
                if let Some(ref callee) = info.callee {
                    req = req.header("X-Callee", callee);
                }
                if let Some(ref auth) = self.config.auth_header {
                    req = req.header("Authorization", auth);
                }

                req.send().await
            }
        };

        match result {
            Ok(resp) if resp.status().is_success() => {
                info!(
                    url = %self.config.url,
                    call_id = %info.call_id,
                    status = %resp.status(),
                    "Recording uploaded via HTTP"
                );

                if self.config.delete_local
                    && let Err(e) = tokio::fs::remove_file(file_path).await
                {
                    error!(error = %e, "Failed to remove local file after HTTP upload");
                }

                Ok(format!("{}#{}", self.config.url, filename))
            }
            Ok(resp) => {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                error!(
                    url = %self.config.url,
                    status = %status,
                    body = %body,
                    "HTTP upload failed with non-success status"
                );
                Err(format!("HTTP upload failed: {} {}", status, body))
            }
            Err(e) => {
                error!(
                    url = %self.config.url,
                    error = %e,
                    "HTTP upload request failed"
                );
                Err(format!("HTTP upload failed: {}", e))
            }
        }
    }

    fn name(&self) -> &str {
        "http"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_storage_config() {
        let config = HttpStorageConfig {
            url: "https://api.example.com/recordings".to_string(),
            method: HttpMethod::Post,
            auth_header: Some("Bearer tok123".to_string()),
            delete_local: false,
        };
        assert_eq!(config.url, "https://api.example.com/recordings");
        assert!(!config.delete_local);
    }

    #[test]
    fn test_http_storage_name() {
        let storage = HttpStorage::new(HttpStorageConfig {
            url: "https://example.com".to_string(),
            method: HttpMethod::Post,
            auth_header: None,
            delete_local: false,
        });
        assert_eq!(storage.name(), "http");
    }
}
