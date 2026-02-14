//! Receive-only RTP stream receiver for SIPREC recording.

use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tracing::{debug, trace, warn};

use super::header::RtpHeader;
use crate::error::{Result, RtpError};
use crate::net::AsyncUdpSocket;
use crate::pcap::{Direction as PcapDirection, PcapWriter};

/// Receive-only RTP session for recording a single audio stream.
///
/// Unlike dialer-core's RtpSession, this has no jitter buffer, no send
/// capability, and no codec conversion. We simply accumulate raw audio
/// bytes in the order received for WAV recording.
pub struct RtpReceiver<S: AsyncUdpSocket = Arc<UdpSocket>> {
    socket: S,
    local_port: u16,
    remote_addr: Option<SocketAddr>,
    expected_ssrc: Option<u32>,
    audio_buffer: Vec<u8>,
    packets_received: u64,
    pcap: Option<Arc<PcapWriter>>,
}

impl RtpReceiver<Arc<UdpSocket>> {
    /// Create a new RTP receiver bound to the given port.
    pub async fn bind(port: u16) -> Result<Self> {
        Self::bind_with_pcap(port, None).await
    }

    /// Create a new RTP receiver with optional PCAP capture.
    pub async fn bind_with_pcap(port: u16, pcap: Option<Arc<PcapWriter>>) -> Result<Self> {
        let addr = format!("0.0.0.0:{}", port);
        let socket = UdpSocket::bind(&addr).await.map_err(|e| {
            RtpError::ParseError(format!("Failed to bind RTP port {}: {}", port, e))
        })?;
        let local_addr = socket.local_addr()?;

        debug!(bind_addr = %addr, local_addr = %local_addr, "RTP receiver bound");

        Ok(Self {
            socket: Arc::new(socket),
            local_port: port,
            remote_addr: None,
            expected_ssrc: None,
            audio_buffer: Vec::with_capacity(480_000), // ~60 seconds at 8kHz
            packets_received: 0,
            pcap,
        })
    }
}

impl<S: AsyncUdpSocket> RtpReceiver<S> {
    /// Create an RTP receiver from an existing socket (for testing).
    pub fn from_socket(socket: S, port: u16, pcap: Option<Arc<PcapWriter>>) -> Self {
        Self {
            socket,
            local_port: port,
            remote_addr: None,
            expected_ssrc: None,
            audio_buffer: Vec::with_capacity(480_000),
            packets_received: 0,
            pcap,
        }
    }

    /// Get the local port.
    pub fn local_port(&self) -> u16 {
        self.local_port
    }

    /// Set the expected remote address.
    pub fn set_remote_addr(&mut self, addr: SocketAddr) {
        self.remote_addr = Some(addr);
        debug!(remote = %addr, port = self.local_port, "RTP receiver remote set");
    }

    /// Receive the next RTP packet and append audio to buffer.
    ///
    /// Returns the number of audio bytes received.
    pub async fn recv_audio(&mut self) -> Result<usize> {
        let mut buf = [0u8; 2048];

        let (len, addr) = self.socket.recv_from(&mut buf).await?;

        // Validate source IP if we have an expected remote
        if let Some(expected) = self.remote_addr {
            if addr.ip() != expected.ip() {
                warn!(from = %addr, expected = %expected, "Ignoring RTP from unexpected IP");
                return Ok(0);
            }
        } else {
            // Learn remote from first packet
            self.remote_addr = Some(addr);
            debug!(remote = %addr, "Learned RTP remote address");
        }

        let data = &buf[..len];

        // Capture to PCAP
        if let Some(ref pcap) = self.pcap {
            pcap.write_rtp(PcapDirection::Received, addr, data);
        }

        // Parse RTP header
        let header = RtpHeader::parse(data)
            .ok_or_else(|| RtpError::ParseError("Invalid RTP header".to_string()))?;

        // SSRC validation
        if let Some(expected_ssrc) = self.expected_ssrc {
            if header.ssrc != expected_ssrc {
                trace!(expected = expected_ssrc, got = header.ssrc, "Wrong SSRC");
                return Ok(0);
            }
        } else {
            self.expected_ssrc = Some(header.ssrc);
            debug!(ssrc = header.ssrc, "Learned RTP SSRC");
        }

        self.packets_received += 1;

        // Extract payload - only accept voice codecs
        let payload = header.payload(data);
        match header.payload_type {
            0 | 8 => {
                // PCMU or PCMA - store raw bytes
                self.audio_buffer.extend_from_slice(payload);
                trace!(
                    seq = header.sequence,
                    pt = header.payload_type,
                    bytes = payload.len(),
                    total = self.audio_buffer.len(),
                    "Received audio"
                );
                Ok(payload.len())
            }
            pt => {
                trace!(pt = pt, "Ignoring non-voice payload type");
                Ok(0)
            }
        }
    }

    /// Get the accumulated audio buffer.
    pub fn get_audio(&self) -> &[u8] {
        &self.audio_buffer
    }

