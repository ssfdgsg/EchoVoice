//! EchoVoice platform independent audio core.
//!
//! Everything in this crate is pure Rust: no WASAPI, no Tauri, no Windows API. That keeps
//! the real-time DSP, the sound decoding and the configuration model unit-testable on any
//! host. The desktop shell (`src-tauri`) wires these primitives to WASAPI and the UI.

pub mod config;
pub mod dsp;
pub mod fx;
pub mod mixer;
pub mod supervisor;
pub mod wav;

pub use config::{
    AppConfig, AudioSettings, CompressorSettings, EqSettings, PitchSettings, SoundItem,
    SoundManager, DEFAULT_CATEGORY, MAX_CONCURRENT_LIMIT, MAX_RECORDING_SECONDS,
};
pub use dsp::{compressor::Compressor, eq::ThreeBandEq, pitch::PitchShifter, DspChain};
pub use fx::SoundBank;
pub use mixer::{FxInstance, Mixer, MixerStats, VoiceInfo, VoiceOptions, DEFAULT_FADE_MS};
pub use supervisor::Backoff;
pub use wav::{MicRecorder, WavWriter};
