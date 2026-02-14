//! SIP message builders for UAS (inbound) role.
//!
//! Builds provisional and final responses per RFC 3261, and extracts
//! dialog-identifying headers from incoming requests.

use rsip::headers::UntypedHeader;
use rsip::{Header, Headers, Request, Response, StatusCode, Version};
use tracing::debug;

/// Supported SIP methods (for Allow header).
const ALLOWED_METHODS: &str = "INVITE, ACK, BYE, OPTIONS";

/// Server identification header value.
const SERVER_NAME: &str = "5060Solutions-SIPREC/0.1";

/// Copy headers common to all responses from a request.
///
/// Per RFC 3261 Section 8.2.6, responses MUST copy:
/// Via, From, To, Call-ID, CSeq, and Record-Route headers.
fn copy_dialog_headers(request: &Request, headers: &mut Headers) {
    for header in request.headers.iter() {
        match header {
            Header::Via(_) | Header::RecordRoute(_) => {
                headers.push(header.clone());
            }
            _ => {}
        }
    }

    // Copy From (as-is, per RFC 3261 Section 8.2.6.1)
    if let Some(from) = request
        .headers
        .iter()
        .find(|h| matches!(h, Header::From(_)))
    {
        headers.push(from.clone());
    }

    // Copy Call-ID
    if let Some(cid) = request
        .headers
        .iter()
        .find(|h| matches!(h, Header::CallId(_)))
    {
        headers.push(cid.clone());
    }

    // Copy CSeq
    if let Some(cseq) = request
        .headers
        .iter()
        .find(|h| matches!(h, Header::CSeq(_)))
    {
        headers.push(cseq.clone());
    }
}

/// Build a 100 Trying provisional response per RFC 3261 Section 8.2.6.
///
/// Sent immediately upon receiving an INVITE to stop retransmissions.
pub fn build_100_trying(invite: &Request) -> Response {
    let mut headers = Headers::default();
    copy_dialog_headers(invite, &mut headers);

    // Copy To without tag (provisional response)
    if let Some(to) = invite.headers.iter().find(|h| matches!(h, Header::To(_))) {
        headers.push(to.clone());
    }

    Response {
        status_code: StatusCode::Trying,
        version: Version::V2,
        headers,
        body: vec![],
    }
}

/// Build a 200 OK response to an INVITE with an SDP body.
///
/// Per RFC 3261 Section 13.3.1.4, the 200 OK to an INVITE MUST include:
/// Via, Record-Route, From, To (with tag), Call-ID, CSeq, Contact,
/// Allow, and the SDP answer body.
pub fn build_200_ok(invite: &Request, sdp_body: &str, contact_uri: &str) -> Response {
    let mut headers = Headers::default();
    copy_dialog_headers(invite, &mut headers);

    // Copy To, adding our tag per RFC 3261 Section 8.2.6.2
    if let Some(Header::To(to)) = invite.headers.iter().find(|h| matches!(h, Header::To(_))) {
        let to_str = to.to_string();
        let tag = format!("srec-{}", uuid::Uuid::new_v4().as_simple());
        let new_to = if to_str.contains("tag=") {
            to_str
        } else {
            format!("{};tag={}", to_str, tag)
        };
        headers.push(Header::To(rsip::headers::To::new(&new_to)));
    }

    // Contact (required per RFC 3261 Section 8.2.6.2 for 2xx to INVITE)
    headers.push(Header::Contact(rsip::headers::Contact::new(contact_uri)));

    // Allow header (RFC 3261 Section 20.5 - SHOULD include in 2xx to INVITE)
    headers.push(Header::Allow(rsip::headers::Allow::new(ALLOWED_METHODS)));

    // Server header (RFC 3261 Section 20.35)
    headers.push(Header::Server(rsip::headers::Server::new(SERVER_NAME)));

    // Content-Type and Content-Length for SDP body
    headers.push(Header::ContentType(rsip::headers::ContentType::new(
        "application/sdp",
    )));
    headers.push(Header::ContentLength(rsip::headers::ContentLength::new(
        sdp_body.len().to_string(),
    )));

    debug!(
        call_id = %extract_call_id_from_headers(&invite.headers).unwrap_or_default(),
        "Built 200 OK for INVITE"
    );

    Response {
        status_code: StatusCode::OK,
        version: Version::V2,
        headers,
        body: sdp_body.as_bytes().to_vec(),
    }
}

