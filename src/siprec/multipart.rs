//! Multipart MIME body parser for SIPREC INVITE messages (RFC 2046).
//!
//! SIPREC INVITEs contain a `multipart/mixed` body with two parts:
//! 1. `application/sdp` - SDP offer with media streams
//! 2. `application/rs-metadata+xml` - Recording metadata (RFC 7865)

use crate::error::{Result, SiprecError};

/// A parsed multipart MIME body.
#[derive(Debug, Clone)]
pub struct MultipartBody {
    pub parts: Vec<BodyPart>,
}

/// A single part of a multipart body.
#[derive(Debug, Clone)]
pub struct BodyPart {
    pub content_type: String,
    pub body: String,
}

impl MultipartBody {
    /// Find the first part with the given content type prefix.
    pub fn find_part(&self, content_type_prefix: &str) -> Option<&BodyPart> {
        self.parts
            .iter()
            .find(|p| p.content_type.starts_with(content_type_prefix))
    }

    /// Get the SDP part.
    pub fn sdp(&self) -> Option<&str> {
        self.find_part("application/sdp").map(|p| p.body.as_str())
    }

    /// Get the recording metadata XML part.
    pub fn metadata_xml(&self) -> Option<&str> {
        self.find_part("application/rs-metadata")
            .map(|p| p.body.as_str())
    }
}

/// Parse a multipart/mixed body per RFC 2046.
///
/// # Arguments
/// * `content_type` - The Content-Type header value (must contain boundary parameter)
/// * `body` - The raw body text
pub fn parse_multipart(content_type: &str, body: &str) -> Result<MultipartBody> {
    // Validate Content-Type is multipart per RFC 2046
    let ct_lower = content_type.to_ascii_lowercase();
    if !ct_lower.contains("multipart") {
        return Err(SiprecError::Parse(
            "Content-Type is not multipart".to_string(),
        ));
    }

    // Extract boundary from Content-Type
    let boundary = extract_boundary(content_type).ok_or_else(|| {
        SiprecError::Parse("No boundary found in Content-Type header".to_string())
    })?;

    let delimiter = format!("--{}", boundary);
    let end_delimiter = format!("--{}--", boundary);

    let mut parts = Vec::new();

    // Per RFC 2046 Section 5.1.1, boundaries must start on a new line.
    // Split the body on line-boundary patterns.
    let sections = split_on_boundary(body, &delimiter, &end_delimiter);

    for section in &sections {
        let section = section.trim();
        if section.is_empty() {
            continue;
        }

        // Split headers from body (blank line separator)
        if let Some(part) = parse_part(section) {
            parts.push(part);
        }
    }

    if parts.is_empty() {
        return Err(SiprecError::Parse(
            "No parts found in multipart body".to_string(),
        ));
    }

    Ok(MultipartBody { parts })
}

/// Split body on boundary markers per RFC 2046 Section 5.1.1.
///
/// Boundaries are preceded by CRLF (or LF) and consist of "--" + boundary.
/// The closing delimiter is "--" + boundary + "--".
/// Text before the first boundary (preamble) and after the last (epilogue) are ignored.
fn split_on_boundary<'a>(body: &'a str, delimiter: &str, end_delimiter: &str) -> Vec<&'a str> {
    let mut sections = Vec::new();
    let mut in_parts = false;

    // Normalize: find boundary at line starts
    let mut remaining = body;
    while let Some(pos) = remaining.find(delimiter) {
        if in_parts {
            // Content between previous boundary and this one
            let content = &remaining[..pos];
            // Strip trailing CRLF before boundary
            let content = content
                .strip_suffix("\r\n")
                .or_else(|| content.strip_suffix('\n'))
                .unwrap_or(content);
            if !content.is_empty() {
                sections.push(content);
            }
        }

        // Move past the delimiter
        remaining = &remaining[pos + delimiter.len()..];

        // Check for closing delimiter
        if remaining.starts_with("--") {
            // This is the end delimiter, stop processing
            break;
        }

        // Skip the CRLF or LF after the delimiter line
        remaining = remaining
            .strip_prefix("\r\n")
            .or_else(|| remaining.strip_prefix('\n'))
            .unwrap_or(remaining);

        in_parts = true;
    }

    // If there's content after the last boundary but before end_delimiter
    if in_parts && !remaining.is_empty() {
        let content = if let Some(pos) = remaining.find(end_delimiter) {
            let c = &remaining[..pos];
            c.strip_suffix("\r\n")
                .or_else(|| c.strip_suffix('\n'))
                .unwrap_or(c)
        } else {
            remaining.trim()
        };
        if !content.is_empty() {
            sections.push(content);
        }
    }

    sections
}

/// Extract boundary parameter from Content-Type header.
///
/// Handles both quoted and unquoted boundary values per RFC 2046 Section 5.1.1.
fn extract_boundary(content_type: &str) -> Option<String> {
    for param in content_type.split(';') {
        let param = param.trim();
        let lower = param.to_ascii_lowercase();
        if let Some(value) = lower.strip_prefix("boundary=") {
            // Use original case for boundary value (boundaries are case-sensitive)
            let original_value = &param[param.len() - value.len()..];
            // Remove surrounding quotes if present
            let value = original_value.trim_matches('"');
            if value.is_empty() {
                return None;
            }
            return Some(value.to_string());
        }
    }
    None
}

