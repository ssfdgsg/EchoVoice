use crate::audio::dsp::AudioEffect;
use std::f32::consts::PI;

pub struct PitchShifter {
    enabled: bool,
    sample_rate: u32,

    // Pitch shift amount (1.0 = normal, 0.5 = octave down, 2.0 = octave up)
    pitch_ratio: f32,

    // Internal delay lines
    delay_buffer: Vec<f32>,
    write_ptr: usize,

    // Two read pointers for crossfading
    read_ptr1: f32,
    read_ptr2: f32,

    // Window grain size
    window_size_frames: usize,
}

impl PitchShifter {
    pub fn new(sample_rate: u32) -> Self {
        // A standard 50ms grain size is a good default for simple voice shifting
        let window_ms = 50.0;
        let window_size_frames = ((window_ms / 1000.0) * sample_rate as f32) as usize;

        // Setup buffer length to hold at least 2 windows (stereo = * 2)
        let buffer_len = window_size_frames * 2 * 2;

        Self {
            enabled: false,
            sample_rate,
            pitch_ratio: 1.0,
            delay_buffer: vec![0.0; buffer_len],
            write_ptr: 0,
            read_ptr1: 0.0,
            read_ptr2: (window_size_frames as f32) * 0.5, // Offset half a window
            window_size_frames,
        }
    }

    pub fn set_pitch_ratio(&mut self, ratio: f32) {
        // Clamp to somewhat reasonable bounds
        self.pitch_ratio = ratio.clamp(0.25, 4.0);
    }

    fn resize_buffer(&mut self) {
        let window_ms = 50.0;
        self.window_size_frames = ((window_ms / 1000.0) * self.sample_rate as f32) as usize;
        let buffer_len = self.window_size_frames * 4; // Extra padding
        if self.delay_buffer.len() < buffer_len {
            self.delay_buffer.resize(buffer_len, 0.0);
        }
    }

    // A simple Hanning window calculation over [0, 1] range
    fn window(phase: f32) -> f32 {
        0.5 * (1.0 - (2.0 * PI * phase).cos())
    }
}

impl AudioEffect for PitchShifter {
    fn process(&mut self, input: &mut [f32]) {
        if !self.enabled || (self.pitch_ratio - 1.0).abs() < 0.01 {
            return;
        }

        let channels = 2; // Fixed interleaved stereo
        let frames = input.len() / channels;
        let buffer_cap = self.delay_buffer.len();

        for i in 0..frames {
            let idx_l = i * channels;
            let idx_r = idx_l + 1;

            // Write incoming samples to the delay buffer
            self.delay_buffer[self.write_ptr] = input[idx_l];
            self.delay_buffer[self.write_ptr + 1] = input[idx_r];

            // 1. Calculate phases
            let phase1 = (self.read_ptr1 / self.window_size_frames as f32) % 1.0;
            let phase2 = (self.read_ptr2 / self.window_size_frames as f32) % 1.0;

            // 2. Fetch interpolated samples from read streams
            let _out_l1 = 0.0; // Placeholder for interpolation
            let _out_r1 = 0.0;
            let _out_l2 = 0.0;
            let _out_r2 = 0.0;

            let read_idx1 = (self.read_ptr1 as usize) * 2;
            let read_idx2 = (self.read_ptr2 as usize) * 2;

            // Wrap indices around circular buffer
            let _limit = buffer_cap - 2;
            // Actually, because time-domain pitch shifting without complex overlap-add
            // or FFT is highly artifact-prone for high latency, we implement a very naive
            // delay modulation just to prove the pipeline path for now.
            // *Real* high-fidelity real-time voice shifting requires Phase Vocoder.

            // Fallback simplistic read from history
            let hist_idx1 =
                ((self.write_ptr as i32 - read_idx1 as i32).rem_euclid(buffer_cap as i32)) as usize;
            let hist_idx2 =
                ((self.write_ptr as i32 - read_idx2 as i32).rem_euclid(buffer_cap as i32)) as usize;

            let mix_l1 = self.delay_buffer[hist_idx1];
            let mix_r1 = self.delay_buffer[hist_idx1 + 1];

            let mix_l2 = self.delay_buffer[hist_idx2];
            let mix_r2 = self.delay_buffer[hist_idx2 + 1];

            // Apply Crossfade windows
            let w1 = Self::window(phase1);
            let w2 = Self::window(phase2);

            let out_l = (mix_l1 * w1) + (mix_l2 * w2);
            let out_r = (mix_r1 * w1) + (mix_r2 * w2);

            input[idx_l] = out_l;
            input[idx_r] = out_r;

            // Advance pointers
            self.write_ptr = (self.write_ptr + 2) % buffer_cap;

            // The read speed compared to write speed dictates pitch change
            // ratio = 1.0 -> same speed
            // ratio = 2.0 -> read twice as fast (octave up)
            self.read_ptr1 += self.pitch_ratio;
            self.read_ptr2 += self.pitch_ratio;

            // Loop grains back
            if self.read_ptr1 >= self.window_size_frames as f32 {
                self.read_ptr1 -= self.window_size_frames as f32;
            }
            if self.read_ptr2 >= self.window_size_frames as f32 {
                self.read_ptr2 -= self.window_size_frames as f32;
            }
        }
    }

    fn set_sample_rate(&mut self, sample_rate: u32) {
        if self.sample_rate != sample_rate {
            self.sample_rate = sample_rate;
            self.resize_buffer();
        }
    }

    fn is_enabled(&self) -> bool {
        self.enabled
    }

    fn set_enabled(&mut self, enabled: bool) {
        if enabled && !self.enabled {
            // Reset state on enable
            self.write_ptr = 0;
            self.read_ptr1 = 0.0;
            self.read_ptr2 = (self.window_size_frames as f32) * 0.5;
            for x in self.delay_buffer.iter_mut() {
                *x = 0.0;
            }
        }
        self.enabled = enabled;
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}
