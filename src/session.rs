//! Recording session management.
//!
//! A `RecordingSession` ties together the SIP dialog, two RTP receivers
//! (caller + callee), and the stereo WAV recorder for a single SIPREC call.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use crate::net::AsyncUdpSocket;
use crate::recording::SessionRecorder;
use crate::rtp::RtpReceiver;
use crate::siprec::RecordingMetadata;

/// A single SIPREC recording session, generic over socket type.
pub struct RecordingSession<S: AsyncUdpSocket + Send + 'static = Arc<UdpSocket>> {
    /// SIP Call-ID for this session.
    pub call_id: String,
    /// SIP From tag.
    pub from_tag: String,
    /// Remote SIP address (SBC).
    pub remote_addr: SocketAddr,
    /// SIPREC metadata (participants, streams).
    pub metadata: Option<RecordingMetadata>,
    /// Caller RTP receiver (audio stream for caller side).
    caller_rtp: Arc<Mutex<RtpReceiver<S>>>,
    /// Callee RTP receiver (audio stream for callee side).
    callee_rtp: Arc<Mutex<RtpReceiver<S>>>,
    /// Stereo WAV recorder.
    recorder: Arc<Mutex<Option<SessionRecorder>>>,
    /// Caller RTP receive task.
    caller_task: Option<JoinHandle<Vec<u8>>>,
    /// Callee RTP receive task.
    callee_task: Option<JoinHandle<Vec<u8>>>,
    /// Timestamp when the session started.
    pub started_at: jiff::Zoned,
}

