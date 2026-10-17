use crate::dsp::AudioEffect;
use std::f32::consts::PI;

/// Grain size of the pitch shifter in milliseconds. 50 ms is the usual compromise
/// between smearing and latency for a two-grain crossfade shifter.
const GRAIN_MS: f32 = 50.0;
/// How many grains of history the circular buffer keeps.
const BUFFER_GRAINS: usize = 4;

/// Time-domain pitch shifter: two overlapping grains read at a different speed than the
/// write pointer, crossfaded with Hanning windows. Channel count agnostic.
pub struct PitchShifter {
    enabled: bool,
    sample_rate: u32,
    pitch_ratio: f32,

    /// Circular history buffer, laid out as `frames * channels`.
    delay_buffer: Vec<f32>,
    channels: usize,
    buffer_frames: usize,
    write_frame: usize,

    /// Read cursors in frames (fractional, linearly interpolated).
    read_ptr1: f32,
    read_ptr2: f32,
    grain_frames: usize,
}

impl PitchShifter {
    pub fn new(sample_rate: u32) -> Self {
        Self::with_channels(sample_rate, 2)
    }

    pub fn with_channels(sample_rate: u32, channels: usize) -> Self {
        let sample_rate = sample_rate.max(1);
        let channels = channels.clamp(1, 8);
        let grain_frames = ((GRAIN_MS / 1000.0) * sample_rate as f32) as usize;
        let grain_frames = grain_frames.max(16);
        let buffer_frames = grain_frames * BUFFER_GRAINS;
        Self {
            enabled: false,
            sample_rate,
            pitch_ratio: 1.0,
            delay_buffer: vec![0.0; buffer_frames * channels],
            channels,
            buffer_frames,
            write_frame: 0,
            read_ptr1: 0.0,
            read_ptr2: grain_frames as f32 * 0.5,
            grain_frames,
        }
    }

    pub fn set_pitch_ratio(&mut self, ratio: f32) {
        self.pitch_ratio = ratio.clamp(0.25, 4.0);
    }

    pub fn pitch_ratio(&self) -> f32 {
        self.pitch_ratio
    }

    fn resize(&mut self, sample_rate: u32, channels: usize) {
        let grain_frames = ((GRAIN_MS / 1000.0) * sample_rate as f32) as usize;
        let grain_frames = grain_frames.max(16);
        let buffer_frames = grain_frames * BUFFER_GRAINS;
        if grain_frames != self.grain_frames || channels != self.channels {
            self.grain_frames = grain_frames;
            self.channels = channels;
            self.buffer_frames = buffer_frames;
            self.delay_buffer = vec![0.0; buffer_frames * channels];
            self.reset();
        } else if self.delay_buffer.len() < buffer_frames * channels {
            self.buffer_frames = buffer_frames;
            self.delay_buffer.resize(buffer_frames * channels, 0.0);
            self.reset();
        }
    }

    fn reset(&mut self) {
        for sample in self.delay_buffer.iter_mut() {
            *sample = 0.0;
        }
        self.write_frame = 0;
        self.read_ptr1 = 0.0;
        self.read_ptr2 = self.grain_frames as f32 * 0.5;
    }

    #[inline]
    fn window(phase: f32) -> f32 {
        0.5 * (1.0 - (2.0 * PI * phase).cos())
    }

    /// Linearly interpolated read `lag` frames behind the write pointer.
    #[inline]
    fn read_lag(&self, lag: f32, out: &mut [f32]) {
        let channels = self.channels;
        let frames = self.buffer_frames;
        let base = self.write_frame as f32 - lag;
        // Bring into [0, frames)
        let mut position = base.rem_euclid(frames as f32);
        if position < 0.0 {
            position += frames as f32;
        }
        let index = position.floor();
        let frac = position - index;
        let i0 = (index as usize) % frames;
        let i1 = (i0 + 1) % frames;
        let first = &self.delay_buffer[i0 * channels..i0 * channels + channels];
        let second = &self.delay_buffer[i1 * channels..i1 * channels + channels];
        for (slot, (&a, &b)) in out.iter_mut().zip(first.iter().zip(second.iter())) {
            *slot = a + (b - a) * frac;
        }
    }
}

