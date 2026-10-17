pub mod audio;
#[cfg(target_os = "windows")]
mod autostart;
#[cfg(target_os = "windows")]
mod diagnostics;
#[cfg(target_os = "windows")]
mod game_hotkey;
pub mod state;

use std::str::FromStr;
use tauri::menu::{CheckMenuItem, Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

use echovoice_core::config::AudioSettings;
use echovoice_core::mixer::VoiceOptions;

fn parse_shortcut(value: &str) -> Result<Shortcut, String> {
    Shortcut::from_str(value).map_err(|err| format!("Invalid shortcut '{value}': {err}"))
}

fn shortcuts_match(left: &str, right: &Shortcut) -> bool {
    Shortcut::from_str(left)
        .map(|shortcut| shortcut.id() == right.id())
        .unwrap_or(false)
}

fn shortcut_conflict(
    config: &state::AppConfig,
    candidate: &Shortcut,
    excluded_sound_id: Option<&str>,
    include_global_stop: bool,
) -> Option<String> {
    if include_global_stop {
        if let Some(global) = config.global_stop_shortcut.as_deref() {
            if shortcuts_match(global, candidate) {
                return Some("the global stop action".to_string());
            }
        }
    }

    for (id, label) in [
        (
            config.mic_mute_shortcut.as_deref(),
            "the microphone mute action",
        ),
        (
            config.fx_mute_shortcut.as_deref(),
            "the soundboard mute action",
        ),
    ] {
        if let Some(value) = id {
            if shortcuts_match(value, candidate) {
                return Some(label.to_string());
            }
        }
    }

    config
        .sounds
        .iter()
        .filter(|sound| Some(sound.id.as_str()) != excluded_sound_id)
        .find(|sound| {
            sound
                .shortcut
                .as_deref()
                .map(|value| shortcuts_match(value, candidate))
                .unwrap_or(false)
        })
        .map(|sound| format!("sound '{}'", sound.name))
}

/// Registers a hotkey.
///
/// While the low-level keyboard hook is alive it is the *only* channel we use. Registering
/// with `RegisterHotKey` as well would make every key press fire twice, which is why the
/// old code needed a 80 ms time based debounce - and that debounce in turn swallowed
/// legitimate fast re-presses. One channel, no debounce.
#[cfg(target_os = "windows")]
fn register_shortcut(app: &tauri::AppHandle, _shortcut: Shortcut) -> Result<(), String> {
    if game_hotkey::is_available() {
        return Ok(());
    }
    app.global_shortcut()
        .register(_shortcut)
        .map_err(|err| format!("Failed to register shortcut: {err}"))
}

#[cfg(not(target_os = "windows"))]
fn register_shortcut(app: &tauri::AppHandle, shortcut: Shortcut) -> Result<(), String> {
    app.global_shortcut()
        .register(shortcut)
        .map_err(|err| format!("Failed to register shortcut: {err}"))
}

/// Unregisters a hotkey. A no-op while the hook owns the channel.
#[cfg(target_os = "windows")]
fn unregister_shortcut(app: &tauri::AppHandle, _shortcut: Shortcut) {
    if game_hotkey::is_available() {
        return;
    }
    let _ = app.global_shortcut().unregister(_shortcut);
}

#[cfg(not(target_os = "windows"))]
fn unregister_shortcut(app: &tauri::AppHandle, shortcut: Shortcut) {
    let _ = app.global_shortcut().unregister(shortcut);
}

/// Builds the playback options for a sound from its persisted configuration.
fn voice_options_for(sound: &state::SoundItem, fade_ms: f32) -> VoiceOptions {
    sound.voice_options(fade_ms)
}

fn trigger_sound(app: &tauri::AppHandle, sound_id: &str) {
    let state_sm = app.state::<state::SoundManager>();
    let Some(sound) = state_sm.get_sound(sound_id) else {
        return;
    };
    let fade_ms = state_sm.audio_settings().fade_ms;
    let options = voice_options_for(&sound, fade_ms);

    let app_clone = app.clone();
    let id = sound.id.clone();
    let path = sound.path.clone();
    thread_priority::spawn_hotkey_playback(move || {
        let state_ae = app_clone.state::<audio::engine::AudioEngine>();
        match state_ae.play_sound_with_options(&id, &path, options) {
            Ok(()) => {
                let _ = app_clone.emit("shortcut-play", id);
            }
            Err(err) => {
                // "Already playing" is a normal outcome when retrigger is disabled.
                if !err.contains("already playing") {
                    let _ = app_clone.emit("playback-error", err);
                }
            }
        }
    });
}

pub(crate) fn handle_shortcut_activation(app: &tauri::AppHandle, shortcut: &Shortcut) {
    // Auto-repeat is already filtered by the hook's pressed-key set, and there is only one
    // dispatch channel, so a key press always maps to exactly one trigger.
    let state_sm = app.state::<state::SoundManager>();
    let config = state_sm.get_app_config();
    let accelerator = shortcut.to_string();

    if config
        .global_stop_shortcut
        .as_deref()
        .and_then(|value| Shortcut::from_str(value).ok())
        .map(|saved| saved.id() == shortcut.id())
        .unwrap_or(false)
    {
        let state_ae = app.state::<audio::engine::AudioEngine>();
        state_ae.stop_sound();
        let _ = app.emit("global-stop", true);
        let _ = app.emit("hotkey-fired", accelerator);
        return;
    }

    if config
        .mic_mute_shortcut
        .as_deref()
        .and_then(|value| Shortcut::from_str(value).ok())
        .map(|saved| saved.id() == shortcut.id())
        .unwrap_or(false)
    {
        let muted = !config.audio.mic_muted;
        apply_mic_mute(app, muted);
        let _ = app.emit("mic-mute-changed", muted);
        let _ = app.emit("hotkey-fired", accelerator);
        return;
    }

    if config
        .fx_mute_shortcut
        .as_deref()
        .and_then(|value| Shortcut::from_str(value).ok())
        .map(|saved| saved.id() == shortcut.id())
        .unwrap_or(false)
    {
        let muted = !config.audio.fx_muted;
        apply_fx_mute(app, muted);
        let _ = app.emit("fx-mute-changed", muted);
        let _ = app.emit("hotkey-fired", accelerator);
        return;
    }

    // Music transport: one key starts/stops the soundtrack mid-game.
    let music_actions = [
        (config.music_toggle_shortcut.as_deref(), "toggle"),
        (config.music_next_shortcut.as_deref(), "next"),
        (config.music_prev_shortcut.as_deref(), "prev"),
        (config.music_stop_shortcut.as_deref(), "stop"),
    ];
    for (bound, action) in music_actions {
        let matches = bound
            .and_then(|value| Shortcut::from_str(value).ok())
            .map(|saved| saved.id() == shortcut.id())
            .unwrap_or(false);
        if matches {
            let engine = app.state::<audio::engine::AudioEngine>();
            let result = match action {
                "toggle" => engine.toggle_music(),
                "next" => engine.next_music(),
                "prev" => engine.prev_music(),
                _ => {
                    engine.stop_music();
                    Ok(())
                }
            };
            if let Err(err) = result {
                let _ = app.emit("playback-error", err);
            }
            let _ = app.emit("hotkey-fired", accelerator);
            let _ = app.emit("music-state-changed", ());
            return;
        }
    }

    if let Some(item) = config.sounds.into_iter().find(|sound| {
        sound
            .shortcut
            .as_deref()
            .and_then(|value| Shortcut::from_str(value).ok())
            .map(|saved| saved.id() == shortcut.id())
            .unwrap_or(false)
    }) {
        let _ = app.emit("hotkey-fired", accelerator);
        trigger_sound(app, &item.id);
    }
}

/// Called when a key is released.
///
/// Only `hold_to_play` sounds care about it. The match is on the main key alone: users
/// routinely let go of the modifier first, which would otherwise strand a playing sound.
pub(crate) fn handle_shortcut_release(app: &tauri::AppHandle, shortcut: &Shortcut) {
    let state_sm = app.state::<state::SoundManager>();
    let config = state_sm.get_app_config();

    let released: Vec<String> = config
        .sounds
        .into_iter()
        .filter(|sound| {
            sound.hold_to_play
                && sound
                    .shortcut
                    .as_deref()
                    .and_then(|value| Shortcut::from_str(value).ok())
                    .map(|saved| saved.key == shortcut.key)
                    .unwrap_or(false)
        })
        .map(|sound| sound.id)
        .collect();

    if released.is_empty() {
        return;
    }

    let state_ae = app.state::<audio::engine::AudioEngine>();
    for id in released {
        state_ae.release_sound(&id);
    }
    let _ = app.emit("shortcut-release", ());
}

fn apply_mic_mute(app: &tauri::AppHandle, muted: bool) {
    let state_sm = app.state::<state::SoundManager>();
    let mut settings = state_sm.audio_settings();
    settings.mic_muted = muted;
    state_sm.set_audio_settings(settings.clone());
    app.state::<audio::engine::AudioEngine>()
        .apply_audio_settings(&settings);
}

fn apply_fx_mute(app: &tauri::AppHandle, muted: bool) {
    let state_sm = app.state::<state::SoundManager>();
    let mut settings = state_sm.audio_settings();
    settings.fx_muted = muted;
    state_sm.set_audio_settings(settings.clone());
    app.state::<audio::engine::AudioEngine>()
        .apply_audio_settings(&settings);
}

mod thread_priority {
    pub fn spawn_hotkey_playback(task: impl FnOnce() + Send + 'static) {
        let _ = std::thread::Builder::new()
            .name("hotkey-playback".into())
            .spawn(move || {
                #[cfg(target_os = "windows")]
                unsafe {
                    use windows_sys::Win32::System::Threading::{
                        GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_HIGHEST,
                    };
                    let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
                }
                task();
            });
    }
}

// --------------------------------------------------------------------- devices

#[tauri::command]
fn get_input_devices() -> Result<Vec<audio::device::AudioDeviceInfo>, String> {
    audio::device::get_input_devices()
}

#[tauri::command]
fn get_output_devices() -> Result<Vec<audio::device::AudioDeviceInfo>, String> {
    audio::device::get_output_devices()
}

// ---------------------------------------------------------------------- bridge

#[tauri::command]
async fn start_bridge(
    input_device_id: String,
    output_device_id: String,
    app: tauri::AppHandle,
    state: tauri::State<'_, audio::engine::AudioEngine>,
    sounds: tauri::State<'_, state::SoundManager>,
) -> Result<(), String> {
    let engine = state.inner().clone();
    let sound_paths = sounds
        .get_sounds()
        .into_iter()
        .map(|sound| sound.path)
        .collect::<Vec<_>>();
    let monitor = sounds.monitor_device();

    tauri::async_runtime::spawn_blocking(move || {
        engine.start_bridge(app, input_device_id, output_device_id, monitor)?;

        // Rebuild the cache with the format selected by the actual output device. This moves
        // decode/resample work out of the first in-game hotkey press.
        for path in sound_paths {
            if let Err(err) = engine.preload_sound(&path) {
                tracing::warn!("Could not prewarm '{}': {}", path, err);
            }
        }
        Ok(())
    })
    .await
    .map_err(|err| format!("Audio startup task failed: {err}"))?
}

#[tauri::command]
fn stop_bridge(state: tauri::State<'_, audio::engine::AudioEngine>) {
    state.stop_bridge();
}

#[tauri::command]
fn get_engine_stats(
    state: tauri::State<'_, audio::engine::AudioEngine>,
) -> audio::engine::EngineStats {
    state.stats()
}

// -------------------------------------------------------------------- playback

#[tauri::command]
async fn play_sound(
    file_path: String,
    state: tauri::State<'_, audio::engine::AudioEngine>,
) -> Result<(), String> {
    let engine = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || engine.play_sound(&file_path))
        .await
        .map_err(|err| format!("Audio playback task failed: {err}"))?
}

