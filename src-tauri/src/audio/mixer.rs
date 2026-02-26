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
}

impl Default for Mixer {
    fn default() -> Self {
        Self::new()
    }
}

impl Mixer {
    pub fn new() -> Self {
        Self {
            active_fx: Arc::new(Mutex::new(Vec::new())),
            mic_volume: AtomicU32::new(1.0f32.to_bits()),
            fx_volume: AtomicU32::new(1.0f32.to_bits()),
        }
    }

    pub fn play_sound(&self, data: Arc<Vec<f32>>, volume: f32) {
        if let Ok(mut fx_list) = self.active_fx.lock() {
            fx_list.push(FxInstance {
                sound_data: data,
                position: 0,
                volume,
            });
        }
    }

    pub fn set_mic_volume(&self, vol: f32) {
        self.mic_volume.store(vol.to_bits(), Ordering::Relaxed);
    }

    pub fn set_fx_volume(&self, vol: f32) {
        self.fx_volume.store(vol.to_bits(), Ordering::Relaxed);
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

        // Initialize output with mic input
        let mic_vol = f32::from_bits(self.mic_volume.load(Ordering::Relaxed));
        for i in 0..frames_count {
            output[i] = mic_input[i] * mic_vol;
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
    }
}
