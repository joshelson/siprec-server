//! Stereo WAV recording for SIPREC sessions.
//!
//! Records caller and callee audio as a stereo WAV file:
//! - Left channel: caller audio
//! - Right channel: callee audio
//! - Format: 8kHz, 2 channels, 8-bit mu-law

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use tracing::{debug, error, info, warn};

/// WAV file header size.
const WAV_HEADER_SIZE: usize = 44;

/// mu-law silence value.
const MULAW_SILENCE: u8 = 0xFF;

/// Stereo call recorder for SIPREC sessions.
pub struct SessionRecorder {
    path: PathBuf,
    writer: BufWriter<File>,
    caller_buffer: Vec<u8>,
    callee_buffer: Vec<u8>,
    call_id: String,
}

impl SessionRecorder {
    /// Create a new session recorder.
    pub fn new(recording_dir: &str, call_id: &str, filename: &str) -> Option<Self> {
        let dir = Path::new(recording_dir);
        if let Err(e) = fs::create_dir_all(dir) {
            error!(error = %e, dir = %recording_dir, "Failed to create recording directory");
            return None;
        }

        let sanitized = sanitize_filename(filename);
        let path = dir.join(format!("{}.wav", sanitized));

        let file = match File::create(&path) {
            Ok(f) => f,
            Err(e) => {
                error!(error = %e, path = %path.display(), "Failed to create recording file");
                return None;
            }
        };

        let mut writer = BufWriter::new(file);

        let header = create_stereo_wav_header(0);
        if let Err(e) = writer.write_all(&header) {
            error!(error = %e, "Failed to write WAV header");
            return None;
        }

        info!(
            path = %path.display(),
            call_id = %call_id,
            "Recording started (L=caller, R=callee)"
        );

        Some(Self {
            path,
            writer,
            caller_buffer: Vec::with_capacity(480_000), // ~60s
            callee_buffer: Vec::with_capacity(480_000),
            call_id: call_id.to_string(),
        })
    }

    /// Write caller audio data (left channel).
    pub fn write_caller_audio(&mut self, data: &[u8]) {
        self.caller_buffer.extend_from_slice(data);
        debug!(
            bytes = data.len(),
            total = self.caller_buffer.len(),
            call_id = %self.call_id,
            "Wrote caller audio (L)"
        );
    }

    /// Write callee audio data (right channel).
    pub fn write_callee_audio(&mut self, data: &[u8]) {
        self.callee_buffer.extend_from_slice(data);
        debug!(
            bytes = data.len(),
            total = self.callee_buffer.len(),
            call_id = %self.call_id,
            "Wrote callee audio (R)"
        );
    }

    /// Finalize the recording and return the file path.
    pub fn finalize(mut self) -> PathBuf {
        let max_len = self.caller_buffer.len().max(self.callee_buffer.len());

        // Pad shorter channel with silence
        self.caller_buffer.resize(max_len, MULAW_SILENCE);
        self.callee_buffer.resize(max_len, MULAW_SILENCE);

        // Interleave: [L0, R0, L1, R1, ...]
        let mut interleaved = Vec::with_capacity(max_len * 2);
        for i in 0..max_len {
            interleaved.push(self.caller_buffer[i]);
            interleaved.push(self.callee_buffer[i]);
        }

        if let Err(e) = self.writer.write_all(&interleaved) {
            error!(error = %e, call_id = %self.call_id, "Failed to write interleaved audio");
        }

        if let Err(e) = self.writer.flush() {
            warn!(error = %e, "Failed to flush recording buffer");
        }

        let file = self.writer.into_inner().expect("Failed to get file");

        let data_size = interleaved.len();
        if let Err(e) = update_wav_header(&file, data_size) {
            warn!(error = %e, "Failed to update WAV header");
        }

        let duration_ms = (max_len as u64 * 1000) / 8000;
        info!(
            path = %self.path.display(),
            call_id = %self.call_id,
            caller_bytes = self.caller_buffer.len(),
            callee_bytes = self.callee_buffer.len(),
            stereo_bytes = data_size,
            duration_ms = duration_ms,
            "Recording finalized (stereo: L=caller, R=callee)"
        );

        self.path
    }

    /// Get the path to the recording file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Get the recording duration in milliseconds.
    pub fn duration_ms(&self) -> u64 {
        let max_len = self.caller_buffer.len().max(self.callee_buffer.len());
        (max_len as u64 * 1000) / 8000
    }
}

