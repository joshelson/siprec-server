//! RTP packet header parsing.

/// Parsed RTP packet header.
#[derive(Debug, Clone)]
pub struct RtpHeader {
    /// RTP version (should be 2).
    pub version: u8,
    /// Padding flag.
    pub padding: bool,
    /// Extension header present.
    pub extension: bool,
    /// CSRC count.
    pub csrc_count: u8,
    /// Marker bit.
    pub marker: bool,
    /// Payload type.
    pub payload_type: u8,
    /// Sequence number.
    pub sequence: u16,
    /// Timestamp.
    pub timestamp: u32,
    /// Synchronization source identifier.
    pub ssrc: u32,
    /// Header length in bytes (including CSRC and extension).
    pub header_len: usize,
}

impl RtpHeader {
    /// Parse RTP header from bytes.
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < 12 {
            return None;
        }

        let version = (data[0] >> 6) & 0x03;
        if version != 2 {
            return None;
        }

        let padding = (data[0] >> 5) & 0x01 != 0;
        let extension = (data[0] >> 4) & 0x01 != 0;
        let csrc_count = data[0] & 0x0F;
        let marker = (data[1] >> 7) & 0x01 != 0;
        let payload_type = data[1] & 0x7F;
        let sequence = u16::from_be_bytes([data[2], data[3]]);
        let timestamp = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
        let ssrc = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);

        let mut header_len = 12 + (csrc_count as usize * 4);

        if extension {
            if data.len() < header_len + 4 {
                return None;
            }
            let ext_len = u16::from_be_bytes([data[header_len + 2], data[header_len + 3]]) as usize;
            header_len += 4 + (ext_len * 4);
        }

        if data.len() < header_len {
            return None;
        }

        Some(Self {
            version,
            padding,
            extension,
            csrc_count,
            marker,
            payload_type,
            sequence,
            timestamp,
            ssrc,
            header_len,
        })
    }

    /// Get payload from packet using this header.
    ///
    /// Per RFC 3550 Section 5.1, if the padding flag is set, the last byte
    /// of the payload indicates the number of padding bytes to remove.
    /// The padding count MUST include itself and be > 0.
    pub fn payload<'a>(&self, data: &'a [u8]) -> &'a [u8] {
        let mut payload = &data[self.header_len..];

        if self.padding && !payload.is_empty() {
            let padding_len = payload[payload.len() - 1] as usize;
            // RFC 3550: padding count must be > 0 and <= payload length
            if padding_len > 0 && padding_len <= payload.len() {
                payload = &payload[..payload.len() - padding_len];
            }
        }

        payload
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal_header() {
        let data = [
            0x80, 0x00, // V=2, PT=0
            0x00, 0x01, // Seq=1
            0x00, 0x00, 0x00, 0xA0, // Timestamp=160
            0x12, 0x34, 0x56, 0x78, // SSRC
        ];

        let header = RtpHeader::parse(&data).unwrap();
        assert_eq!(header.version, 2);
        assert!(!header.padding);
        assert!(!header.extension);
        assert_eq!(header.csrc_count, 0);
        assert!(!header.marker);
        assert_eq!(header.payload_type, 0);
        assert_eq!(header.sequence, 1);
        assert_eq!(header.timestamp, 160);
        assert_eq!(header.ssrc, 0x12345678);
        assert_eq!(header.header_len, 12);
    }

    #[test]
    fn test_parse_with_csrc() {
        let data = vec![
            0x82, 0x00, // V=2, CC=2
            0x00, 0x01, // Seq=1
            0x00, 0x00, 0x00, 0xA0, // Timestamp
            0x12, 0x34, 0x56, 0x78, // SSRC
            0xAA, 0xBB, 0xCC, 0xDD, // CSRC 1
            0x11, 0x22, 0x33, 0x44, // CSRC 2
        ];

        let header = RtpHeader::parse(&data).unwrap();
        assert_eq!(header.csrc_count, 2);
        assert_eq!(header.header_len, 20);
    }

    #[test]
    fn test_parse_with_extension() {
        let data = [
            0x90, 0x00, // V=2, X=1
            0x00, 0x01, // Seq=1
            0x00, 0x00, 0x00, 0xA0, // Timestamp
            0x12, 0x34, 0x56, 0x78, // SSRC
            0xBE, 0xDE, // Extension profile
            0x00, 0x01, // Extension length (1 word)
            0x11, 0x22, 0x33, 0x44, // Extension data
        ];

        let header = RtpHeader::parse(&data).unwrap();
        assert!(header.extension);
        assert_eq!(header.header_len, 20);
    }

    #[test]
    fn test_parse_invalid() {
        // Too short
        assert!(RtpHeader::parse(&[0x80, 0x00]).is_none());

        // Wrong version
        let v1 = [
            0x40, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0xA0, 0x12, 0x34, 0x56, 0x78,
        ];
        assert!(RtpHeader::parse(&v1).is_none());
    }

    #[test]
    fn test_payload_extraction() {
        let data = [
            0x80, 0x00, // V=2, PT=0
            0x00, 0x01, // Seq
            0x00, 0x00, 0x00, 0xA0, // Timestamp
            0x12, 0x34, 0x56, 0x78, // SSRC
            0xFF, 0xFE, 0xFD, // Payload
        ];

        let header = RtpHeader::parse(&data).unwrap();
        let payload = header.payload(&data);
        assert_eq!(payload, &[0xFF, 0xFE, 0xFD]);
    }

    #[test]
    fn test_payload_with_padding() {
        let data = [
            0xA0, 0x00, // V=2, P=1
            0x00, 0x01, // Seq
            0x00, 0x00, 0x00, 0xA0, // Timestamp
            0x12, 0x34, 0x56, 0x78, // SSRC
            0xFF, 0xFF, 0xFF, // Payload
            0x02, // Padding length
        ];

        let header = RtpHeader::parse(&data).unwrap();
        let payload = header.payload(&data);
        assert_eq!(payload.len(), 2);
    }

    #[test]
    fn test_payload_with_zero_padding_byte() {
        // Edge case: padding flag set but last byte is 0 (invalid per RFC 3550)
        let data = [
            0xA0, 0x00, // V=2, P=1
            0x00, 0x01, 0x00, 0x00, 0x00, 0xA0, 0x12, 0x34, 0x56, 0x78, 0xFF, 0xFF,
            0xFF, // Payload
            0x00, // Invalid padding length (0)
        ];

        let header = RtpHeader::parse(&data).unwrap();
        let payload = header.payload(&data);
        // Should not strip anything since padding_len == 0 is invalid
        assert_eq!(payload.len(), 4);
    }

    #[test]
    fn test_payload_type_pcma() {
        let data = [
            0x80, 0x08, // V=2, PT=8 (PCMA)
            0x00, 0x01, 0x00, 0x00, 0x00, 0xA0, 0x12, 0x34, 0x56, 0x78,
        ];

        let header = RtpHeader::parse(&data).unwrap();
        assert_eq!(header.payload_type, 8);
    }

    #[test]
    fn test_marker_bit() {
        let data = [
            0x80, 0x80, // V=2, M=1
            0x00, 0x01, 0x00, 0x00, 0x00, 0xA0, 0x12, 0x34, 0x56, 0x78,
        ];

        let header = RtpHeader::parse(&data).unwrap();
        assert!(header.marker);
    }
}
