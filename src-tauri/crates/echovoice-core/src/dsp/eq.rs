use crate::dsp::AudioEffect;
use std::f32::consts::PI;

/// Highest interleave factor the biquads keep independent state for.
pub const MAX_CHANNELS: usize = 8;

/// Which shelf/peak shape a [`BiquadFilter`] implements.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FilterKind {
    /// Boosts or cuts everything below `freq` (bass).
    LowShelf,
    /// Bell shaped boost or cut centred on `freq` (midrange).
    Peaking,
    /// Boosts or cuts everything above `freq` (treble).
    HighShelf,
}

/// Biquad filter (direct form I) with per-channel state.
///
/// The shelf responses are derived from the analog first-order shelf prototype
/// `H(s) = (s + c) / (s + d)` through the bilinear transform, which keeps the
/// low-frequency gain exactly `gain_db` (the classic RBJ transcription carries an
/// extra `A` factor on the numerator and ends up applying twice the requested dB).
pub struct BiquadFilter {
    kind: FilterKind,
    sample_rate: u32,
    freq: f32,
    q: f32,
    gain_db: f32,

    // Normalised coefficients (a0 is implicitly 1).
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,

    // Direct form I state: two previous inputs and two previous outputs per channel.
    x1: [f32; MAX_CHANNELS],
    x2: [f32; MAX_CHANNELS],
    y1: [f32; MAX_CHANNELS],
    y2: [f32; MAX_CHANNELS],
}

impl BiquadFilter {
    /// Creates a low shelf at `freq` with `gain_db` of boost (positive) or cut (negative).
    pub fn low_shelf(sample_rate: u32, freq: f32, gain_db: f32) -> Self {
        Self::build(FilterKind::LowShelf, sample_rate, freq, 0.707, gain_db)
    }

    /// Creates a peaking (bell) filter at `freq` with quality `q`.
    pub fn peaking(sample_rate: u32, freq: f32, q: f32, gain_db: f32) -> Self {
        Self::build(FilterKind::Peaking, sample_rate, freq, q, gain_db)
    }

    /// Creates a high shelf at `freq` with `gain_db` of boost or cut.
    pub fn high_shelf(sample_rate: u32, freq: f32, gain_db: f32) -> Self {
        Self::build(FilterKind::HighShelf, sample_rate, freq, 0.707, gain_db)
    }

    fn build(kind: FilterKind, sample_rate: u32, freq: f32, q: f32, gain_db: f32) -> Self {
        let mut filter = Self {
            kind,
            sample_rate: sample_rate.max(1),
            freq: freq.max(1.0),
            q: q.max(1e-3),
            gain_db,
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            x1: [0.0; MAX_CHANNELS],
            x2: [0.0; MAX_CHANNELS],
            y1: [0.0; MAX_CHANNELS],
            y2: [0.0; MAX_CHANNELS],
        };
        filter.calculate_coefficients();
        filter
    }

    pub fn set_gain(&mut self, gain_db: f32) {
        self.gain_db = gain_db;
        self.calculate_coefficients();
    }

    pub fn gain_db(&self) -> f32 {
        self.gain_db
    }

    pub fn set_sample_rate(&mut self, sample_rate: u32) {
        let sample_rate = sample_rate.max(1);
        if self.sample_rate != sample_rate {
            self.sample_rate = sample_rate;
            self.calculate_coefficients();
        }
    }

    pub fn reset(&mut self) {
        self.x1 = [0.0; MAX_CHANNELS];
        self.x2 = [0.0; MAX_CHANNELS];
        self.y1 = [0.0; MAX_CHANNELS];
        self.y2 = [0.0; MAX_CHANNELS];
    }

    /// Amplitude gain (linear) of this filter at `freq`, computed from the coefficients.
    /// Used by the unit tests to prove the shelves really deliver the requested dB.
    pub fn magnitude_at(&self, freq: f32) -> f32 {
        let omega = 2.0 * PI * freq / self.sample_rate as f32;
        let num_re = self.b0 + self.b1 * omega.cos() + self.b2 * (2.0 * omega).cos();
        let num_im = -(self.b1 * omega.sin() + self.b2 * (2.0 * omega).sin());
        let den_re = 1.0 + self.a1 * omega.cos() + self.a2 * (2.0 * omega).cos();
        let den_im = -(self.a1 * omega.sin() + self.a2 * (2.0 * omega).sin());
        (num_re.hypot(num_im) / den_re.hypot(den_im)).max(0.0)
    }

