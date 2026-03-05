use crate::audio::dsp::{compressor::Compressor, eq::ThreeBandEq, pitch::PitchShifter, DspChain};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

pub struct FxInstance {
    pub sound_data: Arc<Vec<f32>>,
    pub position: usize,
    pub volume: f32,
}

pub struct Mixer {
    pub active_fx: Arc<Mutex<Vec<FxInstance>>>,
    pub mic_volume: AtomicU32,
    pub fx_volume: AtomicU32,
    pub noise_gate_threshold: AtomicU32,
    pub dsp_chain: Arc<Mutex<DspChain>>,
}

impl Default for Mixer {
    fn default() -> Self {
        Self::new()
    }
}

impl Mixer {
    pub fn new() -> Self {
        let mut dsp = DspChain::new();
        // Assuming default SR of 48000 for instantiation. Will be updated dynamically.
        dsp.add_effect(Box::new(ThreeBandEq::new(48000)));
        dsp.add_effect(Box::new(Compressor::new(48000)));
        dsp.add_effect(Box::new(PitchShifter::new(48000)));

        Self {
            active_fx: Arc::new(Mutex::new(Vec::new())),
            mic_volume: AtomicU32::new(1.0f32.to_bits()),
            fx_volume: AtomicU32::new(1.0f32.to_bits()),
            noise_gate_threshold: AtomicU32::new(0.0f32.to_bits()),
            dsp_chain: Arc::new(Mutex::new(dsp)),
        }
    }

    pub fn play_sound(&self, data: Arc<Vec<f32>>, volume: f32) {
        if let Ok(mut fx_list) = self.active_fx.lock() {
            // Single-playback: stop any previous sound before playing a new one
            fx_list.clear();
            fx_list.push(FxInstance {
                sound_data: data,
                position: 0,
                volume,
            });
        }
    }

    pub fn stop_sound(&self) {
        self.clear();
    }

    pub fn is_playing(&self) -> bool {
        if let Ok(fx_list) = self.active_fx.lock() {
            !fx_list.is_empty()
        } else {
            false
        }
    }

    pub fn get_playback_state(&self) -> Option<(usize, usize)> {
        if let Ok(fx_list) = self.active_fx.lock() {
            if let Some(fx) = fx_list.first() {
                return Some((fx.position, fx.sound_data.len()));
            }
        }
        None
    }

    pub fn seek(&self, position_ratio: f32) {
        if let Ok(mut fx_list) = self.active_fx.lock() {
            if let Some(fx) = fx_list.first_mut() {
                let target_pos = (fx.sound_data.len() as f32 * position_ratio) as usize;
                fx.position = target_pos.min(fx.sound_data.len().saturating_sub(1));
            }
        }
    }

    pub fn set_mic_volume(&self, vol: f32) {
        self.mic_volume.store(vol.to_bits(), Ordering::Relaxed);
    }

    pub fn set_fx_volume(&self, vol: f32) {
        self.fx_volume.store(vol.to_bits(), Ordering::Relaxed);
    }

    pub fn set_noise_gate_threshold(&self, threshold: f32) {
        self.noise_gate_threshold
            .store(threshold.to_bits(), Ordering::Relaxed);
    }

    pub fn clear(&self) {
        if let Ok(mut fx_list) = self.active_fx.lock() {
            fx_list.clear();
        }
    }

    pub fn process_frames(&self, mic_input: &[f32], output: &mut [f32]) {
        let frames_count = mic_input.len();
        if output.len() < frames_count {
            return;
        }

        // Noise Gate processing
        let gate_threshold = f32::from_bits(self.noise_gate_threshold.load(Ordering::Relaxed));
        let mut is_gated = false;

        if gate_threshold > 0.0 {
            // Calculate RMS (Root Mean Square) for the current chunk
            let sum_squares: f32 = mic_input.iter().map(|&s| s * s).sum();
            let rms = (sum_squares / frames_count as f32).sqrt();
            if rms < gate_threshold {
                is_gated = true;
            }
        }

        // Initialize output with mic input (apply noise gate if triggered)
        let mic_vol = f32::from_bits(self.mic_volume.load(Ordering::Relaxed));
        let active_mic_vol = if is_gated { 0.0 } else { mic_vol };

        for i in 0..frames_count {
            output[i] = mic_input[i] * active_mic_vol;
        }

        // Mix in active FX
        let fx_vol_master = f32::from_bits(self.fx_volume.load(Ordering::Relaxed));
        if let Ok(mut fx_list) = self.active_fx.lock() {
            let mut i = 0;
            while i < fx_list.len() {
                let fx = &mut fx_list[i];
                let data = &fx.sound_data;
                let data_len = data.len();
                let mut finished = false;

                for j in 0..frames_count {
                    if fx.position < data_len {
                        let sample = data[fx.position] * fx.volume * fx_vol_master;
                        output[j] += sample;
                        fx.position += 1;
                    } else {
                        finished = true;
                        break;
                    }
                }

                if finished {
                    fx_list.remove(i);
                } else {
                    i += 1;
                }
            }
        }

        // Soft clip or hard clamp
        for sample in output.iter_mut().take(frames_count) {
            if *sample > 1.0 {
                *sample = 1.0;
            } else if *sample < -1.0 {
                *sample = -1.0;
            }
        }

        // --- Execute DSP node chain on final mixed output ---
        if let Ok(mut chain) = self.dsp_chain.lock() {
            let chain: &mut DspChain = &mut *chain;
            chain.process(output);
        }
    }
}