/// Build a 200 OK response to a BYE request.
///
/// Per RFC 3261 Section 15.1.2, the UAS responds 200 OK to BYE.
pub fn build_200_ok_bye(bye: &Request) -> Response {
    let mut headers = Headers::default();
    copy_dialog_headers(bye, &mut headers);

    // Copy To (as-is for in-dialog request, already has tag)
    if let Some(to) = bye.headers.iter().find(|h| matches!(h, Header::To(_))) {
        headers.push(to.clone());
    }

    // Server header
    headers.push(Header::Server(rsip::headers::Server::new(SERVER_NAME)));

    headers.push(Header::ContentLength(rsip::headers::ContentLength::new(
        "0",
    )));

    debug!(
        call_id = %extract_call_id_from_headers(&bye.headers).unwrap_or_default(),
        "Built 200 OK for BYE"
    );

    Response {
        status_code: StatusCode::OK,
        version: Version::V2,
        headers,
        body: vec![],
    }
}

/// Extract Call-ID from a SIP message's headers.
fn extract_call_id_from_headers(headers: &Headers) -> Option<String> {
    headers.iter().find_map(|h| {
        if let Header::CallId(cid) = h {
            Some(cid.to_string().trim().to_string())
        } else {
            None
        }
    })
}

/// Extract Call-ID from a SIP request.
pub fn extract_call_id(request: &Request) -> Option<String> {
    extract_call_id_from_headers(&request.headers)
}

/// Extract the From tag from a SIP request.
pub fn extract_from_tag(request: &Request) -> Option<String> {
    request.headers.iter().find_map(|h| {
        if let Header::From(from) = h {
            let from_str = from.to_string();
            from_str
                .split(';')
                .find(|p| p.trim().starts_with("tag="))
                .map(|t| t.trim().strip_prefix("tag=").unwrap_or("").to_string())
        } else {
            None
        }
    })
}