/// Create a stereo WAV header for mu-law audio (8kHz, 2 channels).
fn create_stereo_wav_header(data_size: u32) -> [u8; WAV_HEADER_SIZE] {
    let mut header = [0u8; WAV_HEADER_SIZE];

    header[0..4].copy_from_slice(b"RIFF");
    let file_size = data_size + 36;
    header[4..8].copy_from_slice(&file_size.to_le_bytes());
    header[8..12].copy_from_slice(b"WAVE");

    header[12..16].copy_from_slice(b"fmt ");
    header[16..20].copy_from_slice(&16u32.to_le_bytes()); // chunk size
    header[20..22].copy_from_slice(&7u16.to_le_bytes()); // mu-law
    header[22..24].copy_from_slice(&2u16.to_le_bytes()); // stereo
    header[24..28].copy_from_slice(&8000u32.to_le_bytes()); // sample rate
    header[28..32].copy_from_slice(&16000u32.to_le_bytes()); // byte rate
    header[32..34].copy_from_slice(&2u16.to_le_bytes()); // block align
    header[34..36].copy_from_slice(&8u16.to_le_bytes()); // bits per sample

    header[36..40].copy_from_slice(b"data");
    header[40..44].copy_from_slice(&data_size.to_le_bytes());

    header
}

/// Update WAV header with correct data size.
fn update_wav_header(file: &File, data_size: usize) -> std::io::Result<()> {
    use std::os::unix::fs::FileExt;

    let data_size = data_size as u32;
    let file_size = data_size + 36;

    file.write_at(&file_size.to_le_bytes(), 4)?;
    file.write_at(&data_size.to_le_bytes(), 40)?;

    Ok(())
}

/// Sanitize a string to be safe for use as a filename.
fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => c,
            ' ' | ':' | '/' | '\\' | '<' | '>' | '|' | '?' | '*' | '"' => '_',
            _ if c.is_control() => '_',
            _ => c,
        })
        .collect::<String>()
        .split('_')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("_")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_stereo_wav_header() {
        let header = create_stereo_wav_header(16000);

        assert_eq!(&header[0..4], b"RIFF");
        assert_eq!(&header[8..12], b"WAVE");
        assert_eq!(&header[12..16], b"fmt ");
        assert_eq!(&header[36..40], b"data");

        assert_eq!(u16::from_le_bytes([header[20], header[21]]), 7); // mu-law
        assert_eq!(u16::from_le_bytes([header[22], header[23]]), 2); // stereo
        assert_eq!(
            u32::from_le_bytes([header[24], header[25], header[26], header[27]]),
            8000
        ); // sample rate
        assert_eq!(
            u32::from_le_bytes([header[28], header[29], header[30], header[31]]),
            16000
        ); // byte rate
        assert_eq!(u16::from_le_bytes([header[32], header[33]]), 2); // block align
    }

    #[test]
    fn test_stereo_recorder() {
        let dir = tempdir().unwrap();
        let mut recorder =
            SessionRecorder::new(dir.path().to_str().unwrap(), "test-call", "test-recording")
                .unwrap();

        // 1 second of caller audio
        let caller_audio = vec![0x7Fu8; 8000];
        recorder.write_caller_audio(&caller_audio);

        // 0.5 seconds of callee audio (will be padded)
        let callee_audio = vec![0x80u8; 4000];
        recorder.write_callee_audio(&callee_audio);

        assert_eq!(recorder.duration_ms(), 1000);

        let path = recorder.finalize();
        assert!(path.exists());

        let metadata = std::fs::metadata(&path).unwrap();
        assert_eq!(metadata.len() as usize, WAV_HEADER_SIZE + 16000);
    }

    #[test]
    fn test_interleaving() {
        let dir = tempdir().unwrap();
        let mut recorder =
            SessionRecorder::new(dir.path().to_str().unwrap(), "test", "test-interleave").unwrap();

        recorder.write_caller_audio(&[0x10, 0x20, 0x30]);
        recorder.write_callee_audio(&[0xA0, 0xB0, 0xC0]);

        let path = recorder.finalize();
        let data = std::fs::read(&path).unwrap();
        let audio = &data[WAV_HEADER_SIZE..];

        assert_eq!(audio, &[0x10, 0xA0, 0x20, 0xB0, 0x30, 0xC0]);
    }

    #[test]
    fn test_unequal_length_padding() {
        let dir = tempdir().unwrap();
        let mut recorder =
            SessionRecorder::new(dir.path().to_str().unwrap(), "test", "test-padding").unwrap();

        recorder.write_caller_audio(&[0x10, 0x20, 0x30]);
        recorder.write_callee_audio(&[0xA0]); // Shorter

        let path = recorder.finalize();
        let data = std::fs::read(&path).unwrap();
        let audio = &data[WAV_HEADER_SIZE..];

        // Callee padded with 0xFF (silence)
        assert_eq!(audio, &[0x10, 0xA0, 0x20, 0xFF, 0x30, 0xFF]);
    }

    #[test]
    fn test_sanitize_filename() {
        assert_eq!(
            sanitize_filename("normal-file_name.123"),
            "normal-file_name.123"
        );
        assert_eq!(
            sanitize_filename("has spaces/slashes"),
            "has_spaces_slashes"
        );
        assert_eq!(sanitize_filename("a::b::c"), "a_b_c");
        assert_eq!(sanitize_filename("___multiple___"), "multiple");
    }
}
