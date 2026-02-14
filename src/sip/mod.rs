//! SIP protocol handling for the SIPREC recording server (UAS role).

pub mod message;
pub mod sdp;
pub mod transport;

pub use message::{
    build_100_trying, build_200_ok, build_200_ok_bye, extract_call_id, extract_from_tag,
    extract_to_tag,
};
pub use sdp::{MediaStream, SdpOffer, build_sdp_answer, parse_sdp};
pub use transport::SipTransport;
