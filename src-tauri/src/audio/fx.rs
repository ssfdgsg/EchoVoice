use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use symphonia::core::audio::AudioBufferRef;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

pub struct SoundBank {
    pub sample_rate: u32,
    pub channels: u16,
    pub data: Vec<f32>,
}

impl SoundBank {
    pub fn load_from_file(path: &str, target_sample_rate: u32) -> Result<Self, String> {
        let file = std::fs::File::open(path).map_err(|e| format!("File open err: {}", e))?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());

        let mut hint = Hint::new();
        if let Some(ext) = std::path::Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
        {
            hint.with_extension(ext);
        }

        let meta_opts: MetadataOptions = Default::default();
        let fmt_opts: FormatOptions = Default::default();

        let probed = symphonia::default::get_probe()
            .format(&hint, mss, &fmt_opts, &meta_opts)
            .map_err(|e| format!("Probe err: {}", e))?;

        let mut format = probed.format;

        // Find the first audio track
        let track = format
            .tracks()
            .iter()
            .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
            .ok_or("No supported audio tracks")?;

        let track_id = track.id;
        let p = &track.codec_params;
        let source_sr = p.sample_rate.unwrap_or(48000);
        let channels = p.channels.map(|c| c.count() as u16).unwrap_or(2);

        let dec_opts: DecoderOptions = Default::default();
        let mut decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &dec_opts)
            .map_err(|e| format!("Decoder err: {}", e))?;

        let mut raw_frames: Vec<Vec<f32>> = vec![Vec::new(); channels as usize];

        // Decode loop
        loop {
            let packet = match format.next_packet() {
                Ok(packet) => packet,
                Err(Error::ResetRequired) => {
                    decoder.reset();
                    continue;
                }
                Err(Error::IoError(ref e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    break;
                }
                Err(e) => return Err(format!("Format read err: {}", e)),
            };

            if packet.track_id() != track_id {
                continue;
            }

            match decoder.decode(&packet) {
                Ok(audio_buf) => {
                    Self::extract_channels(&audio_buf, &mut raw_frames);
                }
                Err(Error::DecodeError(_)) => continue, // Ignore decode errors
                Err(e) => return Err(format!("Decode err: {}", e)),
            }
        }

        // Resample if necessary
        let final_data = if source_sr != target_sample_rate {
            Self::resample(raw_frames, source_sr, target_sample_rate)?
        } else {
            raw_frames
        };

        // Interleave back to standard f32 array ([L, R, L, R])
        let frames_count = final_data[0].len();
        let mut interleaved = Vec::with_capacity(frames_count * channels as usize);
        for f in 0..frames_count {
            for c in 0..channels as usize {
                interleaved.push(final_data[c][f]);
            }
        }

        Ok(SoundBank {
            sample_rate: target_sample_rate,
            channels,
            data: interleaved,
        })
    }

    fn extract_channels(audio_buf: &AudioBufferRef, out: &mut Vec<Vec<f32>>) {
        match audio_buf {
            AudioBufferRef::F32(buf) => {
                for (ch_idx, ch_data) in buf.planes().planes().iter().enumerate() {
                    if ch_idx < out.len() {
                        out[ch_idx].extend_from_slice(ch_data);
                    }
                }
            }
            // For MVP, we handle standard 16-bit implicitly by symphonia converters,
            // but Symphonia yields native buffers. We'll use the generic sample extraction:
            _ => {
                // If it's not F32 natively, we can use a f32 conversion buffer:
                let mut f32_buf = symphonia::core::audio::SampleBuffer::<f32>::new(
                    audio_buf.capacity() as u64,
                    *audio_buf.spec(),
                );
                f32_buf.copy_interleaved_ref(audio_buf.clone());
                let samples = f32_buf.samples();
                let channels = audio_buf.spec().channels.count();
                for (i, &sample) in samples.iter().enumerate() {
                    let ch = i % channels;
                    if ch < out.len() {
                        out[ch].push(sample);
                    }
                }
            }
        }
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
        .map_err(|e| format!("Resampler init err: {}", e))?;

        let mut output = vec![Vec::new(); channels_data.len()];
        let in_len = channels_data[0].len();

        let mut pos = 0;
        while pos < in_len {
            let end = std::cmp::min(pos + chunk_size, in_len);
            let mut chunk = Vec::new();
            for ch in &channels_data {
                chunk.push(ch[pos..end].to_vec());
            }

            // pad if exact chunk is needed but less available:
            if chunk[0].len() < chunk_size {
                for c in &mut chunk {
                    c.resize(chunk_size, 0.0);
                }
            }

            match resampler.process(&chunk, None) {
                Ok(resampled) => {
                    for (i, out_ch) in resampled.into_iter().enumerate() {
                        output[i].extend(out_ch);
                    }
                }
                Err(e) => return Err(format!("Resample process err: {}", e)),
            }
            pos += chunk_size;
        }

        Ok(output)
    }
}
