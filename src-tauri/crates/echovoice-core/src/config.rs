//! Persistent configuration model for EchoVoice.
//!
//! Everything in this module is pure data + pure logic so it can be unit tested on any
//! platform. The desktop shell (`src-tauri/src/state.rs`) only adds the JSON file IO.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Category used when the user has not organised a sound into a folder yet.
pub const DEFAULT_CATEGORY: &str = "Default";
/// Hard safety limit for a single recording, keeps memory bounded.
pub const MAX_RECORDING_SECONDS: u32 = 120;
/// Upper bound for concurrent voices; the render loop cost grows linearly with this.
pub const MAX_CONCURRENT_LIMIT: usize = 32;

fn default_volume() -> f32 {
    1.0
}
fn default_true() -> bool {
    true
}
fn default_category() -> String {
    DEFAULT_CATEGORY.to_string()
}
fn default_ducking_amount() -> f32 {
    0.55
}
fn default_max_concurrent() -> usize {
    8
}
fn default_theme() -> String {
    "dark".to_string()
}
fn default_fade_ms() -> f32 {
    6.0
}

/// One entry of the soundboard.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SoundItem {
    pub id: String,
    pub name: String,
    pub path: String,
    /// Accelerator string understood by `tauri-plugin-global-shortcut`, e.g. `Control+Shift+1`.
    #[serde(default)]
    pub shortcut: Option<String>,
    /// Per-sound gain, independent from the master FX volume. Range 0.0 - 2.0.
    #[serde(default = "default_volume")]
    pub volume: f32,
    /// Loop the sound until it is stopped explicitly.
    #[serde(default)]
    pub loop_playback: bool,
    /// Shown in the favourites filter.
    #[serde(default)]
    pub favorite: bool,
    /// Free-form grouping used by the category filter.
    #[serde(default = "default_category")]
    pub category: String,
    /// Skip the first N ms of the file (trim in-point).
    #[serde(default)]
    pub start_offset_ms: u32,
    /// Stop at N ms of the file (trim out-point). `None` means "play to the end".
    #[serde(default)]
    pub end_offset_ms: Option<u32>,
    /// When `true`, pressing the hotkey again restarts the sound (Soundpad default).
    #[serde(default = "default_true")]
    pub retrigger: bool,
    /// When `true` the sound only plays while the hotkey is held down.
    #[serde(default)]
    pub hold_to_play: bool,
    /// Cached length of the decoded audio in milliseconds, filled in by the engine.
    #[serde(default)]
    pub duration_ms: Option<u32>,
    /// Belongs to the music playlist (one-key background music in game).
    #[serde(default)]
    pub is_music: bool,
}

impl SoundItem {
    pub fn new(id: impl Into<String>, name: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            path: path.into(),
            shortcut: None,
            volume: 1.0,
            loop_playback: false,
            favorite: false,
            category: default_category(),
            start_offset_ms: 0,
            end_offset_ms: None,
            retrigger: true,
            hold_to_play: false,
            duration_ms: None,
            is_music: false,
        }
    }

    /// Clamps every user editable field into a sane range.
    pub fn sanitize(&mut self) {
        self.name = self.name.trim().to_string();
        if self.name.is_empty() {
            self.name = "Unnamed sound".to_string();
        }
        if self.name.chars().count() > 80 {
            self.name = self.name.chars().take(80).collect();
        }
        self.volume = self.volume.clamp(0.0, 2.0);
        self.category = self.category.trim().to_string();
        if self.category.is_empty() {
            self.category = default_category();
        }
        if self.category.chars().count() > 40 {
            self.category = self.category.chars().take(40).collect();
        }
        // start_offset must stay before end_offset.
        if let Some(end) = self.end_offset_ms {
            self.end_offset_ms = Some(end.max(self.start_offset_ms.saturating_add(50)));
        }
    }

    /// Effective out-point in ms, falling back to the real duration of the file.
    pub fn effective_end_ms(&self, duration_ms: u32) -> u32 {
        let start = self.start_offset_ms.min(duration_ms);
        match self.end_offset_ms {
            Some(end) => end.clamp(start.saturating_add(50), duration_ms.max(start + 50)),
            None => duration_ms.max(start),
        }
    }

    /// Playable length in ms.
    pub fn playable_ms(&self, duration_ms: u32) -> u32 {
        self.effective_end_ms(duration_ms)
            .saturating_sub(self.start_offset_ms)
    }

    /// Converts the persisted per-sound settings into mixer playback options.
    pub fn voice_options(&self, fade_ms: f32) -> crate::mixer::VoiceOptions {
        crate::mixer::VoiceOptions {
            volume: self.volume,
            loop_playback: self.loop_playback,
            retrigger: self.retrigger,
            hold_to_play: self.hold_to_play,
            start_offset_ms: self.start_offset_ms,
            end_offset_ms: self.end_offset_ms,
            fade_in_ms: fade_ms,
            fade_out_ms: fade_ms,
        }
    }
}