/// Plays a library sound using its own volume / loop / trim / hold settings.
#[tauri::command]
fn play_sound_by_id(
    id: String,
    state: tauri::State<'_, audio::engine::AudioEngine>,
    sounds: tauri::State<'_, state::SoundManager>,
) -> Result<(), String> {
    let Some(sound) = sounds.get_sound(&id) else {
        return Err(format!("Sound not found: {id}"));
    };
    let options = voice_options_for(&sound, sounds.audio_settings().fade_ms);
    state.play_sound_with_options(&id, &sound.path, options)
}

#[tauri::command]
fn stop_sound(state: tauri::State<'_, audio::engine::AudioEngine>) {
    state.stop_sound();
}

#[tauri::command]
fn stop_sound_by_id(id: String, state: tauri::State<'_, audio::engine::AudioEngine>) {
    state.stop_sound_by_id(&id);
}

#[tauri::command]
fn release_sound(id: String, state: tauri::State<'_, audio::engine::AudioEngine>) {
    state.release_sound(&id);
}

#[tauri::command]
fn set_paused(paused: bool, state: tauri::State<'_, audio::engine::AudioEngine>) {
    state.set_paused(paused);
}

#[tauri::command]
fn get_playback_progress(
    state: tauri::State<'_, audio::engine::AudioEngine>,
) -> Option<(usize, usize)> {
    state.get_mixer().get_playback_state()
}

