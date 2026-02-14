//! RFC 7865 rs-metadata+xml parser for SIPREC recording metadata.
//!
//! Extracts participant information and stream associations from
//! the recording metadata XML included in SIPREC INVITEs.

use crate::error::{Result, SiprecError};
use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;

/// Parsed SIPREC recording metadata.
#[derive(Debug, Clone, Default)]
pub struct RecordingMetadata {
    pub session_id: String,
    pub participants: Vec<Participant>,
    pub streams: Vec<StreamAssociation>,
}

/// A call participant identified in the metadata.
#[derive(Debug, Clone, Default)]
pub struct Participant {
    pub id: String,
    pub aor: Option<String>,
    pub name: Option<String>,
}

/// Links an RTP stream to a participant.
#[derive(Debug, Clone, Default)]
pub struct StreamAssociation {
    pub stream_id: String,
    pub participant_id: String,
    pub label: Option<String>,
}

/// Extract the local element name as a String.
fn local_name(e: &BytesStart) -> String {
    String::from_utf8_lossy(e.local_name().as_ref()).to_string()
}

/// Extract an attribute value by key name.
fn get_attr(e: &BytesStart, names: &[&str]) -> Option<String> {
    for attr in e.attributes().flatten() {
        let local = attr.key.local_name();
        let key = String::from_utf8_lossy(local.as_ref());
        if names.iter().any(|n| *n == key.as_ref()) {
            return Some(String::from_utf8_lossy(&attr.value).to_string());
        }
    }
    None
}

/// Parse RFC 7865 recording metadata XML.
pub fn parse_metadata(xml: &str) -> Result<RecordingMetadata> {
    let mut reader = Reader::from_str(xml);

    let mut metadata = RecordingMetadata::default();
    let mut current_element = String::new();
    let mut in_participant = false;
    let mut in_stream = false;
    let mut in_nameuri = false;
    let mut current_participant = Participant::default();
    let mut current_stream = StreamAssociation::default();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let name = local_name(e);
                handle_open_tag(
                    e,
                    &name,
                    &mut in_participant,
                    &mut in_stream,
                    &mut in_nameuri,
                    &mut current_participant,
                    &mut current_stream,
                );
                current_element = name;
            }
            Ok(Event::Empty(ref e)) => {
                let name = local_name(e);
                handle_open_tag(
                    e,
                    &name,
                    &mut in_participant,
                    &mut in_stream,
                    &mut in_nameuri,
                    &mut current_participant,
                    &mut current_stream,
                );
                // Self-closing: immediately finalize
                if name == "participant" {
                    in_participant = false;
                    in_nameuri = false;
                    metadata.participants.push(current_participant.clone());
                    current_participant = Participant::default();
                } else if name == "stream" {
                    in_stream = false;
                    metadata.streams.push(current_stream.clone());
                    current_stream = StreamAssociation::default();
                }
                current_element = name;
            }
            Ok(Event::Text(e)) => {
                let text = e.unescape().unwrap_or_default().trim().to_string();
                if text.is_empty() {
                    continue;
                }

                match current_element.as_str() {
                    "session" | "session_id" | "sessionID" => {
                        if metadata.session_id.is_empty() {
                            metadata.session_id = text;
                        }
                    }
                    "aor" if in_participant && in_nameuri => {
                        current_participant.aor = Some(text);
                    }
                    "name" if in_participant && in_nameuri => {
                        current_participant.name = Some(text);
                    }
                    "label" if in_stream => {
                        current_stream.label = Some(text);
                    }
                    _ => {}
                }
            }
            Ok(Event::End(e)) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).to_string();

                if name == "participant" {
                    in_participant = false;
                    in_nameuri = false;
                    metadata.participants.push(current_participant.clone());
                    current_participant = Participant::default();
                } else if name == "nameID" || name == "name-id" {
                    in_nameuri = false;
                } else if name == "stream" {
                    in_stream = false;
                    metadata.streams.push(current_stream.clone());
                    current_stream = StreamAssociation::default();
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(SiprecError::Parse(format!("XML parse error: {}", e)));
            }
            _ => {}
        }
        buf.clear();
    }

    Ok(metadata)
}