/// Equalizer settings (three shelving/peaking bands).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EqSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub low_db: f32,
    #[serde(default)]
    pub mid_db: f32,
    #[serde(default)]
    pub high_db: f32,
}

impl Default for EqSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            low_db: 0.0,
            mid_db: 0.0,
            high_db: 0.0,
        }
    }
}

impl EqSettings {
    pub fn sanitize(&mut self) {
        self.low_db = self.low_db.clamp(-18.0, 18.0);
        self.mid_db = self.mid_db.clamp(-18.0, 18.0);
        self.high_db = self.high_db.clamp(-18.0, 18.0);
    }
}

/// Compressor / auto-leveller settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompressorSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_comp_threshold")]
    pub threshold_db: f32,
    #[serde(default = "default_comp_ratio")]
    pub ratio: f32,
    #[serde(default = "default_comp_attack")]
    pub attack_ms: f32,
    #[serde(default = "default_comp_release")]
    pub release_ms: f32,
    #[serde(default)]
    pub makeup_db: f32,
}

fn default_comp_threshold() -> f32 {
    -18.0
}
fn default_comp_ratio() -> f32 {
    3.0
}
fn default_comp_attack() -> f32 {
    8.0
}
fn default_comp_release() -> f32 {
    120.0
}

impl Default for CompressorSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            threshold_db: default_comp_threshold(),
            ratio: default_comp_ratio(),
            attack_ms: default_comp_attack(),
            release_ms: default_comp_release(),
            makeup_db: 0.0,
        }
    }
}

impl CompressorSettings {
    pub fn sanitize(&mut self) {
        self.threshold_db = self.threshold_db.clamp(-60.0, 0.0);
        self.ratio = self.ratio.clamp(1.0, 20.0);
        self.attack_ms = self.attack_ms.clamp(0.5, 500.0);
        self.release_ms = self.release_ms.clamp(5.0, 2000.0);
        self.makeup_db = self.makeup_db.clamp(0.0, 24.0);
    }
}

/// Voice changer settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PitchSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_pitch_ratio")]
    pub ratio: f32,
}

fn default_pitch_ratio() -> f32 {
    1.0
}

impl Default for PitchSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            ratio: 1.0,
        }
    }
}

impl PitchSettings {
    pub fn sanitize(&mut self) {
        self.ratio = self.ratio.clamp(0.5, 2.0);
    }
}