    fn calculate_coefficients(&mut self) {
        // RBJ cookbook convention: `A = 10^(dB/40)`. Both the shelf formulas (which carry an
        // extra `A` on the numerator) and the peaking formula (whose centre response works
        // out to A*A) then deliver exactly `10^(dB/20)` of amplitude gain. Verified by the
        // `magnitude_at` tests below.
        let a = 10.0_f32.powf(self.gain_db / 40.0);
        let omega = 2.0 * PI * self.freq / self.sample_rate as f32;
        let (sin, cos) = omega.sin_cos();
        let alpha = sin / (2.0 * self.q);
        let beta = 2.0 * a.sqrt() * alpha;

        let (b0, b1, b2, a0, a1, a2) = match self.kind {
            FilterKind::LowShelf => (
                a * ((a + 1.0) - (a - 1.0) * cos + beta),
                2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
                a * ((a + 1.0) - (a - 1.0) * cos - beta),
                (a + 1.0) + (a - 1.0) * cos + beta,
                -2.0 * ((a - 1.0) + (a + 1.0) * cos),
                (a + 1.0) + (a - 1.0) * cos - beta,
            ),
            FilterKind::HighShelf => (
                a * ((a + 1.0) + (a - 1.0) * cos + beta),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
                a * ((a + 1.0) + (a - 1.0) * cos - beta),
                (a + 1.0) - (a - 1.0) * cos + beta,
                2.0 * ((a - 1.0) - (a + 1.0) * cos),
                (a + 1.0) - (a - 1.0) * cos - beta,
            ),
            FilterKind::Peaking => (
                1.0 + alpha * a,
                -2.0 * cos,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * cos,
                1.0 - alpha / a,
            ),
        };

        let a0_inv = 1.0 / a0;
        self.b0 = b0 * a0_inv;
        self.b1 = b1 * a0_inv;
        self.b2 = b2 * a0_inv;
        self.a1 = a1 * a0_inv;
        self.a2 = a2 * a0_inv;
    }

