//! WASAPI audio bridge: real microphone -> ring buffer -> mixer -> VB-Cable.
//!
//! Threading model (see CLAUDE.md §7.4):
//! * one capture thread pulls packets from `IAudioCaptureClient`, converts them to the
//!   internal `f32` interleaved format and pushes them into a lock-free ring buffer;
//! * one render thread asks the output client how much space is free, pops that many
//!   samples from the ring buffer, mixes in every active voice and writes the result;
//! * one telemetry thread publishes diagnostics to the UI at 4 Hz so the audio threads
//!   never have to talk to IPC;
//! * one supervisor thread watches the selected devices and re-opens the bridge when a
//!   headset is unplugged and plugged back in (a very common thing to happen mid-game).

use crate::audio::dsp::{compressor::Compressor, eq::ThreeBandEq, pitch::PitchShifter};
use crate::audio::mixer::{Mixer, VoiceOptions};
use echovoice_core::config::{AudioSettings, MAX_RECORDING_SECONDS};
use echovoice_core::supervisor::Backoff;
use echovoice_core::wav::MicRecorder;
use ringbuf::traits::{Consumer, Observer, Producer, Split};
use ringbuf::HeapRb;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::mpsc;
use std::sync::{
    atomic::{AtomicBool, AtomicU32, Ordering},
    Arc, Mutex,
};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tracing::{error, info, warn};
use wasapi::{
    AudioCaptureClient, AudioClient, AudioRenderClient, DeviceEnumerator, Direction, SampleType,
    StreamMode, WaveFormat,
};

#[cfg(target_os = "windows")]
struct MultimediaPriorityGuard(windows_sys::Win32::Foundation::HANDLE);

#[cfg(target_os = "windows")]
impl Drop for MultimediaPriorityGuard {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::System::Threading::AvRevertMmThreadCharacteristics(self.0);
        }
    }
}

#[cfg(target_os = "windows")]
fn prioritize_audio_thread() -> Option<MultimediaPriorityGuard> {
    use windows_sys::Win32::System::Threading::{
        AvSetMmThreadCharacteristicsW, GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_HIGHEST,
    };

    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
        let mut task_index = 0;
        let task_name = [
            b'P' as u16,
            b'r' as u16,
            b'o' as u16,
            b' ' as u16,
            b'A' as u16,
            b'u' as u16,
            b'd' as u16,
            b'i' as u16,
            b'o' as u16,
            0,
        ];
        let handle = AvSetMmThreadCharacteristicsW(task_name.as_ptr(), &mut task_index);
        (!handle.is_null()).then_some(MultimediaPriorityGuard(handle))
    }
}

struct SafeAudioCaptureClient(AudioCaptureClient);
unsafe impl Send for SafeAudioCaptureClient {}

impl std::ops::Deref for SafeAudioCaptureClient {
    type Target = AudioCaptureClient;

    fn deref(&self) -> &AudioCaptureClient {
        &self.0
    }
}

/// One entry of the music playlist.
#[derive(Debug, Clone)]
pub struct MusicEntry {
    pub id: String,
    pub name: String,
    pub path: String,
    pub volume: f32,
    pub duration_ms: Option<u32>,
}

/// Snapshot of the music player for the UI.
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct MusicState {
    pub playing: bool,
    pub paused: bool,
    pub track_id: Option<String>,
    pub track_name: Option<String>,
    pub position_ms: u32,
    pub length_ms: u32,
    pub index: Option<usize>,
    pub playlist_len: usize,
}

