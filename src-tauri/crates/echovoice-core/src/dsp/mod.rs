pub mod compressor;
pub mod eq;
pub mod pitch;

use std::any::Any;

/// A single node of the master effects chain.
pub trait AudioEffect: Send + Sync {
    /// Process a chunk of interleaved audio in-place.
    ///
    /// `channels` is the interleave factor of `input`; implementations must honour it
    /// instead of assuming stereo so mono and surround output both work.
    fn process(&mut self, input: &mut [f32], channels: usize);
    /// Set the dynamic sample rate.
    fn set_sample_rate(&mut self, sample_rate: u32);
    /// Check if the effect should process audio.
    fn is_enabled(&self) -> bool;
    /// Enable or disable the effect.
    fn set_enabled(&mut self, enabled: bool);
    /// Provide downcasting for configuration.
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

/// Serial chain of effects applied to the final mix.
pub struct DspChain {
    effects: Vec<Box<dyn AudioEffect>>,
    channels: usize,
}

impl DspChain {
    pub fn new() -> Self {
        Self {
            effects: Vec::new(),
            channels: 2,
        }
    }

    pub fn add_effect(&mut self, effect: Box<dyn AudioEffect>) {
        self.effects.push(effect);
    }

    pub fn process(&mut self, input: &mut [f32], channels: usize) {
        let channels = channels.max(1);
        self.channels = channels;
        for effect in &mut self.effects {
            if effect.is_enabled() {
                effect.process(input, channels);
            }
        }
    }

    pub fn set_channels(&mut self, channels: usize) {
        self.channels = channels.max(1);
    }

    pub fn channels(&self) -> usize {
        self.channels
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

#[cfg(test)]
mod tests {
    use super::*;

    struct Gain(f32);

    impl AudioEffect for Gain {
        fn process(&mut self, input: &mut [f32], channels: usize) {
            assert!(channels >= 1);
            for sample in input.iter_mut() {
                *sample *= self.0;
            }
        }
        fn set_sample_rate(&mut self, _sample_rate: u32) {}
        fn is_enabled(&self) -> bool {
            true
        }
        fn set_enabled(&mut self, _enabled: bool) {}
        fn as_any_mut(&mut self) -> &mut dyn Any {
            self
        }
    }

    #[test]
    fn chain_processes_every_enabled_effect_in_order() {
        let mut chain = DspChain::new();
        chain.add_effect(Box::new(Gain(2.0)));
        chain.add_effect(Box::new(Gain(0.5)));
        let mut buffer = [1.0f32, -1.0];
        chain.process(&mut buffer, 2);
        assert_eq!(buffer, [1.0, -1.0]);
    }

    #[test]
    fn channels_are_remembered() {
        let mut chain = DspChain::new();
        chain.set_channels(6);
        assert_eq!(chain.channels(), 6);
        chain.set_channels(0);
        assert_eq!(chain.channels(), 1);
    }
}