    /// Processes interleaved audio, keeping independent state per channel.
    pub fn process_interleaved(&mut self, input: &mut [f32], channels: usize) {
        let channels = channels.clamp(1, MAX_CHANNELS);
        let frames = input.len() / channels;
        if frames == 0 {
            return;
        }

        for frame in 0..frames {
            for ch in 0..channels {
                let idx = frame * channels + ch;
                let in_sample = input[idx];
                let out_sample =
                    self.b0 * in_sample + self.b1 * self.x1[ch] + self.b2 * self.x2[ch]
                        - self.a1 * self.y1[ch]
                        - self.a2 * self.y2[ch];
                self.x2[ch] = self.x1[ch];
                self.x1[ch] = in_sample;
                self.y2[ch] = self.y1[ch];
                self.y1[ch] = out_sample;
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
            low_shelf: BiquadFilter::low_shelf(sample_rate, 250.0, 0.0),
            peaking: BiquadFilter::peaking(sample_rate, 1500.0, 1.0, 0.0),
            high_shelf: BiquadFilter::high_shelf(sample_rate, 4000.0, 0.0),
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

    pub fn low_gain(&self) -> f32 {
        self.low_shelf.gain_db()
    }

    pub fn mid_gain(&self) -> f32 {
        self.peaking.gain_db()
    }

    pub fn high_gain(&self) -> f32 {
        self.high_shelf.gain_db()
    }
}

impl AudioEffect for ThreeBandEq {
    fn process(&mut self, input: &mut [f32], channels: usize) {
        if !self.enabled {
            return;
        }
        self.low_shelf.process_interleaved(input, channels);
        self.peaking.process_interleaved(input, channels);
        self.high_shelf.process_interleaved(input, channels);
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
        if enabled && !self.enabled {
            self.low_shelf.reset();
            self.peaking.reset();
            self.high_shelf.reset();
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

    /// Steady-state amplitude of `freq` after passing through `eq`, in dB.
    fn measure_db(eq: &mut ThreeBandEq, freq: f32) -> f32 {
        let rate = 48_000.0f32;
        let mut buffer = Vec::new();
        for i in 0..rate as usize {
            let sample = (2.0 * PI * freq * i as f32 / rate).sin();
            buffer.push(sample);
            buffer.push(sample);
        }
        eq.process(&mut buffer, 2);
        // Measure the tail only: the first samples carry the filter start-up transient.
        let tail = &buffer[buffer.len() / 5 * 4..];
        let amplitude = tail.iter().fold(0.0f32, |peak, s| peak.max(s.abs()));
        20.0 * amplitude.log10()
    }

    fn to_db(linear: f32) -> f32 {
        20.0 * linear.log10()
    }

    #[test]
    fn flat_eq_passes_signal_through_unchanged() {
        let mut eq = ThreeBandEq::new(48_000);
        eq.set_enabled(true);
        let mut buffer: Vec<f32> = (0..64).map(|i| (i as f32 * 0.01).sin()).collect();
        let original = buffer.clone();
        eq.process(&mut buffer, 2);
        for (a, b) in buffer.iter().zip(original.iter()) {
            assert!((a - b).abs() < 1e-4, "{a} vs {b}");
        }
    }

    #[test]
    fn low_shelf_delivers_the_requested_db_at_low_frequencies() {
        let mut filter = BiquadFilter::low_shelf(48_000, 250.0, 12.0);
        let db = to_db(filter.magnitude_at(40.0));
        assert!((db - 12.0).abs() < 0.5, "measured {db} dB at 40 Hz");

        filter.set_gain(-9.0);
        let db = to_db(filter.magnitude_at(40.0));
        assert!((db + 9.0).abs() < 0.5, "measured {db} dB at 40 Hz");
    }

    #[test]
    fn high_shelf_delivers_the_requested_db_at_high_frequencies() {
        let mut filter = BiquadFilter::high_shelf(48_000, 4000.0, -9.0);
        let db = to_db(filter.magnitude_at(12_000.0));
        assert!((db + 9.0).abs() < 0.5, "measured {db} dB at 12 kHz");

        filter.set_gain(6.0);
        let db = to_db(filter.magnitude_at(12_000.0));
        assert!((db - 6.0).abs() < 0.5, "measured {db} dB at 12 kHz");
    }

    #[test]
    fn low_shelf_leaves_high_frequencies_alone() {
        let filter = BiquadFilter::low_shelf(48_000, 250.0, 12.0);
        assert!(to_db(filter.magnitude_at(10_000.0)).abs() < 0.5);
    }

    #[test]
    fn high_shelf_leaves_low_frequencies_alone() {
        let filter = BiquadFilter::high_shelf(48_000, 4000.0, 12.0);
        assert!(to_db(filter.magnitude_at(100.0)).abs() < 0.5);
    }

    #[test]
    fn peaking_filter_centres_on_its_frequency() {
        let filter = BiquadFilter::peaking(48_000, 1500.0, 1.0, 10.0);
        assert!((to_db(filter.magnitude_at(1500.0)) - 10.0).abs() < 0.2);
        assert!(to_db(filter.magnitude_at(60.0)).abs() < 0.5);
        assert!(to_db(filter.magnitude_at(12_000.0)).abs() < 0.5);
    }

    #[test]
    fn bass_shelf_applies_the_requested_gain() {
        let mut eq = ThreeBandEq::new(48_000);
        eq.set_low_gain(12.0);
        eq.set_enabled(true);
        let gained = measure_db(&mut eq, 60.0);
        assert!((gained - 12.0).abs() < 1.5, "measured {gained} dB at 60 Hz");
    }

    #[test]
    fn treble_shelf_applies_the_requested_gain() {
        let mut eq = ThreeBandEq::new(48_000);
        eq.set_high_gain(-9.0);
        eq.set_enabled(true);
        let gained = measure_db(&mut eq, 12_000.0);
        assert!((gained + 9.0).abs() < 1.0, "measured {gained} dB at 12 kHz");
    }

    #[test]
    fn mid_band_only_affects_the_midrange() {
        let mut eq = ThreeBandEq::new(48_000);
        eq.set_mid_gain(10.0);
        eq.set_enabled(true);
        assert!(measure_db(&mut eq, 1_500.0) > 8.0);
        assert!(measure_db(&mut eq, 80.0).abs() < 1.0);
        assert!(measure_db(&mut eq, 12_000.0).abs() < 1.0);
    }

    #[test]
    fn mono_and_surround_do_not_panic() {
        let mut eq = ThreeBandEq::new(48_000);
        eq.set_low_gain(6.0);
        eq.set_high_gain(-6.0);
        eq.set_enabled(true);
        for channels in [1usize, 2, 6, 8] {
            let mut buffer = vec![0.5f32; channels * 32];
            eq.process(&mut buffer, channels);
            assert!(buffer.iter().all(|s| s.is_finite()));
        }
    }
}