impl Default for MusicState {
    fn default() -> Self {
        Self {
            playing: false,
            paused: false,
            track_id: None,
            track_name: None,
            position_ms: 0,
            length_ms: 0,
            index: None,
            playlist_len: 0,
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct SoundCacheKey {
    path: String,
    sample_rate: u32,
    channels: u16,
}

struct BridgeRuntime {
    input_client: AudioClient,
    output_client: AudioClient,
    capture_client: SafeAudioCaptureClient,
    render_client: AudioRenderClient,
    sample_rate: u32,
    channels: usize,
    bytes_per_frame: usize,
    /// Second render client on a *real* output device, so the user hears what the
    /// teammates hear. `None` when monitoring is off or the device vanished.
    monitor: Option<MonitorStream>,
}

/// Sentinel for "use whatever Windows considers the default playback device".
const DEFAULT_MONITOR_DEVICE: &str = "__default__";

/// Local playback of the exact same mix that goes into the virtual cable.
struct MonitorStream {
    client: AudioClient,
    render_client: AudioRenderClient,
    channels: usize,
}

/// Everything the UI wants to know about the running bridge.
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct EngineStats {
    pub running: bool,
    pub sample_rate: u32,
    pub channels: u32,
    /// Ring buffer capacity in frames.
    pub buffer_frames: usize,
    pub active_voices: usize,
    pub voices_started: u64,
    pub rejected_voices: u64,
    /// How many times the render thread starved for microphone samples.
    pub underruns: u64,
    /// Microphone samples dropped because the ring buffer was full.
    pub dropped_samples: u64,
    pub mic_level: f32,
    pub output_level: f32,
    pub clipped_samples: u64,
    pub gate_open: bool,
    pub ducking: bool,
    pub paused: bool,
    pub recording: bool,
    pub recording_ms: u32,
    /// End-to-end buffer latency of the bridge, in milliseconds.
    pub latency_ms: f32,
    /// Non-zero while the supervisor is retrying after a device disappeared.
    pub reconnecting: bool,
    pub reconnect_attempts: u32,
    pub music_playing: bool,
    pub music_paused: bool,
    pub music_track: Option<String>,
    pub music_position_ms: u32,
    pub music_length_ms: u32,
    /// The "试听" debug bridge: listening to the cable's output.
    pub audition_running: bool,
    /// Level measured at the cable's output - what the voice app would receive.
    pub audition_level: f32,
    pub last_error: Option<String>,
}

impl Default for EngineStats {
    fn default() -> Self {
        Self {
            running: false,
            sample_rate: 48_000,
            channels: 2,
            buffer_frames: 0,
            active_voices: 0,
            voices_started: 0,
            rejected_voices: 0,
            underruns: 0,
            dropped_samples: 0,
            mic_level: 0.0,
            output_level: 0.0,
            clipped_samples: 0,
            gate_open: true,
            ducking: false,
            paused: false,
            recording: false,
            recording_ms: 0,
            latency_ms: 0.0,
            reconnecting: false,
            reconnect_attempts: 0,
            music_playing: false,
            music_paused: false,
            music_track: None,
            music_position_ms: 0,
            music_length_ms: 0,
            audition_running: false,
            audition_level: 0.0,
            last_error: None,
        }
    }
}

/// Shared engine state. Lives behind an `Arc` so the supervisor thread can restart the
/// bridge without going through `tauri::State`.
struct EngineInner {
    // Everything the worker threads need to own is behind an `Arc`, so `EngineInner` can
    // hand out clones to threads that outlive the borrow.
    is_running: Arc<AtomicBool>,
    /// Set to `false` when the user stops the bridge, which also disables auto-reconnect.
    auto_reconnect: AtomicBool,
    mixer: Arc<Mixer>,
    sample_rate: Arc<AtomicU32>,
    output_channels: Arc<AtomicU32>,
    buffer_frames: Arc<AtomicU32>,
    sound_cache: Arc<Mutex<HashMap<SoundCacheKey, Arc<Vec<f32>>>>>,
    decoding_files: Arc<Mutex<HashSet<SoundCacheKey>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
    supervisor: Mutex<Option<JoinHandle<()>>>,
    recorder: Arc<Mutex<Option<MicRecorder>>>,
    last_error: Arc<Mutex<Option<String>>>,
    monitor_enabled: AtomicBool,
    /// Set by the supervisor while it is retrying after a device disappeared.
    reconnecting: AtomicBool,
    reconnect_attempts: AtomicU32,
    /// The music playlist, mirrored from the persisted config.
    playlist: Mutex<Vec<MusicEntry>>,
    /// Loop the playlist instead of stopping after the last track.
    music_loop: AtomicBool,
    /// Set when the user stops the music on purpose, so auto-advance stays quiet.
    music_stopped_by_user: AtomicBool,
    /// True while music was playing on the previous telemetry tick.
    music_was_playing: AtomicBool,
    /// Independent second bridge used by the "试听" debug feature: it captures whatever
    /// comes out of the virtual cable and plays it on a real output device.
    audition: Mutex<Option<AuditionState>>,
}

/// Live state of the audition bridge.
struct AuditionState {
    is_running: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    /// Level measured at the cable's output, i.e. what the voice app would receive.
    level: Arc<Mixer>,
}

#[derive(Clone)]
pub struct AudioEngine {
    inner: Arc<EngineInner>,
}

impl Default for AudioEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioEngine {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(EngineInner {
                is_running: Arc::new(AtomicBool::new(false)),
                auto_reconnect: AtomicBool::new(false),
                mixer: Arc::new(Mixer::new(48_000, 2)),
                sample_rate: Arc::new(AtomicU32::new(48_000)),
                output_channels: Arc::new(AtomicU32::new(2)),
                buffer_frames: Arc::new(AtomicU32::new(0)),
                sound_cache: Arc::new(Mutex::new(HashMap::new())),
                decoding_files: Arc::new(Mutex::new(HashSet::new())),
                worker: Mutex::new(None),
                supervisor: Mutex::new(None),
                recorder: Arc::new(Mutex::new(None)),
                last_error: Arc::new(Mutex::new(None)),
                monitor_enabled: AtomicBool::new(false),
                reconnecting: AtomicBool::new(false),
                reconnect_attempts: AtomicU32::new(0),
                playlist: Mutex::new(Vec::new()),
                music_loop: AtomicBool::new(true),
                music_stopped_by_user: AtomicBool::new(false),
                music_was_playing: AtomicBool::new(false),
                audition: Mutex::new(None),
            }),
        }
    }

    pub fn get_mixer(&self) -> Arc<Mixer> {
        self.inner.mixer.clone()
    }

    pub fn is_running(&self) -> bool {
        self.inner.is_running.load(Ordering::SeqCst)
    }

    /// Pushes the persisted audio settings into the live mixer and DSP chain.
    pub fn apply_audio_settings(&self, settings: &AudioSettings) {
        let mixer = &self.inner.mixer;
        mixer.set_mic_volume(settings.mic_volume);
        mixer.set_fx_volume(settings.fx_volume);
        mixer.set_music_volume(settings.music_volume);
        mixer.set_noise_gate_threshold(settings.noise_gate_threshold);
        mixer.set_mic_muted(settings.mic_muted);
        mixer.set_fx_muted(settings.fx_muted);
        mixer.set_ducking(settings.ducking_enabled, settings.ducking_amount);
        mixer.set_max_concurrent(settings.max_concurrent_sounds);
        mixer.set_fade_ms(settings.fade_ms);

        if let Ok(mut chain) = mixer.dsp_chain.lock() {
            if let Some(effect) = chain.get_effect_mut(0) {
                effect.set_enabled(settings.eq.enabled);
                if let Some(eq) = effect.as_any_mut().downcast_mut::<ThreeBandEq>() {
                    eq.set_low_gain(settings.eq.low_db);
                    eq.set_mid_gain(settings.eq.mid_db);
                    eq.set_high_gain(settings.eq.high_db);
                }
            }
            if let Some(effect) = chain.get_effect_mut(1) {
                effect.set_enabled(settings.compressor.enabled);
                if let Some(comp) = effect.as_any_mut().downcast_mut::<Compressor>() {
                    comp.set_threshold(settings.compressor.threshold_db);
                    comp.set_ratio(settings.compressor.ratio);
                    comp.set_attack(settings.compressor.attack_ms);
                    comp.set_release(settings.compressor.release_ms);
                    comp.set_makeup_gain(settings.compressor.makeup_db);
                }
            }
            if let Some(effect) = chain.get_effect_mut(2) {
                effect.set_enabled(settings.pitch.enabled);
                if let Some(pitch) = effect.as_any_mut().downcast_mut::<PitchShifter>() {
                    pitch.set_pitch_ratio(settings.pitch.ratio);
                }
            }
        }
    }

    pub fn start_bridge(
        &self,
        app: AppHandle,
        input_device_id: String,
        output_device_id: String,
        monitor_device_id: Option<String>,
    ) -> Result<(), String> {
        self.inner.auto_reconnect.store(true, Ordering::SeqCst);
        self.spawn_supervisor(
            app.clone(),
            input_device_id.clone(),
            output_device_id.clone(),
            monitor_device_id.clone(),
        );
        self.inner.start_bridge(
            app,
            input_device_id,
            output_device_id,
            monitor_device_id,
            self.inner.monitor_enabled.load(Ordering::SeqCst),
        )
    }

    pub fn stop_bridge(&self) {
        // A user-initiated stop also disables auto-reconnect.
        self.inner.auto_reconnect.store(false, Ordering::SeqCst);
        self.inner.stop_bridge();
    }

    /// Decodes (and caches) a sound, returning its duration in milliseconds.
    pub fn preload_sound(&self, file_path: &str) -> Result<u32, String> {
        let data = self.inner.load_sound_data(file_path)?;
        let channels = self.inner.output_channels.load(Ordering::Relaxed).max(1);
        let rate = self.inner.sample_rate.load(Ordering::Relaxed).max(1);
        Ok(((data.len() / channels as usize) as f32 / rate as f32 * 1000.0) as u32)
    }

    /// Soundpad style trigger: plays a sound with its own per-sound options.
    pub fn play_sound_with_options(
        &self,
        sound_id: &str,
        file_path: &str,
        options: VoiceOptions,
    ) -> Result<(), String> {
        if !self.is_running() {
            return Err("Bridge is not running. Cannot play sound.".to_string());
        }

        let data = self.inner.load_sound_data(file_path)?;
        let voice_id = self.inner.mixer.play_sound(sound_id, data, options);
        if voice_id == 0 {
            return Err(format!("Sound '{sound_id}' is already playing"));
        }
        Ok(())
    }

    pub fn play_sound(&self, file_path: &str) -> Result<(), String> {
        if !self.is_running() {
            return Err("Bridge is not running. Cannot play sound.".to_string());
        }

        let data = self.inner.load_sound_data(file_path)?;
        self.inner
            .mixer
            .play_sound(file_path, data, VoiceOptions::default());
        Ok(())
    }

    // ------------------------------------------------------------- music player

    /// Replaces the playlist. Called whenever the library changes.
    pub fn set_music_playlist(&self, entries: Vec<MusicEntry>, loop_playlist: bool) {
        self.inner.set_playlist(entries, loop_playlist);
    }

    pub fn music_playlist(&self) -> Vec<MusicEntry> {
        self.inner
            .playlist
            .lock()
            .map(|playlist| playlist.clone())
            .unwrap_or_default()
    }

    /// Play / pause. Starting from nothing plays the current (or first) track.
    pub fn toggle_music(&self) -> Result<(), String> {
        if self.inner.mixer.is_music_playing() {
            let paused = !self.inner.mixer.is_music_paused();
            self.inner.mixer.set_music_paused(paused);
            return Ok(());
        }
        let index = self
            .inner
            .mixer
            .music_sound_id()
            .and_then(|id| self.inner.music_index_of(&id))
            .or_else(|| {
                let playlist = self.inner.playlist.lock().ok()?;
                (!playlist.is_empty()).then_some(0)
            })
            .ok_or("The music playlist is empty")?;
        self.inner.play_playlist_index(index)
    }

    pub fn next_music(&self) -> Result<(), String> {
        let len = self
            .inner
            .playlist
            .lock()
            .map(|playlist| playlist.len())
            .unwrap_or(0);
        if len == 0 {
            return Err("The music playlist is empty".to_string());
        }
        let current = self
            .inner
            .mixer
            .music_sound_id()
            .and_then(|id| self.inner.music_index_of(&id));
        let next = match current {
            Some(index) if index + 1 < len => index + 1,
            _ => 0,
        };
        self.inner.play_playlist_index(next)
    }

    pub fn prev_music(&self) -> Result<(), String> {
        let len = self
            .inner
            .playlist
            .lock()
            .map(|playlist| playlist.len())
            .unwrap_or(0);
        if len == 0 {
            return Err("The music playlist is empty".to_string());
        }
        let current = self
            .inner
            .mixer
            .music_sound_id()
            .and_then(|id| self.inner.music_index_of(&id));
        let previous = match current {
            Some(0) | None => len - 1,
            Some(index) => index - 1,
        };
        self.inner.play_playlist_index(previous)
    }

    pub fn play_music_at(&self, index: usize) -> Result<(), String> {
        self.inner.play_playlist_index(index)
    }

    pub fn stop_music(&self) {
        self.inner.stop_music_inner();
    }

    pub fn set_music_paused(&self, paused: bool) {
        self.inner.mixer.set_music_paused(paused);
    }

    pub fn set_music_loop_playlist(&self, enabled: bool) {
        self.inner.music_loop.store(enabled, Ordering::SeqCst);
    }

    pub fn music_state(&self) -> MusicState {
        let mixer = &self.inner.mixer;
        let track_id = mixer.music_sound_id();
        let index = track_id
            .as_deref()
            .and_then(|id| self.inner.music_index_of(id));
        let (position_ms, length_ms) = mixer
            .music_progress()
            .map(|(position, length)| {
                let rate = self.inner.mixer.sample_rate() as f32
                    * self.inner.mixer.channels().max(1) as f32;
                (
                    ((position as f32 / rate) * 1000.0) as u32,
                    ((length as f32 / rate) * 1000.0) as u32,
                )
            })
            .unwrap_or((0, 0));
        let name = index.and_then(|index| {
            self.inner
                .playlist
                .lock()
                .ok()
                .and_then(|playlist| playlist.get(index).map(|entry| entry.name.clone()))
        });
        MusicState {
            playing: mixer.is_music_playing(),
            paused: mixer.is_music_paused(),
            track_id,
            track_name: name,
            position_ms,
            length_ms,
            index,
            playlist_len: self
                .inner
                .playlist
                .lock()
                .map(|playlist| playlist.len())
                .unwrap_or(0),
        }
    }

    // ------------------------------------------------------------------ audition

    /// Starts a second bridge that captures the virtual cable's *output* and plays it on a
    /// real device. This is the definitive debug tool: it listens to exactly what QQ would
    /// receive, so hearing it proves the whole chain works.
    ///
    /// `input_id` is normally `CABLE Output`; `output_id` is your speakers/headset.
    pub fn start_audition(
        &self,
        app: AppHandle,
        input_device_id: String,
        output_device_id: String,
    ) -> Result<(), String> {
        let mut slot = self
            .inner
            .audition
            .lock()
            .map_err(|_| "Audition lock is poisoned".to_string())?;
        if let Some(state) = slot.as_ref() {
            if state.is_running.load(Ordering::SeqCst) {
                return Err("Audition is already running".to_string());
            }
        }
        // Drop any finished handle before starting a new one.
        if let Some(mut state) = slot.take() {
            if let Some(handle) = state.worker.take() {
                let _ = handle.join();
            }
        }

        let is_running = Arc::new(AtomicBool::new(false));
        // A fresh mixer in pass-through: no gate, no ducking, no DSP, nothing muted.
        let level_mixer = Arc::new(Mixer::new(48_000, 2));

        let worker_running = is_running.clone();
        let worker_mixer = level_mixer.clone();
        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<(), String>>(1);
        let app_clone = app.clone();

        let handle = thread::Builder::new()
            .name("audition-bridge".into())
            .spawn(move || {
                #[cfg(target_os = "windows")]
                let _priority = prioritize_audio_thread();

                let runtime = match initialize_bridge(&input_device_id, &output_device_id, None) {
                    Ok(runtime) => runtime,
                    Err(err) => {
                        let _ = ready_tx.send(Err(err.clone()));
                        error!("Audition initialization error: {}", err);
                        return;
                    }
                };
                worker_mixer.set_sample_rate(runtime.sample_rate);
                worker_mixer.set_channels(runtime.channels as u16);
                worker_running.store(true, Ordering::SeqCst);

                if ready_tx.send(Ok(())).is_err() {
                    worker_running.store(false, Ordering::SeqCst);
                    return;
                }
                info!(
                    "Audition bridge listening on {} -> {}",
                    input_device_id, output_device_id
                );

                // A dedicated telemetry loop so the UI can show a live level meter for the
                // cable's output - the number that proves audio is actually arriving.
                let telemetry_running = Arc::clone(&worker_running);
                let telemetry_mixer = Arc::clone(&worker_mixer);
                let telemetry_app = app.clone();
                let telemetry = thread::spawn(move || {
                    while telemetry_running.load(Ordering::Relaxed) {
                        thread::sleep(Duration::from_millis(200));
                        let stats = telemetry_mixer.stats();
                        let _ = telemetry_app.emit(
                            "audition-level",
                            serde_json::json!({
                                "running": true,
                                "level": stats.mic_peak,
                            }),
                        );
                    }
                    let _ = telemetry_app.emit(
                        "audition-level",
                        serde_json::json!({ "running": false, "level": 0.0 }),
                    );
                });

                let result = run_bridge_loop(
                    runtime,
                    worker_running.clone(),
                    worker_mixer,
                    Arc::new(Mutex::new(None)),
                );
                let _ = telemetry.join();
                worker_running.store(false, Ordering::SeqCst);
                if let Err(err) = result {
                    error!("Audition loop error: {}", err);
                    let _ = app_clone.emit("audition-error", err);
                }
            })
            .map_err(|err| format!("Failed to start the audition bridge: {err}"))?;

        *slot = Some(AuditionState {
            is_running,
            worker: Some(handle),
            level: level_mixer,
        });

        match ready_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => Err("Timed out starting audition".to_string()),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err("Audition worker stopped during startup".to_string())
            }
        }
    }

    pub fn stop_audition(&self) {
        if let Ok(mut slot) = self.inner.audition.lock() {
            if let Some(mut state) = slot.take() {
                state.is_running.store(false, Ordering::SeqCst);
                if let Some(handle) = state.worker.take() {
                    let _ = handle.join();
                }
                info!("Audition bridge stopped");
            }
        }
    }

    pub fn is_audition_running(&self) -> bool {
        self.inner
            .audition
            .lock()
            .map(|slot| {
                slot.as_ref()
                    .map(|state| state.is_running.load(Ordering::SeqCst))
                    .unwrap_or(false)
            })
            .unwrap_or(false)
    }

    /// Level currently arriving at the cable's output endpoint.
    pub fn audition_level(&self) -> f32 {
        self.inner
            .audition
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().map(|state| state.level.stats().mic_peak))
            .unwrap_or(0.0)
    }

    pub fn stop_sound(&self) {
        self.inner.mixer.stop_all();
    }

    pub fn stop_sound_by_id(&self, sound_id: &str) {
        self.inner.mixer.stop_sound(sound_id);
    }

    pub fn release_sound(&self, sound_id: &str) {
        self.inner.mixer.release_held(sound_id);
    }

    pub fn set_paused(&self, paused: bool) {
        self.inner.mixer.set_paused(paused);
    }

    pub fn unload_sound(&self, file_path: &str) {
        if let Ok(mut cache) = self.inner.sound_cache.lock() {
            cache.retain(|key, _| key.path != file_path);
            info!("Unloaded sound from cache: {}", file_path);
        }
    }

    pub fn stats(&self) -> EngineStats {
        self.inner.stats()
    }

    // ------------------------------------------------------------- recording

    /// Starts capturing the raw microphone into memory (capped, see `MAX_RECORDING_SECONDS`).
    pub fn start_recording(&self) -> Result<(), String> {
        if !self.is_running() {
            return Err("Start the bridge before recording".to_string());
        }
        let mut guard = self
            .inner
            .recorder
            .lock()
            .map_err(|_| "Recorder lock is poisoned".to_string())?;
        if guard.is_some() {
            return Err("Already recording".to_string());
        }
        *guard = Some(MicRecorder::new(
            self.inner.sample_rate.load(Ordering::Relaxed),
            self.inner.output_channels.load(Ordering::Relaxed) as u16,
            MAX_RECORDING_SECONDS,
        ));
        info!("Microphone recording started");
        Ok(())
    }

    pub fn is_recording(&self) -> bool {
        self.inner
            .recorder
            .lock()
            .map(|guard| guard.is_some())
            .unwrap_or(false)
    }

    /// Stops recording and writes the captured audio to `path` as a WAV file.
    pub fn stop_recording_to(&self, path: &str) -> Result<u32, String> {
        let recorder = {
            let mut guard = self
                .inner
                .recorder
                .lock()
                .map_err(|_| "Recorder lock is poisoned".to_string())?;
            guard.take()
        };
        let Some(recorder) = recorder else {
            return Err("Not recording".to_string());
        };
        let writer = recorder.into_writer();
        if writer.is_empty() {
            return Err("Nothing was recorded".to_string());
        }
        let duration = writer.duration_ms();
        writer.write_to_file(path)?;
        info!("Recording saved to {} ({} ms)", path, duration);
        Ok(duration)
    }

    pub fn stop_recording(&self) {
        if let Ok(mut guard) = self.inner.recorder.lock() {
            if guard.take().is_some() {
                info!("Recording discarded");
            }
        }
    }
}