impl AudioEffect for PitchShifter {
    fn process(&mut self, input: &mut [f32], channels: usize) {
        if !self.enabled || (self.pitch_ratio - 1.0).abs() < 0.01 {
            return;
        }
        let channels = channels.clamp(1, 8);
        if channels != self.channels {
            self.resize(self.sample_rate, channels);
        }
        let frames = input.len() / channels;
        if frames == 0 {
            return;
        }

        let mut grain_a = [0.0f32; 8];
        let mut grain_b = [0.0f32; 8];

        for frame in input.chunks_exact_mut(channels).take(frames) {
            // Store the incoming frame.
            for (ch, sample) in frame.iter().enumerate() {
                self.delay_buffer[self.write_frame * channels + ch] = *sample;
            }

            let phase1 = (self.read_ptr1 / self.grain_frames as f32).rem_euclid(1.0);
            let phase2 = (self.read_ptr2 / self.grain_frames as f32).rem_euclid(1.0);
            self.read_lag(self.read_ptr1, &mut grain_a);
            self.read_lag(self.read_ptr2, &mut grain_b);
            let w1 = Self::window(phase1);
            let w2 = Self::window(phase2);

            for (ch, sample) in frame.iter_mut().enumerate() {
                *sample = grain_a[ch] * w1 + grain_b[ch] * w2;
            }

            self.write_frame = (self.write_frame + 1) % self.buffer_frames;
            self.read_ptr1 += self.pitch_ratio;
            self.read_ptr2 += self.pitch_ratio;
            if self.read_ptr1 >= self.grain_frames as f32 {
                self.read_ptr1 -= self.grain_frames as f32;
            }
            if self.read_ptr2 >= self.grain_frames as f32 {
                self.read_ptr2 -= self.grain_frames as f32;
            }
        }
    }

    fn set_sample_rate(&mut self, sample_rate: u32) {
        let sample_rate = sample_rate.max(1);
        if self.sample_rate != sample_rate {
            self.sample_rate = sample_rate;
            self.resize(sample_rate, self.channels);
        }
    }

    fn is_enabled(&self) -> bool {
        self.enabled
    }

    fn set_enabled(&mut self, enabled: bool) {
        if enabled && !self.enabled {
            self.reset();
        }
        self.enabled = enabled;
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_shifter_is_a_no_op() {
        let mut shifter = PitchShifter::new(48_000);
        let mut buffer = vec![0.25f32; 64];
        shifter.process(&mut buffer, 2);
        assert_eq!(buffer, vec![0.25f32; 64]);
    }

    #[test]
    fn ratio_of_one_is_a_no_op() {
        let mut shifter = PitchShifter::new(48_000);
        shifter.set_pitch_ratio(1.0);
        shifter.set_enabled(true);
        let mut buffer = vec![0.25f32; 64];
        shifter.process(&mut buffer, 2);
        assert_eq!(buffer, vec![0.25f32; 64]);
    }

    #[test]
    fn shifting_keeps_the_signal_finite_and_bounded() {
        let mut shifter = PitchShifter::new(48_000);
        shifter.set_pitch_ratio(1.5);
        shifter.set_enabled(true);
        let mut buffer: Vec<f32> = (0..4800)
            .map(|i| (2.0 * PI * 220.0 * i as f32 / 48_000.0).sin() * 0.8)
            .collect();
        shifter.process(&mut buffer, 1);
        assert!(buffer.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
    }

    #[test]
    fn sample_rate_change_reallocates_the_delay_line() {
        let mut shifter = PitchShifter::new(48_000);
        shifter.set_pitch_ratio(2.0);
        shifter.set_enabled(true);
        let mut buffer = vec![0.5f32; 512];
        shifter.process(&mut buffer, 2);
        shifter.set_sample_rate(44_100);
        let mut buffer = vec![0.5f32; 512];
        shifter.process(&mut buffer, 2);
        assert!(buffer.iter().all(|s| s.is_finite()));
    }
}
