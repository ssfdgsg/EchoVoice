pub mod device;
pub mod engine;

// The mixer, decoder, DSP chain and configuration model live in `echovoice-core` so they
// can be unit tested on any host without WASAPI/Tauri in the way. They are re-exported
// here so the rest of the crate keeps using `crate::audio::mixer::Mixer` style paths.
pub use echovoice_core::{dsp, fx, mixer};