#[tauri::command]
fn get_active_voices(
    state: tauri::State<'_, audio::engine::AudioEngine>,
) -> Vec<echovoice_core::mixer::VoiceInfo> {
    state.get_mixer().active_voices()
}

#[tauri::command]
fn seek_sound(position_ratio: f32, state: tauri::State<'_, audio::engine::AudioEngine>) {
    state.get_mixer().seek(position_ratio);
}

// --------------------------------------------------------------------- volumes

/// Applies an audio-settings patch: persists it and pushes it into the live engine.
fn patch_audio_settings(app: &tauri::AppHandle, mutate: impl FnOnce(&mut AudioSettings)) {
    let state_sm = app.state::<state::SoundManager>();
    let mut settings = state_sm.audio_settings();
    mutate(&mut settings);
    settings.sanitize();
    state_sm.set_audio_settings(settings.clone());
    app.state::<audio::engine::AudioEngine>()
        .apply_audio_settings(&settings);
}

#[tauri::command]
fn set_mic_volume(volume: f32, app: tauri::AppHandle) {
    patch_audio_settings(&app, |settings| settings.mic_volume = volume);
}

#[tauri::command]
fn set_fx_volume(volume: f32, app: tauri::AppHandle) {
    patch_audio_settings(&app, |settings| settings.fx_volume = volume);
}

#[tauri::command]
fn set_noise_gate_threshold(threshold: f32, app: tauri::AppHandle) {
    patch_audio_settings(&app, |settings| settings.noise_gate_threshold = threshold);
}

#[tauri::command]
fn set_mic_muted(muted: bool, app: tauri::AppHandle) {
    apply_mic_mute(&app, muted);
    let _ = app.emit("mic-mute-changed", muted);
}

#[tauri::command]
fn set_fx_muted(muted: bool, app: tauri::AppHandle) {
    apply_fx_mute(&app, muted);
    let _ = app.emit("fx-mute-changed", muted);
}

#[tauri::command]
fn set_ducking(enabled: bool, amount: f32, app: tauri::AppHandle) {
    patch_audio_settings(&app, |settings| {
        settings.ducking_enabled = enabled;
        settings.ducking_amount = amount;
    });
}

#[tauri::command]
fn set_max_concurrent(max: usize, app: tauri::AppHandle) {
    patch_audio_settings(&app, |settings| settings.max_concurrent_sounds = max);
}

#[tauri::command]
fn set_fade_ms(fade_ms: f32, app: tauri::AppHandle) {
    patch_audio_settings(&app, |settings| settings.fade_ms = fade_ms);
}

// ----------------------------------------------------------------- sound library

#[tauri::command]
fn get_sounds(state: tauri::State<'_, state::SoundManager>) -> Vec<state::SoundItem> {
    state.get_sounds()
}

#[tauri::command]
fn get_categories(state: tauri::State<'_, state::SoundManager>) -> Vec<String> {
    state.get_app_config().categories()
}

#[tauri::command]
async fn add_sound(
    item: state::SoundItem,
    state: tauri::State<'_, state::SoundManager>,
    engine: tauri::State<'_, audio::engine::AudioEngine>,
) -> Result<(), String> {
    let path = item.path.clone();
    let id = item.id.clone();
    let engine = engine.inner().clone();
    let preloader = engine.clone();
    let duration = tauri::async_runtime::spawn_blocking(move || preloader.preload_sound(&path))
        .await
        .map_err(|err| format!("Audio loader task failed: {err}"))??;

    state.add_sound(item);
    state.patch_sound(&id, |sound| sound.duration_ms = Some(duration));
    sync_music_playlist(&state, &engine);
    Ok(())
}