impl<S: AsyncUdpSocket + Send + 'static> RecordingSession<S> {
    /// Create a new recording session.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        call_id: String,
        from_tag: String,
        remote_addr: SocketAddr,
        metadata: Option<RecordingMetadata>,
        caller_rtp: RtpReceiver<S>,
        callee_rtp: RtpReceiver<S>,
        recorder: SessionRecorder,
    ) -> Self {
        Self {
            call_id,
            from_tag,
            remote_addr,
            metadata,
            caller_rtp: Arc::new(Mutex::new(caller_rtp)),
            callee_rtp: Arc::new(Mutex::new(callee_rtp)),
            recorder: Arc::new(Mutex::new(Some(recorder))),
            caller_task: None,
            callee_task: None,
            started_at: jiff::Zoned::now(),
        }
    }

    /// Start receiving RTP audio on both streams.
    pub fn start_receiving(&mut self) {
        let caller_rtp = Arc::clone(&self.caller_rtp);
        let callee_rtp = Arc::clone(&self.callee_rtp);
        let call_id = self.call_id.clone();

        // Spawn caller RTP receive task
        let cid = call_id.clone();
        self.caller_task = Some(tokio::spawn(async move {
            receive_loop(caller_rtp, &cid, "caller").await
        }));

        // Spawn callee RTP receive task
        self.callee_task = Some(tokio::spawn(async move {
            receive_loop(callee_rtp, &call_id, "callee").await
        }));

        info!(call_id = %self.call_id, "Started RTP receivers for caller and callee");
    }

    /// Stop the session, finalize recording, and return the WAV file path.
    pub async fn finalize(mut self) -> Option<PathBuf> {
        info!(call_id = %self.call_id, "Finalizing recording session");

        // Abort RTP tasks and collect audio
        let caller_audio = if let Some(task) = self.caller_task.take() {
            task.abort();
            // Get whatever audio was accumulated
            let rtp = self.caller_rtp.lock().await;
            rtp.get_audio().to_vec()
        } else {
            let rtp = self.caller_rtp.lock().await;
            rtp.get_audio().to_vec()
        };

        let callee_audio = if let Some(task) = self.callee_task.take() {
            task.abort();
            let rtp = self.callee_rtp.lock().await;
            rtp.get_audio().to_vec()
        } else {
            let rtp = self.callee_rtp.lock().await;
            rtp.get_audio().to_vec()
        };

        debug!(
            call_id = %self.call_id,
            caller_bytes = caller_audio.len(),
            callee_bytes = callee_audio.len(),
            "Collected audio from RTP receivers"
        );

        // Write audio to recorder and finalize
        let mut recorder_guard = self.recorder.lock().await;
        if let Some(mut recorder) = recorder_guard.take() {
            recorder.write_caller_audio(&caller_audio);
            recorder.write_callee_audio(&callee_audio);
            let path = recorder.finalize();
            Some(path)
        } else {
            warn!(call_id = %self.call_id, "No recorder available");
            None
        }
    }

    /// Get session duration in milliseconds.
    pub fn duration_ms(&self) -> u64 {
        let now = jiff::Zoned::now();
        let span = now.since(&self.started_at);
        match span {
            Ok(s) => s.get_milliseconds().unsigned_abs(),
            Err(_) => 0,
        }
    }

    /// Get caller participant AOR from metadata.
    pub fn caller_aor(&self) -> Option<String> {
        self.metadata.as_ref().and_then(|m| {
            m.streams
                .iter()
                .find(|s| s.label.as_deref() == Some("caller") || s.label.as_deref() == Some("1"))
                .and_then(|s| m.participants.iter().find(|p| p.id == s.participant_id))
                .and_then(|p| p.aor.clone())
        })
    }

    /// Get callee participant AOR from metadata.
    pub fn callee_aor(&self) -> Option<String> {
        self.metadata.as_ref().and_then(|m| {
            m.streams
                .iter()
                .find(|s| s.label.as_deref() == Some("callee") || s.label.as_deref() == Some("2"))
                .and_then(|s| m.participants.iter().find(|p| p.id == s.participant_id))
                .and_then(|p| p.aor.clone())
        })
    }

    /// Get caller participant name from metadata.
    pub fn caller_name(&self) -> Option<String> {
        self.metadata.as_ref().and_then(|m| {
            m.streams
                .iter()
                .find(|s| s.label.as_deref() == Some("caller") || s.label.as_deref() == Some("1"))
                .and_then(|s| m.participants.iter().find(|p| p.id == s.participant_id))
                .and_then(|p| p.name.clone())
        })
    }

    /// Get callee participant name from metadata.
    pub fn callee_name(&self) -> Option<String> {
        self.metadata.as_ref().and_then(|m| {
            m.streams
                .iter()
                .find(|s| s.label.as_deref() == Some("callee") || s.label.as_deref() == Some("2"))
                .and_then(|s| m.participants.iter().find(|p| p.id == s.participant_id))
                .and_then(|p| p.name.clone())
        })
    }
}

