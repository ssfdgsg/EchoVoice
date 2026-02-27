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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