impl EngineInner {
    /// Opens the bridge and spawns the audio worker + telemetry thread.
    ///
    /// `spawn_supervisor` is deliberately *not* called from here: only the outer
    /// [`AudioEngine`] owns the device ids the supervisor should retry with.
    fn start_bridge(
        self: &Arc<Self>,
        app: AppHandle,
        input_device_id: String,
        output_device_id: String,
        monitor_device_id: Option<String>,
        monitor_enabled: bool,
    ) -> Result<(), String> {
        // Owned handle so the worker/telemetry threads can outlive this borrow.
        let inner = Arc::clone(self);

        // A stopped worker must be fully gone before a new one can observe is_running=true.
        if !inner.is_running.load(Ordering::SeqCst) {
            if let Some(old_worker) = self
                .worker
                .lock()
                .map_err(|_| "Audio worker lock is poisoned".to_string())?
                .take()
            {
                let _ = old_worker.join();
            }
        }

        inner
            .is_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| "Engine is already running".to_string())?;

        if let Ok(mut last_error) = inner.last_error.lock() {
            *last_error = None;
        }

        info!(
            "Starting audio bridge from {} to {}",
            input_device_id, output_device_id
        );

        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<(), String>>(1);

        let worker_inner = Arc::clone(&inner);
        let handle = thread::spawn(move || {
            #[cfg(target_os = "windows")]
            let _priority = prioritize_audio_thread();

            // `None` means "use the system default output", which still has to open a stream.
            let monitor_id = monitor_device_id
                .clone()
                .or_else(|| monitor_enabled.then(|| String::from(DEFAULT_MONITOR_DEVICE)));
            let runtime =
                match initialize_bridge(&input_device_id, &output_device_id, monitor_id.as_deref())
                {
                    Ok(runtime) => runtime,
                    Err(err) => {
                        worker_inner.is_running.store(false, Ordering::SeqCst);
                        if let Ok(mut slot) = worker_inner.last_error.lock() {
                            *slot = Some(err.clone());
                        }
                        let _ = ready_tx.send(Err(err.clone()));
                        error!("Bridge initialization error: {}", err);
                        return;
                    }
                };

            worker_inner
                .sample_rate
                .store(runtime.sample_rate, Ordering::Relaxed);
            worker_inner
                .output_channels
                .store(runtime.channels as u32, Ordering::Relaxed);
            let ring_frames = (runtime.sample_rate as usize * runtime.channels).max(1024);
            worker_inner
                .buffer_frames
                .store(ring_frames as u32, Ordering::Relaxed);
            worker_inner.mixer.set_sample_rate(runtime.sample_rate);
            worker_inner.mixer.set_channels(runtime.channels as u16);

            if ready_tx.send(Ok(())).is_err() {
                worker_inner.is_running.store(false, Ordering::SeqCst);
                return;
            }

            let telemetry_app = app.clone();
            let telemetry_inner = Arc::clone(&worker_inner);
            let telemetry = thread::spawn(move || {
                while telemetry_inner.is_running.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(250));
                    let stats = telemetry_inner.mixer.stats();
                    let (recording_active, recording) = telemetry_inner
                        .recorder
                        .lock()
                        .map(|guard| {
                            guard
                                .as_ref()
                                .map(|r| (true, r.duration_ms()))
                                .unwrap_or((false, 0))
                        })
                        .unwrap_or((false, 0));
                    let voices: Vec<serde_json::Value> = telemetry_inner
                        .mixer
                        .active_voices()
                        .into_iter()
                        .map(|voice| {
                            serde_json::json!({
                                "voice_id": voice.voice_id,
                                "sound_id": voice.sound_id,
                                "position": voice.position,
                                "length": voice.length,
                                "loop_playback": voice.loop_playback,
                                "hold_to_play": voice.hold_to_play,
                            })
                        })
                        .collect();
                    let payload = serde_json::json!({
                        "running": true,
                        "voices": voices,
                        "sample_rate": telemetry_inner.mixer.sample_rate(),
                        "channels": telemetry_inner.mixer.channels(),
                        "buffer_frames": ring_frames,
                        "active_voices": stats.active_voices,
                        "voices_started": stats.total_voices_started,
                        "rejected_voices": stats.rejected_voices,
                        "underruns": stats.underruns,
                        "dropped_samples": stats.dropped_samples,
                        "mic_level": stats.mic_peak,
                        "output_level": stats.output_peak,
                        "clipped_samples": stats.clipped_samples,
                        "gate_open": stats.gate_open,
                        "ducking": stats.ducking,
                        "paused": stats.paused,
                        "recording": recording_active,
                        "recording_ms": recording,
                        "latency_ms": (ring_frames as f32 / telemetry_inner.mixer.sample_rate() as f32) * 1000.0,
                        "music_playing": telemetry_inner.mixer.is_music_playing(),
                        "music_paused": telemetry_inner.mixer.is_music_paused(),
                        "music_track": telemetry_inner.mixer.music_sound_id(),
                        "music_position_ms": telemetry_inner.music_ms_at(|p| p.0),
                        "music_length_ms": telemetry_inner.music_ms_at(|p| p.1),
                        "reconnecting": telemetry_inner.reconnecting.load(Ordering::Relaxed),
                        "reconnect_attempts": telemetry_inner
                            .reconnect_attempts
                            .load(Ordering::Relaxed),
                        "last_error": telemetry_inner.last_error.lock().ok().and_then(|slot| slot.clone()),
                    });
                    let _ = telemetry_app.emit("engine-stats", payload);

                    // A track that ran to its end advances the playlist.
                    if telemetry_inner
                        .music_was_playing
                        .swap(false, Ordering::SeqCst)
                        && !telemetry_inner.mixer.is_music_playing()
                        && !telemetry_inner.music_stopped_by_user.load(Ordering::SeqCst)
                    {
                        let current = telemetry_inner
                            .mixer
                            .music_sound_id()
                            .and_then(|id| telemetry_inner.music_index_of(&id));
                        let next = match current {
                            Some(index) => index + 1,
                            None => 0,
                        };
                        let len = telemetry_inner
                            .playlist
                            .lock()
                            .map(|playlist| playlist.len())
                            .unwrap_or(0);
                        let looping = telemetry_inner.music_loop.load(Ordering::SeqCst);
                        let target = if next < len {
                            Some(next)
                        } else if looping && len > 0 {
                            Some(0)
                        } else {
                            None
                        };
                        if let Some(target) = target {
                            if let Err(err) = telemetry_inner.play_playlist_index(target) {
                                warn!("Could not advance the music playlist: {}", err);
                            }
                        } else {
                            let _ = telemetry_app.emit("music-ended", ());
                        }
                    }
                }
            });