/// RTP receive loop - runs until cancelled, returns accumulated audio.
async fn receive_loop<S: AsyncUdpSocket + Send + 'static>(
    rtp: Arc<Mutex<RtpReceiver<S>>>,
    call_id: &str,
    label: &str,
) -> Vec<u8> {
    let mut total_bytes = 0u64;
    loop {
        let result = {
            let mut receiver = rtp.lock().await;
            receiver.recv_audio().await
        };
        match result {
            Ok(bytes) => {
                total_bytes += bytes as u64;
            }
            Err(e) => {
                // WouldBlock is expected when using mock sockets
                let err_str = e.to_string();
                if err_str.contains("WouldBlock") || err_str.contains("would block") {
                    debug!(
                        call_id = call_id,
                        label = label,
                        total_bytes = total_bytes,
                        "RTP receive ended (no more packets)"
                    );
                    break;
                }
                error!(
                    call_id = call_id,
                    label = label,
                    error = %e,
                    "RTP receive error"
                );
                break;
            }
        }
    }

    let receiver = rtp.lock().await;
    receiver.get_audio().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::MockUdpSocket;
    use crate::siprec::metadata::{Participant, StreamAssociation};
    use tempfile::tempdir;

    fn make_rtp_receiver(port: u16) -> RtpReceiver<MockUdpSocket> {
        let local_addr: std::net::SocketAddr = format!("10.0.0.1:{}", port).parse().unwrap();
        let mock = MockUdpSocket::new(local_addr);
        RtpReceiver::from_socket(mock, port, None)
    }

    fn make_metadata() -> RecordingMetadata {
        RecordingMetadata {
            session_id: "test-session".to_string(),
            participants: vec![
                Participant {
                    id: "p1".to_string(),
                    aor: Some("sip:+15551234567@example.com".to_string()),
                    name: Some("Alice".to_string()),
                },
                Participant {
                    id: "p2".to_string(),
                    aor: Some("sip:+15559876543@example.com".to_string()),
                    name: Some("Bob".to_string()),
                },
            ],
            streams: vec![
                StreamAssociation {
                    stream_id: "s1".to_string(),
                    participant_id: "p1".to_string(),
                    label: Some("caller".to_string()),
                },
                StreamAssociation {
                    stream_id: "s2".to_string(),
                    participant_id: "p2".to_string(),
                    label: Some("callee".to_string()),
                },
            ],
        }
    }

    #[test]
    fn test_caller_aor() {
        let dir = tempdir().unwrap();
        let recorder = SessionRecorder::new(dir.path().to_str().unwrap(), "test", "test").unwrap();
        let session = RecordingSession::new(
            "call-1".to_string(),
            "tag-1".to_string(),
            "10.0.0.1:5060".parse().unwrap(),
            Some(make_metadata()),
            make_rtp_receiver(20000),
            make_rtp_receiver(20002),
            recorder,
        );

        assert_eq!(
            session.caller_aor().as_deref(),
            Some("sip:+15551234567@example.com")
        );
    }

    #[test]
    fn test_callee_aor() {
        let dir = tempdir().unwrap();
        let recorder = SessionRecorder::new(dir.path().to_str().unwrap(), "test", "test").unwrap();
        let session = RecordingSession::new(
            "call-1".to_string(),
            "tag-1".to_string(),
            "10.0.0.1:5060".parse().unwrap(),
            Some(make_metadata()),
            make_rtp_receiver(20000),
            make_rtp_receiver(20002),
            recorder,
        );

        assert_eq!(
            session.callee_aor().as_deref(),
            Some("sip:+15559876543@example.com")
        );
    }

    #[test]
    fn test_caller_callee_name() {
        let dir = tempdir().unwrap();
        let recorder = SessionRecorder::new(dir.path().to_str().unwrap(), "test", "test").unwrap();
        let session = RecordingSession::new(
            "call-1".to_string(),
            "tag-1".to_string(),
            "10.0.0.1:5060".parse().unwrap(),
            Some(make_metadata()),
            make_rtp_receiver(20000),
            make_rtp_receiver(20002),
            recorder,
        );

        assert_eq!(session.caller_name().as_deref(), Some("Alice"));
        assert_eq!(session.callee_name().as_deref(), Some("Bob"));
    }

    #[test]
    fn test_aor_with_no_metadata() {
        let dir = tempdir().unwrap();
        let recorder = SessionRecorder::new(dir.path().to_str().unwrap(), "test", "test").unwrap();
        let session = RecordingSession::new(
            "call-1".to_string(),
            "tag-1".to_string(),
            "10.0.0.1:5060".parse().unwrap(),
            None,
            make_rtp_receiver(20000),
            make_rtp_receiver(20002),
            recorder,
        );

        assert!(session.caller_aor().is_none());
        assert!(session.callee_aor().is_none());
        assert!(session.caller_name().is_none());
        assert!(session.callee_name().is_none());
    }

    #[test]
    fn test_aor_with_numeric_labels() {
        let dir = tempdir().unwrap();
        let recorder = SessionRecorder::new(dir.path().to_str().unwrap(), "test", "test").unwrap();

        let metadata = RecordingMetadata {
            session_id: "s1".to_string(),
            participants: vec![
                Participant {
                    id: "p1".to_string(),
                    aor: Some("sip:caller@test.com".to_string()),
                    name: None,
                },
                Participant {
                    id: "p2".to_string(),
                    aor: Some("sip:callee@test.com".to_string()),
                    name: None,
                },
            ],
            streams: vec![
                StreamAssociation {
                    stream_id: "s1".to_string(),
                    participant_id: "p1".to_string(),
                    label: Some("1".to_string()),
                },
                StreamAssociation {
                    stream_id: "s2".to_string(),
                    participant_id: "p2".to_string(),
                    label: Some("2".to_string()),
                },
            ],
        };

        let session = RecordingSession::new(
            "call-1".to_string(),
            "tag-1".to_string(),
            "10.0.0.1:5060".parse().unwrap(),
            Some(metadata),
            make_rtp_receiver(20000),
            make_rtp_receiver(20002),
            recorder,
        );

        assert_eq!(session.caller_aor().as_deref(), Some("sip:caller@test.com"));
        assert_eq!(session.callee_aor().as_deref(), Some("sip:callee@test.com"));
    }

    #[test]
    fn test_duration_ms() {
        let dir = tempdir().unwrap();
        let recorder = SessionRecorder::new(dir.path().to_str().unwrap(), "test", "test").unwrap();
        let session = RecordingSession::new(
            "call-1".to_string(),
            "tag-1".to_string(),
            "10.0.0.1:5060".parse().unwrap(),
            None,
            make_rtp_receiver(20000),
            make_rtp_receiver(20002),
            recorder,
        );

        // Duration should be very small since we just created it
        let duration = session.duration_ms();
        assert!(
            duration < 1000,
            "Duration should be < 1s, got {}ms",
            duration
        );
    }

    #[tokio::test]
    async fn test_finalize_produces_wav() {
        let dir = tempdir().unwrap();
        let recorder =
            SessionRecorder::new(dir.path().to_str().unwrap(), "test", "finalize-test").unwrap();
        let session = RecordingSession::new(
            "call-1".to_string(),
            "tag-1".to_string(),
            "10.0.0.1:5060".parse().unwrap(),
            None,
            make_rtp_receiver(20000),
            make_rtp_receiver(20002),
            recorder,
        );

        let path = session.finalize().await;
        assert!(path.is_some());
        let path = path.unwrap();
        assert!(path.exists());
        assert!(path.extension().is_some_and(|ext| ext == "wav"));
    }

    #[tokio::test]
    async fn test_finalize_with_no_audio() {
        let dir = tempdir().unwrap();
        let recorder =
            SessionRecorder::new(dir.path().to_str().unwrap(), "test", "empty-test").unwrap();
        let session = RecordingSession::new(
            "call-1".to_string(),
            "tag-1".to_string(),
            "10.0.0.1:5060".parse().unwrap(),
            None,
            make_rtp_receiver(20000),
            make_rtp_receiver(20002),
            recorder,
        );

        // Should produce a valid WAV even with no audio
        let path = session.finalize().await;
        assert!(path.is_some());
        let path = path.unwrap();
        let file_meta = std::fs::metadata(&path).unwrap();
        // Should have at least the WAV header (44 bytes)
        assert!(file_meta.len() >= 44);
    }

    #[test]
    fn test_session_fields() {
        let dir = tempdir().unwrap();
        let recorder = SessionRecorder::new(dir.path().to_str().unwrap(), "test", "test").unwrap();
        let addr: std::net::SocketAddr = "10.0.0.1:5060".parse().unwrap();
        let session = RecordingSession::new(
            "call-abc".to_string(),
            "tag-xyz".to_string(),
            addr,
            None,
            make_rtp_receiver(20000),
            make_rtp_receiver(20002),
            recorder,
        );

        assert_eq!(session.call_id, "call-abc");
        assert_eq!(session.from_tag, "tag-xyz");
        assert_eq!(session.remote_addr, addr);
        assert!(session.metadata.is_none());
    }
}
