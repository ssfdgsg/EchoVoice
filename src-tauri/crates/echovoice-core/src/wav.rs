//! Minimal WAV reader/writer used by the microphone recorder.

use std::fs;
use std::io::Write;
use std::path::Path;

/// Accumulates interleaved `f32` samples and serialises them as 16-bit PCM WAV.
#[derive(Debug, Clone)]
pub struct WavWriter {
    sample_rate: u32,
    channels: u16,
    samples: Vec<f32>,
}

impl WavWriter {
    pub fn new(sample_rate: u32, channels: u16) -> Self {
        Self {
            sample_rate: sample_rate.max(1),
            channels: channels.clamp(1, 8),
            samples: Vec::new(),
        }
    }

    pub fn push_interleaved(&mut self, samples: &[f32]) {
        self.samples.extend_from_slice(samples);
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels as usize
    }

    pub fn duration_ms(&self) -> u32 {
        ((self.frames() as f32 / self.sample_rate as f32) * 1000.0) as u32
    }

    pub fn peak(&self) -> f32 {
        self.samples
            .iter()
            .fold(0.0f32, |peak, sample| peak.max(sample.abs()))
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Writes a 16-bit PCM RIFF/WAVE file.
    pub fn write_to_file(&self, path: &str) -> Result<(), String> {
        let path = Path::new(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|err| format!("Create dir err: {err}"))?;
        }

        let channels = self.channels;
        let bits_per_sample = 16u16;
        let block_align = channels * bits_per_sample / 8;
        let byte_rate = self.sample_rate * block_align as u32;
        let data_bytes = (self.samples.len() * 2) as u32;

        let mut file = fs::File::create(path).map_err(|err| format!("File create err: {err}"))?;

        // RIFF header
        file.write_all(b"RIFF").map_err(write_err)?;
        file.write_all(&(36 + data_bytes).to_le_bytes())
            .map_err(write_err)?;
        file.write_all(b"WAVE").map_err(write_err)?;

        // fmt chunk
        file.write_all(b"fmt ").map_err(write_err)?;
        file.write_all(&16u32.to_le_bytes()).map_err(write_err)?;
        file.write_all(&1u16.to_le_bytes()).map_err(write_err)?; // PCM
        file.write_all(&channels.to_le_bytes()).map_err(write_err)?;
        file.write_all(&self.sample_rate.to_le_bytes())
            .map_err(write_err)?;
        file.write_all(&byte_rate.to_le_bytes())
            .map_err(write_err)?;
        file.write_all(&block_align.to_le_bytes())
            .map_err(write_err)?;
        file.write_all(&bits_per_sample.to_le_bytes())
            .map_err(write_err)?;

        // data chunk
        file.write_all(b"data").map_err(write_err)?;
        file.write_all(&data_bytes.to_le_bytes())
            .map_err(write_err)?;

        let mut buffer = Vec::with_capacity(self.samples.len() * 2);
        for sample in &self.samples {
            let clamped = sample.clamp(-1.0, 1.0);
            let quantised = (clamped * 32_767.0) as i16;
            buffer.extend_from_slice(&quantised.to_le_bytes());
        }
        file.write_all(&buffer).map_err(write_err)?;
        file.flush().map_err(write_err)?;

        Ok(())
    }
}

fn write_err(err: std::io::Error) -> String {
    format!("Write err: {err}")
}

/// Microphone recorder with a hard length guard so memory stays bounded.
#[derive(Debug)]
pub struct MicRecorder {
    writer: WavWriter,
    max_frames: usize,
    full: bool,
}

impl MicRecorder {
    pub fn new(sample_rate: u32, channels: u16, max_seconds: u32) -> Self {
        let sample_rate = sample_rate.max(1);
        Self {
            writer: WavWriter::new(sample_rate, channels),
            max_frames: sample_rate as usize * max_seconds.max(1) as usize,
            full: false,
        }
    }

    /// Appends audio; returns `false` once the length limit has been reached.
    pub fn push_interleaved(&mut self, samples: &[f32]) -> bool {
        if self.full {
            return false;
        }
        let channels = self.writer.channels() as usize;
        let available = self.max_frames.saturating_sub(self.writer.frames()) * channels;
        let take = samples.len().min(available);
        self.writer.push_interleaved(&samples[..take]);
        if take < samples.len() || self.writer.frames() >= self.max_frames {
            self.full = true;
        }
        !self.full
    }

    pub fn is_full(&self) -> bool {
        self.full
    }

    pub fn duration_ms(&self) -> u32 {
        self.writer.duration_ms()
    }

    pub fn peak(&self) -> f32 {
        self.writer.peak()
    }

    pub fn into_writer(self) -> WavWriter {
        self.writer
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> std::path::PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("echovoice-wav-{}-{name}", std::process::id()));
        path
    }

    #[test]
    fn writes_a_valid_wav_header() {
        let dir = temp_path("header");
        let path = dir.join("out.wav");
        let mut writer = WavWriter::new(48_000, 2);
        writer.push_interleaved(&[0.5f32, -0.5, 0.25, -0.25]);
        writer.write_to_file(path.to_str().unwrap()).unwrap();

        let bytes = fs::read(&path).unwrap();
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(&bytes[12..16], b"fmt ");
        assert_eq!(&bytes[36..40], b"data");
        // 4 samples * 2 bytes
        assert_eq!(u32::from_le_bytes(bytes[40..44].try_into().unwrap()), 8);
        // 16-bit little endian quantisation of 0.5
        assert_eq!(i16::from_le_bytes(bytes[44..46].try_into().unwrap()), 16383);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn round_trips_through_a_reader() {
        let dir = temp_path("roundtrip");
        let path = dir.join("tone.wav");
        let mut writer = WavWriter::new(8_000, 1);
        let tone: Vec<f32> = (0..800).map(|i| (i as f32 / 800.0) - 0.5).collect();
        writer.push_interleaved(&tone);
        writer.write_to_file(path.to_str().unwrap()).unwrap();

        let bank = crate::fx::SoundBank::load_from_file(path.to_str().unwrap(), 8_000, 1).unwrap();
        assert_eq!(bank.channels, 1);
        assert_eq!(bank.sample_rate, 8_000);
        assert_eq!(bank.data.len(), 800);
        // 16-bit quantisation loses precision, allow ~1 LSB.
        assert!((bank.data[400] - tone[400]).abs() < 1e-4);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn recorder_stops_at_the_length_limit() {
        let mut recorder = MicRecorder::new(48_000, 1, 1);
        assert!(recorder.push_interleaved(&vec![0.1; 47_999]));
        assert!(!recorder.is_full());
        // The next chunk would exceed the limit, so recording is closed.
        assert!(!recorder.push_interleaved(&[0.1; 10]));
        assert!(recorder.is_full());
        assert_eq!(recorder.duration_ms(), 1000);
    }
}