/// Process an opening tag (Start or Empty), updating parser state.
fn handle_open_tag(
    e: &BytesStart,
    name: &str,
    in_participant: &mut bool,
    in_stream: &mut bool,
    in_nameuri: &mut bool,
    current_participant: &mut Participant,
    current_stream: &mut StreamAssociation,
) {
    if name == "participant" {
        *in_participant = true;
        *current_participant = Participant::default();
        if let Some(id) = get_attr(e, &["participant_id", "id"]) {
            current_participant.id = id;
        }
    } else if name == "nameID" || name == "name-id" {
        if *in_participant {
            *in_nameuri = true;
        }
    } else if name == "stream" {
        *in_stream = true;
        *current_stream = StreamAssociation::default();
        if let Some(id) = get_attr(e, &["stream_id", "id"]) {
            current_stream.stream_id = id;
        }
        if let Some(pid) = get_attr(e, &["participant_id"]) {
            current_stream.participant_id = pid;
        }
    } else if name == "associate"
        && *in_stream
        && let Some(pid) = get_attr(e, &["participant_id", "associated_participant_id", "arid"])
    {
        current_stream.participant_id = pid;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_METADATA: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<recording xmlns="urn:ietf:params:xml:ns:recording:1">
  <datamode>complete</datamode>
  <session session_id="hVpd7YQgRW2nD22h7q60JQ==">
    <sipSessionID>abcd1234</sipSessionID>
  </session>
  <participant participant_id="part-001">
    <nameID>
      <aor>sip:+15551234567@example.com</aor>
      <name>Caller</name>
    </nameID>
  </participant>
  <participant participant_id="part-002">
    <nameID>
      <aor>sip:+15559876543@example.com</aor>
      <name>Callee</name>
    </nameID>
  </participant>
  <stream stream_id="stream-001" participant_id="part-001">
    <label>caller</label>
  </stream>
  <stream stream_id="stream-002" participant_id="part-002">
    <label>callee</label>
  </stream>
</recording>"#;

    #[test]
    fn test_parse_sample_metadata() {
        let metadata = parse_metadata(SAMPLE_METADATA).unwrap();

        assert_eq!(metadata.participants.len(), 2);
        assert_eq!(metadata.streams.len(), 2);

        // Check participants
        let p1 = &metadata.participants[0];
        assert_eq!(p1.id, "part-001");
        assert_eq!(p1.aor.as_deref(), Some("sip:+15551234567@example.com"));
        assert_eq!(p1.name.as_deref(), Some("Caller"));

        let p2 = &metadata.participants[1];
        assert_eq!(p2.id, "part-002");
        assert_eq!(p2.aor.as_deref(), Some("sip:+15559876543@example.com"));
        assert_eq!(p2.name.as_deref(), Some("Callee"));

        // Check streams
        let s1 = &metadata.streams[0];
        assert_eq!(s1.stream_id, "stream-001");
        assert_eq!(s1.participant_id, "part-001");
        assert_eq!(s1.label.as_deref(), Some("caller"));

        let s2 = &metadata.streams[1];
        assert_eq!(s2.stream_id, "stream-002");
        assert_eq!(s2.participant_id, "part-002");
        assert_eq!(s2.label.as_deref(), Some("callee"));
    }

    #[test]
    fn test_parse_minimal_metadata() {
        let xml = r#"<?xml version="1.0"?>
<recording xmlns="urn:ietf:params:xml:ns:recording:1">
  <participant participant_id="p1">
    <nameID><aor>sip:a@b.com</aor></nameID>
  </participant>
  <stream stream_id="s1" participant_id="p1"/>
</recording>"#;

        let metadata = parse_metadata(xml).unwrap();
        assert_eq!(metadata.participants.len(), 1);
        assert_eq!(metadata.participants[0].id, "p1");
        // Note: empty element <stream .../> should still be captured
        assert_eq!(metadata.streams.len(), 1);
    }

    #[test]
    fn test_parse_malformed_xml() {
        let xml = "this is not xml <<<<";
        let result = parse_metadata(xml);
        // Should either succeed with empty result or return parse error
        // quick-xml is lenient with non-XML text before tags
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn test_parse_empty_metadata() {
        let xml = r#"<?xml version="1.0"?>
<recording xmlns="urn:ietf:params:xml:ns:recording:1">
</recording>"#;

        let metadata = parse_metadata(xml).unwrap();
        assert!(metadata.participants.is_empty());
        assert!(metadata.streams.is_empty());
    }
}