#[tauri::command]
fn remove_sound(
    id: String,
    state: tauri::State<'_, state::SoundManager>,
    state_ae: tauri::State<'_, audio::engine::AudioEngine>,
    app: tauri::AppHandle,
) {
    let old = state.get_sounds().into_iter().find(|s| s.id == id);
    if let Some(old_item) = old {
        // 1. Unregister any existing shortcut for this sound before removing it
        if let Some(old_sc) = old_item.shortcut {
            if let Ok(sc) = Shortcut::from_str(&old_sc) {
                unregister_shortcut(&app, sc);
            }
        }

        // 2. Stop playback to prevent active playing slices from holding references
        state_ae.stop_sound_by_id(&id);

        // 3. Unload raw audio from cache memory (Free RAM)
        state_ae.unload_sound(&old_item.path);
    }

    // 4. Remove from state
    state.remove_sound(&id);
    sync_music_playlist(&state, &state_ae);
}

#[tauri::command]
fn rename_sound(
    id: String,
    name: String,
    state: tauri::State<'_, state::SoundManager>,
) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("Name cannot be empty".to_string());
    }
    if state.patch_sound(&id, |sound| sound.name = name) {
        Ok(())
    } else {
        Err(format!("Sound not found: {id}"))
    }
}

#[tauri::command]
fn set_sound_volume(
    id: String,
    volume: f32,
    state: tauri::State<'_, state::SoundManager>,
) -> Result<(), String> {
    patch_sound(&state, &id, |sound| sound.volume = volume)
}

#[tauri::command]
fn set_sound_loop(
    id: String,
    loop_playback: bool,
    state: tauri::State<'_, state::SoundManager>,
) -> Result<(), String> {
    patch_sound(&state, &id, |sound| sound.loop_playback = loop_playback)
}

#[tauri::command]
fn toggle_sound_favorite(
    id: String,
    state: tauri::State<'_, state::SoundManager>,
) -> Result<bool, String> {
    let mut favorite = false;
    patch_sound(&state, &id, |sound| {
        sound.favorite = !sound.favorite;
        favorite = sound.favorite;
    })?;
    Ok(favorite)
}

#[tauri::command]
fn set_sound_category(
    id: String,
    category: String,
    state: tauri::State<'_, state::SoundManager>,
) -> Result<(), String> {
    patch_sound(&state, &id, |sound| sound.category = category)
}

#[tauri::command]
fn set_sound_trim(
    id: String,
    start_offset_ms: u32,
    end_offset_ms: Option<u32>,
    state: tauri::State<'_, state::SoundManager>,
) -> Result<(), String> {
    patch_sound(&state, &id, |sound| {
        sound.start_offset_ms = start_offset_ms;
        sound.end_offset_ms = end_offset_ms;
    })
}

#[tauri::command]
fn set_sound_retrigger(
    id: String,
    retrigger: bool,
    state: tauri::State<'_, state::SoundManager>,
) -> Result<(), String> {
    patch_sound(&state, &id, |sound| sound.retrigger = retrigger)
}

#[tauri::command]
fn set_sound_hold_to_play(
    id: String,
    hold_to_play: bool,
    state: tauri::State<'_, state::SoundManager>,
) -> Result<(), String> {
    patch_sound(&state, &id, |sound| sound.hold_to_play = hold_to_play)
}

fn patch_sound(
    state: &tauri::State<'_, state::SoundManager>,
    id: &str,
    mutate: impl FnOnce(&mut state::SoundItem),
) -> Result<(), String> {
    if state.patch_sound(id, mutate) {
        Ok(())
    } else {
        Err(format!("Sound not found: {id}"))
    }
}

// -------------------------------------------------------------------- shortcuts

#[tauri::command]
fn update_shortcut(
    id: String,
    shortcut: Option<String>,
    state: tauri::State<'_, state::SoundManager>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let config = state.get_app_config();
    let old_item = config
        .sounds
        .iter()
        .find(|sound| sound.id == id)
        .ok_or_else(|| format!("Sound not found: {id}"))?;
    let old_shortcut = old_item
        .shortcut
        .as_deref()
        .and_then(|value| Shortcut::from_str(value).ok());
    let new_shortcut = shortcut.as_deref().map(parse_shortcut).transpose()?;

    if let Some(candidate) = new_shortcut.as_ref() {
        if let Some(conflict) = shortcut_conflict(&config, candidate, Some(&id), true) {
            return Err(format!("Shortcut is already assigned to {conflict}"));
        }
        if old_shortcut.as_ref().map(Shortcut::id) == Some(candidate.id()) {
            state.update_shortcut(&id, Some(candidate.to_string()));
            return Ok(());
        }
    }

    if let Some(old) = old_shortcut.as_ref() {
        unregister_shortcut(&app, *old);
    }

    if let Some(candidate) = new_shortcut {
        if let Err(err) = register_shortcut(&app, candidate) {
            if let Some(old) = old_shortcut {
                let _ = register_shortcut(&app, old);
            }
            return Err(err);
        }
        state.update_shortcut(&id, Some(candidate.to_string()));
    } else {
        state.update_shortcut(&id, None);
    }

    Ok(())
}

#[tauri::command]
fn update_global_stop_shortcut(
    shortcut: Option<String>,
    state: tauri::State<'_, state::SoundManager>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    update_global_action_shortcut(state, app, GlobalAction::Stop, shortcut)
}

#[tauri::command]
fn update_mic_mute_shortcut(
    shortcut: Option<String>,
    state: tauri::State<'_, state::SoundManager>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    update_global_action_shortcut(state, app, GlobalAction::MicMute, shortcut)
}

#[tauri::command]
fn update_fx_mute_shortcut(
    shortcut: Option<String>,
    state: tauri::State<'_, state::SoundManager>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    update_global_action_shortcut(state, app, GlobalAction::FxMute, shortcut)
}

enum GlobalAction {
    Stop,
    MicMute,
    FxMute,
}

