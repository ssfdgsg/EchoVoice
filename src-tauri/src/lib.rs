pub mod audio;

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
fn play_sound(
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt::init();

    let audio_engine = audio::engine::AudioEngine::new();

    tauri::Builder::default()
        .manage(audio_engine)
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            get_input_devices,
            get_output_devices,
            start_bridge,
            stop_bridge,
            play_sound,
            set_mic_volume,
            set_fx_volume,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