/// Extract the To tag from a SIP request.
pub fn extract_to_tag(request: &Request) -> Option<String> {
    request.headers.iter().find_map(|h| {
        if let Header::To(to) = h {
            let to_str = to.to_string();
            to_str
                .split(';')
                .find(|p| p.trim().starts_with("tag="))
                .map(|t| t.trim().strip_prefix("tag=").unwrap_or("").to_string())
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::convert::TryFrom;

    fn sample_invite() -> Request {
        let raw = "INVITE sip:recorder@10.0.0.1:5060 SIP/2.0\r\n\
Via: SIP/2.0/UDP 192.168.1.1:5060;branch=z9hG4bK-test;rport\r\n\
Record-Route: <sip:192.168.1.1;lr>\r\n\
From: <sip:src@192.168.1.1>;tag=from-tag-123\r\n\
To: <sip:recorder@10.0.0.1>\r\n\
Call-ID: siprec-call-001@192.168.1.1\r\n\
CSeq: 1 INVITE\r\n\
Content-Type: multipart/mixed;boundary=foo\r\n\
Content-Length: 0\r\n\
\r\n";
        Request::try_from(raw.to_string()).unwrap()
    }

    fn sample_invite_no_record_route() -> Request {
        let raw = "INVITE sip:recorder@10.0.0.1:5060 SIP/2.0\r\n\
Via: SIP/2.0/UDP 192.168.1.1:5060;branch=z9hG4bK-test;rport\r\n\
From: <sip:src@192.168.1.1>;tag=from-tag-123\r\n\
To: <sip:recorder@10.0.0.1>\r\n\
Call-ID: siprec-call-001@192.168.1.1\r\n\
CSeq: 1 INVITE\r\n\
Content-Length: 0\r\n\
\r\n";
        Request::try_from(raw.to_string()).unwrap()
    }

    fn sample_bye() -> Request {
        let raw = "BYE sip:recorder@10.0.0.1:5060 SIP/2.0\r\n\
Via: SIP/2.0/UDP 192.168.1.1:5060;branch=z9hG4bK-bye;rport\r\n\
Record-Route: <sip:192.168.1.1;lr>\r\n\
From: <sip:src@192.168.1.1>;tag=from-tag-123\r\n\
To: <sip:recorder@10.0.0.1>;tag=to-tag-456\r\n\
Call-ID: siprec-call-001@192.168.1.1\r\n\
CSeq: 2 BYE\r\n\
Content-Length: 0\r\n\
\r\n";
        Request::try_from(raw.to_string()).unwrap()
    }

    #[test]
    fn test_extract_call_id() {
        let invite = sample_invite();
        let call_id = extract_call_id(&invite);
        assert!(call_id.is_some());
        assert!(call_id.unwrap().contains("siprec-call-001"));
    }

    #[test]
    fn test_extract_from_tag() {
        let invite = sample_invite();
        let tag = extract_from_tag(&invite);
        assert_eq!(tag, Some("from-tag-123".to_string()));
    }

    #[test]
    fn test_extract_to_tag() {
        let bye = sample_bye();
        let tag = extract_to_tag(&bye);
        assert_eq!(tag, Some("to-tag-456".to_string()));
    }

    #[test]
    fn test_extract_to_tag_missing() {
        let invite = sample_invite();
        let tag = extract_to_tag(&invite);
        assert!(tag.is_none());
    }

    #[test]
    fn test_build_100_trying() {
        let invite = sample_invite();
        let response = build_100_trying(&invite);

        assert_eq!(response.status_code, StatusCode::Trying);
        assert!(response.body.is_empty());

        // Must include Via, From, To, Call-ID, CSeq
        let resp_str = response.to_string();
        assert!(resp_str.contains("Via:"));
        assert!(resp_str.contains("From:"));
        assert!(resp_str.contains("To:"));
        assert!(resp_str.contains("Call-ID:") || resp_str.contains("Call-Id:"));
        assert!(resp_str.contains("CSeq:"));
    }

    #[test]
    fn test_build_200_ok() {
        let invite = sample_invite();
        let sdp = "v=0\r\no=- 1 1 IN IP4 10.0.0.1\r\ns=-\r\nc=IN IP4 10.0.0.1\r\nt=0 0\r\nm=audio 40000 RTP/AVP 0\r\na=recvonly\r\n";

        let response = build_200_ok(&invite, sdp, "<sip:recorder@10.0.0.1:5060>");
        assert_eq!(response.status_code, StatusCode::OK);
        assert!(!response.body.is_empty());

        let resp_str = response.to_string();

        // RFC 3261: Must include Record-Route from request
        assert!(
            resp_str.contains("Record-Route:"),
            "200 OK must include Record-Route"
        );

        // Must include Allow header
        assert!(resp_str.contains("Allow:"), "200 OK must include Allow");
        assert!(resp_str.contains("INVITE"));
        assert!(resp_str.contains("BYE"));

        // Must include Server header
        assert!(
            resp_str.contains("Server:"),
            "200 OK must include Server header"
        );

        // Must include Contact
        assert!(resp_str.contains("Contact:"), "200 OK must include Contact");

        // Must have To tag
        assert!(resp_str.contains("tag="), "200 OK To must have tag");
    }

    #[test]
    fn test_build_200_ok_without_record_route() {
        let invite = sample_invite_no_record_route();
        let sdp = "v=0\r\n";
        let response = build_200_ok(&invite, sdp, "<sip:recorder@10.0.0.1:5060>");

        assert_eq!(response.status_code, StatusCode::OK);
        // Should still work without Record-Route
        let resp_str = response.to_string();
        assert!(resp_str.contains("Via:"));
    }

    #[test]
    fn test_build_200_ok_bye() {
        let bye = sample_bye();
        let response = build_200_ok_bye(&bye);
        assert_eq!(response.status_code, StatusCode::OK);
        assert!(response.body.is_empty());

        let resp_str = response.to_string();

        // RFC 3261: Must include Record-Route from request
        assert!(
            resp_str.contains("Record-Route:"),
            "200 OK BYE must include Record-Route"
        );

        // Must include Server header
        assert!(resp_str.contains("Server:"));
    }

    #[test]
    fn test_build_200_ok_preserves_existing_to_tag() {
        // If To already has a tag, don't add another
        let raw = "INVITE sip:recorder@10.0.0.1:5060 SIP/2.0\r\n\
Via: SIP/2.0/UDP 192.168.1.1:5060;branch=z9hG4bK-test\r\n\
From: <sip:src@192.168.1.1>;tag=from-tag\r\n\
To: <sip:recorder@10.0.0.1>;tag=existing-tag\r\n\
Call-ID: test-call@10.0.0.1\r\n\
CSeq: 1 INVITE\r\n\
Content-Length: 0\r\n\
\r\n";
        let invite = Request::try_from(raw.to_string()).unwrap();
        let response = build_200_ok(&invite, "v=0\r\n", "<sip:recorder@10.0.0.1>");

        let resp_str = response.to_string();
        // Should not have double tags
        let tag_count = resp_str.matches("tag=").count();
        // From has tag + To has exactly one tag
        assert_eq!(tag_count, 2, "Should have exactly 2 tags (From + To)");
    }
}
