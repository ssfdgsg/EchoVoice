use crate::dsp::AudioEffect;

/// Feed-forward compressor with peak detection and hysteresis-free smoothing.
pub struct Compressor {
    enabled: bool,
    sample_rate: u32,

    threshold_db: f32,
    ratio: f32,
    attack_ms: f32,
    release_ms: f32,
    makeup_gain_db: f32,

    /// Envelope follower state.
    env: f32,
}

impl Compressor {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            enabled: false,
            sample_rate: sample_rate.max(1),
            threshold_db: -18.0,
            ratio: 3.0,
            attack_ms: 8.0,
            release_ms: 120.0,
            makeup_gain_db: 0.0,
            env: 0.0,
        }
    }

    pub fn set_threshold(&mut self, db: f32) {
        self.threshold_db = db.clamp(-60.0, 0.0);
    }

    pub fn set_ratio(&mut self, r: f32) {
        self.ratio = r.clamp(1.0, 20.0);
    }

    pub fn set_attack(&mut self, ms: f32) {
        self.attack_ms = ms.clamp(0.5, 500.0);
    }

    pub fn set_release(&mut self, ms: f32) {
        self.release_ms = ms.clamp(5.0, 2000.0);
    }

    pub fn set_makeup_gain(&mut self, db: f32) {
        self.makeup_gain_db = db.clamp(0.0, 24.0);
    }

    pub fn threshold(&self) -> f32 {
        self.threshold_db
    }

    pub fn ratio(&self) -> f32 {
        self.ratio
    }

    pub fn attack_ms(&self) -> f32 {
        self.attack_ms
    }

    pub fn release_ms(&self) -> f32 {
        self.release_ms
    }

    pub fn makeup_gain_db(&self) -> f32 {
        self.makeup_gain_db
    }

    /// Current gain reduction in dB (negative or zero). Useful for a UI meter.
    pub fn gain_reduction_db(&self) -> f32 {
        let env_db = if self.env > 0.0 {
            20.0 * self.env.log10()
        } else {
            -144.0
        };
        if env_db > self.threshold_db {
            self.threshold_db + (env_db - self.threshold_db) / self.ratio - env_db
        } else {
            0.0
        }
    }

    pub fn reset(&mut self) {
        self.env = 0.0;
    }

    fn coefficients(&self) -> (f32, f32) {
        let attack_sec = self.attack_ms / 1000.0;
        let release_sec = self.release_ms / 1000.0;
        let alpha_a = (-1.0 / (attack_sec * self.sample_rate as f32)).exp();
        let alpha_r = (-1.0 / (release_sec * self.sample_rate as f32)).exp();
        (alpha_a, alpha_r)
    }
}

impl AudioEffect for Compressor {
    fn process(&mut self, input: &mut [f32], channels: usize) {
        if !self.enabled {
            return;
        }
        let channels = channels.max(1);
        let frames = input.len() / channels;
        if frames == 0 {
            return;
        }

        let (alpha_a, alpha_r) = self.coefficients();
        let makeup = 10.0_f32.powf(self.makeup_gain_db / 20.0);

        for frame in 0..frames {
            // Linked detection: use the loudest channel of the frame.
            let mut peak = 0.0f32;
            for ch in 0..channels {
                let sample = input[frame * channels + ch].abs();
                if sample > peak {
                    peak = sample;
                }
            }

            let alpha = if peak > self.env { alpha_a } else { alpha_r };
            self.env = alpha * self.env + (1.0 - alpha) * peak;

            let mut env_db = 20.0 * self.env.log10();
            if !env_db.is_finite() {
                env_db = -144.0;
            }

            let gain_reduction_db = if env_db > self.threshold_db {
                self.threshold_db + (env_db - self.threshold_db) / self.ratio - env_db
            } else {
                0.0
            };
            let linear_gain = 10.0_f32.powf(gain_reduction_db / 20.0) * makeup;

            for ch in 0..channels {
                let idx = frame * channels + ch;
                input[idx] *= linear_gain;
            }
        }
    }

    fn set_sample_rate(&mut self, sample_rate: u32) {
        let sample_rate = sample_rate.max(1);
        if self.sample_rate != sample_rate {
            self.sample_rate = sample_rate;
            self.reset();
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
    fn quiet_signal_is_left_alone() {
        let mut comp = Compressor::new(48_000);
        comp.set_threshold(-20.0);
        comp.set_enabled(true);
        let mut buffer = vec![0.01f32; 64];
        comp.process(&mut buffer, 1);
        assert!((buffer[63] - 0.01).abs() < 1e-6);
    }

    #[test]
    fn loud_signal_is_attenuated() {
        let mut comp = Compressor::new(48_000);
        comp.set_threshold(-20.0);
        comp.set_ratio(4.0);
        comp.set_enabled(true);
        let mut buffer = vec![0.9f32; 4800];
        comp.process(&mut buffer, 1);
        assert!(
            buffer[4799] < 0.9,
            "expected compression, got {}",
            buffer[4799]
        );
        assert!(comp.gain_reduction_db() < -1.0);
    }

    #[test]
    fn makeup_gain_boosts_the_result() {
        let mut comp = Compressor::new(48_000);
        comp.set_threshold(-20.0);
        comp.set_makeup_gain(12.0);
        comp.set_enabled(true);
        let mut buffer = vec![0.001f32; 64];
        comp.process(&mut buffer, 1);
        assert!(buffer[63] > 0.003);
    }

    #[test]
    fn works_for_mono_and_surround() {
        let mut comp = Compressor::new(48_000);
        comp.set_threshold(-30.0);
        comp.set_enabled(true);
        for channels in [1usize, 2, 6] {
            let mut buffer = vec![0.8f32; channels * 128];
            comp.process(&mut buffer, channels);
            assert!(buffer.iter().all(|s| s.is_finite() && s.abs() <= 0.8));
        }
    }
}