fn update_global_action_shortcut(
    state: tauri::State<'_, state::SoundManager>,
    app: tauri::AppHandle,
    action: GlobalAction,
    shortcut: Option<String>,
) -> Result<(), String> {
    let config = state.get_app_config();
    let current = match action {
        GlobalAction::Stop => config.global_stop_shortcut.clone(),
        GlobalAction::MicMute => config.mic_mute_shortcut.clone(),
        GlobalAction::FxMute => config.fx_mute_shortcut.clone(),
    };
    let old_shortcut = current
        .as_deref()
        .and_then(|value| Shortcut::from_str(value).ok());
    let new_shortcut = shortcut.as_deref().map(parse_shortcut).transpose()?;

    if let Some(candidate) = new_shortcut.as_ref() {
        if let Some(conflict) = shortcut_conflict(
            &config,
            candidate,
            None,
            matches!(action, GlobalAction::Stop),
        ) {
            return Err(format!("Shortcut is already assigned to {conflict}"));
        }
        if old_shortcut.as_ref().map(Shortcut::id) == Some(candidate.id()) {
            save_global_shortcut(&state, &action, Some(candidate.to_string()));
            return Ok(());
        }
    }

    if let Some(old) = old_shortcut.as_ref() {
        unregister_shortcut(&app, *old);
    }

    if let Some(candidate) = new_shortcut {
        if let Err(err) = register_shortcut(&app, candidate) {
            if let Some(old) = old_shortcut {
                let _ = register_shortcut(&app, old);
            }
            return Err(err);
        }
        save_global_shortcut(&state, &action, Some(candidate.to_string()));
    } else {
        save_global_shortcut(&state, &action, None);
    }

    Ok(())
}

fn save_global_shortcut(
    state: &tauri::State<'_, state::SoundManager>,
    action: &GlobalAction,
    shortcut: Option<String>,
) {
    match action {
        GlobalAction::Stop => state.update_global_stop_shortcut(shortcut),
        GlobalAction::MicMute => state.set_mic_mute_shortcut(shortcut),
        GlobalAction::FxMute => state.set_fx_mute_shortcut(shortcut),
    }
}

// ---------------------------------------------------------------------- config

#[tauri::command]
fn get_app_config(state: tauri::State<'_, state::SoundManager>) -> state::AppConfig {
    state.get_app_config()
}

#[tauri::command]
fn set_default_devices(
    input_id: Option<String>,
    output_id: Option<String>,
    state: tauri::State<'_, state::SoundManager>,
) {
    state.set_default_devices(input_id, output_id)
}

#[tauri::command]
fn set_language(lang: Option<String>, state: tauri::State<'_, state::SoundManager>) {
    state.set_language(lang)
}

#[tauri::command]
fn set_theme(theme: String, state: tauri::State<'_, state::SoundManager>) {
    state.set_theme(theme)
}

#[tauri::command]
fn set_auto_start_bridge(enabled: bool, state: tauri::State<'_, state::SoundManager>) {
    state.set_auto_start_bridge(enabled)
}

#[tauri::command]
fn set_minimize_to_tray(enabled: bool, state: tauri::State<'_, state::SoundManager>) {
    state.set_minimize_to_tray(enabled)
}

#[tauri::command]
fn set_launch_at_startup(
    enabled: bool,
    state: tauri::State<'_, state::SoundManager>,
) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    autostart::set_enabled(enabled)?;
    state.set_launch_at_startup(enabled);
    Ok(())
}