            let result = run_bridge_loop(
                runtime,
                worker_inner.is_running.clone(),
                worker_inner.mixer.clone(),
                worker_inner.recorder.clone(),
            );

            let _ = telemetry.join();
            worker_inner.is_running.store(false, Ordering::SeqCst);
            if let Err(err) = &result {
                if let Ok(mut slot) = worker_inner.last_error.lock() {
                    *slot = Some(err.clone());
                }
                let _ = app.emit("bridge-error", err.clone());
            }
            let _ = app.emit("bridge-stopped", ());
            if let Err(err) = result {
                error!("Bridge loop error: {}", err);
            }
        });

        *self
            .worker
            .lock()
            .map_err(|_| "Audio worker lock is poisoned".to_string())? = Some(handle);

        match ready_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                inner.is_running.store(false, Ordering::SeqCst);
                Err("Timed out while initializing audio devices".to_string())
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                inner.is_running.store(false, Ordering::SeqCst);
                Err("Audio worker stopped during initialization".to_string())
            }
        }
    }

    fn stop_bridge(&self) {
        info!("Stopping audio bridge");
        self.is_running.store(false, Ordering::SeqCst);

        if let Ok(mut worker) = self.worker.lock() {
            if let Some(handle) = worker.take() {
                let _ = handle.join();
            }
        }
        self.mixer.clear();
        self.music_was_playing.store(false, Ordering::SeqCst);
        if let Ok(mut recorder) = self.recorder.lock() {
            recorder.take();
        }
    }

    fn load_sound_data(&self, file_path: &str) -> Result<Arc<Vec<f32>>, String> {
        if !Path::new(file_path).is_file() {
            return Err(format!("Audio file does not exist: {file_path}"));
        }

        let key = SoundCacheKey {
            path: file_path.to_string(),
            sample_rate: self.sample_rate.load(Ordering::Relaxed),
            channels: self.output_channels.load(Ordering::Relaxed) as u16,
        };

        if let Some(data) = self
            .sound_cache
            .lock()
            .map_err(|_| "Sound cache lock is poisoned".to_string())?
            .get(&key)
            .cloned()
        {
            return Ok(data);
        }

        {
            let mut decoding = self
                .decoding_files
                .lock()
                .map_err(|_| "Decoder state lock is poisoned".to_string())?;
            if !decoding.insert(key.clone()) {
                return Err("This sound is still loading; please try again shortly".to_string());
            }
        }

        let result =
            echovoice_core::fx::SoundBank::load_from_file(&key.path, key.sample_rate, key.channels)
                .map(|bank| {
                    info!(
                        "Loaded sound: {} ({} samples, {} Hz, {} ch)",
                        key.path,
                        bank.data.len(),
                        bank.sample_rate,
                        bank.channels
                    );
                    Arc::new(bank.data)
                })
                .map_err(|err| {
                    error!("Failed to load sound {}: {}", key.path, err);
                    format!("Failed to load sound: {err}")
                });

        if let Ok(mut decoding) = self.decoding_files.lock() {
            decoding.remove(&key);
        }

        if let Ok(data) = &result {
            self.sound_cache
                .lock()
                .map_err(|_| "Sound cache lock is poisoned".to_string())?
                .insert(key, data.clone());
        }

        result
    }

    // ------------------------------------------------------------- music player

    fn set_playlist(&self, entries: Vec<MusicEntry>, loop_playlist: bool) {
        if let Ok(mut playlist) = self.playlist.lock() {
            *playlist = entries;
        }
        self.music_loop.store(loop_playlist, Ordering::SeqCst);
    }

    /// Starts a playlist entry by index. Restarts the track if it is already current.
    fn play_playlist_index(&self, index: usize) -> Result<(), String> {
        if !self.is_running.load(Ordering::SeqCst) {
            return Err("Bridge is not running. Cannot play music.".to_string());
        }
        let entry = {
            let playlist = self
                .playlist
                .lock()
                .map_err(|_| "Playlist lock is poisoned".to_string())?;
            playlist
                .get(index)
                .cloned()
                .ok_or_else(|| format!("No music track at index {index}"))?
        };

        let data = self.load_sound_data(&entry.path)?;
        let voice_id = self.mixer.play_music(
            entry.id.clone(),
            data,
            VoiceOptions {
                volume: entry.volume,
                loop_playback: false,
                ..Default::default()
            },
        );
        if voice_id == 0 {
            return Err("Music track could not be started".to_string());
        }
        self.music_stopped_by_user.store(false, Ordering::SeqCst);
        self.music_was_playing.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn music_index_of(&self, sound_id: &str) -> Option<usize> {
        let playlist = self.playlist.lock().ok()?;
        playlist.iter().position(|entry| entry.id == sound_id)
    }

    fn stop_music_inner(&self) {
        self.mixer.stop_music();
        self.music_was_playing.store(false, Ordering::SeqCst);
        self.music_stopped_by_user.store(true, Ordering::SeqCst);
    }

    fn audition_running(&self) -> bool {
        self.audition
            .lock()
            .map(|slot| {
                slot.as_ref()
                    .map(|state| state.is_running.load(Ordering::SeqCst))
                    .unwrap_or(false)
            })
            .unwrap_or(false)
    }

    fn audition_level(&self) -> f32 {
        self.audition
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().map(|state| state.level.stats().mic_peak))
            .unwrap_or(0.0)
    }

    /// Interleaved sample offset -> milliseconds, using the live mixer format.
    fn music_ms_at(&self, pick: impl Fn((usize, usize)) -> usize) -> u32 {
        let rate = self.mixer.sample_rate() as f32 * self.mixer.channels().max(1) as f32;
        self.mixer
            .music_progress()
            .map(|progress| ((pick(progress) as f32 / rate) * 1000.0) as u32)
            .unwrap_or(0)
    }

    fn stats(&self) -> EngineStats {
        let mixer_stats = self.mixer.stats();
        let ring_frames = self.buffer_frames.load(Ordering::Relaxed) as usize;
        let sample_rate = self.sample_rate.load(Ordering::Relaxed).max(1);
        let recording_guard = self.recorder.lock().ok();
        let recording = recording_guard
            .as_ref()
            .and_then(|guard| guard.as_ref().map(|recorder| recorder.duration_ms()))
            .unwrap_or(0);
        EngineStats {
            running: self.is_running.load(Ordering::SeqCst),
            sample_rate,
            channels: self.output_channels.load(Ordering::Relaxed),
            buffer_frames: ring_frames,
            active_voices: mixer_stats.active_voices,
            voices_started: mixer_stats.total_voices_started,
            rejected_voices: mixer_stats.rejected_voices,
            underruns: mixer_stats.underruns,
            dropped_samples: mixer_stats.dropped_samples,
            mic_level: mixer_stats.mic_peak,
            output_level: mixer_stats.output_peak,
            clipped_samples: mixer_stats.clipped_samples,
            gate_open: mixer_stats.gate_open,
            ducking: mixer_stats.ducking,
            paused: mixer_stats.paused,
            recording: recording_guard
                .as_ref()
                .map(|guard| guard.is_some())
                .unwrap_or(false),
            recording_ms: recording,
            latency_ms: (ring_frames as f32 / sample_rate as f32) * 1000.0,
            reconnecting: self.reconnecting.load(Ordering::SeqCst),
            reconnect_attempts: self.reconnect_attempts.load(Ordering::SeqCst),
            music_playing: self.mixer.is_music_playing(),
            music_paused: self.mixer.is_music_paused(),
            music_track: self.mixer.music_sound_id(),
            music_position_ms: self.music_ms_at(|progress| progress.0),
            music_length_ms: self.music_ms_at(|progress| progress.1),
            audition_running: self.audition_running(),
            audition_level: self.audition_level(),
            last_error: self.last_error.lock().ok().and_then(|slot| slot.clone()),
        }
    }
}

