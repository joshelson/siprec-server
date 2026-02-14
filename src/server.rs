//! Main SIPREC recording server.
//!
//! Accepts SIPREC INVITEs from the SBC, manages recording sessions,
//! and handles the SIP dialog lifecycle (INVITE -> ACK -> BYE).

use dashmap::DashMap;
use rsip::{Method, SipMessage};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};
use tracing::{debug, error, info, warn};

use crate::error::Result;
use crate::recording::SessionRecorder;
use crate::rtp::RtpReceiver;
use crate::session::RecordingSession;
use crate::sip::message::{
    build_100_trying, build_200_ok, build_200_ok_bye, extract_call_id, extract_from_tag,
};
use crate::sip::sdp::{build_sdp_answer, parse_sdp};
use crate::sip::transport::SipTransport;
use crate::siprec::metadata::parse_metadata;
use crate::siprec::multipart::parse_multipart;
use crate::storage::{RecordingInfo, StorageBackend};

/// Server configuration.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// SIP listen port.
    pub sip_port: u16,
    /// Start of RTP port range.
    pub rtp_port_min: u16,
    /// End of RTP port range.
    pub rtp_port_max: u16,
    /// Directory for recording files.
    pub recording_dir: String,
    /// Local IP address for SDP answers.
    pub local_ip: String,
}

/// The SIPREC recording server.
pub struct SiprecServer {
    config: ServerConfig,
    sessions: Arc<DashMap<String, RecordingSession>>,
    next_rtp_port: AtomicU16,
    storage: Arc<dyn StorageBackend>,
}

impl SiprecServer {
    /// Create a new SIPREC server with the given storage backend.
    pub fn new(config: ServerConfig, storage: Arc<dyn StorageBackend>) -> Self {
        let next_rtp_port = AtomicU16::new(config.rtp_port_min);
        info!(storage = storage.name(), "Using storage backend");
        Self {
            config,
            sessions: Arc::new(DashMap::new()),
            next_rtp_port,
            storage,
        }
    }

    /// Run the server main loop.
    pub async fn run(&self) -> Result<()> {
        let transport = SipTransport::bind(self.config.sip_port).await?;

        info!(
            sip_port = self.config.sip_port,
            rtp_range = %format!("{}-{}", self.config.rtp_port_min, self.config.rtp_port_max),
            recording_dir = %self.config.recording_dir,
            local_ip = %self.config.local_ip,
            storage = self.storage.name(),
            "SIPREC server started"
        );

        loop {
            match transport.recv().await {
                Ok((SipMessage::Request(request), from_addr)) => match request.method {
                    Method::Invite => {
                        if let Err(e) = self.handle_invite(&transport, &request, from_addr).await {
                            error!(error = %e, "Failed to handle INVITE");
                        }
                    }
                    Method::Ack => {
                        let call_id = extract_call_id(&request).unwrap_or_default();
                        debug!(call_id = %call_id, "Received ACK");
                    }
                    Method::Bye => {
                        if let Err(e) = self.handle_bye(&transport, &request, from_addr).await {
                            error!(error = %e, "Failed to handle BYE");
                        }
                    }
                    method => {
                        debug!(method = ?method, "Ignoring unsupported SIP method");
                    }
                },
                Ok((SipMessage::Response(_response), _addr)) => {
                    debug!("Ignoring unexpected SIP response");
                }
                Err(e) => {
                    error!(error = %e, "Failed to receive SIP message");
                }
            }
        }
    }