/// Whether EchoVoice is currently registered to start with Windows.
#[tauri::command]
fn get_launch_at_startup() -> bool {
    #[cfg(target_os = "windows")]
    {
        autostart::is_enabled()
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

// ------------------------------------------------------------------------ DSP

#[tauri::command]
fn set_eq_enabled(enabled: bool, app: tauri::AppHandle) {
    patch_audio_settings(&app, |settings| settings.eq.enabled = enabled);
}

#[tauri::command]
fn set_eq_gains(low: f32, mid: f32, high: f32, app: tauri::AppHandle) {
    patch_audio_settings(&app, |settings| {
        settings.eq.low_db = low;
        settings.eq.mid_db = mid;
        settings.eq.high_db = high;
    });
}

#[tauri::command]
fn set_compressor_enabled(enabled: bool, app: tauri::AppHandle) {
    patch_audio_settings(&app, |settings| settings.compressor.enabled = enabled);
}

#[tauri::command]
fn set_compressor_params(
    threshold: f32,
    ratio: f32,
    attack: f32,
    release: f32,
    gain: f32,
    app: tauri::AppHandle,
) {
    patch_audio_settings(&app, |settings| {
        settings.compressor.threshold_db = threshold;
        settings.compressor.ratio = ratio;
        settings.compressor.attack_ms = attack;
        settings.compressor.release_ms = release;
        settings.compressor.makeup_db = gain;
    });
}

#[tauri::command]
fn set_pitch_enabled(enabled: bool, app: tauri::AppHandle) {
    patch_audio_settings(&app, |settings| settings.pitch.enabled = enabled);
}

#[tauri::command]
fn set_pitch_ratio(ratio: f32, app: tauri::AppHandle) {
    patch_audio_settings(&app, |settings| settings.pitch.ratio = ratio);
}

// ----------------------------------------------------------------------- music

/// Rebuilds the engine playlist from the persisted library.
fn sync_music_playlist(
    state: &tauri::State<'_, state::SoundManager>,
    engine: &audio::engine::AudioEngine,
) {
    let config = state.get_app_config();
    let entries: Vec<audio::engine::MusicEntry> = config
        .music_tracks()
        .into_iter()
        .map(|sound| audio::engine::MusicEntry {
            id: sound.id.clone(),
            name: sound.name.clone(),
            path: sound.path.clone(),
            volume: sound.volume,
            duration_ms: sound.duration_ms,
        })
        .collect();
    engine.set_music_playlist(entries, config.music_loop_playlist);
    engine.set_music_loop_playlist(config.music_loop_playlist);
}

#[tauri::command]
fn get_music_playlist(state: tauri::State<'_, state::SoundManager>) -> Vec<state::SoundItem> {
    state
        .get_app_config()
        .music_tracks()
        .into_iter()
        .cloned()
        .collect()
}

#[tauri::command]
fn get_music_state(
    state: tauri::State<'_, audio::engine::AudioEngine>,
) -> audio::engine::MusicState {
    state.music_state()
}

#[tauri::command]
fn play_music_at(
    index: usize,
    state: tauri::State<'_, audio::engine::AudioEngine>,
) -> Result<(), String> {
    state.play_music_at(index)
}

#[tauri::command]
fn toggle_music(
    app: tauri::AppHandle,
    state: tauri::State<'_, audio::engine::AudioEngine>,
) -> Result<(), String> {
    state.toggle_music()?;
    let _ = app.emit("music-state-changed", ());
    Ok(())
}

#[tauri::command]
fn next_music(
    app: tauri::AppHandle,
    state: tauri::State<'_, audio::engine::AudioEngine>,
) -> Result<(), String> {
    state.next_music()?;
    let _ = app.emit("music-state-changed", ());
    Ok(())
}

#[tauri::command]
fn prev_music(
    app: tauri::AppHandle,
    state: tauri::State<'_, audio::engine::AudioEngine>,
) -> Result<(), String> {
    state.prev_music()?;
    let _ = app.emit("music-state-changed", ());
    Ok(())
}

#[tauri::command]
fn stop_music(app: tauri::AppHandle, state: tauri::State<'_, audio::engine::AudioEngine>) {
    state.stop_music();
    let _ = app.emit("music-state-changed", ());
}

#[tauri::command]
fn set_music_paused(paused: bool, state: tauri::State<'_, audio::engine::AudioEngine>) {
    state.set_music_paused(paused);
}

#[tauri::command]
fn set_music_volume(volume: f32, app: tauri::AppHandle) {
    patch_audio_settings(&app, |settings| settings.music_volume = volume);
}

#[tauri::command]
fn set_music_loop_playlist(enabled: bool, state: tauri::State<'_, state::SoundManager>) {
    state.set_music_loop_playlist(enabled);
}

/// Turns local monitoring on/off and picks the output device. Takes effect the next time
/// the bridge starts.
#[tauri::command]
fn set_monitor(
    enabled: bool,
    device_id: Option<String>,
    state: tauri::State<'_, state::SoundManager>,
) {
    state.set_monitor(enabled, device_id);
}

#[tauri::command]
fn set_sound_is_music(
    id: String,
    is_music: bool,
    state: tauri::State<'_, state::SoundManager>,
    engine: tauri::State<'_, audio::engine::AudioEngine>,
) -> Result<(), String> {
    patch_sound(&state, &id, |sound| sound.is_music = is_music)?;
    sync_music_playlist(&state, &engine);
    Ok(())
}

#[tauri::command]
fn update_music_shortcut(
    action: String,
    shortcut: Option<String>,
    state: tauri::State<'_, state::SoundManager>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let config = state.get_app_config();
    let current = match action.as_str() {
        "toggle" => config.music_toggle_shortcut.clone(),
        "next" => config.music_next_shortcut.clone(),
        "prev" => config.music_prev_shortcut.clone(),
        "stop" => config.music_stop_shortcut.clone(),
        other => return Err(format!("Unknown music action: {other}")),
    };
    let old_shortcut = current
        .as_deref()
        .and_then(|value| Shortcut::from_str(value).ok());
    let new_shortcut = shortcut.as_deref().map(parse_shortcut).transpose()?;

    if let Some(candidate) = new_shortcut.as_ref() {
        if let Some(conflict) = shortcut_conflict(&config, candidate, None, false) {
            return Err(format!("Shortcut is already assigned to {conflict}"));
        }
        if old_shortcut.as_ref().map(Shortcut::id) == Some(candidate.id()) {
            state.set_music_shortcut(&action, Some(candidate.to_string()));
            return Ok(());
        }
    }

    if let Some(old) = old_shortcut.as_ref() {
        unregister_shortcut(&app, *old);
    }
    if let Some(candidate) = new_shortcut {
        if let Err(err) = register_shortcut(&app, candidate) {
            if let Some(old) = old_shortcut {
                let _ = register_shortcut(&app, old);
            }
            return Err(err);
        }
        state.set_music_shortcut(&action, Some(candidate.to_string()));
    } else {
        state.set_music_shortcut(&action, None);
    }
    Ok(())
}

// ------------------------------------------------------------------- recording

#[tauri::command]
fn start_recording(state: tauri::State<'_, audio::engine::AudioEngine>) -> Result<(), String> {
    state.start_recording()
}

#[tauri::command]
fn stop_recording(
    path: String,
    state: tauri::State<'_, audio::engine::AudioEngine>,
) -> Result<u32, String> {
    state.stop_recording_to(&path)
}

#[tauri::command]
fn is_recording(state: tauri::State<'_, audio::engine::AudioEngine>) -> bool {
    state.is_recording()
}

/// Snapshot of everything that decides whether an in-game hotkey can work.
#[derive(serde::Serialize)]
pub struct HotkeyDiagnostics {
    /// The low-level keyboard hook is installed and receiving keys.
    pub listener_active: bool,
    /// Keystrokes the hook has observed since launch. Zero while a game is focused and
    /// EchoVoice is not elevated is the classic "game runs as admin" dead end.
    pub keystrokes_seen: u64,
    /// True when EchoVoice itself is running elevated.
    pub elevated: bool,
    /// The channel actually in use.
    pub channel: &'static str,
    /// Every bound accelerator, so the user can eyeball what should fire.
    pub bound_hotkeys: Vec<BoundHotkey>,
    pub sound_count: usize,
}

#[derive(serde::Serialize)]
pub struct BoundHotkey {
    pub accelerator: String,
    pub target: String,
    pub hold_to_play: bool,
}

#[tauri::command]
fn get_hotkey_diagnostics(state: tauri::State<'_, state::SoundManager>) -> HotkeyDiagnostics {
    let config = state.get_app_config();
    let mut bound = Vec::new();

    if let Some(shortcut) = config.global_stop_shortcut.clone() {
        bound.push(BoundHotkey {
            accelerator: shortcut,
            target: "Stop all sounds".to_string(),
            hold_to_play: false,
        });
    }
    if let Some(shortcut) = config.mic_mute_shortcut.clone() {
        bound.push(BoundHotkey {
            accelerator: shortcut,
            target: "Mute microphone".to_string(),
            hold_to_play: false,
        });
    }
    if let Some(shortcut) = config.fx_mute_shortcut.clone() {
        bound.push(BoundHotkey {
            accelerator: shortcut,
            target: "Mute soundboard".to_string(),
            hold_to_play: false,
        });
    }
    for sound in &config.sounds {
        if let Some(shortcut) = sound.shortcut.clone() {
            bound.push(BoundHotkey {
                accelerator: shortcut,
                target: sound.name.clone(),
                hold_to_play: sound.hold_to_play,
            });
        }
    }

    #[cfg(target_os = "windows")]
    let (listener_active, keystrokes_seen, channel) = (
        game_hotkey::is_available(),
        game_hotkey::keystroke_count(),
        if game_hotkey::is_available() {
            "keyboard-hook"
        } else {
            "register-hotkey"
        },
    );
    #[cfg(not(target_os = "windows"))]
    let (listener_active, keystrokes_seen, channel) = (false, 0u64, "unavailable");

    HotkeyDiagnostics {
        listener_active,
        keystrokes_seen,
        elevated: diagnostics::is_elevated(),
        channel,
        bound_hotkeys: bound,
        sound_count: config.sounds.len(),
    }
}

/// Relaunches EchoVoice as administrator (shows the UAC prompt, then restarts).
#[tauri::command]
fn restart_as_administrator() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        diagnostics::restart_elevated()
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err("Only available on Windows".to_string())
    }
}