impl AudioEngine {
    /// Spawns the device-loss watchdog.
    ///
    /// Unplugging a headset (or VB-Cable being reinstalled) tears the WASAPI stream down
    /// mid-game. Without this the user has to alt-tab and press "Start Bridge" again, so
    /// the supervisor keeps polling and re-opens the bridge with exponential backoff.
    fn spawn_supervisor(
        &self,
        app: AppHandle,
        input_id: String,
        output_id: String,
        monitor_id: Option<String>,
    ) {
        let _ = &monitor_id;
        let inner = self.inner.clone();
        // Retire any previous supervisor first: two of them would fight over the bridge.
        let previous = inner
            .supervisor
            .lock()
            .ok()
            .and_then(|mut slot| slot.take());
        if let Some(previous) = previous {
            inner.auto_reconnect.store(false, Ordering::SeqCst);
            let _ = previous.join();
            inner.auto_reconnect.store(true, Ordering::SeqCst);
        }

        {
            let supervisor_input = input_id.clone();
            let supervisor_output = output_id.clone();
            let supervisor_monitor = monitor_id.clone();
            let supervisor_inner = Arc::clone(&inner);
            let handle = thread::Builder::new()
                .name("bridge-supervisor".into())
                .spawn(move || {
                    let mut backoff = Backoff::new(1, 8);

                    loop {
                        thread::sleep(Duration::from_secs(1));

                        // The user stopped the bridge: retire quietly.
                        if !supervisor_inner.auto_reconnect.load(Ordering::SeqCst) {
                            break;
                        }
                        // A restart is already in flight (or the bridge never opened).
                        if !supervisor_inner.is_running.load(Ordering::SeqCst) {
                            continue;
                        }

                        let missing = if !device_present(Direction::Capture, &supervisor_input) {
                            Some("microphone")
                        } else if !device_present(Direction::Render, &supervisor_output) {
                            Some("output")
                        } else {
                            None
                        };

                        let Some(which) = missing else {
                            backoff.record_success();
                            continue;
                        };

                        warn!("Audio {} device disappeared; closing the bridge", which);
                        supervisor_inner.reconnecting.store(true, Ordering::SeqCst);
                        supervisor_inner
                            .reconnect_attempts
                            .store(0, Ordering::SeqCst);
                        supervisor_inner.stop_bridge();
                        if let Ok(mut slot) = supervisor_inner.last_error.lock() {
                            *slot = Some(format!("The {which} device was disconnected"));
                        }
                        let _ =
                            app.emit("bridge-device-lost", serde_json::json!({ "device": which }));

                        // Retry until the device comes back (or the user stops us).
                        loop {
                            if !supervisor_inner.auto_reconnect.load(Ordering::SeqCst) {
                                break;
                            }
                            let delay = backoff.next_delay_secs();
                            supervisor_inner
                                .reconnect_attempts
                                .store(backoff.attempts() + 1, Ordering::SeqCst);
                            let _ = app.emit(
                                "bridge-reconnecting",
                                serde_json::json!({
                                    "device": which,
                                    "attempt": backoff.attempts() + 1,
                                    "retry_in_secs": delay,
                                }),
                            );
                            thread::sleep(Duration::from_secs(delay));
                            if !supervisor_inner.auto_reconnect.load(Ordering::SeqCst) {
                                break;
                            }
                            if !device_present(Direction::Capture, &supervisor_input)
                                || !device_present(Direction::Render, &supervisor_output)
                            {
                                backoff.record_failure();
                                continue;
                            }

                            match supervisor_inner.start_bridge(
                                app.clone(),
                                supervisor_input.clone(),
                                supervisor_output.clone(),
                                supervisor_monitor.clone(),
                                supervisor_inner.monitor_enabled.load(Ordering::SeqCst),
                            ) {
                                Ok(()) => {
                                    backoff.record_success();
                                    supervisor_inner.reconnecting.store(false, Ordering::SeqCst);
                                    supervisor_inner
                                        .reconnect_attempts
                                        .store(0, Ordering::SeqCst);
                                    info!("Bridge reconnected after device loss");
                                    let _ = app.emit("bridge-reconnected", ());
                                    break;
                                }
                                Err(err) => {
                                    warn!("Reconnect attempt failed: {}", err);
                                    backoff.record_failure();
                                }
                            }
                        }
                    }
                })
                .map_err(|err| {
                    error!("Failed to start the bridge supervisor: {}", err);
                    err
                })
                .ok();
            if let Some(handle) = handle {
                if let Ok(mut slot) = inner.supervisor.lock() {
                    *slot = Some(handle);
                }
            }
        }
    }
}