/// Every audio related knob, persisted so the user does not have to re-tune after a restart.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AudioSettings {
    #[serde(default = "default_volume")]
    pub mic_volume: f32,
    #[serde(default = "default_volume")]
    pub fx_volume: f32,
    /// Music keeps its own level: muting the soundboard to talk must not kill the soundtrack.
    #[serde(default = "default_volume")]
    pub music_volume: f32,
    /// 0.0 disables the noise gate.
    #[serde(default)]
    pub noise_gate_threshold: f32,
    #[serde(default)]
    pub mic_muted: bool,
    #[serde(default)]
    pub fx_muted: bool,
    /// Lower the microphone while a soundboard effect is playing (side-chain ducking).
    #[serde(default)]
    pub ducking_enabled: bool,
    #[serde(default = "default_ducking_amount")]
    pub ducking_amount: f32,
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent_sounds: usize,
    #[serde(default = "default_fade_ms")]
    pub fade_ms: f32,
    #[serde(default)]
    pub eq: EqSettings,
    #[serde(default)]
    pub compressor: CompressorSettings,
    #[serde(default)]
    pub pitch: PitchSettings,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            mic_volume: 1.0,
            fx_volume: 1.0,
            music_volume: 1.0,
            noise_gate_threshold: 0.0,
            mic_muted: false,
            fx_muted: false,
            ducking_enabled: false,
            ducking_amount: default_ducking_amount(),
            max_concurrent_sounds: default_max_concurrent(),
            fade_ms: default_fade_ms(),
            eq: EqSettings::default(),
            compressor: CompressorSettings::default(),
            pitch: PitchSettings::default(),
        }
    }
}

impl AudioSettings {
    pub fn sanitize(&mut self) {
        self.mic_volume = self.mic_volume.clamp(0.0, 2.0);
        self.fx_volume = self.fx_volume.clamp(0.0, 2.0);
        self.music_volume = self.music_volume.clamp(0.0, 2.0);
        self.noise_gate_threshold = self.noise_gate_threshold.clamp(0.0, 0.2);
        self.ducking_amount = self.ducking_amount.clamp(0.0, 1.0);
        self.max_concurrent_sounds = self.max_concurrent_sounds.clamp(1, MAX_CONCURRENT_LIMIT);
        self.fade_ms = self.fade_ms.clamp(0.0, 50.0);
        self.eq.sanitize();
        self.compressor.sanitize();
        self.pitch.sanitize();
    }
}

/// Whole application configuration, serialised to `echovoice_config.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppConfig {
    #[serde(default)]
    pub sounds: Vec<SoundItem>,
    #[serde(default)]
    pub default_input_id: Option<String>,
    #[serde(default)]
    pub default_output_id: Option<String>,
    /// Stops every playing sound.
    #[serde(default)]
    pub global_stop_shortcut: Option<String>,
    /// Toggles the microphone mute.
    #[serde(default)]
    pub mic_mute_shortcut: Option<String>,
    /// Toggles the soundboard mute.
    #[serde(default)]
    pub fx_mute_shortcut: Option<String>,
    /// Play / pause the music playlist.
    #[serde(default)]
    pub music_toggle_shortcut: Option<String>,
    /// Skip to the next track.
    #[serde(default)]
    pub music_next_shortcut: Option<String>,
    /// Back to the previous track.
    #[serde(default)]
    pub music_prev_shortcut: Option<String>,
    /// Stop the music outright.
    #[serde(default)]
    pub music_stop_shortcut: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default = "default_theme")]
    pub theme: String,
    /// Open the audio bridge automatically when the app starts.
    #[serde(default)]
    pub auto_start_bridge: bool,
    /// Closing the window hides it into the tray instead of quitting.
    #[serde(default)]
    pub minimize_to_tray: bool,
    #[serde(default)]
    pub launch_at_startup: bool,
    #[serde(default)]
    pub audio: AudioSettings,
    /// Also play the mix on a real output device so the user hears what teammates hear.
    #[serde(default)]
    pub monitor_enabled: bool,
    /// Which physical output device to monitor on. `None` = the system default.
    #[serde(default)]
    pub monitor_device_id: Option<String>,
    /// Loop the playlist instead of stopping after the last track.
    #[serde(default)]
    pub music_loop_playlist: bool,
    /// Index of the track that was playing when the app was closed.
    #[serde(default)]
    pub music_index: Option<usize>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            sounds: Vec::new(),
            default_input_id: None,
            default_output_id: None,
            global_stop_shortcut: None,
            mic_mute_shortcut: None,
            fx_mute_shortcut: None,
            language: None,
            music_toggle_shortcut: None,
            music_next_shortcut: None,
            music_prev_shortcut: None,
            music_stop_shortcut: None,
            theme: default_theme(),
            auto_start_bridge: false,
            minimize_to_tray: true,
            launch_at_startup: false,
            monitor_enabled: false,
            monitor_device_id: None,
            audio: AudioSettings::default(),
            music_loop_playlist: true,
            music_index: None,
        }
    }
}