/// Starts the "试听" debug bridge: captures the virtual cable's output and plays it on a
/// real device, so the user can hear exactly what QQ would receive.
#[tauri::command]
fn start_audition(
    input_device_id: String,
    output_device_id: String,
    app: tauri::AppHandle,
    state: tauri::State<'_, audio::engine::AudioEngine>,
) -> Result<(), String> {
    state.start_audition(app, input_device_id, output_device_id)
}

#[tauri::command]
fn stop_audition(state: tauri::State<'_, audio::engine::AudioEngine>) {
    state.stop_audition();
}

/// Picks the best default audition pair: CABLE Output -> the system default output.
#[tauri::command]
fn resolve_audition_devices() -> Result<(String, String), String> {
    let enumerator = wasapi::DeviceEnumerator::new()
        .map_err(|err| format!("Failed to get enumerator: {err}"))?;

    let mut cable_output = None;
    if let Ok(collection) = enumerator.get_device_collection(&wasapi::Direction::Capture) {
        for index in 0..collection.get_nbr_devices().unwrap_or(0) {
            if let Ok(device) = collection.get_device_at_index(index) {
                let name = device.get_friendlyname().unwrap_or_default().to_lowercase();
                if name.contains("cable") && name.contains("output") {
                    if let Ok(id) = device.get_id() {
                        cable_output = Some(id);
                        break;
                    }
                }
            }
        }
    }
    let input = cable_output.ok_or_else(|| {
        "No CABLE Output capture device found - is VB-Cable installed?".to_string()
    })?;

    let output = enumerator
        .get_default_device(&wasapi::Direction::Render)
        .map_err(|err| format!("No default playback device: {err}"))?
        .get_id()
        .map_err(|err| format!("Could not read the playback device id: {err}"))?;

    Ok((input, output))
}

/// Full routing self-check, surfaced in the UI so a broken chain is obvious.
#[tauri::command]
fn get_routing_diagnostics(
    state: tauri::State<'_, state::SoundManager>,
    engine: tauri::State<'_, audio::engine::AudioEngine>,
) -> Result<audio::device::RoutingDiagnostics, String> {
    let config = state.get_app_config();
    let input = config
        .default_input_id
        .clone()
        .ok_or("No microphone selected yet")?;
    let output = config
        .default_output_id
        .clone()
        .ok_or("No output device selected yet")?;
    Ok(audio::device::get_routing_diagnostics(
        &input,
        &output,
        engine.is_running(),
    ))
}

/// Where microphone recordings are written: `<app data>/EchoVoice/recordings`.
#[tauri::command]
fn get_recordings_dir(app: tauri::AppHandle) -> Result<String, String> {
    let base = app
        .path()
        .app_local_data_dir()
        .map_err(|err| format!("Could not resolve the app data directory: {err}"))?;
    let dir = base.join("EchoVoice").join("recordings");
    std::fs::create_dir_all(&dir)
        .map_err(|err| format!("Could not create the recordings folder: {err}"))?;
    dir.to_str()
        .map(|path| path.to_string())
        .ok_or_else(|| "Recordings path is not valid UTF-8".to_string())
}