/// `true` when the device is still enumerated by WASAPI.
fn device_present(direction: Direction, device_id: &str) -> bool {
    let Ok(enumerator) = DeviceEnumerator::new() else {
        // Cannot tell: assume present so we never tear down a healthy bridge.
        return true;
    };
    let Ok(collection) = enumerator.get_device_collection(&direction) else {
        return true;
    };
    let Ok(count) = collection.get_nbr_devices() else {
        return true;
    };
    for index in 0..count {
        if let Ok(device) = collection.get_device_at_index(index) {
            if device.get_id().ok().as_deref() == Some(device_id) {
                return true;
            }
        }
    }
    false
}

fn get_device_by_id(dir: Direction, target_id: &str) -> Result<wasapi::Device, String> {
    let enumerator =
        DeviceEnumerator::new().map_err(|err| format!("Failed to get enumerator: {err}"))?;
    let collection = enumerator
        .get_device_collection(&dir)
        .map_err(|err| format!("Failed to get device collection: {err}"))?;
    let count = collection
        .get_nbr_devices()
        .map_err(|err| format!("Failed to get device count: {err}"))?;

    for index in 0..count {
        if let Ok(device) = collection.get_device_at_index(index) {
            if device.get_id().ok().as_deref() == Some(target_id) {
                return Ok(device);
            }
        }
    }

    Err(format!("Device not found: {target_id}"))
}

