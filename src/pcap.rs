//! PCAP packet capture for SIP and RTP analysis in Wireshark.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, info};

/// Protocol type for packet capture.
#[derive(Debug, Clone, Copy)]
pub enum Protocol {
    Sip,
    Rtp,
}

/// Direction of the packet.
#[derive(Debug, Clone, Copy)]
pub enum Direction {
    Sent,
    Received,
}

/// PCAP file writer with synthetic Ethernet/IP/UDP headers.
pub struct PcapWriter {
    writer: Mutex<BufWriter<File>>,
    local_addr: SocketAddr,
    enabled: AtomicBool,
    path: PathBuf,
}

impl PcapWriter {
    /// Create a new PCAP writer.
    pub fn new<P: AsRef<Path>>(path: P, local_addr: SocketAddr) -> std::io::Result<Self> {
        let file = File::create(path.as_ref())?;
        let mut writer = BufWriter::new(file);

        // Write PCAP global header
        writer.write_all(&0xa1b2c3d4_u32.to_le_bytes())?; // Magic
        writer.write_all(&2_u16.to_le_bytes())?; // Version major
        writer.write_all(&4_u16.to_le_bytes())?; // Version minor
        writer.write_all(&0_i32.to_le_bytes())?; // Timezone
        writer.write_all(&0_u32.to_le_bytes())?; // Timestamp accuracy
        writer.write_all(&65535_u32.to_le_bytes())?; // Snapshot length
        writer.write_all(&1_u32.to_le_bytes())?; // Link-layer (Ethernet)

        writer.flush()?;

        info!(path = %path.as_ref().display(), "PCAP capture started");

        Ok(Self {
            writer: Mutex::new(writer),
            local_addr,
            path: path.as_ref().to_path_buf(),
            enabled: AtomicBool::new(true),
        })
    }

    /// Check if capture is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// Disable capture.
    pub fn disable(&self) {
        self.enabled.store(false, Ordering::Relaxed);
        if let Ok(mut writer) = self.writer.lock() {
            let _ = writer.flush();
        }
    }

    /// Get the path to the PCAP file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Finalize and return the path.
    pub fn finalize(&self) -> PathBuf {
        self.disable();
        self.path.clone()
    }

    /// Write a packet to the PCAP file.
    pub fn write_packet(
        &self,
        direction: Direction,
        remote_addr: SocketAddr,
        protocol: Protocol,
        data: &[u8],
    ) {
        if !self.is_enabled() {
            return;
        }

        let (src_addr, dst_addr) = match direction {
            Direction::Sent => (self.local_addr, remote_addr),
            Direction::Received => (remote_addr, self.local_addr),
        };

        let packet = build_packet(src_addr, dst_addr, data);

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let ts_sec = now.as_secs() as u32;
        let ts_usec = now.subsec_micros();

        if let Ok(mut writer) = self.writer.lock() {
            let _ = writer.write_all(&ts_sec.to_le_bytes());
            let _ = writer.write_all(&ts_usec.to_le_bytes());
            let _ = writer.write_all(&(packet.len() as u32).to_le_bytes());
            let _ = writer.write_all(&(packet.len() as u32).to_le_bytes());
            let _ = writer.write_all(&packet);
            let _ = writer.flush();

            debug!(
                direction = ?direction,
                protocol = ?protocol,
                len = data.len(),
                "Captured packet"
            );
        }
    }

    /// Write a SIP packet.
    pub fn write_sip(&self, direction: Direction, remote_addr: SocketAddr, data: &[u8]) {
        self.write_packet(direction, remote_addr, Protocol::Sip, data);
    }

    /// Write an RTP packet.
    pub fn write_rtp(&self, direction: Direction, remote_addr: SocketAddr, data: &[u8]) {
        self.write_packet(direction, remote_addr, Protocol::Rtp, data);
    }
}

impl Drop for PcapWriter {
    fn drop(&mut self) {
        if let Ok(mut writer) = self.writer.lock() {
            let _ = writer.flush();
        }
        debug!("PCAP capture stopped");
    }
}

/// Shared PCAP writer type.
pub type SharedPcapWriter = Option<Arc<PcapWriter>>;