/// Parse a single MIME part (headers + body).
fn parse_part(section: &str) -> Option<BodyPart> {
    // Find the blank line separating headers from body per RFC 2046
    // Try both \r\n\r\n and \n\n
    let (headers_str, body) = if let Some(pos) = section.find("\r\n\r\n") {
        (&section[..pos], section[pos + 4..].trim().to_string())
    } else if let Some(pos) = section.find("\n\n") {
        (&section[..pos], section[pos + 2..].trim().to_string())
    } else {
        return None;
    };

    // Extract Content-Type from headers (case-insensitive per RFC 2045)
    let mut content_type = String::new();
    for line in headers_str.lines() {
        let line = line.trim();
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("content-type:") {
            content_type = line["content-type:".len()..].trim().to_string();
        }
    }

    if content_type.is_empty() || body.is_empty() {
        return None;
    }

    Some(BodyPart { content_type, body })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_boundary() {
        assert_eq!(
            extract_boundary("multipart/mixed;boundary=uniqueBoundary"),
            Some("uniqueBoundary".to_string())
        );
        assert_eq!(
            extract_boundary("multipart/mixed; boundary=\"foobar\""),
            Some("foobar".to_string())
        );
        assert_eq!(extract_boundary("application/sdp"), None);
    }

    #[test]
    fn test_extract_boundary_case_insensitive() {
        assert_eq!(
            extract_boundary("multipart/mixed; Boundary=MyBoundary"),
            Some("MyBoundary".to_string())
        );
        assert_eq!(
            extract_boundary("multipart/mixed; BOUNDARY=test123"),
            Some("test123".to_string())
        );
    }

    #[test]
    fn test_extract_boundary_empty() {
        assert_eq!(extract_boundary("multipart/mixed;boundary="), None);
        assert_eq!(extract_boundary("multipart/mixed;boundary=\"\""), None);
    }

    #[test]
    fn test_parse_two_part_body() {
        let content_type = "multipart/mixed;boundary=uniqueBoundary";
        let body = "\r\n\
--uniqueBoundary\r\n\
Content-Type: application/sdp\r\n\
\r\n\
v=0\r\n\
o=- 12345 12345 IN IP4 192.168.1.1\r\n\
s=-\r\n\
c=IN IP4 192.168.1.1\r\n\
t=0 0\r\n\
m=audio 20000 RTP/AVP 0\r\n\
a=sendonly\r\n\
a=label:caller\r\n\
m=audio 20002 RTP/AVP 0\r\n\
a=sendonly\r\n\
a=label:callee\r\n\
--uniqueBoundary\r\n\
Content-Type: application/rs-metadata+xml\r\n\
\r\n\
<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n\
<recording xmlns=\"urn:ietf:params:xml:ns:recording:1\">\r\n\
  <session>test-session</session>\r\n\
</recording>\r\n\
--uniqueBoundary--";

        let result = parse_multipart(content_type, body).unwrap();
        assert_eq!(result.parts.len(), 2);

        assert!(result.sdp().is_some());
        assert!(result.sdp().unwrap().contains("m=audio"));

        assert!(result.metadata_xml().is_some());
        assert!(result.metadata_xml().unwrap().contains("<recording"));
    }

    #[test]
    fn test_parse_lf_only() {
        // Test with LF-only line endings
        let content_type = "multipart/mixed;boundary=bnd";
        let body = "--bnd\n\
Content-Type: application/sdp\n\
\n\
v=0\n\
c=IN IP4 10.0.0.1\n\
--bnd--";

        let result = parse_multipart(content_type, body).unwrap();
        assert_eq!(result.parts.len(), 1);
        assert!(result.sdp().is_some());
    }

    #[test]
    fn test_parse_with_preamble() {
        // RFC 2046: text before first boundary is preamble and should be ignored
        let content_type = "multipart/mixed;boundary=bnd";
        let body = "This is the preamble.\r\n\
--bnd\r\n\
Content-Type: application/sdp\r\n\
\r\n\
v=0\r\n\
--bnd--\r\n\
This is the epilogue.";

        let result = parse_multipart(content_type, body).unwrap();
        assert_eq!(result.parts.len(), 1);
    }

    #[test]
    fn test_parse_missing_boundary() {
        let result = parse_multipart("application/sdp", "some body");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_not_multipart() {
        let result = parse_multipart("application/sdp; boundary=bnd", "some body");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_empty_parts() {
        let content_type = "multipart/mixed;boundary=bnd";
        let body = "--bnd\r\n--bnd--";
        let result = parse_multipart(content_type, body);
        assert!(result.is_err()); // No valid parts
    }

    #[test]
    fn test_find_part() {
        let body = MultipartBody {
            parts: vec![
                BodyPart {
                    content_type: "application/sdp".to_string(),
                    body: "v=0\r\n".to_string(),
                },
                BodyPart {
                    content_type: "application/rs-metadata+xml".to_string(),
                    body: "<recording/>".to_string(),
                },
            ],
        };

        assert!(body.find_part("application/sdp").is_some());
        assert!(body.find_part("application/rs-metadata").is_some());
        assert!(body.find_part("text/plain").is_none());
    }
}
