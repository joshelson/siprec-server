//! SDP parsing for multi-stream SIPREC offers and answer generation.
//!
//! SIPREC INVITEs contain SDP with multiple `m=audio` lines, one per
//! recorded stream (caller and callee). Each stream has `a=sendonly`
//! and optionally `a=label:` to identify which participant it carries.
//!
//! Parsing follows RFC 4566 (SDP) and RFC 3264 (offer/answer model).

use crate::error::{Result, SiprecError};

/// Parsed SDP offer from a SIPREC INVITE.
#[derive(Debug, Clone)]
pub struct SdpOffer {
    /// Session-level connection address from the c= line.
    pub connection_addr: String,
    /// Media streams (one per m=audio line).
    pub streams: Vec<MediaStream>,
}

/// A single media stream from the SDP.
#[derive(Debug, Clone)]
pub struct MediaStream {
    /// Media type (always "audio" for SIPREC).
    pub media_type: String,
    /// Remote RTP port.
    pub port: u16,
    /// Payload types offered (e.g., [0, 8, 101] for PCMU, PCMA, telephone-event).
    pub codecs: Vec<u8>,
    /// Label from a=label: attribute (identifies caller vs callee).
    pub label: Option<String>,
    /// Media-level connection address (overrides session-level per RFC 4566 Section 5.7).
    pub connection_addr: Option<String>,
    /// Stream direction attribute.
    pub direction: Option<String>,
}

impl MediaStream {
    /// Get the preferred codec (first voice codec in the list).
    pub fn preferred_codec(&self) -> u8 {
        self.codecs
            .iter()
            .find(|&&pt| pt == 0 || pt == 8)
            .copied()
            .unwrap_or(self.codecs.first().copied().unwrap_or(0))
    }

    /// Get the effective connection address (media-level overrides session-level).
    pub fn effective_addr<'a>(&'a self, session_addr: &'a str) -> &'a str {
        self.connection_addr.as_deref().unwrap_or(session_addr)
    }
}

/// Parse a multi-stream SDP offer per RFC 4566.
pub fn parse_sdp(sdp: &str) -> Result<SdpOffer> {
    let mut session_connection_addr = String::new();
    let mut streams: Vec<MediaStream> = Vec::new();
    let mut current_stream: Option<MediaStream> = None;

    for line in sdp.lines() {
        let line = line.trim();

        if let Some(rest) = line.strip_prefix("c=") {
            // c=IN IP4 192.168.1.1 (RFC 4566 Section 5.7)
            if let Some(addr) = rest.split_whitespace().nth(2) {
                if current_stream.is_some() {
                    // Media-level c= overrides session-level per RFC 4566
                    if let Some(ref mut stream) = current_stream {
                        stream.connection_addr = Some(addr.to_string());
                    }
                } else if session_connection_addr.is_empty() {
                    session_connection_addr = addr.to_string();
                }
            }
        } else if let Some(rest) = line.strip_prefix("m=") {
            // Save previous stream if any
            if let Some(stream) = current_stream.take() {
                streams.push(stream);
            }

            // m=audio 20000 RTP/AVP 0 8 101 (RFC 4566 Section 5.14)
            let parts: Vec<&str> = rest.split_whitespace().collect();
            if parts.len() >= 3 {
                let media_type = parts[0].to_string();
                let port: u16 = parts[1].parse().unwrap_or(0);

                // Parse all payload types from m= line (parts[3..])
                let codecs: Vec<u8> = parts
                    .iter()
                    .skip(3)
                    .filter_map(|p| p.parse().ok())
                    .collect();

                current_stream = Some(MediaStream {
                    media_type,
                    port,
                    codecs,
                    label: None,
                    connection_addr: None,
                    direction: None,
                });
            }
        } else if let Some(rest) = line.strip_prefix("a=label:") {
            if let Some(ref mut stream) = current_stream {
                stream.label = Some(rest.trim().to_string());
            }
        } else if let Some(rest) = line.strip_prefix("a=rtpmap:") {
            // a=rtpmap:0 PCMU/8000 (RFC 4566 Section 6)
            // Validate codec mappings but we mainly use the m= line payload types
            if let Some(ref mut stream) = current_stream
                && let Some(pt) = rest
                    .split_whitespace()
                    .next()
                    .and_then(|pt_str| pt_str.parse::<u8>().ok())
                && !stream.codecs.contains(&pt)
            {
                // Add dynamically mapped codec
                stream.codecs.push(pt);
            }
        } else if (line == "a=sendonly"
            || line == "a=recvonly"
            || line == "a=sendrecv"
            || line == "a=inactive")
            && let Some(ref mut stream) = current_stream
        {
            stream.direction = Some(line.strip_prefix("a=").unwrap_or(line).to_string());
        }
    }

    // Don't forget the last stream
    if let Some(stream) = current_stream {
        streams.push(stream);
    }

    if session_connection_addr.is_empty() {
        // Check if all streams have media-level c= addresses
        let all_have_addr = !streams.is_empty()
            && streams
                .iter()
                .all(|s| s.connection_addr.is_some() || s.port == 0);
        if !all_have_addr {
            return Err(SiprecError::Parse(
                "No connection address in SDP".to_string(),
            ));
        }
    }

    if streams.is_empty() {
        return Err(SiprecError::Parse(
            "No media streams found in SDP".to_string(),
        ));
    }

    Ok(SdpOffer {
        connection_addr: session_connection_addr,
        streams,
    })
}

