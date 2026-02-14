//! Multi-backend composite storage.
//!
//! Forwards recordings to multiple storage backends simultaneously.
//! Useful for writing to local disk AND uploading to S3, or sending
//! to both S3 and a webhook for real-time processing.

use async_trait::async_trait;
use std::path::Path;
use tracing::{error, info, warn};

use super::{RecordingInfo, StorageBackend, StorageResult};

/// Composite storage that writes to multiple backends.
///
/// All backends are invoked concurrently. The operation succeeds if
/// at least one backend succeeds (best-effort semantics). Use
/// [`MultiStorage::strict`] if all backends must succeed.
pub struct MultiStorage {
    backends: Vec<Box<dyn StorageBackend>>,
    strict: bool,
}

impl MultiStorage {
    /// Create a multi-backend storage with best-effort semantics.
    ///
    /// Returns success if at least one backend succeeds.
    pub fn new(backends: Vec<Box<dyn StorageBackend>>) -> Self {
        Self {
            backends,
            strict: false,
        }
    }

    /// Create a multi-backend storage with strict semantics.
    ///
    /// Returns an error if any backend fails.
    pub fn strict(backends: Vec<Box<dyn StorageBackend>>) -> Self {
        Self {
            backends,
            strict: true,
        }
    }
}

#[async_trait]
impl StorageBackend for MultiStorage {
    async fn store(&self, file_path: &Path, info: &RecordingInfo) -> StorageResult {
        let mut results = Vec::with_capacity(self.backends.len());
        let mut successes = Vec::new();
        let mut failures = Vec::new();

        // Run all backends sequentially (file may be deleted by a backend
        // with delete_local=true, so ordering matters).
        for backend in &self.backends {
            match backend.store(file_path, info).await {
                Ok(location) => {
                    info!(
                        backend = backend.name(),
                        location = %location,
                        "Multi-storage: backend succeeded"
                    );
                    successes.push(location.clone());
                    results.push(Ok(location));
                }
                Err(e) => {
                    warn!(
                        backend = backend.name(),
                        error = %e,
                        "Multi-storage: backend failed"
                    );
                    failures.push(format!("{}: {}", backend.name(), e));
                    results.push(Err(e));
                }
            }
        }

        if self.strict && !failures.is_empty() {
            error!(
                failures = ?failures,
                "Multi-storage strict mode: some backends failed"
            );
            return Err(format!("Storage failures: {}", failures.join("; ")));
        }

        if successes.is_empty() {
            return Err(format!(
                "All storage backends failed: {}",
                failures.join("; ")
            ));
        }

        Ok(successes.join(", "))
    }

    fn name(&self) -> &str {
        "multi"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test backend that always succeeds.
    struct SuccessBackend(&'static str);

    #[async_trait]
    impl StorageBackend for SuccessBackend {
        async fn store(&self, _path: &Path, _info: &RecordingInfo) -> StorageResult {
            Ok(format!("{}://stored", self.0))
        }
        fn name(&self) -> &str {
            self.0
        }
    }

    /// Test backend that always fails.
    struct FailBackend(&'static str);

    #[async_trait]
    impl StorageBackend for FailBackend {
        async fn store(&self, _path: &Path, _info: &RecordingInfo) -> StorageResult {
            Err(format!("{} failed", self.0))
        }
        fn name(&self) -> &str {
            self.0
        }
    }

    #[tokio::test]
    async fn test_multi_all_succeed() {
        let multi = MultiStorage::new(vec![
            Box::new(SuccessBackend("a")),
            Box::new(SuccessBackend("b")),
        ]);
        let info = RecordingInfo::default();
        let result = multi.store(Path::new("/tmp/test.wav"), &info).await;
        assert!(result.is_ok());
        let loc = result.unwrap();
        assert!(loc.contains("a://stored"));
        assert!(loc.contains("b://stored"));
    }

    #[tokio::test]
    async fn test_multi_partial_failure_best_effort() {
        let multi = MultiStorage::new(vec![
            Box::new(SuccessBackend("a")),
            Box::new(FailBackend("b")),
        ]);
        let info = RecordingInfo::default();
        let result = multi.store(Path::new("/tmp/test.wav"), &info).await;
        // Best-effort: succeeds because at least one backend worked
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_multi_partial_failure_strict() {
        let multi = MultiStorage::strict(vec![
            Box::new(SuccessBackend("a")),
            Box::new(FailBackend("b")),
        ]);
        let info = RecordingInfo::default();
        let result = multi.store(Path::new("/tmp/test.wav"), &info).await;
        // Strict: fails because one backend failed
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_multi_all_fail() {
        let multi = MultiStorage::new(vec![Box::new(FailBackend("a")), Box::new(FailBackend("b"))]);
        let info = RecordingInfo::default();
        let result = multi.store(Path::new("/tmp/test.wav"), &info).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_multi_storage_name() {
        let multi = MultiStorage::new(vec![]);
        assert_eq!(multi.name(), "multi");
    }
}
