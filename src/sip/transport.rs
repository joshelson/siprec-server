//! SIP UDP transport for UAS (inbound server) role.
//!
//! Adapted from dialer-core's transport, simplified for a recording server
//! that only receives requests and sends responses.

use rsip::{Request, Response, SipMessage};
use std::convert::TryFrom;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tracing::{debug, error, trace, warn};

use crate::error::{Result, SipError};
use crate::net::AsyncUdpSocket;
use crate::pcap::{Direction as PcapDirection, PcapWriter};

/// SIP transport over UDP (UAS role).
pub struct SipTransport<S: AsyncUdpSocket = Arc<UdpSocket>> {
    socket: S,
    local_addr: SocketAddr,
    pcap: Option<Arc<PcapWriter>>,
}

impl SipTransport<Arc<UdpSocket>> {
    /// Create a new SIP transport bound to the given port.
    pub async fn bind(port: u16) -> Result<Self> {
        Self::bind_with_pcap(port, None).await
    }

    /// Create a new SIP transport with optional PCAP capture.
    pub async fn bind_with_pcap(port: u16, pcap: Option<Arc<PcapWriter>>) -> Result<Self> {
        let addr = format!("0.0.0.0:{}", port);
        let socket = UdpSocket::bind(&addr).await?;
        let local_addr = socket.local_addr()?;

        debug!(local_addr = %local_addr, "SIP transport bound");

        Ok(Self {
            socket: Arc::new(socket),
            local_addr,
            pcap,
        })
    }
}

impl<S: AsyncUdpSocket> SipTransport<S> {
    /// Create a new SIP transport from an existing socket (for testing).
    pub fn from_socket(socket: S, pcap: Option<Arc<PcapWriter>>) -> Result<Self> {
        let local_addr = socket.local_addr()?;
        Ok(Self {
            socket,
            local_addr,
            pcap,
        })
    }

    /// Get the local address.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Get a reference to the underlying socket.
    pub fn socket_ref(&self) -> &S {
        &self.socket
    }

    /// Get a reference to the PCAP writer.
    pub fn pcap(&self) -> Option<&Arc<PcapWriter>> {
        self.pcap.as_ref()
    }

    /// Send a SIP response.
    pub async fn send_response(&self, response: &Response, addr: SocketAddr) -> Result<()> {
        let msg_str = response.to_string();
        let bytes = msg_str.as_bytes();

        trace!(
            status = response.status_code.code(),
            addr = %addr,
            len = bytes.len(),
            "Sending SIP response"
        );

        if let Some(pcap) = &self.pcap {
            pcap.write_sip(PcapDirection::Sent, addr, bytes);
        }

        self.socket
            .send_to(bytes, addr)
            .await
            .map_err(|e| SipError::SendFailed(e.to_string()))?;
        Ok(())
    }