/// Build a recvonly SDP answer per RFC 3264 (offer/answer model).
///
/// # Arguments
/// * `offer` - Parsed SDP offer
/// * `local_ip` - Our local IP for the c= line
/// * `rtp_ports` - Local RTP port for each stream (must match offer stream count)
pub fn build_sdp_answer(offer: &SdpOffer, local_ip: &str, rtp_ports: &[u16]) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let mut lines = vec![
        "v=0".to_string(),
        format!("o=siprec-server {} {} IN IP4 {}", now, now, local_ip),
        "s=SIPREC Recording".to_string(),
        format!("c=IN IP4 {}", local_ip),
        "t=0 0".to_string(),
    ];

    for (i, stream) in offer.streams.iter().enumerate() {
        let port = rtp_ports.get(i).copied().unwrap_or(0);
        let pt = stream.preferred_codec();

        // Echo all offered codecs in the m= line per RFC 3264
        let codec_list: String = if stream.codecs.is_empty() {
            pt.to_string()
        } else {
            stream
                .codecs
                .iter()
                .map(|c| c.to_string())
                .collect::<Vec<_>>()
                .join(" ")
        };

        lines.push(format!("m=audio {} RTP/AVP {}", port, codec_list));

        // Add rtpmap for each codec
        for &codec in &stream.codecs {
            match codec {
                0 => lines.push(format!("a=rtpmap:{} PCMU/8000", codec)),
                8 => lines.push(format!("a=rtpmap:{} PCMA/8000", codec)),
                // Skip dynamic payload types we don't handle
                _ => {}
            }
        }
        // If codecs was empty, default to PCMU
        if stream.codecs.is_empty() {
            lines.push(format!("a=rtpmap:{} PCMU/8000", pt));
        }

        lines.push("a=recvonly".to_string());
        lines.push("a=ptime:20".to_string());

        if let Some(ref label) = stream.label {
            lines.push(format!("a=label:{}", label));
        }
    }

    lines.join("\r\n") + "\r\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIPREC_SDP: &str = "v=0\r\n\