impl AppConfig {
    /// Tracks flagged as music, in playlist order.
    pub fn music_tracks(&self) -> Vec<&SoundItem> {
        self.sounds.iter().filter(|sound| sound.is_music).collect()
    }

    /// Resolves a stored playlist index against the current library.
    pub fn music_track_at(&self, index: usize) -> Option<&SoundItem> {
        self.music_tracks().get(index).copied()
    }

    /// Normalises user supplied data and removes duplicate ids.
    pub fn sanitize(&mut self) {
        self.theme = if self.theme.eq_ignore_ascii_case("light") {
            "light".to_string()
        } else {
            "dark".to_string()
        };
        self.audio.sanitize();
        let mut seen = std::collections::HashSet::new();
        self.sounds.retain(|sound| seen.insert(sound.id.clone()));
        for sound in &mut self.sounds {
            sound.sanitize();
        }
    }

    pub fn find_sound(&self, id: &str) -> Option<&SoundItem> {
        self.sounds.iter().find(|sound| sound.id == id)
    }

    pub fn find_sound_mut(&mut self, id: &str) -> Option<&mut SoundItem> {
        self.sounds.iter_mut().find(|sound| sound.id == id)
    }

    /// Distinct, alphabetically sorted categories present in the library.
    pub fn categories(&self) -> Vec<String> {
        let mut categories: Vec<String> = self
            .sounds
            .iter()
            .map(|sound| sound.category.clone())
            .collect();
        categories.sort();
        categories.dedup();
        categories
    }
}

/// Owns the configuration file and provides mutation helpers.
pub struct SoundManager {
    config: Arc<Mutex<AppConfig>>,
    config_path: Arc<Mutex<Option<PathBuf>>>,
}

impl Default for SoundManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SoundManager {
    pub fn new() -> Self {
        Self {
            config: Arc::new(Mutex::new(AppConfig::default())),
            config_path: Arc::new(Mutex::new(None)),
        }
    }

    pub fn init_path_and_load(&self, path: PathBuf) {
        *self.config_path.lock().unwrap() = Some(path.clone());
        if let Ok(data) = fs::read_to_string(&path) {
            match serde_json::from_str::<AppConfig>(&data) {
                Ok(mut parsed) => {
                    parsed.sanitize();
                    *self.config.lock().unwrap() = parsed;
                    tracing::info!("Loaded configuration from {}", path.display());
                }
                Err(err) => {
                    tracing::error!("Ignoring corrupt configuration file: {}", err);
                }
            }
        }
    }

    /// Applies a mutation and persists the result. Keeps the write path in one place.
    pub fn update(&self, mutate: impl FnOnce(&mut AppConfig)) {
        let snapshot = {
            let mut config = match self.config.lock() {
                Ok(config) => config,
                Err(poisoned) => poisoned.into_inner(),
            };
            mutate(&mut config);
            config.sanitize();
            config.clone()
        };
        self.save_snapshot(&snapshot);
        if let Ok(mut config) = self.config.lock() {
            *config = snapshot;
        }
    }