    /// Handle an incoming SIPREC INVITE.
    async fn handle_invite<S: crate::net::AsyncUdpSocket>(
        &self,
        transport: &SipTransport<S>,
        request: &rsip::Request,
        from_addr: SocketAddr,
    ) -> Result<()> {
        let call_id = extract_call_id(request).unwrap_or_else(|| "unknown".to_string());
        let from_tag = extract_from_tag(request).unwrap_or_default();

        info!(call_id = %call_id, from = %from_addr, "Received SIPREC INVITE");

        // Send 100 Trying immediately per RFC 3261 Section 8.2.6
        let trying = build_100_trying(request);
        transport.send_response(&trying, from_addr).await?;

        // Extract Content-Type and body
        let content_type = request
            .headers
            .iter()
            .find_map(|h| {
                if let rsip::Header::ContentType(ct) = h {
                    Some(ct.to_string())
                } else {
                    None
                }
            })
            .unwrap_or_default();

        let body = std::str::from_utf8(&request.body).unwrap_or("");

        // Parse multipart body
        let multipart = if content_type.contains("multipart") {
            match parse_multipart(&content_type, body) {
                Ok(mp) => Some(mp),
                Err(e) => {
                    warn!(error = %e, "Failed to parse multipart body, trying body as SDP");
                    None
                }
            }
        } else {
            None
        };

        // Extract SDP
        let sdp_text = if let Some(ref mp) = multipart {
            mp.sdp().unwrap_or(body)
        } else {
            body
        };

        // Parse SDP offer
        let sdp_offer = parse_sdp(sdp_text)?;

        // Parse recording metadata (optional)
        let metadata = multipart.as_ref().and_then(|mp| {
            mp.metadata_xml().and_then(|xml| {
                parse_metadata(xml)
                    .map_err(|e| warn!(error = %e, "Failed to parse SIPREC metadata"))
                    .ok()
            })
        });

        // Allocate RTP ports (two per session: caller + callee)
        let caller_port = self.allocate_rtp_port();
        let callee_port = self.allocate_rtp_port();

        info!(
            call_id = %call_id,
            caller_port = caller_port,
            callee_port = callee_port,
            streams = sdp_offer.streams.len(),
            "Allocated RTP ports"
        );

        // Build SDP answer
        let rtp_ports: Vec<u16> = vec![caller_port, callee_port];
        let sdp_answer = build_sdp_answer(&sdp_offer, &self.config.local_ip, &rtp_ports);

        // Build and send 200 OK
        let contact = format!(
            "<sip:siprec@{}:{}>",
            self.config.local_ip, self.config.sip_port
        );
        let response = build_200_ok(request, &sdp_answer, &contact);
        transport.send_response(&response, from_addr).await?;

        // Create RTP receivers
        let caller_rtp = RtpReceiver::bind(caller_port).await?;
        let callee_rtp = RtpReceiver::bind(callee_port).await?;

        // Create recorder
        let filename = format!(
            "{}_{}",
            call_id,
            jiff::Zoned::now().strftime("%Y%m%d_%H%M%S")
        );
        let recorder = match SessionRecorder::new(&self.config.recording_dir, &call_id, &filename) {
            Some(r) => r,
            None => {
                error!(call_id = %call_id, "Failed to create recorder");
                return Ok(());
            }
        };

        // Create and start session
        let mut session = RecordingSession::new(
            call_id.clone(),
            from_tag,
            from_addr,
            metadata,
            caller_rtp,
            callee_rtp,
            recorder,
        );
        session.start_receiving();

        self.sessions.insert(call_id.clone(), session);

        info!(
            call_id = %call_id,
            active_sessions = self.sessions.len(),
            "Recording session started"
        );

        Ok(())
    }