o=- 12345 12345 IN IP4 192.168.1.1\r\n\
s=-\r\n\
c=IN IP4 192.168.1.1\r\n\
t=0 0\r\n\
m=audio 20000 RTP/AVP 0\r\n\
a=rtpmap:0 PCMU/8000\r\n\
a=sendonly\r\n\
a=label:caller\r\n\
m=audio 20002 RTP/AVP 0\r\n\
a=rtpmap:0 PCMU/8000\r\n\
a=sendonly\r\n\
a=label:callee\r\n";

    #[test]
    fn test_parse_multi_stream_sdp() {
        let offer = parse_sdp(SIPREC_SDP).unwrap();

        assert_eq!(offer.connection_addr, "192.168.1.1");
        assert_eq!(offer.streams.len(), 2);

        assert_eq!(offer.streams[0].port, 20000);
        assert_eq!(offer.streams[0].preferred_codec(), 0);
        assert_eq!(offer.streams[0].label.as_deref(), Some("caller"));
        assert_eq!(offer.streams[0].direction.as_deref(), Some("sendonly"));

        assert_eq!(offer.streams[1].port, 20002);
        assert_eq!(offer.streams[1].preferred_codec(), 0);
        assert_eq!(offer.streams[1].label.as_deref(), Some("callee"));
    }

    #[test]
    fn test_parse_sdp_pcma() {
        let sdp = "v=0\r\n\
c=IN IP4 10.0.0.1\r\n\
t=0 0\r\n\
m=audio 30000 RTP/AVP 8\r\n\
a=rtpmap:8 PCMA/8000\r\n\
a=sendonly\r\n";

        let offer = parse_sdp(sdp).unwrap();
        assert_eq!(offer.streams[0].preferred_codec(), 8);
    }

    #[test]
    fn test_parse_sdp_multi_codec() {
        let sdp = "v=0\r\n\
c=IN IP4 10.0.0.1\r\n\
t=0 0\r\n\
m=audio 30000 RTP/AVP 0 8 101\r\n\
a=rtpmap:0 PCMU/8000\r\n\
a=rtpmap:8 PCMA/8000\r\n\
a=rtpmap:101 telephone-event/8000\r\n\
a=sendonly\r\n";

        let offer = parse_sdp(sdp).unwrap();
        assert_eq!(offer.streams[0].codecs, vec![0, 8, 101]);
        assert_eq!(offer.streams[0].preferred_codec(), 0);
    }

    #[test]
    fn test_parse_sdp_media_level_connection() {
        let sdp = "v=0\r\n\
c=IN IP4 10.0.0.1\r\n\
t=0 0\r\n\
m=audio 20000 RTP/AVP 0\r\n\
c=IN IP4 10.0.0.2\r\n\
a=sendonly\r\n\
m=audio 20002 RTP/AVP 0\r\n\
c=IN IP4 10.0.0.3\r\n\
a=sendonly\r\n";

        let offer = parse_sdp(sdp).unwrap();
        assert_eq!(offer.connection_addr, "10.0.0.1");
        assert_eq!(
            offer.streams[0].connection_addr.as_deref(),
            Some("10.0.0.2")
        );
        assert_eq!(
            offer.streams[1].connection_addr.as_deref(),
            Some("10.0.0.3")
        );
        assert_eq!(
            offer.streams[0].effective_addr(&offer.connection_addr),
            "10.0.0.2"
        );
        assert_eq!(
            offer.streams[1].effective_addr(&offer.connection_addr),
            "10.0.0.3"
        );
    }

    #[test]
    fn test_parse_sdp_media_level_c_only() {
        // No session-level c= but all streams have media-level c=
        let sdp = "v=0\r\n\
t=0 0\r\n\
m=audio 20000 RTP/AVP 0\r\n\
c=IN IP4 10.0.0.2\r\n\
m=audio 20002 RTP/AVP 0\r\n\
c=IN IP4 10.0.0.3\r\n";

        let offer = parse_sdp(sdp).unwrap();
        assert!(offer.connection_addr.is_empty());
        assert_eq!(
            offer.streams[0].connection_addr.as_deref(),
            Some("10.0.0.2")
        );
        assert_eq!(
            offer.streams[1].connection_addr.as_deref(),
            Some("10.0.0.3")
        );
    }

    #[test]
    fn test_parse_sdp_missing_connection() {
        let sdp = "v=0\r\nt=0 0\r\nm=audio 20000 RTP/AVP 0\r\n";
        assert!(parse_sdp(sdp).is_err());
    }

    #[test]
    fn test_parse_sdp_no_media() {
        let sdp = "v=0\r\nc=IN IP4 10.0.0.1\r\nt=0 0\r\n";
        assert!(parse_sdp(sdp).is_err());
    }

    #[test]
    fn test_parse_sdp_direction_attributes() {
        let sdp = "v=0\r\n\
c=IN IP4 10.0.0.1\r\n\
t=0 0\r\n\
m=audio 20000 RTP/AVP 0\r\n\
a=sendonly\r\n\
m=audio 20002 RTP/AVP 0\r\n\
a=recvonly\r\n\
m=audio 20004 RTP/AVP 0\r\n\
a=inactive\r\n";

        let offer = parse_sdp(sdp).unwrap();
        assert_eq!(offer.streams[0].direction.as_deref(), Some("sendonly"));
        assert_eq!(offer.streams[1].direction.as_deref(), Some("recvonly"));
        assert_eq!(offer.streams[2].direction.as_deref(), Some("inactive"));
    }

    #[test]
    fn test_build_sdp_answer() {
        let offer = parse_sdp(SIPREC_SDP).unwrap();
        let answer = build_sdp_answer(&offer, "10.0.0.100", &[40000, 40002]);

        assert!(answer.contains("v=0"));
        assert!(answer.contains("c=IN IP4 10.0.0.100"));
        assert!(answer.contains("m=audio 40000 RTP/AVP 0"));
        assert!(answer.contains("m=audio 40002 RTP/AVP 0"));
        assert!(answer.contains("a=recvonly"));
        assert!(answer.contains("a=label:caller"));
        assert!(answer.contains("a=label:callee"));
    }

    #[test]
    fn test_build_sdp_answer_echoes_codec() {
        let sdp = "v=0\r\n\
c=IN IP4 10.0.0.1\r\n\
t=0 0\r\n\
m=audio 30000 RTP/AVP 8\r\n\
a=rtpmap:8 PCMA/8000\r\n\
a=sendonly\r\n";

        let offer = parse_sdp(sdp).unwrap();
        let answer = build_sdp_answer(&offer, "10.0.0.100", &[40000]);

        assert!(answer.contains("m=audio 40000 RTP/AVP 8"));
        assert!(answer.contains("PCMA/8000"));
    }

    #[test]
    fn test_build_sdp_answer_multi_codec() {
        let sdp = "v=0\r\n\
c=IN IP4 10.0.0.1\r\n\
t=0 0\r\n\
m=audio 30000 RTP/AVP 0 8 101\r\n\
a=rtpmap:0 PCMU/8000\r\n\
a=rtpmap:8 PCMA/8000\r\n\
a=rtpmap:101 telephone-event/8000\r\n\
a=sendonly\r\n";

        let offer = parse_sdp(sdp).unwrap();
        let answer = build_sdp_answer(&offer, "10.0.0.100", &[40000]);

        // Should echo all offered codecs
        assert!(answer.contains("m=audio 40000 RTP/AVP 0 8 101"));
        assert!(answer.contains("a=rtpmap:0 PCMU/8000"));
        assert!(answer.contains("a=rtpmap:8 PCMA/8000"));
    }

    #[test]
    fn test_preferred_codec() {
        let stream = MediaStream {
            media_type: "audio".to_string(),
            port: 20000,
            codecs: vec![101, 8, 0],
            label: None,
            connection_addr: None,
            direction: None,
        };
        // Prefers first voice codec (8) over non-voice (101)
        assert_eq!(stream.preferred_codec(), 8);
    }

    #[test]
    fn test_preferred_codec_empty() {
        let stream = MediaStream {
            media_type: "audio".to_string(),
            port: 20000,
            codecs: vec![],
            label: None,
            connection_addr: None,
            direction: None,
        };
        assert_eq!(stream.preferred_codec(), 0);
    }
}
