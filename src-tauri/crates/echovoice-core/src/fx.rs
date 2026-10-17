//! Audio file decoding into the internal interleaved `f32` format.

use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use symphonia::core::audio::{AudioBufferRef, SampleBuffer, SignalSpec};
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;

/// Decoded audio, always interleaved `f32` at the requested rate/channel count.
#[derive(Debug, Clone)]
pub struct SoundBank {
    pub sample_rate: u32,
    pub channels: u16,
    /// Interleaved samples: `[L, R, L, R, ...]`.
    pub data: Vec<f32>,
}

impl SoundBank {
    /// Decodes `path` and converts it to `target_sample_rate` / `target_channels`.
    pub fn load_from_file(
        path: &str,
        target_sample_rate: u32,
        target_channels: u16,
    ) -> Result<Self, String> {
        if target_sample_rate == 0 {
            return Err("Target sample rate must be greater than zero".to_string());
        }
        if target_channels == 0 {
            return Err("Target channel count must be greater than zero".to_string());
        }

        let file = std::fs::File::open(path).map_err(|err| format!("File open err: {err}"))?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());

        let mut hint = Hint::new();
        if let Some(ext) = std::path::Path::new(path)
            .extension()
            .and_then(|ext| ext.to_str())
        {
            hint.with_extension(ext);
        }

        let meta_opts: MetadataOptions = Default::default();
        let fmt_opts: FormatOptions = Default::default();

        let probed = symphonia::default::get_probe()
            .format(&hint, mss, &fmt_opts, &meta_opts)
            .map_err(|err| format!("Probe err: {err}"))?;

        let mut format = probed.format;

        let track = format
            .tracks()
            .iter()
            .find(|track| track.codec_params.codec != CODEC_TYPE_NULL)
            .ok_or("No supported audio tracks")?;

        let track_id = track.id;
        let params = &track.codec_params;
        let source_sr = params.sample_rate.unwrap_or(48_000);
        let source_channels = params.channels.map(|ch| ch.count()).unwrap_or(2).max(1);