// ----------------------------------------------------------------- app startup

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt::init();

    let audio_engine = audio::engine::AudioEngine::new();
    let sound_manager = state::SoundManager::new();

    tauri::Builder::default()
        .setup(|app| {
            let config_dir = app
                .path()
                .app_local_data_dir()
                .unwrap_or_else(|_| std::env::current_dir().unwrap());
            std::fs::create_dir_all(&config_dir).ok();
            let config_path = config_dir.join("echovoice_config.json");

            let sm = app.state::<state::SoundManager>();
            sm.init_path_and_load(config_path);

            #[cfg(target_os = "windows")]
            if let Err(err) = game_hotkey::install(app.handle().clone()) {
                tracing::warn!("Game-compatible hotkey listener is unavailable: {}", err);
            }

            // register all shortcuts initially
            let config = sm.get_app_config();
            for sound in &config.sounds {
                if let Some(sc_str) = sound.shortcut.as_deref() {
                    match parse_shortcut(sc_str) {
                        Ok(sc) => {
                            if let Err(err) = register_shortcut(app.handle(), sc) {
                                tracing::error!(
                                    "Failed to restore shortcut for '{}': {}",
                                    sound.name,
                                    err
                                );
                            }
                        }
                        Err(err) => {
                            tracing::error!("Invalid saved shortcut for '{}': {}", sound.name, err)
                        }
                    }
                }
            }
            for action in [
                &config.global_stop_shortcut,
                &config.mic_mute_shortcut,
                &config.fx_mute_shortcut,
                &config.music_toggle_shortcut,
                &config.music_next_shortcut,
                &config.music_prev_shortcut,
                &config.music_stop_shortcut,
            ] {
                if let Some(sc_str) = action.as_deref() {
                    match parse_shortcut(sc_str) {
                        Ok(sc) => {
                            if let Err(err) = register_shortcut(app.handle(), sc) {
                                tracing::error!("Failed to restore global shortcut: {}", err);
                            }
                        }
                        Err(err) => tracing::error!("Invalid saved global shortcut: {}", err),
                    }
                }
            }

            // Restore the audio settings into the (not yet running) engine so the mixer is
            // already configured when the bridge opens.
            let engine = app.state::<audio::engine::AudioEngine>();
            engine.apply_audio_settings(&config.audio);
            sync_music_playlist(&sm, &engine);

            let quit_i = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let toggle_i =
                MenuItem::with_id(app, "toggle_bridge", "Toggle Bridge", true, None::<&str>)?;
            let stop_i = MenuItem::with_id(app, "stop_all", "Stop All Sounds", true, None::<&str>)?;
            let mic_mute_i = CheckMenuItem::with_id(
                app,
                "mic_mute",
                "Mute Microphone",
                true,
                config.audio.mic_muted,
                None::<&str>,
            )?;
            let fx_mute_i = CheckMenuItem::with_id(
                app,
                "fx_mute",
                "Mute Soundboard",
                true,
                config.audio.fx_muted,
                None::<&str>,
            )?;
            let menu =
                Menu::with_items(app, &[&toggle_i, &stop_i, &mic_mute_i, &fx_mute_i, &quit_i])?;

            let _tray = TrayIconBuilder::new()
                .menu(&menu)
                .show_menu_on_left_click(false)
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("EchoVoice")
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "quit" => {
                        std::process::exit(0);
                    }
                    "toggle_bridge" => {
                        let _ = app.emit("toggle-bridge", ());
                    }
                    "stop_all" => {
                        app.state::<audio::engine::AudioEngine>().stop_sound();
                        let _ = app.emit("global-stop", true);
                    }
                    "mic_mute" => {
                        let muted = !app
                            .state::<state::SoundManager>()
                            .audio_settings()
                            .mic_muted;
                        apply_mic_mute(app, muted);
                        let _ = app.emit("mic-mute-changed", muted);
                    }
                    "fx_mute" => {
                        let muted = !app.state::<state::SoundManager>().audio_settings().fx_muted;
                        apply_fx_mute(app, muted);
                        let _ = app.emit("fx-mute-changed", muted);
                    }
                    _ => {
                        println!("menu item {:?} not handled", event.id.as_ref());
                    }
                })
                .on_tray_icon_event(|tray, event| match event {
                    TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } => {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    _ => {}
                })
                .build(app)?;

            Ok(())
        })
        .manage(audio_engine)
        .manage(sound_manager)
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| match event.state() {
                    ShortcutState::Pressed => handle_shortcut_activation(app, shortcut),
                    ShortcutState::Released => handle_shortcut_release(app, shortcut),
                })
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            get_input_devices,
            get_output_devices,
            start_bridge,
            stop_bridge,
            get_engine_stats,
            play_sound,
            play_sound_by_id,
            stop_sound,
            stop_sound_by_id,
            release_sound,
            set_paused,
            get_playback_progress,
            get_active_voices,
            seek_sound,
            set_mic_volume,
            set_fx_volume,
            set_noise_gate_threshold,
            set_mic_muted,
            set_fx_muted,
            set_ducking,
            set_max_concurrent,
            set_fade_ms,
            get_sounds,
            get_categories,
            add_sound,
            remove_sound,
            rename_sound,
            set_sound_volume,
            set_sound_loop,
            toggle_sound_favorite,
            set_sound_category,
            set_sound_trim,
            set_sound_retrigger,
            set_sound_hold_to_play,
            update_shortcut,
            update_global_stop_shortcut,
            update_mic_mute_shortcut,
            update_fx_mute_shortcut,
            get_app_config,
            set_default_devices,
            set_language,
            set_theme,
            set_auto_start_bridge,
            set_minimize_to_tray,
            set_launch_at_startup,
            get_launch_at_startup,
            set_eq_enabled,
            set_eq_gains,
            set_compressor_enabled,
            set_compressor_params,
            set_pitch_enabled,
            set_pitch_ratio,
            get_music_playlist,
            get_music_state,
            play_music_at,
            toggle_music,
            next_music,
            prev_music,
            stop_music,
            set_music_paused,
            set_music_volume,
            set_music_loop_playlist,
            set_monitor,
            set_sound_is_music,
            update_music_shortcut,
            start_recording,
            stop_recording,
            is_recording,
            get_recordings_dir,
            start_audition,
            stop_audition,
            resolve_audition_devices,
            get_routing_diagnostics,
            get_hotkey_diagnostics,
            restart_as_administrator,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