fn initialize_bridge(
    input_device_id: &str,
    output_device_id: &str,
    monitor_device_id: Option<&str>,
) -> Result<BridgeRuntime, String> {
    wasapi::initialize_mta()
        .ok()
        .map_err(|err| format!("COM initialization failed: {err}"))?;

    info!("Locating input device: {}", input_device_id);
    let input_device = get_device_by_id(Direction::Capture, input_device_id)?;
    info!("Locating output device: {}", output_device_id);
    let output_device = get_device_by_id(Direction::Render, output_device_id)?;

    let mut input_client = input_device
        .get_iaudioclient()
        .map_err(|err| format!("Input client error: {err}"))?;
    let mut output_client = output_device
        .get_iaudioclient()
        .map_err(|err| format!("Output client error: {err}"))?;
    let output_mix_format = output_client
        .get_mixformat()
        .map_err(|err| format!("Output format error: {err}"))?;

    let sample_rate = output_mix_format.get_samplespersec();
    let channels = output_mix_format.get_nchannels() as usize;
    if sample_rate == 0 || channels == 0 {
        return Err("Output device reported an invalid audio format".to_string());
    }

    // Both streams use one explicit internal format. WASAPI performs device-side conversion.
    let stream_format = WaveFormat::new(
        32,
        32,
        &SampleType::Float,
        sample_rate as usize,
        channels,
        None,
    );
    let stream_mode = StreamMode::PollingShared {
        autoconvert: true,
        buffer_duration_hns: 0,
    };

    input_client
        .initialize_client(&stream_format, &Direction::Capture, &stream_mode)
        .map_err(|err| format!("Input device initialization failed: {err}"))?;
    output_client
        .initialize_client(&stream_format, &Direction::Render, &stream_mode)
        .map_err(|err| format!("Output device initialization failed: {err}"))?;

    let capture_client = SafeAudioCaptureClient(
        input_client
            .get_audiocaptureclient()
            .map_err(|err| format!("Input capture error: {err}"))?,
    );
    let render_client = output_client
        .get_audiorenderclient()
        .map_err(|err| format!("Output render error: {err}"))?;

    input_client
        .start_stream()
        .map_err(|err| format!("Input stream failed to start: {err}"))?;
    if let Err(err) = output_client.start_stream() {
        let _ = input_client.stop_stream();
        return Err(format!("Output stream failed to start: {err}"));
    }

    // A monitor is best-effort: if it cannot be opened the bridge still runs, because
    // getting audio to the teammates matters far more than local convenience.
    let monitor = match monitor_device_id {
        Some(id) => match open_monitor(id, &stream_format) {
            Ok(monitor) => {
                info!("Local monitor attached to {}", id);
                Some(monitor)
            }
            Err(err) => {
                warn!("Local monitor unavailable on {}: {}", id, err);
                None
            }
        },
        None => None,
    };

    info!(
        "Audio bridge ready at {} Hz with {} channels{}",
        sample_rate,
        channels,
        if monitor.is_some() {
            " (+local monitor)"
        } else {
            ""
        }
    );

    Ok(BridgeRuntime {
        input_client,
        output_client,
        capture_client,
        render_client,
        sample_rate,
        channels,
        bytes_per_frame: stream_format.get_blockalign() as usize,
        monitor,
    })
}