        let dec_opts: DecoderOptions = Default::default();
        let mut decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &dec_opts)
            .map_err(|err| format!("Decoder err: {err}"))?;

        let mut raw_frames: Vec<Vec<f32>> = vec![Vec::new(); source_channels];
        let mut actual_channels = source_channels;
        let mut actual_rate = source_sr;

        loop {
            let packet = match format.next_packet() {
                Ok(packet) => packet,
                Err(Error::ResetRequired) => {
                    decoder.reset();
                    continue;
                }
                Err(Error::IoError(ref err)) if err.kind() == std::io::ErrorKind::UnexpectedEof => {
                    break;
                }
                Err(Error::DecodeError(_)) => continue,
                Err(err) => return Err(format!("Format read err: {err}")),
            };

            if packet.track_id() != track_id {
                continue;
            }

            match decoder.decode(&packet) {
                Ok(audio_buf) => {
                    let spec = *audio_buf.spec();
                    actual_rate = spec.rate;
                    let channels = spec.channels.count().max(1);
                    if channels != actual_channels {
                        actual_channels = channels;
                        raw_frames = vec![Vec::new(); channels];
                    }
                    Self::extract_channels(&audio_buf, channels, &mut raw_frames);
                }
                Err(Error::DecodeError(_)) => continue,
                Err(err) => return Err(format!("Decode err: {err}")),
            }
        }

        if raw_frames.is_empty() || raw_frames[0].is_empty() {
            return Err("Audio file contains no decodable samples".to_string());
        }

        let final_data = if actual_rate != target_sample_rate {
            Self::resample(raw_frames, actual_rate, target_sample_rate)?
        } else {
            raw_frames
        };

        let final_data = Self::remap_channels(final_data, target_channels as usize);
        let frames_count = final_data[0].len();
        let mut interleaved = Vec::with_capacity(frames_count * target_channels as usize);
        for frame in 0..frames_count {
            for channel in &final_data {
                interleaved.push(channel[frame]);
            }
        }

        Ok(SoundBank {
            sample_rate: target_sample_rate,
            channels: target_channels,
            data: interleaved,
        })
    }

    /// Length of the decoded audio in milliseconds.
    pub fn duration_ms(&self) -> u32 {
        let channels = self.channels.max(1) as usize;
        let frames = self.data.len() / channels;
        ((frames as f32 / self.sample_rate.max(1) as f32) * 1000.0) as u32
    }

    /// Peak absolute sample value, used to report over-loud imports.
    pub fn peak(&self) -> f32 {
        self.data
            .iter()
            .fold(0.0f32, |peak, sample| peak.max(sample.abs()))
    }

    fn extract_channels(audio_buf: &AudioBufferRef, channels: usize, out: &mut Vec<Vec<f32>>) {
        if out.len() != channels {
            out.resize(channels, Vec::new());
        }
        let spec: SignalSpec = *audio_buf.spec();
        let mut buffer = SampleBuffer::<f32>::new(audio_buf.capacity() as u64, spec);
        buffer.copy_interleaved_ref(audio_buf.clone());
        let samples = buffer.samples();
        for (index, sample) in samples.iter().enumerate() {
            let channel = index % channels;
            if let Some(target) = out.get_mut(channel) {
                target.push(*sample);
            }
        }
    }

    fn remap_channels(channels_data: Vec<Vec<f32>>, target_channels: usize) -> Vec<Vec<f32>> {
        let source_channels = channels_data.len();
        let frames = channels_data.first().map_or(0, Vec::len);

        if source_channels == target_channels || target_channels == 0 {
            return channels_data;
        }

        if target_channels == 1 {
            let mut mono = vec![0.0; frames];
            for channel in &channels_data {
                for (sample_out, sample_in) in mono.iter_mut().zip(channel.iter()) {
                    *sample_out += *sample_in / source_channels as f32;
                }
            }
            return vec![mono];
        }

        if source_channels == 1 {
            return vec![channels_data[0].clone(); target_channels];
        }

        let mut remapped = vec![vec![0.0; frames]; target_channels];
        for (target, source) in remapped.iter_mut().zip(channels_data.iter()) {
            target.copy_from_slice(source);
        }
        remapped
    }

    fn resample(
        channels_data: Vec<Vec<f32>>,
        from_sr: u32,
        to_sr: u32,
    ) -> Result<Vec<Vec<f32>>, String> {
        let params = SincInterpolationParameters {
            sinc_len: 256,
            f_cutoff: 0.95,
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: 256,
            window: WindowFunction::BlackmanHarris2,
        };

        let chunk_size = 1024;
        let mut resampler = SincFixedIn::<f32>::new(
            to_sr as f64 / from_sr as f64,
            2.0,
            params,
            chunk_size,
            channels_data.len(),
        )
        .map_err(|err| format!("Resampler init err: {err}"))?;

        let mut output = vec![Vec::new(); channels_data.len()];
        let in_len = channels_data[0].len();

        let mut pos = 0;
        while pos < in_len {
            let end = (pos + chunk_size).min(in_len);
            let mut chunk = Vec::with_capacity(channels_data.len());
            for channel in &channels_data {
                let mut slice = channel[pos..end].to_vec();
                // rubato needs exact size chunks; pad the tail with silence.
                slice.resize(chunk_size, 0.0);
                chunk.push(slice);
            }

            match resampler.process(&chunk, None) {
                Ok(resampled) => {
                    for (index, out_ch) in resampled.into_iter().enumerate() {
                        output[index].extend(out_ch);
                    }
                }
                Err(err) => return Err(format!("Resample process err: {err}")),
            }
            pos += chunk_size;
        }

        Ok(output)
    }
}

/// Re-exported so the engine can report timestamps without depending on symphonia.
pub type AudioTime = Time;

#[cfg(test)]
mod tests {
    use super::SoundBank;

    #[test]
    fn duplicates_mono_into_stereo() {
        let mapped = SoundBank::remap_channels(vec![vec![0.25, -0.5]], 2);
        assert_eq!(mapped, vec![vec![0.25, -0.5], vec![0.25, -0.5]]);
    }

    #[test]
    fn averages_stereo_into_mono() {
        let mapped = SoundBank::remap_channels(vec![vec![1.0, -1.0], vec![0.0, 0.5]], 1);
        assert_eq!(mapped, vec![vec![0.5, -0.25]]);
    }

    #[test]
    fn keeps_surround_layout_untouched() {
        let mapped = SoundBank::remap_channels(
            vec![
                vec![1.0],
                vec![2.0],
                vec![3.0],
                vec![4.0],
                vec![5.0],
                vec![6.0],
            ],
            6,
        );
        assert_eq!(mapped[5], vec![6.0]);
    }

    #[test]
    fn downmixes_surround_to_stereo_by_dropping_extra_channels() {
        let mapped = SoundBank::remap_channels(vec![vec![1.0], vec![2.0], vec![3.0]], 2);
        assert_eq!(mapped, vec![vec![1.0], vec![2.0]]);
    }

    #[test]
    fn rejects_missing_files() {
        let err = SoundBank::load_from_file("definitely-not-here.wav", 48_000, 2).unwrap_err();
        assert!(err.contains("File open err"), "{err}");
    }

    #[test]
    fn duration_is_derived_from_the_frame_count() {
        let bank = SoundBank {
            sample_rate: 48_000,
            channels: 2,
            data: vec![0.0; 48_000 * 2],
        };
        assert_eq!(bank.duration_ms(), 1000);
        assert_eq!(bank.peak(), 0.0);
    }
}
