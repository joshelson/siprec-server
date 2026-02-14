//! Error types for the SIPREC recording server.

use thiserror::Error;

/// Top-level SIPREC server errors.
#[derive(Error, Debug)]
pub enum SiprecError {
    #[error("SIP error: {0}")]
    Sip(#[from] SipError),

    #[error("RTP error: {0}")]
    Rtp(#[from] RtpError),

    #[error("Recording error: {0}")]
    Recording(String),

    #[error("Parse error: {0}")]
    Parse(String),

    #[error("Timeout: {0}")]
    Timeout(String),

    #[error("Transport error: {0}")]
    Transport(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

/// SIP-specific errors.
#[derive(Error, Debug)]
pub enum SipError {
    #[error("Failed to parse SIP message: {0}")]
    ParseError(String),

    #[error("SIP transaction timeout")]
    Timeout,

    #[error("Transport error: {0}")]
    Transport(String),

    #[error("Failed to bind socket: {0}")]
    SocketBind(String),

    #[error("Failed to send: {0}")]
    SendFailed(String),

    #[error("Failed to receive: {0}")]
    ReceiveFailed(String),
}

/// RTP-specific errors.
#[derive(Error, Debug)]
pub enum RtpError {
    #[error("Failed to parse RTP packet: {0}")]
    ParseError(String),

    #[error("No RTP packets received")]
    NoPackets,

    #[error("RTP port allocation failed")]
    PortAllocation,

    #[error("Codec not supported: {0}")]
    UnsupportedCodec(String),
}

/// Result type alias for this crate.
pub type Result<T> = std::result::Result<T, SiprecError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sip_error_display() {
        let err = SipError::ParseError("invalid header".into());
        assert!(err.to_string().contains("invalid header"));

        let err = SipError::Timeout;
        assert!(err.to_string().contains("timeout"));

        let err = SipError::Transport("connection refused".into());
        assert!(err.to_string().contains("connection refused"));
    }

    #[test]
    fn test_rtp_error_display() {
        let err = RtpError::ParseError("bad packet".into());
        assert!(err.to_string().contains("bad packet"));

        let err = RtpError::NoPackets;
        assert!(err.to_string().contains("No RTP packets"));

        let err = RtpError::PortAllocation;
        assert!(err.to_string().contains("port allocation"));

        let err = RtpError::UnsupportedCodec("G729".into());
        assert!(err.to_string().contains("G729"));
    }

    #[test]
    fn test_siprec_error_display() {
        let err = SiprecError::Recording("disk full".into());
        assert!(err.to_string().contains("disk full"));

        let err = SiprecError::Parse("bad XML".into());
        assert!(err.to_string().contains("bad XML"));

        let err = SiprecError::Timeout("no response".into());
        assert!(err.to_string().contains("no response"));
    }

    #[test]
    fn test_sip_error_conversion() {
        let sip_err = SipError::Timeout;
        let siprec_err: SiprecError = sip_err.into();
        assert!(matches!(siprec_err, SiprecError::Sip(SipError::Timeout)));
    }

    #[test]
    fn test_rtp_error_conversion() {
        let rtp_err = RtpError::NoPackets;
        let siprec_err: SiprecError = rtp_err.into();
        assert!(matches!(siprec_err, SiprecError::Rtp(RtpError::NoPackets)));
    }

    #[test]
    fn test_io_error_conversion() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let siprec_err: SiprecError = io_err.into();
        assert!(matches!(siprec_err, SiprecError::Io(_)));
    }

    #[test]
    fn test_error_source_chain() {
        use std::error::Error;

        let sip_err = SipError::ParseError("bad header".into());
        let siprec_err: SiprecError = sip_err.into();
        let source = siprec_err.source();
        assert!(source.is_some());
    }
}