/// Opens a second render stream on a physical output device using the bridge's format.
/// WASAPI converts on the device side, so a 44.1 kHz headset works next to a 48 kHz cable.
fn open_monitor(device_id: &str, stream_format: &WaveFormat) -> Result<MonitorStream, String> {
    let device = if device_id == DEFAULT_MONITOR_DEVICE {
        DeviceEnumerator::new()
            .map_err(|err| format!("Monitor enumerator error: {err}"))?
            .get_default_device(&Direction::Render)
            .map_err(|err| format!("No default playback device: {err}"))?
    } else {
        get_device_by_id(Direction::Render, device_id)?
    };
    let mut client = device
        .get_iaudioclient()
        .map_err(|err| format!("Monitor client error: {err}"))?;

    // The channel count must come from the *stream* format we are about to initialise
    // with, not from the device's mix format: the buffer handed to `write_to_device` is
    // laid out for `stream_format`, and a 5.1 headset next to a stereo cable would
    // otherwise be fed mis-interleaved data.
    let channels = stream_format.get_nchannels() as usize;
    if channels == 0 {
        return Err("Stream format reported no channels".to_string());
    }

    let stream_mode = StreamMode::PollingShared {
        autoconvert: true,
        buffer_duration_hns: 0,
    };
    client
        .initialize_client(stream_format, &Direction::Render, &stream_mode)
        .map_err(|err| format!("Monitor initialization failed: {err}"))?;
    let render_client = client
        .get_audiorenderclient()
        .map_err(|err| format!("Monitor render error: {err}"))?;
    client
        .start_stream()
        .map_err(|err| format!("Monitor stream failed to start: {err}"))?;

    Ok(MonitorStream {
        client,
        render_client,
        channels,
    })
}

fn run_bridge_loop(
    runtime: BridgeRuntime,
    is_running: Arc<AtomicBool>,
    mixer: Arc<Mixer>,
    recorder: Arc<Mutex<Option<MicRecorder>>>,
) -> Result<(), String> {
    // One second of history is plenty: the render thread drains the ring buffer every
    // few milliseconds, so a deep buffer does not add latency.
    let buffer_size = (runtime.sample_rate as usize * runtime.channels).max(4096);
    let ring_buffer = HeapRb::<f32>::new(buffer_size);
    let (mut producer, mut consumer) = ring_buffer.split();

    let capture_running = is_running.clone();
    let bytes_per_frame = runtime.bytes_per_frame;
    let capture_client = runtime.capture_client;
    let capture_mixer = mixer.clone();
    let capture_recorder = recorder.clone();
    let capture_thread = thread::spawn(move || {
        #[cfg(target_os = "windows")]
        let _priority = prioritize_audio_thread();

        let _ = wasapi::initialize_mta();
        // Reused across packets: the capture loop must not allocate per packet.
        let mut scratch: Vec<u8> = Vec::new();

        while capture_running.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(2));
            loop {
                let frames = match capture_client.get_next_packet_size() {
                    Ok(Some(frames)) if frames > 0 => frames,
                    Ok(_) => break,
                    Err(err) => {
                        error!("Failed to query capture packet: {}", err);
                        capture_running.store(false, Ordering::SeqCst);
                        return;
                    }
                };

                let needed = frames as usize * bytes_per_frame;
                if scratch.len() < needed {
                    scratch.resize(needed, 0);
                }
                match capture_client.read_from_device(&mut scratch[..needed]) {
                    Ok((read_frames, _)) => {
                        let samples: &[f32] = bytemuck::cast_slice(
                            &scratch[..read_frames as usize * bytes_per_frame],
                        );
                        // `push_slice` returns how many samples were actually written; the
                        // remainder is reported as dropped so the UI can show buffer health.
                        let pushed = producer.push_slice(samples);
                        let dropped = samples.len() - pushed;
                        if dropped > 0 {
                            capture_mixer.note_dropped(dropped as u64);
                        }
                        if let Ok(mut guard) = capture_recorder.lock() {
                            if let Some(active) = guard.as_mut() {
                                if !active.push_interleaved(samples) {
                                    warn!("Recording reached its length limit");
                                }
                            }
                        }
                    }
                    Err(err) => {
                        error!("Failed to read capture data: {}", err);
                        capture_running.store(false, Ordering::SeqCst);
                        return;
                    }
                }
            }
        }
    });

    let loop_result = (|| -> Result<(), String> {
        let channels = runtime.channels;
        // Pre-allocated once: the render loop is real-time and must not allocate.
        let mut microphone = vec![0.0f32; channels * 1024];
        let mut output = vec![0.0f32; channels * 1024];

        while is_running.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(2));
            let frames = runtime
                .output_client
                .get_available_space_in_frames()
                .map_err(|err| format!("Failed to query output buffer: {err}"))?;
            if frames == 0 {
                continue;
            }

            let samples_needed = frames as usize * channels;
            if microphone.len() < samples_needed {
                microphone.resize(samples_needed, 0.0);
                output.resize(samples_needed, 0.0);
            }
            let microphone = &mut microphone[..samples_needed];
            let output = &mut output[..samples_needed];

            if consumer.occupied_len() < samples_needed {
                mixer.note_underrun(1);
            }
            for sample in microphone.iter_mut() {
                *sample = consumer.try_pop().unwrap_or(0.0);
            }

            mixer.process_frames(microphone, output);
            runtime
                .render_client
                .write_to_device(frames as usize, bytemuck::cast_slice(output), None)
                .map_err(|err| format!("Failed to write output data: {err}"))?;

            // Local monitoring: send the very same mix to a real output device so the user
            // hears what the teammates hear. Best-effort - a full or vanished monitor must
            // never take the bridge down.
            if let Some(monitor) = runtime.monitor.as_ref() {
                let space = monitor.client.get_available_space_in_frames().unwrap_or(0);
                if space > 0 {
                    let wanted = space as usize * monitor.channels;
                    let take = wanted.min(output.len());
                    let _ = monitor.render_client.write_to_device(
                        take / monitor.channels.max(1),
                        bytemuck::cast_slice(&output[..take]),
                        None,
                    );
                }
            }
        }
        Ok(())
    })();

    is_running.store(false, Ordering::SeqCst);
    let _ = capture_thread.join();
    let _ = runtime.input_client.stop_stream();
    let _ = runtime.output_client.stop_stream();
    if let Some(monitor) = runtime.monitor.as_ref() {
        let _ = monitor.client.stop_stream();
    }
    info!("Audio bridge loop stopped gracefully");
    loop_result
}
