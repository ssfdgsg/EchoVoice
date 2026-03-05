pub mod audio;
pub mod state;

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
fn get_input_devices() -> Result<Vec<audio::device::AudioDeviceInfo>, String> {
    audio::device::get_input_devices()
}

#[tauri::command]
fn get_output_devices() -> Result<Vec<audio::device::AudioDeviceInfo>, String> {
    audio::device::get_output_devices()
}

#[tauri::command]
fn start_bridge(
    input_device_id: String,
    output_device_id: String,
    state: tauri::State<'_, audio::engine::AudioEngine>,
) -> Result<(), String> {
    state.start_bridge(input_device_id, output_device_id)
}

#[tauri::command]
fn stop_bridge(state: tauri::State<'_, audio::engine::AudioEngine>) {
    state.stop_bridge();
}

#[tauri::command]
async fn play_sound(
    file_path: String,
    state: tauri::State<'_, audio::engine::AudioEngine>,
) -> Result<(), String> {
    state.play_sound(&file_path)
}

#[tauri::command]
fn set_mic_volume(volume: f32, state: tauri::State<'_, audio::engine::AudioEngine>) {
    state.get_mixer().set_mic_volume(volume);
}

#[tauri::command]
fn set_fx_volume(volume: f32, state: tauri::State<'_, audio::engine::AudioEngine>) {
    state.get_mixer().set_fx_volume(volume);
}

#[tauri::command]
fn stop_sound(state: tauri::State<'_, audio::engine::AudioEngine>) {
    state.stop_sound();
}

#[tauri::command]
fn get_sounds(state: tauri::State<'_, state::SoundManager>) -> Vec<state::SoundItem> {
    state.get_sounds()
}

#[tauri::command]
fn get_playback_progress(
    state: tauri::State<'_, audio::engine::AudioEngine>,
) -> Option<(usize, usize)> {
    state.get_mixer().get_playback_state()
}

#[tauri::command]
fn seek_sound(
    position_ratio: f32, // 0.0 to 1.0
    state: tauri::State<'_, audio::engine::AudioEngine>,
) {
    state.get_mixer().seek(position_ratio);
}

#[tauri::command]
fn add_sound(item: state::SoundItem, state: tauri::State<'_, state::SoundManager>) {
    state.add_sound(item);
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
                let _ = app.global_shortcut().unregister(sc);
            }
        }

        // 2. Stop playback to prevent active playing slices from holding references
        state_ae.stop_sound();

        // 3. Unload raw audio from cache memory (Free RAM)
        state_ae.unload_sound(&old_item.path);
    }

    // 4. Remove from state
    state.remove_sound(&id);
}

use std::str::FromStr;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

#[tauri::command]
fn update_shortcut(
    id: String,
    shortcut: Option<String>,
    state: tauri::State<'_, state::SoundManager>,
    app: tauri::AppHandle,
) {
    let old = state.get_sounds().into_iter().find(|s| s.id == id);
    if let Some(old) = old {
        if let Some(old_sc) = old.shortcut {
            if let Ok(sc) = Shortcut::from_str(&old_sc) {
                let _ = app.global_shortcut().unregister(sc);
            }
        }
    }

    if let Some(ref new_sc) = shortcut {
        if let Ok(sc) = Shortcut::from_str(new_sc) {
            let _ = app.global_shortcut().register(sc.clone());
            // Save normalized string so it matches `shortcut.to_string()` in the handler!
            state.update_shortcut(&id, Some(sc.to_string()));
            return;
        }
    }

    state.update_shortcut(&id, None);
}

#[tauri::command]
fn update_global_stop_shortcut(
    shortcut: Option<String>,
    state: tauri::State<'_, state::SoundManager>,
    app: tauri::AppHandle,
) {
    let config = state.get_app_config();
    if let Some(old_sc) = config.global_stop_shortcut {
        if let Ok(sc) = Shortcut::from_str(&old_sc) {
            let _ = app.global_shortcut().unregister(sc);
        }
    }

    if let Some(new_sc) = &shortcut {
        // Normalize Svelte shortcut string (e.g. from "CommandOrControl+X" to "CommandOrControl+X")
        let mut corrected = new_sc
            .replace("Ctrl", "Control")
            .replace("Command", "Super")
            .replace("CommandOrControl", "SuperKey"); // Tauri might map CommandOrControl internally or just keep Super/Control

        if corrected.contains("SuperKey") {
            #[cfg(target_os = "macos")]
            {
                corrected = corrected.replace("SuperKey", "Super");
            }
            #[cfg(not(target_os = "macos"))]
            {
                corrected = corrected.replace("SuperKey", "Control");
            }
        }

        if let Ok(sc) = Shortcut::from_str(&corrected) {
            let _ = app.global_shortcut().register(sc);
            state.update_global_stop_shortcut(Some(corrected));
            return;
        }
    }

    state.update_global_stop_shortcut(None);
}