/// Build a complete Ethernet + IP + UDP packet.
fn build_packet(src_addr: SocketAddr, dst_addr: SocketAddr, payload: &[u8]) -> Vec<u8> {
    let mut packet = Vec::with_capacity(14 + 20 + 8 + payload.len());

    // Ethernet header (14 bytes)
    packet.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x01]); // Dst MAC
    packet.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x02]); // Src MAC
    packet.extend_from_slice(&[0x08, 0x00]); // EtherType IPv4

    // IPv4 header (20 bytes)
    let ip_total_len = 20 + 8 + payload.len() as u16;

    packet.push(0x45); // Version + IHL
    packet.push(0x00); // DSCP + ECN
    packet.extend_from_slice(&ip_total_len.to_be_bytes());
    packet.extend_from_slice(&[0x00, 0x00]); // Identification
    packet.extend_from_slice(&[0x40, 0x00]); // Don't fragment
    packet.push(64); // TTL
    packet.push(17); // Protocol = UDP
    let checksum_pos = packet.len();
    packet.extend_from_slice(&[0x00, 0x00]); // Checksum placeholder

    match src_addr.ip() {
        IpAddr::V4(ip) => packet.extend_from_slice(&ip.octets()),
        IpAddr::V6(_) => packet.extend_from_slice(&[127, 0, 0, 1]),
    }
    match dst_addr.ip() {
        IpAddr::V4(ip) => packet.extend_from_slice(&ip.octets()),
        IpAddr::V6(_) => packet.extend_from_slice(&[127, 0, 0, 1]),
    }

    // Calculate IP header checksum
    let ip_header = &packet[14..14 + 20];
    let checksum = ip_checksum(ip_header);
    packet[checksum_pos] = (checksum >> 8) as u8;
    packet[checksum_pos + 1] = (checksum & 0xff) as u8;

    // UDP header (8 bytes)
    let udp_len = 8 + payload.len() as u16;
    packet.extend_from_slice(&src_addr.port().to_be_bytes());
    packet.extend_from_slice(&dst_addr.port().to_be_bytes());
    packet.extend_from_slice(&udp_len.to_be_bytes());
    packet.extend_from_slice(&[0x00, 0x00]); // Checksum disabled

    // Payload
    packet.extend_from_slice(payload);

    packet
}

/// Calculate IP header checksum.
fn ip_checksum(header: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    for i in (0..header.len()).step_by(2) {
        let word = if i + 1 < header.len() {
            ((header[i] as u32) << 8) | (header[i + 1] as u32)
        } else {
            (header[i] as u32) << 8
        };
        if i != 10 {
            sum += word;
        }
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !sum as u16
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, SocketAddrV4};

    #[test]
    fn test_build_packet() {
        let src = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 100), 5060));
        let dst = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 200), 5060));
        let payload = b"INVITE sip:test@example.com SIP/2.0\r\n\r\n";

        let packet = build_packet(src, dst, payload);

        assert_eq!(&packet[12..14], &[0x08, 0x00]); // IPv4
        assert_eq!(packet[14], 0x45); // Version 4, IHL 5
        assert_eq!(packet[23], 17); // UDP
        assert_eq!(&packet[26..30], &[192, 168, 1, 100]); // Src IP
        assert_eq!(&packet[30..34], &[192, 168, 1, 200]); // Dst IP

        let src_port = u16::from_be_bytes([packet[34], packet[35]]);
        let dst_port = u16::from_be_bytes([packet[36], packet[37]]);
        assert_eq!(src_port, 5060);
        assert_eq!(dst_port, 5060);
        assert_eq!(&packet[42..], payload);
    }

    #[test]
    fn test_ip_checksum_consistency() {
        let header = [
            0x45, 0x00, 0x00, 0x28, 0x00, 0x01, 0x00, 0x00, 0x40, 0x11, 0x00, 0x00, 0xC0, 0xA8,
            0x01, 0x01, 0xC0, 0xA8, 0x01, 0x02,
        ];
        let c1 = ip_checksum(&header);
        let c2 = ip_checksum(&header);
        assert_eq!(c1, c2);
        assert_ne!(c1, 0);
    }

    #[test]
    fn test_protocol_debug() {
        assert!(format!("{:?}", Protocol::Sip).contains("Sip"));
        assert!(format!("{:?}", Protocol::Rtp).contains("Rtp"));
    }

    #[test]
    fn test_direction_debug() {
        assert!(format!("{:?}", Direction::Sent).contains("Sent"));
        assert!(format!("{:?}", Direction::Received).contains("Received"));
    }
}
