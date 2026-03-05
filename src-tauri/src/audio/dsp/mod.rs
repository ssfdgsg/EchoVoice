pub mod compressor;
pub mod eq;
pub mod pitch;

use std::any::Any;

pub trait AudioEffect: Send + Sync {
    /// Process a chunk of audio in-place.
    fn process(&mut self, input: &mut [f32]);
    /// Set the dynamic sample rate.
    fn set_sample_rate(&mut self, sample_rate: u32);
    /// Check if the effect should process audio.
    fn is_enabled(&self) -> bool;
    /// Enable or disable the effect.
    fn set_enabled(&mut self, enabled: bool);
    /// Provide downcasting for configuration.
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

pub struct DspChain {
    effects: Vec<Box<dyn AudioEffect>>,
}

impl DspChain {
    pub fn new() -> Self {
        Self {
            effects: Vec::new(),
        }
    }

    pub fn add_effect(&mut self, effect: Box<dyn AudioEffect>) {
        self.effects.push(effect);
    }

    pub fn process(&mut self, input: &mut [f32]) {
        for effect in &mut self.effects {
            if effect.is_enabled() {
                effect.process(input);
            }
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: u32) {
        for effect in &mut self.effects {
            effect.set_sample_rate(sample_rate);
        }
    }

    pub fn get_effect_mut(&mut self, index: usize) -> Option<&mut Box<dyn AudioEffect>> {
        self.effects.get_mut(index)
    }

    pub fn get_effects_len(&self) -> usize {
        self.effects.len()
    }
}

impl Default for DspChain {
    fn default() -> Self {
        Self::new()
    }
}