#[tauri::command]
fn get_app_config(state: tauri::State<'_, state::SoundManager>) -> state::AppConfig {
    state.get_app_config()
}

#[tauri::command]
fn set_bg_image(path: Option<String>, state: tauri::State<'_, state::SoundManager>) {
    state.set_bg_image(path)
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
fn set_noise_gate_threshold(
    threshold: Option<f32>,
    state: tauri::State<'_, state::SoundManager>,
    engine: tauri::State<'_, audio::engine::AudioEngine>,
) {
    state.set_noise_gate_threshold(threshold);
    if let Some(val) = threshold {
        engine.get_mixer().set_noise_gate_threshold(val);
    } else {
        engine.get_mixer().set_noise_gate_threshold(0.0);
    }
}

use crate::audio::dsp::{compressor::Compressor, eq::ThreeBandEq, pitch::PitchShifter};

#[tauri::command]
fn set_eq_enabled(enabled: bool, state: tauri::State<'_, audio::engine::AudioEngine>) {
    if let Ok(mut chain) = state.get_mixer().dsp_chain.lock() {
        if let Some(effect) = chain.get_effect_mut(0) {
            effect.set_enabled(enabled);
        }
    }
}

#[tauri::command]
fn set_eq_gains(
    low: f32,
    mid: f32,
    high: f32,
    state: tauri::State<'_, audio::engine::AudioEngine>,
) {
    if let Ok(mut chain) = state.get_mixer().dsp_chain.lock() {
        if let Some(effect) = chain.get_effect_mut(0) {
            if let Some(eq) = effect.as_any_mut().downcast_mut::<ThreeBandEq>() {
                eq.set_low_gain(low);
                eq.set_mid_gain(mid);
                eq.set_high_gain(high);
            }
        }
    }
}

#[tauri::command]
fn set_compressor_enabled(enabled: bool, state: tauri::State<'_, audio::engine::AudioEngine>) {
    if let Ok(mut chain) = state.get_mixer().dsp_chain.lock() {
        if let Some(effect) = chain.get_effect_mut(1) {
            effect.set_enabled(enabled);
        }
    }
}

#[tauri::command]
fn set_compressor_params(
    threshold: f32,
    ratio: f32,
    attack: f32,
    release: f32,
    gain: f32,
    state: tauri::State<'_, audio::engine::AudioEngine>,
) {
    if let Ok(mut chain) = state.get_mixer().dsp_chain.lock() {
        if let Some(effect) = chain.get_effect_mut(1) {
            if let Some(comp) = effect.as_any_mut().downcast_mut::<Compressor>() {
                comp.set_threshold(threshold);
                comp.set_ratio(ratio);
                comp.set_attack(attack);
                comp.set_release(release);
                comp.set_makeup_gain(gain);
            }
        }
    }
}

#[tauri::command]
fn set_pitch_enabled(enabled: bool, state: tauri::State<'_, audio::engine::AudioEngine>) {
    if let Ok(mut chain) = state.get_mixer().dsp_chain.lock() {
        if let Some(effect) = chain.get_effect_mut(2) {
            effect.set_enabled(enabled);
        }
    }
}

#[tauri::command]
fn set_pitch_ratio(ratio: f32, state: tauri::State<'_, audio::engine::AudioEngine>) {
    if let Ok(mut chain) = state.get_mixer().dsp_chain.lock() {
        if let Some(effect) = chain.get_effect_mut(2) {
            if let Some(pitch) = effect.as_any_mut().downcast_mut::<PitchShifter>() {
                pitch.set_pitch_ratio(ratio);
            }
        }
    }
}

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager};

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

            // Helper to parse shortcuts
            let parse_sc = |sc_str: &str| -> Option<Shortcut> {
                let mut corrected = sc_str
                    .replace("Ctrl", "Control")
                    .replace("Command", "Super")
                    .replace("CommandOrControl", "SuperKey");
                if corrected.contains("SuperKey") {
                    #[cfg(target_os = "macos")]
                    {
                        corrected = corrected.replace("SuperKey", "Super");
                    }
                    #[cfg(not(target_os = "macos"))]
                    {
                        corrected = corrected.replace("SuperKey", "Control");
                    }
                }
                Shortcut::from_str(&corrected).ok()
            };

            // register all shortcuts initially
            let config = sm.get_app_config();
            for sound in config.sounds {
                if let Some(sc_str) = sound.shortcut {
                    if let Some(sc) = parse_sc(&sc_str) {
                        let _ = app.global_shortcut().register(sc);
                    }
                }
            }
            if let Some(stop_sc_str) = config.global_stop_shortcut {
                if let Some(sc) = parse_sc(&stop_sc_str) {
                    let _ = app.global_shortcut().register(sc);
                }
            }

            let quit_i = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let toggle_i =
                MenuItem::with_id(app, "toggle_bridge", "Toggle Bridge", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&toggle_i, &quit_i])?;

            let _tray = TrayIconBuilder::new()
                .menu(&menu)
                .show_menu_on_left_click(false)
                .icon(app.default_window_icon().unwrap().clone())
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "quit" => {
                        std::process::exit(0);
                    }
                    "toggle_bridge" => {
                        let _ = app.emit("toggle-bridge", ());
                    }
                    _ => {
                        println!("menu item {:?} not handled", event.id);
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
                .with_handler(|app, shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        let sc_string = shortcut.to_string();
                        let state_sm = app.state::<state::SoundManager>();
                        let config = state_sm.get_app_config();

                        // 1. Check if it's the global stop shortcut
                        if let Some(stop_sc_str) = config.global_stop_shortcut {
                            // Helper to parse the saved shortcut string down to the `Shortcut` struct which `matches()` understands
                            let parsed = {
                                let mut corrected = stop_sc_str
                                    .replace("Ctrl", "Control")
                                    .replace("Command", "Super")
                                    .replace("CommandOrControl", "SuperKey");
                                if corrected.contains("SuperKey") {
                                    #[cfg(target_os = "macos")]
                                    {
                                        corrected = corrected.replace("SuperKey", "Super");
                                    }
                                    #[cfg(not(target_os = "macos"))]
                                    {
                                        corrected = corrected.replace("SuperKey", "Control");
                                    }
                                }
                                Shortcut::from_str(&corrected).ok()
                            };

                            if let Some(stop_sc) = parsed {
                                if shortcut.id() == stop_sc.id() {
                                    let state_ae = app.state::<audio::engine::AudioEngine>();
                                    state_ae.stop_sound();
                                    let _ = app.emit("global-stop", true);
                                    return; // Stop processing further matches
                                }
                            }
                        }

                        // 2. Otherwise check sound shortcuts
                        if let Some(item) = state_sm.get_sound_by_shortcut(&sc_string) {
                            let app_clone = app.clone();
                            let path = item.path.clone();
                            let id = item.id.clone();
                            std::thread::spawn(move || {
                                let state_ae = app_clone.state::<audio::engine::AudioEngine>();
                                let _ = state_ae.play_sound(&path);
                                let _ = app_clone.emit("shortcut-play", id);
                            });
                        }
                    }
                })
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            get_input_devices,
            get_output_devices,
            start_bridge,
            stop_bridge,
            play_sound,
            stop_sound,
            get_playback_progress,
            seek_sound,
            set_mic_volume,
            set_fx_volume,
            get_sounds,
            add_sound,
            remove_sound,
            update_shortcut,
            get_app_config,
            set_bg_image,
            set_default_devices,
            update_global_stop_shortcut,
            set_language,
            set_noise_gate_threshold,
            set_eq_enabled,
            set_eq_gains,
            set_compressor_enabled,
            set_compressor_params,
            set_pitch_enabled,
            set_pitch_ratio,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