    /// Handle a BYE request - finalize and store recording.
    async fn handle_bye<S: crate::net::AsyncUdpSocket>(
        &self,
        transport: &SipTransport<S>,
        request: &rsip::Request,
        from_addr: SocketAddr,
    ) -> Result<()> {
        let call_id = extract_call_id(request).unwrap_or_else(|| "unknown".to_string());

        info!(call_id = %call_id, "Received BYE");

        // Send 200 OK for BYE
        let response = build_200_ok_bye(request);
        transport.send_response(&response, from_addr).await?;

        // Find and remove session
        if let Some((_, session)) = self.sessions.remove(&call_id) {
            let caller = session.caller_aor();
            let callee = session.callee_aor();
            let started_at = session
                .started_at
                .strftime("%Y-%m-%dT%H:%M:%SZ")
                .to_string();

            // Finalize recording
            if let Some(wav_path) = session.finalize().await {
                info!(
                    call_id = %call_id,
                    path = %wav_path.display(),
                    "Recording saved"
                );

                // Compute duration from file size
                let duration_ms = std::fs::metadata(&wav_path)
                    .map(|m| {
                        let audio_bytes = m.len().saturating_sub(44);
                        (audio_bytes * 1000) / 16000
                    })
                    .unwrap_or(0);

                let info = RecordingInfo {
                    call_id: call_id.clone(),
                    caller,
                    callee,
                    duration_ms,
                    started_at,
                };

                // Store via configured backend (in background)
                let storage = Arc::clone(&self.storage);
                let path = wav_path.clone();
                tokio::spawn(async move {
                    match storage.store(&path, &info).await {
                        Ok(location) => {
                            tracing::info!(
                                location = %location,
                                backend = storage.name(),
                                "Recording stored"
                            );
                        }
                        Err(e) => {
                            tracing::error!(
                                error = %e,
                                backend = storage.name(),
                                "Failed to store recording"
                            );
                        }
                    }
                });
            }

            info!(
                call_id = %call_id,
                active_sessions = self.sessions.len(),
                "Recording session ended"
            );
        } else {
            warn!(call_id = %call_id, "BYE for unknown session");
        }

        Ok(())
    }

    /// Allocate the next available RTP port (even-numbered).
    fn allocate_rtp_port(&self) -> u16 {
        loop {
            let port = self.next_rtp_port.fetch_add(2, Ordering::SeqCst);
            if port >= self.config.rtp_port_max {
                // Wrap around
                self.next_rtp_port
                    .store(self.config.rtp_port_min, Ordering::SeqCst);
                return self.config.rtp_port_min;
            }
            // Ensure even port (RTP convention)
            if port.is_multiple_of(2) {
                return port;
            }
        }
    }

    /// Get the number of active sessions.
    pub fn active_sessions(&self) -> usize {
        self.sessions.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::LocalStorage;

    fn test_config() -> ServerConfig {
        ServerConfig {
            sip_port: 5060,
            rtp_port_min: 20000,
            rtp_port_max: 30000,
            recording_dir: "/tmp/test-recordings".to_string(),
            local_ip: "10.0.0.1".to_string(),
        }
    }

    #[test]
    fn test_server_creation() {
        let storage: Arc<dyn StorageBackend> = Arc::new(LocalStorage);
        let server = SiprecServer::new(test_config(), storage);
        assert_eq!(server.active_sessions(), 0);
    }

    #[test]
    fn test_rtp_port_allocation() {
        let storage: Arc<dyn StorageBackend> = Arc::new(LocalStorage);
        let server = SiprecServer::new(test_config(), storage);

        let port1 = server.allocate_rtp_port();
        let port2 = server.allocate_rtp_port();

        assert_eq!(port1, 20000);
        assert_eq!(port2, 20002);
        assert_eq!(port1 % 2, 0); // Even
        assert_eq!(port2 % 2, 0); // Even
    }

    #[test]
    fn test_rtp_port_wrap() {
        let config = ServerConfig {
            rtp_port_min: 20000,
            rtp_port_max: 20004,
            ..test_config()
        };
        let storage: Arc<dyn StorageBackend> = Arc::new(LocalStorage);
        let server = SiprecServer::new(config, storage);

        let p1 = server.allocate_rtp_port();
        let p2 = server.allocate_rtp_port();
        let p3 = server.allocate_rtp_port(); // Should wrap

        assert_eq!(p1, 20000);
        assert_eq!(p2, 20002);
        assert_eq!(p3, 20000); // Wrapped
    }
}
