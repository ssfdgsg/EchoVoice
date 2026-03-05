use crate::audio::dsp::AudioEffect;

pub struct Compressor {
    enabled: bool,
    sample_rate: u32,

    // Parameters
    threshold_db: f32,   // e.g., -20.0
    ratio: f32,          // e.g., 4.0 for 4:1 compression
    attack_ms: f32,      // e.g., 5.0
    release_ms: f32,     // e.g., 50.0
    makeup_gain_db: f32, // e.g., 0.0

    // Internal state
    env: f32,
}

impl Compressor {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            enabled: false,
            sample_rate,
            threshold_db: -20.0,
            ratio: 4.0,
            attack_ms: 10.0,
            release_ms: 100.0,
            makeup_gain_db: 0.0,
            env: 0.0,
        }
    }

    pub fn set_threshold(&mut self, db: f32) {
        self.threshold_db = db;
    }

    pub fn set_ratio(&mut self, r: f32) {
        self.ratio = r.max(1.0);
    }

    pub fn set_attack(&mut self, ms: f32) {
        self.attack_ms = ms.max(0.1);
    }

    pub fn set_release(&mut self, ms: f32) {
        self.release_ms = ms.max(1.0);
    }

    pub fn set_makeup_gain(&mut self, db: f32) {
        self.makeup_gain_db = db;
    }

    fn calculate_coefficients(&self) -> (f32, f32) {
        // alpha = exp(-1.0 / (time_sec * sample_rate))
        // approx: 1.0 - exp(...)
        let attack_sec = self.attack_ms / 1000.0;
        let release_sec = self.release_ms / 1000.0;

        let alpha_a = (-1.0 / (attack_sec * self.sample_rate as f32)).exp();
        let alpha_r = (-1.0 / (release_sec * self.sample_rate as f32)).exp();

        (alpha_a, alpha_r)
    }
}

impl AudioEffect for Compressor {
    fn process(&mut self, input: &mut [f32]) {
        if !self.enabled {
            return;
        }

        let channels = 2; // Stereo interleaved
        let frames = input.len() / channels;
        let (alpha_a, alpha_r) = self.calculate_coefficients();
        let makeup = 10.0_f32.powf(self.makeup_gain_db / 20.0);

        for i in 0..frames {
            // Calculate sidechain linked RMS (or peak, we'll use peak for simplicity here)
            // L and R
            let idx_l = i * channels;
            let idx_r = i * channels + 1;
            let sample_l = input[idx_l];
            let sample_r = input[idx_r];

            // Use the max absolute value of L/R for detection
            let peak = sample_l.abs().max(sample_r.abs());

            // Smooth envelope
            if peak > self.env {
                self.env = alpha_a * self.env + (1.0 - alpha_a) * peak;
            } else {
                self.env = alpha_r * self.env + (1.0 - alpha_r) * peak;
            }

            // Convert envelope to dB
            let mut env_db = 20.0 * self.env.log10();
            if env_db < -144.0 {
                env_db = -144.0; // clamp -inf
            }

            // Calculate gain reduction in dB
            let gain_reduction_db = if env_db > self.threshold_db {
                self.threshold_db + (env_db - self.threshold_db) / self.ratio - env_db
            } else {
                0.0
            };

            // Convert gain reduction back to linear multiplier
            let linear_gain = 10.0_f32.powf(gain_reduction_db / 20.0);

            // Apply gain and makeup to both channels
            input[idx_l] = sample_l * linear_gain * makeup;
            input[idx_r] = sample_r * linear_gain * makeup;
        }
    }

    fn set_sample_rate(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate;
    }

    fn is_enabled(&self) -> bool {
        self.enabled
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}