    fn save_snapshot(&self, config: &AppConfig) {
        let path = match self.config_path.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        };
        let Some(path) = path else { return };
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        match serde_json::to_string_pretty(config) {
            Ok(data) => {
                // Write to a temp file first so a crash never leaves a truncated config behind.
                let tmp = path.with_extension("json.tmp");
                if fs::write(&tmp, &data).is_ok() {
                    let _ = fs::rename(&tmp, &path);
                } else {
                    let _ = fs::write(&path, data);
                }
            }
            Err(err) => tracing::error!("Failed to serialise configuration: {}", err),
        }
    }

    pub fn get_app_config(&self) -> AppConfig {
        match self.config.lock() {
            Ok(config) => config.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    pub fn get_sounds(&self) -> Vec<SoundItem> {
        self.get_app_config().sounds
    }

    pub fn get_sound(&self, id: &str) -> Option<SoundItem> {
        self.get_app_config().find_sound(id).cloned()
    }

    pub fn add_sound(&self, item: SoundItem) {
        self.update(|config| config.sounds.push(item));
    }

    pub fn remove_sound(&self, id: &str) {
        self.update(|config| config.sounds.retain(|sound| sound.id != id));
    }

    /// Generic per-sound patch helper used by every `set_sound_*` command.
    pub fn patch_sound(&self, id: &str, mutate: impl FnOnce(&mut SoundItem)) -> bool {
        let mut applied = false;
        self.update(|config| {
            if let Some(sound) = config.find_sound_mut(id) {
                mutate(sound);
                applied = true;
            }
        });
        applied
    }

    pub fn update_shortcut(&self, id: &str, shortcut: Option<String>) {
        self.patch_sound(id, |sound| sound.shortcut = shortcut);
    }

    pub fn update_global_stop_shortcut(&self, shortcut: Option<String>) {
        self.update(|config| config.global_stop_shortcut = shortcut);
    }

    pub fn set_music_shortcut(&self, action: &str, shortcut: Option<String>) {
        self.update(|config| match action {
            "toggle" => config.music_toggle_shortcut = shortcut,
            "next" => config.music_next_shortcut = shortcut,
            "prev" => config.music_prev_shortcut = shortcut,
            "stop" => config.music_stop_shortcut = shortcut,
            _ => {}
        });
    }

    pub fn set_monitor(&self, enabled: bool, device_id: Option<String>) {
        self.update(|config| {
            config.monitor_enabled = enabled;
            config.monitor_device_id = device_id;
        });
    }

    /// The device id the engine should mirror the mix to, or `None` when disabled.
    pub fn monitor_device(&self) -> Option<String> {
        let config = self.get_app_config();
        config
            .monitor_enabled
            .then_some(config.monitor_device_id)
            .flatten()
    }

    pub fn set_music_loop_playlist(&self, enabled: bool) {
        self.update(|config| config.music_loop_playlist = enabled);
    }

    pub fn set_music_index(&self, index: Option<usize>) {
        self.update(|config| config.music_index = index);
    }

    pub fn set_mic_mute_shortcut(&self, shortcut: Option<String>) {
        self.update(|config| config.mic_mute_shortcut = shortcut);
    }

    pub fn set_fx_mute_shortcut(&self, shortcut: Option<String>) {
        self.update(|config| config.fx_mute_shortcut = shortcut);
    }

    pub fn set_default_devices(&self, input_id: Option<String>, output_id: Option<String>) {
        self.update(|config| {
            config.default_input_id = input_id;
            config.default_output_id = output_id;
        });
    }

    pub fn set_language(&self, lang: Option<String>) {
        self.update(|config| config.language = lang);
    }

    pub fn set_theme(&self, theme: String) {
        self.update(|config| config.theme = theme);
    }

    pub fn set_auto_start_bridge(&self, enabled: bool) {
        self.update(|config| config.auto_start_bridge = enabled);
    }

    pub fn set_minimize_to_tray(&self, enabled: bool) {
        self.update(|config| config.minimize_to_tray = enabled);
    }

    pub fn set_launch_at_startup(&self, enabled: bool) {
        self.update(|config| config.launch_at_startup = enabled);
    }

    pub fn set_audio_settings(&self, audio: AudioSettings) {
        self.update(|config| config.audio = audio);
    }

    pub fn audio_settings(&self) -> AudioSettings {
        self.get_app_config().audio
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!("echovoice-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn legacy_config_without_new_fields_still_loads() {
        let json = r#"{
            "sounds": [
                {"id": "1", "name": "Airhorn", "path": "C:\\a.wav", "shortcut": "Control+1"}
            ],
            "default_input_id": "mic",
            "default_output_id": "cable",
            "global_stop_shortcut": "Control+Q",
            "language": "zh",
            "noise_gate_threshold": 0.01
        }"#;
        let config: AppConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.sounds.len(), 1);
        assert_eq!(config.sounds[0].volume, 1.0);
        assert!(!config.sounds[0].loop_playback);
        assert!(config.sounds[0].retrigger);
        assert_eq!(config.sounds[0].category, DEFAULT_CATEGORY);
        assert_eq!(config.audio.max_concurrent_sounds, 8);
        assert!(!config.audio.ducking_enabled);
        assert_eq!(config.theme, "dark");
    }

    #[test]
    fn unknown_fields_from_older_versions_are_ignored() {
        // `bg_image_path` was removed in a later version; serde must not reject the file.
        let json = r#"{
            "sounds": [],
            "bg_image_path": "C:\\bg.png",
            "some_future_field": 42,
            "audio": { "mic_volume": 0.7, "unknown_nested": true }
        }"#;
        let config: AppConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.audio.mic_volume, 0.7);
        assert_eq!(config.sounds.len(), 0);
    }

    #[test]
    fn full_config_round_trips() {
        let mut config = AppConfig::default();
        let mut sound = SoundItem::new("1", "Airhorn", "C:\\a.wav");
        sound.volume = 0.5;
        sound.loop_playback = true;
        sound.hold_to_play = true;
        sound.start_offset_ms = 100;
        sound.end_offset_ms = Some(5000);
        sound.category = "Memes".into();
        sound.favorite = true;
        sound.duration_ms = Some(8000);
        config.sounds.push(sound);
        config.audio.ducking_enabled = true;
        config.audio.max_concurrent_sounds = 4;
        config.theme = "light".into();
        config.auto_start_bridge = true;
        config.mic_mute_shortcut = Some("Control+M".into());

        let json = serde_json::to_string(&config).unwrap();
        let back: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config, back);
    }

    #[test]
    fn sanitize_clamps_and_dedups() {
        let mut config = AppConfig::default();
        config.sounds.push(SoundItem::new("a", "  ", "x.wav"));
        config.sounds.push(SoundItem::new("a", "dup", "y.wav"));
        config.sounds.push(SoundItem::new("b", "ok", "z.wav"));
        config.audio.mic_volume = 99.0;
        config.audio.noise_gate_threshold = -1.0;
        config.sanitize();
        assert_eq!(config.sounds.len(), 2);
        assert_eq!(config.sounds[0].name, "Unnamed sound");
        assert_eq!(config.audio.mic_volume, 2.0);
        assert_eq!(config.audio.noise_gate_threshold, 0.0);
    }

    #[test]
    fn trim_in_out_points_stay_ordered() {
        let mut sound = SoundItem::new("1", "s", "p");
        sound.start_offset_ms = 5000;
        sound.end_offset_ms = Some(1000);
        sound.sanitize();
        assert_eq!(sound.end_offset_ms, Some(5050));
        assert_eq!(sound.effective_end_ms(8000), 5050);
        assert_eq!(sound.playable_ms(8000), 50);
    }

    #[test]
    fn manager_persists_to_disk() {
        let dir = temp_dir("persist");
        let path = dir.join("cfg.json");
        let manager = SoundManager::new();
        manager.init_path_and_load(path.clone());
        manager.add_sound(SoundItem::new("1", "Airhorn", "C:\\a.wav"));
        manager.update(|config| config.audio.mic_volume = 0.5);

        let reloaded = SoundManager::new();
        reloaded.init_path_and_load(path);
        let config = reloaded.get_app_config();
        assert_eq!(config.sounds.len(), 1);
        assert_eq!(config.sounds[0].name, "Airhorn");
        assert_eq!(config.audio.mic_volume, 0.5);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn patch_sound_reports_missing_ids() {
        let dir = temp_dir("patch");
        let manager = SoundManager::new();
        manager.init_path_and_load(dir.join("cfg.json"));
        assert!(!manager.patch_sound("nope", |s| s.volume = 0.1));
        manager.add_sound(SoundItem::new("1", "s", "p"));
        assert!(manager.patch_sound("1", |s| s.volume = 0.25));
        assert_eq!(manager.get_sound("1").unwrap().volume, 0.25);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn voice_options_carry_every_per_sound_setting() {
        let mut sound = SoundItem::new("1", "s", "p");
        sound.volume = 0.4;
        sound.loop_playback = true;
        sound.retrigger = false;
        sound.hold_to_play = true;
        sound.start_offset_ms = 250;
        sound.end_offset_ms = Some(4000);

        let options = sound.voice_options(7.5);
        assert_eq!(options.volume, 0.4);
        assert!(options.loop_playback);
        assert!(!options.retrigger);
        assert!(options.hold_to_play);
        assert_eq!(options.start_offset_ms, 250);
        assert_eq!(options.end_offset_ms, Some(4000));
        assert_eq!(options.fade_in_ms, 7.5);
        assert_eq!(options.fade_out_ms, 7.5);
    }

    #[test]
    fn default_voice_options_match_soundpad_defaults() {
        let options = SoundItem::new("1", "s", "p").voice_options(6.0);
        assert_eq!(options.volume, 1.0);
        assert!(!options.loop_playback);
        assert!(options.retrigger);
        assert!(!options.hold_to_play);
        assert_eq!(options.start_offset_ms, 0);
        assert_eq!(options.end_offset_ms, None);
    }

    #[test]
    fn music_tracks_are_filtered_in_order() {
        let mut config = AppConfig::default();
        for (idx, is_music) in [false, true, false, true].iter().enumerate() {
            let mut item = SoundItem::new(idx.to_string(), format!("s{idx}"), "p");
            item.is_music = *is_music;
            config.sounds.push(item);
        }
        let tracks: Vec<&str> = config
            .music_tracks()
            .into_iter()
            .map(|sound| sound.name.as_str())
            .collect();
        assert_eq!(tracks, vec!["s1", "s3"]);
        assert_eq!(
            config.music_track_at(0).map(|s| s.name.as_str()),
            Some("s1")
        );
        assert_eq!(
            config.music_track_at(1).map(|s| s.name.as_str()),
            Some("s3")
        );
        assert!(config.music_track_at(2).is_none());
    }

    #[test]
    fn music_settings_default_sensibly() {
        let config = AppConfig::default();
        assert_eq!(config.audio.music_volume, 1.0);
        assert!(config.music_loop_playlist);
        assert!(config.music_toggle_shortcut.is_none());
        assert!(config.music_tracks().is_empty());
    }

    #[test]
    fn music_settings_round_trip() {
        let mut config = AppConfig::default();
        config.audio.music_volume = 0.35;
        config.music_loop_playlist = false;
        config.music_toggle_shortcut = Some("Control+M".into());
        config.music_next_shortcut = Some("Control+Period".into());
        let json = serde_json::to_string(&config).unwrap();
        let back: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config, back);
    }

    #[test]
    fn categories_are_sorted_and_unique() {
        let mut config = AppConfig::default();
        for (idx, cat) in ["Memes", "Memes", "Alerts"].iter().enumerate() {
            let mut item = SoundItem::new(idx.to_string(), "n", "p");
            item.category = (*cat).to_string();
            config.sounds.push(item);
        }
        assert_eq!(config.categories(), vec!["Alerts", "Memes"]);
    }
}
