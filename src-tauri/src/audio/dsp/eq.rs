use crate::audio::dsp::AudioEffect;
use std::f32::consts::PI;

#[derive(Clone, Copy)]
pub enum FilterType {
    LowShelf,
    Peaking,
    HighShelf,
}

pub struct BiquadFilter {
    filter_type: FilterType,
    sample_rate: u32,
    freq: f32,
    q: f32,
    gain_db: f32,

    // Coefficients
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,

    // State (we need one set of states per channel, so we assume stereo and use an array or just process mono.
    // Since EchoVoice uses interleaved stereo (channels=2), we keep 2 sets.
    z1: [f32; 2],
    z2: [f32; 2],
}

impl BiquadFilter {
    pub fn new(filter_type: FilterType, sample_rate: u32, freq: f32, q: f32, gain_db: f32) -> Self {
        let mut filter = Self {
            filter_type,
            sample_rate,
            freq,
            q,
            gain_db,
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            z1: [0.0; 2],
            z2: [0.0; 2],
        };
        filter.calculate_coefficients();
        filter
    }

    pub fn set_gain(&mut self, gain_db: f32) {
        self.gain_db = gain_db;
        self.calculate_coefficients();
    }

    pub fn set_sample_rate(&mut self, sample_rate: u32) {
        if self.sample_rate != sample_rate {
            self.sample_rate = sample_rate;
            self.calculate_coefficients();
        }
    }

    fn calculate_coefficients(&mut self) {
        let a = 10.0_f32.powf(self.gain_db / 40.0);
        let omega = 2.0 * PI * self.freq / (self.sample_rate as f32);
        let sn = omega.sin();
        let cs = omega.cos();
        let alpha = sn / (2.0 * self.q);

        let (b0, b1, b2, a0, a1, a2) = match self.filter_type {
            FilterType::LowShelf => {
                let beta = (a).sqrt() * 2.0 * alpha;
                (
                    a * ((a + 1.0) - (a - 1.0) * cs + beta),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cs),
                    a * ((a + 1.0) - (a - 1.0) * cs - beta),
                    (a + 1.0) + (a - 1.0) * cs + beta,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cs),
                    (a + 1.0) + (a - 1.0) * cs - beta,
                )
            }
            FilterType::Peaking => (
                1.0 + alpha * a,
                -2.0 * cs,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * cs,
                1.0 - alpha / a,
            ),
            FilterType::HighShelf => {
                let beta = (a).sqrt() * 2.0 * alpha;
                (
                    a * ((a + 1.0) + (a - 1.0) * cs + beta),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cs),
                    a * ((a + 1.0) + (a - 1.0) * cs - beta),
                    (a + 1.0) - (a - 1.0) * cs + beta,
                    2.0 * ((a - 1.0) - (a + 1.0) * cs),
                    (a + 1.0) - (a - 1.0) * cs - beta,
                )
            }
        };

        let a0_inv = 1.0 / a0;
        self.b0 = b0 * a0_inv;
        self.b1 = b1 * a0_inv;
        self.b2 = b2 * a0_inv;
        self.a1 = a1 * a0_inv;
        self.a2 = a2 * a0_inv;
    }

    pub fn process_interleaved(&mut self, input: &mut [f32]) {
        // Assume stereo processing
        let channels = 2;
        let frames = input.len() / channels;

        for i in 0..frames {
            for ch in 0..channels {
                let idx = i * channels + ch;
                let in_sample = input[idx];

                let out_sample =
                    self.b0 * in_sample + self.b1 * self.z1[ch] + self.b2 * self.z2[ch]
                        - self.a1 * self.z1[ch]
                        - self.a2 * self.z2[ch];

                self.z2[ch] = self.z1[ch];
                self.z1[ch] = out_sample;

                input[idx] = out_sample;
            }
        }
    }
}

pub struct ThreeBandEq {
    enabled: bool,
    low_shelf: BiquadFilter,
    peaking: BiquadFilter,
    high_shelf: BiquadFilter,
}

impl ThreeBandEq {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            enabled: false,
            // 250Hz, Q=0.707
            low_shelf: BiquadFilter::new(FilterType::LowShelf, sample_rate, 250.0, 0.707, 0.0),
            // 1500Hz, Q=1.0
            peaking: BiquadFilter::new(FilterType::Peaking, sample_rate, 1500.0, 1.0, 0.0),
            // 4000Hz, Q=0.707
            high_shelf: BiquadFilter::new(FilterType::HighShelf, sample_rate, 4000.0, 0.707, 0.0),
        }
    }

    pub fn set_low_gain(&mut self, db: f32) {
        self.low_shelf.set_gain(db);
    }

    pub fn set_mid_gain(&mut self, db: f32) {
        self.peaking.set_gain(db);
    }

    pub fn set_high_gain(&mut self, db: f32) {
        self.high_shelf.set_gain(db);
    }
}

impl AudioEffect for ThreeBandEq {
    fn process(&mut self, input: &mut [f32]) {
        if !self.enabled {
            return;
        }
        self.low_shelf.process_interleaved(input);
        self.peaking.process_interleaved(input);
        self.high_shelf.process_interleaved(input);
    }

    fn set_sample_rate(&mut self, sample_rate: u32) {
        self.low_shelf.set_sample_rate(sample_rate);
        self.peaking.set_sample_rate(sample_rate);
        self.high_shelf.set_sample_rate(sample_rate);
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
