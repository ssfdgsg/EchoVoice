//! Configuration state.
//!
//! The data model and the mutation helpers live in `echovoice-core` (platform independent
//! and unit tested); this module only wires them into the Tauri app.

pub use echovoice_core::config::{
    AppConfig, AudioSettings, CompressorSettings, EqSettings, PitchSettings, SoundItem,
    SoundManager, DEFAULT_CATEGORY, MAX_CONCURRENT_LIMIT, MAX_RECORDING_SECONDS,
};