    /// Take the audio buffer, replacing it with an empty one.
    pub fn take_audio(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.audio_buffer)
    }

    /// Get the number of packets received.
    pub fn packets_received(&self) -> u64 {
        self.packets_received
    }

    /// Get the audio duration in milliseconds (assumes 8kHz).
    pub fn audio_duration_ms(&self) -> u64 {
        (self.audio_buffer.len() as u64 * 1000) / 8000
    }

    /// Get a reference to the underlying socket.
    pub fn socket_ref(&self) -> &S {
        &self.socket
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::MockUdpSocket;

    fn create_rtp_packet(seq: u16, timestamp: u32, ssrc: u32, payload: &[u8]) -> Vec<u8> {
        let mut packet = vec![
            0x80,
            0x00, // V=2, PT=0 (PCMU)
            (seq >> 8) as u8,
            seq as u8,
            (timestamp >> 24) as u8,
            (timestamp >> 16) as u8,
            (timestamp >> 8) as u8,
            timestamp as u8,
            (ssrc >> 24) as u8,
            (ssrc >> 16) as u8,
            (ssrc >> 8) as u8,
            ssrc as u8,
        ];
        packet.extend_from_slice(payload);
        packet
    }

    #[tokio::test]
    async fn test_receiver_from_mock() {
        let local_addr: SocketAddr = "10.0.0.1:20000".parse().unwrap();
        let mock = MockUdpSocket::new(local_addr);
        let receiver = RtpReceiver::from_socket(mock, 20000, None);
        assert_eq!(receiver.local_port(), 20000);
        assert_eq!(receiver.packets_received(), 0);
        assert!(receiver.get_audio().is_empty());
    }

    #[tokio::test]
    async fn test_receive_single_packet() {
        let local_addr: SocketAddr = "10.0.0.1:20000".parse().unwrap();
        let remote_addr: SocketAddr = "192.168.1.1:30000".parse().unwrap();
        let mock = MockUdpSocket::new(local_addr);

        let payload = vec![0xFF; 160]; // 20ms of silence
        let rtp = create_rtp_packet(1, 0, 0xAABBCCDD, &payload);
        mock.queue_recv(rtp, remote_addr);

        let mut receiver = RtpReceiver::from_socket(mock, 20000, None);
        receiver.set_remote_addr(remote_addr);

        let bytes = receiver.recv_audio().await.unwrap();
        assert_eq!(bytes, 160);
        assert_eq!(receiver.packets_received(), 1);
        assert_eq!(receiver.get_audio().len(), 160);
        assert_eq!(receiver.audio_duration_ms(), 20);
    }

    #[tokio::test]
    async fn test_receive_multiple_packets() {
        let local_addr: SocketAddr = "10.0.0.1:20000".parse().unwrap();
        let remote_addr: SocketAddr = "192.168.1.1:30000".parse().unwrap();
        let mock = MockUdpSocket::new(local_addr);

        for i in 0..5 {
            let payload = vec![0xFF; 160];
            let rtp = create_rtp_packet(i, i as u32 * 160, 0xAABBCCDD, &payload);
            mock.queue_recv(rtp, remote_addr);
        }

        let mut receiver = RtpReceiver::from_socket(mock, 20000, None);
        receiver.set_remote_addr(remote_addr);

        for _ in 0..5 {
            receiver.recv_audio().await.unwrap();
        }

        assert_eq!(receiver.packets_received(), 5);
        assert_eq!(receiver.get_audio().len(), 800);
        assert_eq!(receiver.audio_duration_ms(), 100);
    }

    #[tokio::test]
    async fn test_ssrc_validation() {
        let local_addr: SocketAddr = "10.0.0.1:20000".parse().unwrap();
        let remote_addr: SocketAddr = "192.168.1.1:30000".parse().unwrap();
        let mock = MockUdpSocket::new(local_addr);

        // First packet establishes SSRC
        let rtp1 = create_rtp_packet(1, 0, 0xAABBCCDD, &[0xFF; 160]);
        mock.queue_recv(rtp1, remote_addr);

        // Second packet with different SSRC should be ignored
        let rtp2 = create_rtp_packet(2, 160, 0x11223344, &[0xFF; 160]);
        mock.queue_recv(rtp2, remote_addr);

        let mut receiver = RtpReceiver::from_socket(mock, 20000, None);
        receiver.set_remote_addr(remote_addr);

        receiver.recv_audio().await.unwrap();
        let bytes = receiver.recv_audio().await.unwrap();

        assert_eq!(bytes, 0); // Second packet rejected
        assert_eq!(receiver.get_audio().len(), 160); // Only first packet's audio
    }

    #[tokio::test]
    async fn test_take_audio() {
        let local_addr: SocketAddr = "10.0.0.1:20000".parse().unwrap();
        let remote_addr: SocketAddr = "192.168.1.1:30000".parse().unwrap();
        let mock = MockUdpSocket::new(local_addr);

        let rtp = create_rtp_packet(1, 0, 0xAABBCCDD, &[0xFF; 160]);
        mock.queue_recv(rtp, remote_addr);

        let mut receiver = RtpReceiver::from_socket(mock, 20000, None);
        receiver.set_remote_addr(remote_addr);
        receiver.recv_audio().await.unwrap();

        let audio = receiver.take_audio();
        assert_eq!(audio.len(), 160);
        assert!(receiver.get_audio().is_empty()); // Buffer cleared after take
    }

    fn create_rtp_packet_pt(
        seq: u16,
        timestamp: u32,
        ssrc: u32,
        pt: u8,
        payload: &[u8],
    ) -> Vec<u8> {
        let mut packet = vec![
            0x80,
            pt,
            (seq >> 8) as u8,
            seq as u8,
            (timestamp >> 24) as u8,
            (timestamp >> 16) as u8,
            (timestamp >> 8) as u8,
            timestamp as u8,
            (ssrc >> 24) as u8,
            (ssrc >> 16) as u8,
            (ssrc >> 8) as u8,
            ssrc as u8,
        ];
        packet.extend_from_slice(payload);
        packet
    }

    #[tokio::test]
    async fn test_receive_pcma() {
        let local_addr: SocketAddr = "10.0.0.1:20000".parse().unwrap();
        let remote_addr: SocketAddr = "192.168.1.1:30000".parse().unwrap();
        let mock = MockUdpSocket::new(local_addr);

        let payload = vec![0xD5; 160]; // PCMA data
        let rtp = create_rtp_packet_pt(1, 0, 0xAABBCCDD, 8, &payload);
        mock.queue_recv(rtp, remote_addr);

        let mut receiver = RtpReceiver::from_socket(mock, 20000, None);
        receiver.set_remote_addr(remote_addr);

        let bytes = receiver.recv_audio().await.unwrap();
        assert_eq!(bytes, 160);
        assert_eq!(receiver.get_audio().len(), 160);
    }

    #[tokio::test]
    async fn test_ignore_non_voice_payload() {
        let local_addr: SocketAddr = "10.0.0.1:20000".parse().unwrap();
        let remote_addr: SocketAddr = "192.168.1.1:30000".parse().unwrap();
        let mock = MockUdpSocket::new(local_addr);

        // PT 101 = telephone-event, should be ignored
        let rtp = create_rtp_packet_pt(1, 0, 0xAABBCCDD, 101, &[0xFF; 4]);
        mock.queue_recv(rtp, remote_addr);

        let mut receiver = RtpReceiver::from_socket(mock, 20000, None);
        receiver.set_remote_addr(remote_addr);

        let bytes = receiver.recv_audio().await.unwrap();
        assert_eq!(bytes, 0);
        assert!(receiver.get_audio().is_empty());
    }

    #[tokio::test]
    async fn test_learn_remote_from_first_packet() {
        let local_addr: SocketAddr = "10.0.0.1:20000".parse().unwrap();
        let remote_addr: SocketAddr = "192.168.1.1:30000".parse().unwrap();
        let mock = MockUdpSocket::new(local_addr);

        let rtp = create_rtp_packet(1, 0, 0xAABBCCDD, &[0xFF; 160]);
        mock.queue_recv(rtp, remote_addr);

        // Don't set remote_addr - should learn from first packet
        let mut receiver = RtpReceiver::from_socket(mock, 20000, None);

        let bytes = receiver.recv_audio().await.unwrap();
        assert_eq!(bytes, 160);
    }

    #[tokio::test]
    async fn test_audio_duration() {
        let local_addr: SocketAddr = "10.0.0.1:20000".parse().unwrap();
        let remote_addr: SocketAddr = "192.168.1.1:30000".parse().unwrap();
        let mock = MockUdpSocket::new(local_addr);

        // 5 packets * 160 bytes = 800 bytes = 100ms at 8kHz
        for i in 0..5 {
            let rtp = create_rtp_packet(i, i as u32 * 160, 0xAABBCCDD, &[0xFF; 160]);
            mock.queue_recv(rtp, remote_addr);
        }

        let mut receiver = RtpReceiver::from_socket(mock, 20000, None);
        receiver.set_remote_addr(remote_addr);

        for _ in 0..5 {
            receiver.recv_audio().await.unwrap();
        }

        assert_eq!(receiver.audio_duration_ms(), 100);
    }

    #[tokio::test]
    async fn test_wrong_ip_rejected() {
        let local_addr: SocketAddr = "10.0.0.1:20000".parse().unwrap();
        let expected_addr: SocketAddr = "192.168.1.1:30000".parse().unwrap();
        let wrong_addr: SocketAddr = "10.99.99.99:30000".parse().unwrap();
        let mock = MockUdpSocket::new(local_addr);

        let rtp = create_rtp_packet(1, 0, 0xAABBCCDD, &[0xFF; 160]);
        mock.queue_recv(rtp, wrong_addr);

        let mut receiver = RtpReceiver::from_socket(mock, 20000, None);
        receiver.set_remote_addr(expected_addr);

        let bytes = receiver.recv_audio().await.unwrap();
        assert_eq!(bytes, 0);
        assert!(receiver.get_audio().is_empty());
    }
}