    /// Receive the next SIP message.
    pub async fn recv(&self) -> Result<(SipMessage, SocketAddr)> {
        let mut buf = vec![0u8; 65535];

        loop {
            let (len, addr) = self
                .socket
                .recv_from(&mut buf)
                .await
                .map_err(|e| SipError::ReceiveFailed(e.to_string()))?;

            let data = &buf[..len];

            if let Some(pcap) = &self.pcap {
                pcap.write_sip(PcapDirection::Received, addr, data);
            }

            match std::str::from_utf8(data) {
                Ok(text) => {
                    trace!(addr = %addr, len = len, "Received SIP data");

                    if text.starts_with("SIP/2.0") {
                        match Response::try_from(text.to_string()) {
                            Ok(response) => {
                                debug!(
                                    status = response.status_code.code(),
                                    addr = %addr,
                                    "Received SIP response"
                                );
                                return Ok((SipMessage::Response(response), addr));
                            }
                            Err(e) => {
                                error!(error = ?e, "Failed to parse SIP response");
                                continue;
                            }
                        }
                    } else {
                        match Request::try_from(text.to_string()) {
                            Ok(request) => {
                                debug!(
                                    method = ?request.method,
                                    addr = %addr,
                                    "Received SIP request"
                                );
                                return Ok((SipMessage::Request(request), addr));
                            }
                            Err(e) => {
                                error!(error = ?e, "Failed to parse SIP request");
                                continue;
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!(error = %e, "Received non-UTF8 data on SIP port");
                    continue;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::MockUdpSocket;

    fn create_sip_invite() -> Vec<u8> {
        b"INVITE sip:recorder@10.0.0.1:5060 SIP/2.0\r\n\
Via: SIP/2.0/UDP 192.168.1.1:5060;branch=z9hG4bK-test\r\n\
From: <sip:caller@example.com>;tag=from-tag\r\n\
To: <sip:recorder@10.0.0.1>\r\n\
Call-ID: test-call-123\r\n\
CSeq: 1 INVITE\r\n\
Content-Length: 0\r\n\
\r\n"
            .to_vec()
    }

    fn create_sip_bye() -> Vec<u8> {
        b"BYE sip:recorder@10.0.0.1:5060 SIP/2.0\r\n\
Via: SIP/2.0/UDP 192.168.1.1:5060;branch=z9hG4bK-bye\r\n\
From: <sip:caller@example.com>;tag=from-tag\r\n\
To: <sip:recorder@10.0.0.1>;tag=to-tag\r\n\
Call-ID: test-call-123\r\n\
CSeq: 2 BYE\r\n\
Content-Length: 0\r\n\
\r\n"
            .to_vec()
    }

    #[tokio::test]
    async fn test_transport_from_mock() {
        let local_addr: SocketAddr = "10.0.0.1:5060".parse().unwrap();
        let mock = MockUdpSocket::new(local_addr);
        let transport = SipTransport::from_socket(mock, None).unwrap();
        assert_eq!(transport.local_addr(), local_addr);
    }

    #[tokio::test]
    async fn test_recv_invite() {
        let local_addr: SocketAddr = "10.0.0.1:5060".parse().unwrap();
        let remote_addr: SocketAddr = "192.168.1.1:5060".parse().unwrap();
        let mock = MockUdpSocket::new(local_addr);

        mock.queue_recv(create_sip_invite(), remote_addr);

        let transport = SipTransport::from_socket(mock, None).unwrap();
        let (message, from) = transport.recv().await.unwrap();
        assert_eq!(from, remote_addr);

        match message {
            SipMessage::Request(req) => {
                assert_eq!(req.method, rsip::Method::Invite);
            }
            _ => panic!("Expected INVITE request"),
        }
    }

    #[tokio::test]
    async fn test_recv_bye() {
        let local_addr: SocketAddr = "10.0.0.1:5060".parse().unwrap();
        let remote_addr: SocketAddr = "192.168.1.1:5060".parse().unwrap();
        let mock = MockUdpSocket::new(local_addr);

        mock.queue_recv(create_sip_bye(), remote_addr);

        let transport = SipTransport::from_socket(mock, None).unwrap();
        let (message, from) = transport.recv().await.unwrap();
        assert_eq!(from, remote_addr);

        match message {
            SipMessage::Request(req) => {
                assert_eq!(req.method, rsip::Method::Bye);
            }
            _ => panic!("Expected BYE request"),
        }
    }

    #[tokio::test]
    async fn test_send_response() {
        let local_addr: SocketAddr = "10.0.0.1:5060".parse().unwrap();
        let remote_addr: SocketAddr = "192.168.1.1:5060".parse().unwrap();
        let mock = MockUdpSocket::new(local_addr);

        let transport = SipTransport::from_socket(mock, None).unwrap();

        let response = Response {
            status_code: rsip::StatusCode::OK,
            version: rsip::Version::V2,
            headers: Default::default(),
            body: vec![],
        };

        transport
            .send_response(&response, remote_addr)
            .await
            .unwrap();
        assert_eq!(transport.socket_ref().sent_count(), 1);
    }
}
