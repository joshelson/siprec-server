//! Network abstractions for dependency injection and testing.

use async_trait::async_trait;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;

/// Trait for async UDP socket operations.
///
/// This abstraction allows mocking UDP sockets in tests without
/// requiring actual network I/O.
#[async_trait]
pub trait AsyncUdpSocket: Send + Sync {
    /// Send data to the specified address.
    async fn send_to(&self, buf: &[u8], target: SocketAddr) -> io::Result<usize>;

    /// Receive data and the sender's address (blocks until data available).
    async fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)>;

    /// Try to receive data without blocking.
    fn try_recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)>;

    /// Get the local address this socket is bound to.
    fn local_addr(&self) -> io::Result<SocketAddr>;
}

#[async_trait]
impl AsyncUdpSocket for UdpSocket {
    async fn send_to(&self, buf: &[u8], target: SocketAddr) -> io::Result<usize> {
        UdpSocket::send_to(self, buf, target).await
    }

    async fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        UdpSocket::recv_from(self, buf).await
    }

    fn try_recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        UdpSocket::try_recv_from(self, buf)
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        UdpSocket::local_addr(self)
    }
}

#[async_trait]
impl AsyncUdpSocket for Arc<UdpSocket> {
    async fn send_to(&self, buf: &[u8], target: SocketAddr) -> io::Result<usize> {
        UdpSocket::send_to(self, buf, target).await
    }

    async fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        UdpSocket::recv_from(self, buf).await
    }

    fn try_recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        UdpSocket::try_recv_from(self, buf)
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        UdpSocket::local_addr(self)
    }
}

/// Mock UDP socket for testing.
#[derive(Debug)]
pub struct MockUdpSocket {
    local_addr: SocketAddr,
    sent_packets: std::sync::Mutex<Vec<(Vec<u8>, SocketAddr)>>,
    recv_queue: std::sync::Mutex<Vec<(Vec<u8>, SocketAddr)>>,
    block_recv: std::sync::atomic::AtomicBool,
}

impl MockUdpSocket {
    /// Create a new mock socket bound to the given address.
    pub fn new(local_addr: SocketAddr) -> Self {
        Self {
            local_addr,
            sent_packets: std::sync::Mutex::new(Vec::new()),
            recv_queue: std::sync::Mutex::new(Vec::new()),
            block_recv: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Create a mock socket with a default local address.
    pub fn new_default() -> Self {
        Self::new("127.0.0.1:5060".parse().unwrap())
    }

    /// Queue a packet to be returned by recv_from.
    pub fn queue_recv(&self, data: Vec<u8>, from: SocketAddr) {
        self.recv_queue.lock().unwrap().push((data, from));
    }

    /// Set whether recv should block forever.
    pub fn set_block_recv(&self, block: bool) {
        self.block_recv
            .store(block, std::sync::atomic::Ordering::SeqCst);
    }

    /// Get all packets that were sent.
    pub fn get_sent_packets(&self) -> Vec<(Vec<u8>, SocketAddr)> {
        self.sent_packets.lock().unwrap().clone()
    }

    /// Clear the sent packets list.
    pub fn clear_sent(&self) {
        self.sent_packets.lock().unwrap().clear();
    }

    /// Get the number of packets sent.
    pub fn sent_count(&self) -> usize {
        self.sent_packets.lock().unwrap().len()
    }
}

#[async_trait]
impl AsyncUdpSocket for MockUdpSocket {
    async fn send_to(&self, buf: &[u8], target: SocketAddr) -> io::Result<usize> {
        self.sent_packets
            .lock()
            .unwrap()
            .push((buf.to_vec(), target));
        Ok(buf.len())
    }

    async fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        if self.block_recv.load(std::sync::atomic::Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        self.try_recv_from(buf)
    }

    fn try_recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        let mut queue = self.recv_queue.lock().unwrap();
        let packet = if queue.is_empty() {
            None
        } else {
            Some(queue.remove(0))
        };
        drop(queue);

        match packet {
            Some((data, from)) => {
                let len = data.len().min(buf.len());
                buf[..len].copy_from_slice(&data[..len]);
                Ok((len, from))
            }
            None => Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "No packets in mock queue",
            )),
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.local_addr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_socket_send() {
        let socket = MockUdpSocket::new_default();
        let target: SocketAddr = "192.168.1.1:5060".parse().unwrap();

        let data = b"test message";
        let len = socket.send_to(data, target).await.unwrap();

        assert_eq!(len, data.len());
        let sent = socket.get_sent_packets();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, data.to_vec());
        assert_eq!(sent[0].1, target);
    }

    #[tokio::test]
    async fn test_mock_socket_recv() {
        let socket = MockUdpSocket::new_default();
        let from: SocketAddr = "192.168.1.1:5060".parse().unwrap();

        socket.queue_recv(b"response".to_vec(), from);

        let mut buf = [0u8; 1024];
        let (len, recv_from) = socket.recv_from(&mut buf).await.unwrap();

        assert_eq!(len, 8);
        assert_eq!(&buf[..len], b"response");
        assert_eq!(recv_from, from);
    }

    #[tokio::test]
    async fn test_mock_socket_recv_empty_queue() {
        let socket = MockUdpSocket::new_default();

        let mut buf = [0u8; 1024];
        let result = socket.recv_from(&mut buf).await;

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::WouldBlock);
    }

    #[tokio::test]
    async fn test_mock_socket_local_addr() {
        let addr: SocketAddr = "10.0.0.1:5080".parse().unwrap();
        let socket = MockUdpSocket::new(addr);

        assert_eq!(socket.local_addr().unwrap(), addr);
    }

    #[tokio::test]
    async fn test_mock_socket_fifo_recv() {
        let socket = MockUdpSocket::new_default();
        let from: SocketAddr = "192.168.1.1:5060".parse().unwrap();

        socket.queue_recv(b"first".to_vec(), from);
        socket.queue_recv(b"second".to_vec(), from);

        let mut buf = [0u8; 1024];

        let (len, _) = socket.recv_from(&mut buf).await.unwrap();
        assert_eq!(&buf[..len], b"first");

        let (len, _) = socket.recv_from(&mut buf).await.unwrap();
        assert_eq!(&buf[..len], b"second");
    }

    #[tokio::test]
    async fn test_mock_socket_clear_sent() {
        let socket = MockUdpSocket::new_default();
        let target: SocketAddr = "192.168.1.1:5060".parse().unwrap();

        socket.send_to(b"msg", target).await.unwrap();
        assert_eq!(socket.sent_count(), 1);

        socket.clear_sent();
        assert_eq!(socket.sent_count(), 0);
    }
}
