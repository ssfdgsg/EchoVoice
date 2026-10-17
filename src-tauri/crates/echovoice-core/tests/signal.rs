//! Signal-analysis helpers for the audio pipeline integration tests.
//!
//! Everything here is deliberately dependency-free (no FFT crate) so the measurements
//! are easy to audit by eye: RMS, peak, Goertzel single-bin magnitude, THD.

use std::f32::consts::PI;

/// Root-mean-square of a block.
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f32 = samples.iter().map(|s| s * s).sum();
    (sum / samples.len() as f32).sqrt()
}

/// Peak absolute value.
pub fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |p, s| p.max(s.abs()))
}

/// Amplitude of a single frequency bin, via the Goertzel algorithm.
///
/// Good enough to measure "how much 1 kHz is in this buffer" without pulling in an FFT.
pub fn goertzel(samples: &[f32], sample_rate: f32, freq: f32) -> f32 {
    if samples.is_empty() || sample_rate <= 0.0 {
        return 0.0;
    }
    let k = (freq / sample_rate * samples.len() as f32).round() as usize;
    if k == 0 || k > samples.len() / 2 {
        return 0.0;
    }
    let omega = 2.0 * PI * k as f32 / samples.len() as f32;
    let coeff = 2.0 * omega.cos();
    let mut s1 = 0.0f32;
    let mut s2 = 0.0f32;
    for &sample in samples {
        let s = sample + coeff * s1 - s2;
        s2 = s1;
        s1 = s;
    }
    let real = s1 - s2 * omega.cos();
    let imag = s2 * omega.sin();
    ((real * real + imag * imag).sqrt() / samples.len() as f32) * 2.0
}

/// Amplitude of the strongest non-fundamental harmonic, used for THD.
pub fn strongest_harmonic(
    samples: &[f32],
    sample_rate: f32,
    fundamental: f32,
    harmonics: usize,
) -> f32 {
    (2..=harmonics)
        .map(|n| goertzel(samples, sample_rate, fundamental * n as f32))
        .fold(0.0f32, f32::max)
}

/// Total harmonic distortion of a (mostly) single-tone signal.
pub fn thd(samples: &[f32], sample_rate: f32, fundamental: f32) -> f32 {
    let base = goertzel(samples, sample_rate, fundamental);
    if base <= 0.0 {
        return 0.0;
    }
    strongest_harmonic(samples, sample_rate, fundamental, 8) / base
}

/// Summed magnitude of harmonics 5..=11.
///
/// This is what makes clipping *audible*: a hard clamp dumps energy into high odd
/// harmonics, while a soft knee keeps the distortion in the low orders. Total THD is a
/// poor proxy because the soft knee bends more of the waveform and can raise H3.
pub fn high_order_harmonics(samples: &[f32], sample_rate: f32, fundamental: f32) -> f32 {
    (5..=11)
        .map(|n| goertzel(samples, sample_rate, fundamental * n as f32))
        .sum()
}

/// Sine at `freq`, amplitude `amp`.
pub fn sine(len: usize, sample_rate: f32, freq: f32, amp: f32) -> Vec<f32> {
    (0..len)
        .map(|i| (2.0 * PI * freq * i as f32 / sample_rate).sin() * amp)
        .collect()
}

/// dB difference between two amplitudes.
pub fn db(ratio: f32) -> f32 {
    20.0 * ratio.max(1e-9).log10()
}
