//! Real-time mixer.
//!
//! Design rules (see CLAUDE.md §14):
//! * `process_frames` never allocates and never blocks: voices live in a pre-sized `Vec`
//!   that is compacted in place, sound ids are `Arc<str>` so cloning a voice is only a
//!   refcount bump, and every parameter is an atomic or a plain field.
//! * Every gain change is ramped so hotkey presses never click.

use crate::dsp::{compressor::Compressor, eq::ThreeBandEq, pitch::PitchShifter, DspChain};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// Default cross-fade used when a sound starts or is released, keeps transients click free.
pub const DEFAULT_FADE_MS: f32 = 6.0;
/// Number of finished voice ids remembered so the UI can stop polling stale progress.
const VOICE_HISTORY: usize = 64;
/// Hard ceiling for a `hold_to_play` voice. If a key-up event is ever missed (hook crash,
/// sleep/resume) the sound still stops instead of looping forever in the middle of a game.
pub const MAX_HOLD_SECONDS: usize = 60;

/// How a single playback should behave.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VoiceOptions {
    pub volume: f32,
    pub loop_playback: bool,
    /// `false` -> ignore the trigger while the same sound is already playing.
    pub retrigger: bool,
    /// `true` -> the sound stops as soon as the hotkey is released.
    pub hold_to_play: bool,
    pub start_offset_ms: u32,
    pub end_offset_ms: Option<u32>,
    pub fade_in_ms: f32,
    pub fade_out_ms: f32,
}

impl Default for VoiceOptions {
    fn default() -> Self {
        Self {
            volume: 1.0,
            loop_playback: false,
            retrigger: true,
            hold_to_play: false,
            start_offset_ms: 0,
            end_offset_ms: None,
            fade_in_ms: DEFAULT_FADE_MS,
            fade_out_ms: DEFAULT_FADE_MS,
        }
    }
}

/// One playing (or looping) instance of a sound.
#[derive(Debug, Clone)]
pub struct FxInstance {
    pub voice_id: u64,
    /// `Arc<str>` on purpose: cloning a voice inside the render loop must not allocate.
    pub sound_id: Arc<str>,
    pub sound_data: Arc<Vec<f32>>,
    /// Inclusive interleaved sample cursor where playback started.
    pub start_position: usize,
    /// Interleaved sample cursor.
    pub position: usize,
    /// Exclusive end cursor in interleaved samples.
    pub end_position: usize,
    pub volume: f32,
    pub loop_playback: bool,
    pub hold_to_play: bool,
    pub fade_in_samples: usize,
    pub fade_out_samples: usize,
    /// When `Some(n)` the voice is fading out and will be retired after `n` samples.
    pub release_remaining: Option<usize>,
}

/// Immutable snapshot of a voice, handed to the UI.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct VoiceInfo {
    pub voice_id: u64,
    pub sound_id: String,
    pub position: usize,
    pub length: usize,
    pub volume: f32,
    pub loop_playback: bool,
    pub hold_to_play: bool,
}

/// Counters + meters exposed to the UI for diagnostics.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MixerStats {
    pub active_voices: usize,
    pub total_voices_started: u64,
    pub rejected_voices: u64,
    /// Ring buffer starvations: the microphone could not deliver samples in time.
    pub underruns: u64,
    /// Samples dropped because the ring buffer was full.
    pub dropped_samples: u64,
    pub mic_peak: f32,
    pub output_peak: f32,
    pub clipped_samples: u64,
    pub gate_open: bool,
    pub ducking: bool,
    pub paused: bool,
    pub music_playing: bool,
    pub music_paused: bool,
}

fn f32_from_bits(bits: u32) -> f32 {
    f32::from_bits(bits)
}

pub struct Mixer {
    voices: Mutex<Vec<FxInstance>>,
    /// The music track. Kept in its own slot (not in `voices`) so pausing music never
    /// pauses the soundboard and vice versa.
    music: Mutex<Option<FxInstance>>,
    /// Recently retired voice ids, used by the UI to detect "this sound finished".
    finished: Mutex<VecDeque<u64>>,
    next_voice_id: AtomicU64,

    sample_rate: AtomicU32,
    channels: AtomicU32,

    mic_volume: AtomicU32,
    fx_volume: AtomicU32,
    music_volume: AtomicU32,
    music_paused: AtomicBool,
    noise_gate_threshold: AtomicU32,
    mic_muted: AtomicBool,
    fx_muted: AtomicBool,
    paused: AtomicBool,
    ducking_enabled: AtomicBool,
    ducking_amount: AtomicU32,
    fade_ms: AtomicU32,
    max_concurrent: AtomicUsize,

    voices_started: AtomicU64,
    voices_rejected: AtomicU64,
    underruns: AtomicU64,
    dropped_samples: AtomicU64,
    mic_peak: AtomicU32,
    output_peak: AtomicU32,
    clipped_samples: AtomicU64,
    gate_open: AtomicBool,
    ducking_active: AtomicBool,

    /// Envelope follower / gate / ducking state.
    ///
    /// These are atomics rather than plain fields so `process_frames` can take `&self`:
    /// the mixer lives behind an `Arc` shared with the UI and telemetry threads, and a
    /// render loop that needs `&mut` would force an `unsafe` aliasing dance.
    /// They are touched once per buffer, not once per sample, so the cost is irrelevant.
    gate_env: AtomicU32,
    gate_gain: AtomicU32,
    duck_env: AtomicU32,

    pub dsp_chain: Arc<Mutex<DspChain>>,
}

impl Default for Mixer {
    fn default() -> Self {
        Self::new(48_000, 2)
    }
}

impl Mixer {
    pub fn new(sample_rate: u32, channels: u16) -> Self {
        let mut dsp = DspChain::new();
        dsp.add_effect(Box::new(ThreeBandEq::new(sample_rate)));
        dsp.add_effect(Box::new(Compressor::new(sample_rate)));
        dsp.add_effect(Box::new(PitchShifter::new(sample_rate)));
        let channels = channels.max(1) as u32;
        dsp.set_channels(channels as usize);

        Self {
            voices: Mutex::new(Vec::with_capacity(32)),
            music: Mutex::new(None),
            finished: Mutex::new(VecDeque::with_capacity(VOICE_HISTORY)),
            next_voice_id: AtomicU64::new(1),
            sample_rate: AtomicU32::new(sample_rate.max(1)),
            channels: AtomicU32::new(channels),
            mic_volume: AtomicU32::new(1.0f32.to_bits()),
            fx_volume: AtomicU32::new(1.0f32.to_bits()),
            music_volume: AtomicU32::new(1.0f32.to_bits()),
            music_paused: AtomicBool::new(false),
            noise_gate_threshold: AtomicU32::new(0.0f32.to_bits()),
            mic_muted: AtomicBool::new(false),
            fx_muted: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            ducking_enabled: AtomicBool::new(false),
            ducking_amount: AtomicU32::new(0.55f32.to_bits()),
            fade_ms: AtomicU32::new(DEFAULT_FADE_MS.to_bits()),
            max_concurrent: AtomicUsize::new(8),
            voices_started: AtomicU64::new(0),
            voices_rejected: AtomicU64::new(0),
            underruns: AtomicU64::new(0),
            dropped_samples: AtomicU64::new(0),
            mic_peak: AtomicU32::new(0.0f32.to_bits()),
            output_peak: AtomicU32::new(0.0f32.to_bits()),
            clipped_samples: AtomicU64::new(0),
            gate_open: AtomicBool::new(true),
            ducking_active: AtomicBool::new(false),
            gate_env: AtomicU32::new(0.0f32.to_bits()),
            gate_gain: AtomicU32::new(1.0f32.to_bits()),
            duck_env: AtomicU32::new(1.0f32.to_bits()),
            dsp_chain: Arc::new(Mutex::new(dsp)),
        }
    }

    // ---------------------------------------------------------------- parameters

    pub fn set_sample_rate(&self, sample_rate: u32) {
        let sample_rate = sample_rate.max(1);
        self.sample_rate.store(sample_rate, Ordering::Relaxed);
        if let Ok(mut chain) = self.dsp_chain.lock() {
            chain.set_sample_rate(sample_rate);
        }
    }

    pub fn set_channels(&self, channels: u16) {
        let channels = channels.max(1) as u32;
        self.channels.store(channels, Ordering::Relaxed);
        if let Ok(mut chain) = self.dsp_chain.lock() {
            chain.set_channels(channels as usize);
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate.load(Ordering::Relaxed)
    }

    pub fn channels(&self) -> u32 {
        self.channels.load(Ordering::Relaxed)
    }

    pub fn set_mic_volume(&self, volume: f32) {
        self.mic_volume
            .store(volume.clamp(0.0, 2.0).to_bits(), Ordering::Relaxed);
    }

    pub fn mic_volume(&self) -> f32 {
        f32_from_bits(self.mic_volume.load(Ordering::Relaxed))
    }

    pub fn set_fx_volume(&self, volume: f32) {
        self.fx_volume
            .store(volume.clamp(0.0, 2.0).to_bits(), Ordering::Relaxed);
    }

    pub fn fx_volume(&self) -> f32 {
        f32_from_bits(self.fx_volume.load(Ordering::Relaxed))
    }

    /// Music has its own level, independent from the soundboard FX slider: muting the
    /// soundboard to talk must not silence the soundtrack.
    pub fn set_music_volume(&self, volume: f32) {
        self.music_volume
            .store(volume.clamp(0.0, 2.0).to_bits(), Ordering::Relaxed);
    }

    pub fn music_volume(&self) -> f32 {
        f32_from_bits(self.music_volume.load(Ordering::Relaxed))
    }

    pub fn set_music_paused(&self, paused: bool) {
        self.music_paused.store(paused, Ordering::Relaxed);
    }

    pub fn is_music_paused(&self) -> bool {
        self.music_paused.load(Ordering::Relaxed)
    }

    /// Applies a whole `AudioSettings` block. Used at startup and by the settings UI.
    pub fn apply_settings(&self, settings: &crate::config::AudioSettings) {
        self.set_mic_volume(settings.mic_volume);
        self.set_fx_volume(settings.fx_volume);
        self.set_music_volume(settings.music_volume);
        self.set_noise_gate_threshold(settings.noise_gate_threshold);
        self.set_mic_muted(settings.mic_muted);
        self.set_fx_muted(settings.fx_muted);
        self.set_ducking(settings.ducking_enabled, settings.ducking_amount);
        self.set_max_concurrent(settings.max_concurrent_sounds);
        self.set_fade_ms(settings.fade_ms);
    }

    pub fn set_noise_gate_threshold(&self, threshold: f32) {
        self.noise_gate_threshold
            .store(threshold.max(0.0).to_bits(), Ordering::Relaxed);
    }

    pub fn noise_gate_threshold(&self) -> f32 {
        f32_from_bits(self.noise_gate_threshold.load(Ordering::Relaxed))
    }

    pub fn set_mic_muted(&self, muted: bool) {
        self.mic_muted.store(muted, Ordering::Relaxed);
    }

    pub fn is_mic_muted(&self) -> bool {
        self.mic_muted.load(Ordering::Relaxed)
    }

    pub fn set_fx_muted(&self, muted: bool) {
        self.fx_muted.store(muted, Ordering::Relaxed);
    }

    pub fn is_fx_muted(&self) -> bool {
        self.fx_muted.load(Ordering::Relaxed)
    }

    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    pub fn set_ducking(&self, enabled: bool, amount: f32) {
        self.ducking_enabled.store(enabled, Ordering::Relaxed);
        self.ducking_amount
            .store(amount.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    pub fn fade_ms(&self) -> f32 {
        f32_from_bits(self.fade_ms.load(Ordering::Relaxed))
    }

    pub fn set_fade_ms(&self, fade_ms: f32) {
        self.fade_ms
            .store(fade_ms.clamp(0.0, 50.0).to_bits(), Ordering::Relaxed);
    }

    pub fn set_max_concurrent(&self, max: usize) {
        self.max_concurrent
            .store(max.clamp(1, 32), Ordering::Relaxed);
    }

    // ------------------------------------------------------------------ voices

    /// Starts a new voice and returns its id, or `0` when the request was ignored.
    pub fn play_sound(
        &self,
        sound_id: impl Into<Arc<str>>,
        sound_data: Arc<Vec<f32>>,
        options: VoiceOptions,
    ) -> u64 {
        let sound_id: Arc<str> = sound_id.into();
        if sound_data.is_empty() {
            self.voices_rejected.fetch_add(1, Ordering::Relaxed);
            return 0;
        }

        let mut voices = match self.voices.lock() {
            Ok(voices) => voices,
            Err(poisoned) => poisoned.into_inner(),
        };

        let already_playing = voices
            .iter()
            .any(|voice| voice.sound_id == sound_id && voice.release_remaining.is_none());
        if already_playing {
            if !options.retrigger {
                self.voices_rejected.fetch_add(1, Ordering::Relaxed);
                return 0;
            }
            // Soundpad style: pressing the hotkey again restarts the sound.
            voices.retain(|voice| voice.sound_id != sound_id);
        }

        let channels = self.channels.load(Ordering::Relaxed) as usize;
        let sample_rate = self.sample_rate.load(Ordering::Relaxed) as f32;
        let mut voice = FxInstance {
            voice_id: 0,
            sound_id,
            sound_data,
            start_position: 0,
            position: 0,
            end_position: 0,
            volume: options.volume.clamp(0.0, 2.0),
            loop_playback: options.loop_playback,
            hold_to_play: options.hold_to_play,
            fade_in_samples: 0,
            fade_out_samples: 0,
            release_remaining: None,
        };
        apply_voice_geometry(&mut voice, &options, channels, sample_rate);

        let max = self.max_concurrent.load(Ordering::Relaxed).max(1);
        while voices.len() >= max {
            // Evict the oldest voice so the newest key press always feels instant.
            voices.remove(0);
        }

        voice.voice_id = self.next_voice_id.fetch_add(1, Ordering::Relaxed);
        let voice_id = voice.voice_id;
        voices.push(voice);
        drop(voices);

        self.voices_started.fetch_add(1, Ordering::Relaxed);
        voice_id
    }

    /// Called when a `hold_to_play` hotkey is released: fade the sound out and retire it.
    pub fn release_held(&self, sound_id: &str) {
        let mut voices = match self.voices.lock() {
            Ok(voices) => voices,
            Err(poisoned) => poisoned.into_inner(),
        };
        for voice in voices.iter_mut() {
            if &*voice.sound_id == sound_id && voice.release_remaining.is_none() {
                let fade = voice.fade_out_samples.max(1);
                voice.release_remaining = Some(fade);
                voice.loop_playback = false;
                voice.end_position = voice.end_position.min(voice.position + fade);
            }
        }
    }

    pub fn stop_voice(&self, voice_id: u64) {
        let mut voices = match self.voices.lock() {
            Ok(voices) => voices,
            Err(poisoned) => poisoned.into_inner(),
        };
        voices.retain(|voice| voice.voice_id != voice_id);
    }

    pub fn stop_sound(&self, sound_id: &str) {
        let mut voices = match self.voices.lock() {
            Ok(voices) => voices,
            Err(poisoned) => poisoned.into_inner(),
        };
        voices.retain(|voice| &*voice.sound_id != sound_id);
    }

    pub fn stop_all(&self) {
        let mut voices = match self.voices.lock() {
            Ok(voices) => voices,
            Err(poisoned) => poisoned.into_inner(),
        };
        voices.clear();
    }

    /// Legacy alias used by the "stop everything" hotkey and the bridge teardown.
    pub fn clear(&self) {
        self.stop_all();
        self.stop_music();
    }

    pub fn is_playing(&self) -> bool {
        self.active_voice_count() > 0
    }

    pub fn active_voice_count(&self) -> usize {
        match self.voices.lock() {
            Ok(voices) => voices.len(),
            Err(poisoned) => poisoned.into_inner().len(),
        }
    }

    pub fn active_voices(&self) -> Vec<VoiceInfo> {
        let voices = match self.voices.lock() {
            Ok(voices) => voices,
            Err(poisoned) => poisoned.into_inner(),
        };
        voices
            .iter()
            .map(|voice| VoiceInfo {
                voice_id: voice.voice_id,
                sound_id: voice.sound_id.to_string(),
                position: voice.position,
                length: voice.end_position,
                volume: voice.volume,
                loop_playback: voice.loop_playback,
                hold_to_play: voice.hold_to_play,
            })
            .collect()
    }

    /// Voice ids that finished since the last call.
    pub fn drain_finished(&self) -> Vec<u64> {
        let mut finished = match self.finished.lock() {
            Ok(finished) => finished,
            Err(poisoned) => poisoned.into_inner(),
        };
        finished.drain(..).collect()
    }

    /// Progress of the most recently started voice: `(position, length)`.
    pub fn get_playback_state(&self) -> Option<(usize, usize)> {
        let voices = match self.voices.lock() {
            Ok(voices) => voices,
            Err(poisoned) => poisoned.into_inner(),
        };
        voices
            .last()
            .map(|voice| (voice.position, voice.end_position))
    }

    pub fn seek(&self, position_ratio: f32) {
        let ratio = position_ratio.clamp(0.0, 1.0);
        let mut voices = match self.voices.lock() {
            Ok(voices) => voices,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(voice) = voices.last_mut() {
            let span = voice.end_position.saturating_sub(voice.start_position);
            voice.position = voice.start_position + (span as f32 * ratio) as usize;
            voice.release_remaining = None;
        }
    }

    // ------------------------------------------------------------------- stats

    pub fn note_underrun(&self, samples: u64) {
        if samples > 0 {
            self.underruns.fetch_add(samples, Ordering::Relaxed);
        }
    }

    pub fn note_dropped(&self, samples: u64) {
        if samples > 0 {
            self.dropped_samples.fetch_add(samples, Ordering::Relaxed);
        }
    }

    pub fn stats(&self) -> MixerStats {
        MixerStats {
            active_voices: self.active_voice_count(),
            total_voices_started: self.voices_started.load(Ordering::Relaxed),
            rejected_voices: self.voices_rejected.load(Ordering::Relaxed),
            underruns: self.underruns.load(Ordering::Relaxed),
            dropped_samples: self.dropped_samples.load(Ordering::Relaxed),
            mic_peak: f32_from_bits(self.mic_peak.load(Ordering::Relaxed)),
            output_peak: f32_from_bits(self.output_peak.load(Ordering::Relaxed)),
            clipped_samples: self.clipped_samples.load(Ordering::Relaxed),
            gate_open: self.gate_open.load(Ordering::Relaxed),
            ducking: self.ducking_active.load(Ordering::Relaxed),
            paused: self.paused.load(Ordering::Relaxed),
            music_playing: self.is_music_playing(),
            music_paused: self.music_paused.load(Ordering::Relaxed),
        }
    }

    // ------------------------------------------------------------- music player

    /// Starts (or restarts) the music track. Returns its voice id.
    pub fn play_music(
        &self,
        sound_id: impl Into<Arc<str>>,
        sound_data: Arc<Vec<f32>>,
        options: VoiceOptions,
    ) -> u64 {
        if sound_data.is_empty() {
            return 0;
        }
        let mut guard = match self.music.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

        let channels = self.channels.load(Ordering::Relaxed) as usize;
        let sample_rate = self.sample_rate.load(Ordering::Relaxed) as f32;
        let mut voice = FxInstance {
            voice_id: 0,
            sound_id: sound_id.into(),
            sound_data,
            start_position: 0,
            position: 0,
            end_position: 0,
            volume: options.volume.clamp(0.0, 2.0),
            loop_playback: options.loop_playback,
            hold_to_play: false,
            fade_in_samples: 0,
            fade_out_samples: 0,
            release_remaining: None,
        };
        apply_voice_geometry(&mut voice, &options, channels, sample_rate);
        voice.voice_id = self.next_voice_id.fetch_add(1, Ordering::Relaxed);
        let voice_id = voice.voice_id;
        *guard = Some(voice);
        self.music_paused.store(false, Ordering::Relaxed);
        voice_id
    }

    pub fn stop_music(&self) {
        let mut guard = match self.music.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(voice) = guard.take() {
            remember_finished(&self.finished, voice.voice_id);
        }
        self.music_paused.store(false, Ordering::Relaxed);
    }

    pub fn is_music_playing(&self) -> bool {
        let guard = match self.music.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.is_some()
    }

    /// `(position, length)` of the music track, in interleaved samples.
    pub fn music_progress(&self) -> Option<(usize, usize)> {
        let guard = match self.music.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .as_ref()
            .map(|voice| (voice.position, voice.end_position))
    }

    pub fn music_sound_id(&self) -> Option<String> {
        let guard = match self.music.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.as_ref().map(|voice| voice.sound_id.to_string())
    }

    // ---------------------------------------------------------------- rendering

    /// Mixes the microphone and every active voice into `output`.
    ///
    /// Real-time safe: no allocation, no syscalls, no long lock hold times.
    pub fn process_frames(&self, mic_input: &[f32], output: &mut [f32]) {
        let frames = mic_input.len().min(output.len());
        if frames == 0 {
            return;
        }
        let sample_rate = self.sample_rate.load(Ordering::Relaxed).max(1) as f32;

        // ---- microphone path: noise gate -> ducking -> volume -> mute
        let gate_threshold = f32_from_bits(self.noise_gate_threshold.load(Ordering::Relaxed));
        let gate_gain = if gate_threshold > 0.0 {
            self.update_gate(mic_input, gate_threshold, sample_rate)
        } else {
            self.gate_open.store(true, Ordering::Relaxed);
            let previous = self.load_gate_gain();
            let next = smooth(previous, 1.0, 0.010, sample_rate, frames);
            self.gate_gain.store(next.to_bits(), Ordering::Relaxed);
            next
        };

        // Ducking is driven by soundboard effects only: dipping the microphone for the
        // entire length of a song would make you inaudible the whole time.
        let duck_target = if self.ducking_enabled.load(Ordering::Relaxed) && self.is_playing() {
            1.0 - f32_from_bits(self.ducking_amount.load(Ordering::Relaxed))
        } else {
            1.0
        };
        let duck_env = smooth(
            self.load_duck_env(),
            duck_target,
            0.120,
            sample_rate,
            frames,
        );
        self.duck_env.store(duck_env.to_bits(), Ordering::Relaxed);
        self.ducking_active
            .store(duck_env < 0.999, Ordering::Relaxed);

        let mic_muted = self.mic_muted.load(Ordering::Relaxed);
        let mic_gain = if mic_muted {
            0.0
        } else {
            f32_from_bits(self.mic_volume.load(Ordering::Relaxed)) * gate_gain * duck_env
        };

        let fx_master = if self.fx_muted.load(Ordering::Relaxed) {
            0.0
        } else {
            f32_from_bits(self.fx_volume.load(Ordering::Relaxed))
        };
        let paused = self.paused.load(Ordering::Relaxed);

        let mut mic_peak = 0.0f32;
        let mut out_peak = 0.0f32;
        let mut clipped = 0u64;

        {
            let mut voices = match self.voices.lock() {
                Ok(voices) => voices,
                Err(poisoned) => poisoned.into_inner(),
            };

            for i in 0..frames {
                let mic_sample = mic_input[i];
                let abs = mic_sample.abs();
                if abs > mic_peak {
                    mic_peak = abs;
                }
                output[i] = mic_sample * mic_gain;
            }

            // Music first: it is the bed everything else sits on top of.
            if !self.music_paused.load(Ordering::Relaxed) {
                let music_gain = f32_from_bits(self.music_volume.load(Ordering::Relaxed));
                let mut guard = match self.music.lock() {
                    Ok(guard) => guard,
                    Err(poisoned) => poisoned.into_inner(),
                };
                if let Some(mut voice) = guard.take() {
                    let gain = voice.volume * music_gain;
                    let channels = self.channels.load(Ordering::Relaxed) as usize;
                    if render_voice(&mut voice, &mut output[..frames], gain, channels) {
                        *guard = Some(voice);
                    } else {
                        remember_finished(&self.finished, voice.voice_id);
                    }
                }
            }

            if !paused && fx_master > 0.0 {
                let channels = self.channels.load(Ordering::Relaxed) as usize;
                // Compaction happens in place: no Vec churn inside the render loop.
                let mut write = 0usize;
                for read in 0..voices.len() {
                    // Moving (not cloning) the voice keeps this loop allocation free.
                    let mut voice = std::mem::replace(&mut voices[read], empty_voice().clone());
                    let gain = voice.volume * fx_master;
                    if render_voice(&mut voice, &mut output[..frames], gain, channels) {
                        voices[write] = voice;
                        write += 1;
                    } else {
                        remember_finished(&self.finished, voice.voice_id);
                    }
                }
                voices.truncate(write);
            }
        }

        // ---- soft clip: kinder to the ear than a hard clamp.
        for sample in output.iter_mut().take(frames) {
            let abs = sample.abs();
            if abs > out_peak {
                out_peak = abs;
            }
            if abs > SOFT_KNEE {
                *sample = soft_clip(*sample);
                clipped += 1;
            }
        }
        if clipped > 0 {
            self.clipped_samples.fetch_add(clipped, Ordering::Relaxed);
        }

        // ---- master DSP chain (EQ / compressor / voice changer)
        let channels = self.channels.load(Ordering::Relaxed) as usize;
        if let Ok(mut chain) = self.dsp_chain.lock() {
            chain.process(&mut output[..frames], channels);
        }

        // ---- meters (peak hold with ~350 ms decay)
        self.mic_peak.store(
            decay_peak(
                f32_from_bits(self.mic_peak.load(Ordering::Relaxed)),
                mic_peak,
                sample_rate,
                frames,
            )
            .to_bits(),
            Ordering::Relaxed,
        );
        self.output_peak.store(
            decay_peak(
                f32_from_bits(self.output_peak.load(Ordering::Relaxed)),
                out_peak,
                sample_rate,
                frames,
            )
            .to_bits(),
            Ordering::Relaxed,
        );
    }

    /// Noise gate with hysteresis; returns the extra multiplier for the microphone.
    fn update_gate(&self, mic_input: &[f32], threshold: f32, sample_rate: f32) -> f32 {
        let mut peak = 0.0f32;
        for sample in mic_input {
            let abs = sample.abs();
            if abs > peak {
                peak = abs;
            }
        }
        // Fast attack (5 ms), slow release (200 ms) envelope follower.
        let env = self.load_gate_env();
        let tau = if peak > env { 0.005 } else { 0.200 };
        let env = smooth(env, peak, tau, sample_rate, mic_input.len());
        self.gate_env.store(env.to_bits(), Ordering::Relaxed);

        let open = self.gate_open.load(Ordering::Relaxed);
        if !open && env > threshold * 1.6 {
            self.gate_open.store(true, Ordering::Relaxed);
        } else if open && env < threshold * 0.6 {
            self.gate_open.store(false, Ordering::Relaxed);
        }

        let target = if self.gate_open.load(Ordering::Relaxed) {
            1.0
        } else {
            0.0
        };
        let mut gain = smooth(
            self.load_gate_gain(),
            target,
            0.010,
            sample_rate,
            mic_input.len(),
        );
        if gain < 0.001 {
            gain = 0.0;
        }
        self.gate_gain.store(gain.to_bits(), Ordering::Relaxed);
        gain
    }

    fn load_gate_env(&self) -> f32 {
        f32_from_bits(self.gate_env.load(Ordering::Relaxed))
    }

    fn load_gate_gain(&self) -> f32 {
        f32_from_bits(self.gate_gain.load(Ordering::Relaxed))
    }

    fn load_duck_env(&self) -> f32 {
        f32_from_bits(self.duck_env.load(Ordering::Relaxed))
    }
}

/// Turns the millisecond based `VoiceOptions` into concrete sample cursors.
///
/// Shared by `play_sound` and `play_music` so a sound effect and a music track are
/// trimmed, faded and loop-bounded in exactly the same way.
fn apply_voice_geometry(
    voice: &mut FxInstance,
    options: &VoiceOptions,
    channels: usize,
    sample_rate: f32,
) {
    let total = voice.sound_data.len();
    if total == 0 {
        voice.end_position = 0;
        return;
    }
    let channels = channels.max(1);
    let sample_rate = sample_rate.max(1.0);
    let frames = total / channels;

    let start_frame = ((options.start_offset_ms as f32 / 1000.0) * sample_rate) as usize;
    let start_frame = start_frame.min(frames.saturating_sub(1));
    let end_frame = match options.end_offset_ms {
        Some(end_ms) => {
            let end = ((end_ms as f32 / 1000.0) * sample_rate) as usize;
            end.clamp(start_frame + 1, frames)
        }
        None => frames,
    };

    let start = start_frame * channels;
    let mut end = (end_frame * channels).min(total).max(start + channels);

    if options.hold_to_play {
        let hold_limit = start + MAX_HOLD_SECONDS * sample_rate as usize * channels;
        end = end.min(hold_limit);
    }

    // Fade lengths are stored in interleaved samples so the ramp can be frame aligned.
    let fade_in = ((options.fade_in_ms.max(0.0) / 1000.0) * sample_rate) as usize * channels;
    let fade_out = ((options.fade_out_ms.max(0.0) / 1000.0) * sample_rate) as usize * channels;
    // Never fade longer than the sound itself.
    let span = end - start;
    let fade_in = fade_in.min(span / 2);
    let fade_out = fade_out.min(span / 2);

    voice.start_position = start;
    voice.position = start;
    voice.end_position = end;
    voice.fade_in_samples = fade_in;
    voice.fade_out_samples = fade_out;
}

/// Advances one voice into `output`, applying its fade-in / fade-out.
///
/// Returns `false` once the voice has finished and should be retired. Real-time safe:
/// no allocation, no locks.
fn render_voice(voice: &mut FxInstance, output: &mut [f32], gain: f32, channels: usize) -> bool {
    if gain <= 0.0 {
        // Muted: still advance so the track keeps its place in time.
        let end = voice.end_position.min(voice.sound_data.len());
        let step = output.len().min(end.saturating_sub(voice.position));
        voice.position += step;
        return voice.position < end || voice.loop_playback;
    }

    let data = &voice.sound_data;
    let end = voice.end_position.min(data.len());
    let mut pos = voice.position;
    let mut alive = true;

    for out_sample in output.iter_mut() {
        if pos >= end {
            if voice.loop_playback {
                pos = voice.start_position.min(end.saturating_sub(1));
            } else {
                alive = false;
                break;
            }
        }

        // The output buffer is interleaved, so a fade ramp must advance once per *frame*.
        // Advancing per sample would make it run `channels` times too fast and give the
        // left and right channel of the same frame different gains - audible as a stereo
        // imbalance plus a click. Everything is therefore measured from `frame_start`, the
        // first sample of the frame `pos` belongs to, so all channels of a frame share one
        // gain value.
        let channels = channels.max(1);
        let frame_start = (pos / channels) * channels;

        let played = frame_start.saturating_sub(voice.start_position);
        let mut sample_gain = gain;
        if voice.fade_in_samples > 0 && played < voice.fade_in_samples {
            sample_gain *= played as f32 / voice.fade_in_samples as f32;
        }
        let remaining = end.saturating_sub(frame_start);
        if voice.fade_out_samples > 0 && remaining <= voice.fade_out_samples {
            sample_gain *= remaining as f32 / voice.fade_out_samples as f32;
        }

        *out_sample += data[pos] * sample_gain;
        pos += 1;
    }

    voice.position = pos;
    if let Some(remaining) = voice.release_remaining.as_mut() {
        *remaining = remaining.saturating_sub(output.len());
        if *remaining == 0 {
            alive = false;
        }
    }
    if pos >= end && !voice.loop_playback {
        alive = false;
    }
    alive
}

/// Placeholder used to move voices out of the `Vec` without allocating.
static EMPTY_VOICE: std::sync::OnceLock<FxInstance> = std::sync::OnceLock::new();

fn empty_voice() -> &'static FxInstance {
    EMPTY_VOICE.get_or_init(|| FxInstance {
        voice_id: 0,
        sound_id: Arc::from(""),
        sound_data: Arc::new(Vec::new()),
        start_position: 0,
        position: 0,
        end_position: 0,
        volume: 0.0,
        loop_playback: false,
        hold_to_play: false,
        fade_in_samples: 0,
        fade_out_samples: 0,
        release_remaining: None,
    })
}

/// Level below which the signal is untouched.
const SOFT_KNEE: f32 = 0.85;

/// Soft knee limiter.
///
/// Linear up to [`SOFT_KNEE`], then `t / (1 + t)` towards unity. That rational curve is
/// chosen because its derivative at `t = 0` is exactly 1, so the slope is *continuous*
/// through the knee - a `tanh` knee instead drops from slope 1 to `1 - SOFT_KNEE` in one
/// step, which injects its own harmonics. It also never quite reaches 1.0, so a badly
/// overloaded mix keeps a rounded crest instead of the flat top a hard clamp produces.
fn soft_clip(sample: f32) -> f32 {
    let magnitude = sample.abs();
    if magnitude <= SOFT_KNEE {
        return sample;
    }
    let headroom = 1.0 - SOFT_KNEE;
    let over = (magnitude - SOFT_KNEE) / headroom;
    let shaped = SOFT_KNEE + headroom * (over / (1.0 + over));
    shaped.copysign(sample)
}

fn smooth(
    current: f32,
    target: f32,
    time_constant_sec: f32,
    sample_rate: f32,
    frames: usize,
) -> f32 {
    if frames == 0 {
        return current;
    }
    let alpha = 1.0 - (-(frames as f32) / (time_constant_sec.max(1e-4) * sample_rate)).exp();
    current + (target - current) * alpha.clamp(0.0, 1.0)
}

fn decay_peak(previous: f32, peak: f32, sample_rate: f32, frames: usize) -> f32 {
    let decayed = previous * (-(frames as f32) / (0.35 * sample_rate)).exp();
    decayed.max(peak).min(1.0)
}

fn remember_finished(finished: &Mutex<VecDeque<u64>>, voice_id: u64) {
    if let Ok(mut queue) = finished.lock() {
        if queue.len() >= VOICE_HISTORY {
            queue.pop_front();
        }
        queue.push_back(voice_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mono(samples: &[f32]) -> Arc<Vec<f32>> {
        Arc::new(samples.to_vec())
    }

    fn silence(len: usize) -> Vec<f32> {
        vec![0.0; len]
    }

    #[test]
    fn mixes_multiple_voices_simultaneously() {
        let mixer = Mixer::new(48_000, 2);
        let options = VoiceOptions {
            fade_in_ms: 0.0,
            fade_out_ms: 0.0,
            ..Default::default()
        };
        let a = mixer.play_sound("a", mono(&[0.5; 4]), options);
        let b = mixer.play_sound("b", mono(&[0.25; 4]), options);
        assert_ne!(a, 0);
        assert_ne!(b, 0);

        let mut out = silence(4);
        mixer.process_frames(&[0.0; 4], &mut out);
        for sample in &out {
            assert!((sample - 0.75).abs() < 1e-6, "got {sample}");
        }
        assert_eq!(mixer.active_voice_count(), 0);
    }

    #[test]
    fn overlapping_the_same_sound_restarts_it() {
        let mixer = Mixer::new(48_000, 1);
        mixer.play_sound("a", mono(&[1.0; 8]), VoiceOptions::default());
        mixer.process_frames(&[0.0; 4], &mut silence(4));
        assert_eq!(mixer.get_playback_state(), Some((4, 8)));

        mixer.play_sound("a", mono(&[1.0; 8]), VoiceOptions::default());
        assert_eq!(mixer.get_playback_state(), Some((0, 8)));
        assert_eq!(mixer.active_voice_count(), 1);
    }

    #[test]
    fn retrigger_disabled_ignores_the_second_press() {
        let mixer = Mixer::new(48_000, 1);
        let options = VoiceOptions {
            retrigger: false,
            ..Default::default()
        };
        assert_ne!(mixer.play_sound("a", mono(&[1.0; 8]), options), 0);
        assert_eq!(mixer.play_sound("a", mono(&[1.0; 8]), options), 0);
        assert_eq!(mixer.active_voice_count(), 1);
        assert_eq!(mixer.stats().rejected_voices, 1);
    }

    #[test]
    fn looped_sound_never_retires() {
        let mixer = Mixer::new(48_000, 1);
        mixer.play_sound(
            "loop",
            mono(&[0.2; 16]),
            VoiceOptions {
                loop_playback: true,
                fade_in_ms: 0.0,
                fade_out_ms: 0.0,
                ..Default::default()
            },
        );
        for _ in 0..20 {
            mixer.process_frames(&[0.0; 4], &mut silence(4));
        }
        assert_eq!(mixer.active_voice_count(), 1);
        mixer.stop_all();
        assert_eq!(mixer.active_voice_count(), 0);
    }

    #[test]
    fn per_sound_volume_is_applied() {
        let mixer = Mixer::new(48_000, 1);
        mixer.play_sound(
            "quiet",
            mono(&[1.0; 4]),
            VoiceOptions {
                volume: 0.25,
                fade_in_ms: 0.0,
                fade_out_ms: 0.0,
                ..Default::default()
            },
        );
        let mut out = silence(4);
        mixer.process_frames(&[0.0; 4], &mut out);
        assert!((out[0] - 0.25).abs() < 1e-6);
    }

    #[test]
    fn hold_to_play_release_fades_out_and_stops() {
        let mixer = Mixer::new(48_000, 1);
        mixer.play_sound(
            "held",
            mono(&[1.0; 48_000]),
            VoiceOptions {
                hold_to_play: true,
                fade_in_ms: 0.0,
                fade_out_ms: 5.0,
                ..Default::default()
            },
        );
        mixer.process_frames(&[0.0; 100], &mut silence(100));
        assert_eq!(mixer.active_voice_count(), 1);
        mixer.release_held("held");
        for _ in 0..10 {
            mixer.process_frames(&[0.0; 100], &mut silence(100));
        }
        assert_eq!(mixer.active_voice_count(), 0);
    }

    #[test]
    fn mic_and_fx_can_be_muted_independently() {
        let mixer = Mixer::new(48_000, 1);
        mixer.set_mic_muted(true);
        mixer.play_sound(
            "a",
            mono(&[0.5; 4]),
            VoiceOptions {
                fade_in_ms: 0.0,
                fade_out_ms: 0.0,
                ..Default::default()
            },
        );
        let mut out = silence(4);
        mixer.process_frames(&[0.9; 4], &mut out);
        assert!(out[0] > 0.0, "fx must survive a muted microphone");

        mixer.set_fx_muted(true);
        let mut out = silence(4);
        mixer.process_frames(&[0.9; 4], &mut out);
        assert_eq!(out[0], 0.0);
    }

    #[test]
    fn ducking_lowers_the_microphone_while_a_sound_plays() {
        let mixer = Mixer::new(48_000, 1);
        mixer.set_ducking(true, 0.5);
        let mut out = silence(480);
        mixer.process_frames(&[1.0; 480], &mut out);
        let before = out[479];

        // A silent voice: ducking must react to *any* playing sound, not to its level.
        mixer.play_sound("a", mono(&vec![0.0; 48_000]), VoiceOptions::default());
        let mut during = 0.0;
        for step in 0..100 {
            mixer.process_frames(&[1.0; 480], &mut out);
            if step == 50 {
                during = out[479];
            }
        }
        assert!(during < before * 0.75, "before={before} during={during}");
    }

    #[test]
    fn ducking_ignores_music_on_purpose() {
        // Dipping the mic for the whole length of a song would make you inaudible.
        let mixer = Mixer::new(48_000, 1);
        mixer.set_ducking(true, 0.5);
        let mut out = silence(480);
        mixer.process_frames(&[1.0; 480], &mut out);
        let before = out[479];

        mixer.play_music("track", mono(&vec![0.5; 48_000]), VoiceOptions::default());
        for _ in 0..100 {
            mixer.process_frames(&[1.0; 480], &mut out);
        }
        assert!(
            (out[479] - before).abs() < 0.01,
            "music must not duck the mic: {} vs {}",
            out[479],
            before
        );
    }

    #[test]
    fn noise_gate_silences_quiet_input_only() {
        let mixer = Mixer::new(48_000, 1);
        mixer.set_noise_gate_threshold(0.05);
        let mut out = silence(4800);
        mixer.process_frames(&[0.001; 4800], &mut out);
        assert!(
            out[4799].abs() < 1e-4,
            "quiet input must be gated: {}",
            out[4799]
        );

        mixer.process_frames(&[0.5; 4800], &mut out);
        assert!(out[4799] > 0.4, "loud input must pass: {}", out[4799]);
    }

    #[test]
    fn max_concurrent_evicts_the_oldest_voice() {
        let mixer = Mixer::new(48_000, 1);
        mixer.set_max_concurrent(2);
        mixer.play_sound("a", mono(&[0.1; 100]), VoiceOptions::default());
        mixer.play_sound("b", mono(&[0.1; 100]), VoiceOptions::default());
        mixer.play_sound("c", mono(&[0.1; 100]), VoiceOptions::default());
        let ids: Vec<String> = mixer
            .active_voices()
            .into_iter()
            .map(|voice| voice.sound_id)
            .collect();
        assert_eq!(ids, vec!["b".to_string(), "c".to_string()]);
    }

    #[test]
    fn start_offset_skips_the_beginning_of_the_file() {
        let mixer = Mixer::new(48_000, 1);
        // 1000 frames at 48 kHz ~= 20.8 ms, so a 10 ms offset skips ~480 frames.
        mixer.play_sound(
            "trim",
            mono(&vec![1.0; 1000]),
            VoiceOptions {
                start_offset_ms: 10,
                fade_in_ms: 0.0,
                fade_out_ms: 0.0,
                ..Default::default()
            },
        );
        assert!(mixer.get_playback_state().unwrap().0 >= 480);
    }

    #[test]
    fn fades_avoid_clicks_on_start_and_end() {
        let mixer = Mixer::new(48_000, 1);
        mixer.set_fade_ms(10.0);
        mixer.play_sound("click", mono(&vec![1.0; 480]), VoiceOptions::default());
        let mut out = silence(480);
        mixer.process_frames(&[0.0; 480], &mut out);
        assert!(
            out[0].abs() < 0.05,
            "fade-in must start near silence: {}",
            out[0]
        );
        assert!(
            out[479].abs() < 0.05,
            "fade-out must end near silence: {}",
            out[479]
        );
        assert!(out[240].abs() > 0.5, "middle must be at full level");
    }

    #[test]
    fn stereo_fades_do_not_unbalance_the_channels() {
        // The output buffer is interleaved, so a fade must advance once per *frame*.
        // A per-sample ramp gave L and R of the same frame different gains.
        let mixer = Mixer::new(48_000, 2);
        // 480 frames of stereo at full scale.
        let data: Vec<f32> = vec![1.0f32; 960];
        mixer.play_sound("stereo", mono(&data), VoiceOptions::default());
        let mut out = silence(960);
        mixer.process_frames(&[0.0; 960], &mut out);

        let left: Vec<f32> = out.iter().step_by(2).copied().collect();
        let right: Vec<f32> = out.iter().skip(1).step_by(2).copied().collect();
        let max_diff = left
            .iter()
            .zip(right.iter())
            .map(|(l, r)| (l - r).abs())
            .fold(0.0f32, f32::max);
        assert!(
            max_diff < 1e-6,
            "channels diverged by {max_diff}; the fade ramp is per-sample, not per-frame"
        );
    }

    #[test]
    fn soft_clip_stays_in_range() {
        for value in [-10.0f32, -1.5, -1.0, 0.0, 1.0, 1.5, 10.0] {
            let clipped = soft_clip(value);
            assert!(clipped.abs() <= 1.0 + 1e-6, "{value} -> {clipped}");
            assert!(clipped.is_finite());
            assert_eq!(clipped.signum(), value.signum());
        }
    }

    #[test]
    fn soft_clip_is_continuous_and_monotonic() {
        let mut previous = f32::NEG_INFINITY;
        let mut x = 0.0f32;
        while x <= 2.0 {
            let y = soft_clip(x);
            assert!(y >= previous, "not monotonic at {x}: {previous} -> {y}");
            assert!((y - x).abs() < 0.2 || x > SOFT_KNEE, "kink at {x} -> {y}");
            previous = y;
            x += 0.001;
        }
        // Below the knee the curve is the identity.
        assert_eq!(soft_clip(SOFT_KNEE), SOFT_KNEE);
        // Above it, it approaches but never exceeds unity.
        assert!(soft_clip(100.0) <= 1.0);
    }

    #[test]
    fn soft_clip_slope_is_continuous_through_the_knee() {
        // A slope step at the knee is itself an audible distortion source.
        let eps = 1e-4f32;
        let below = (soft_clip(SOFT_KNEE) - soft_clip(SOFT_KNEE - eps)) / eps;
        let above = (soft_clip(SOFT_KNEE + eps) - soft_clip(SOFT_KNEE)) / eps;
        assert!(
            (below - above).abs() < 0.02,
            "slope jumps from {below} to {above} at the knee"
        );
    }

    #[test]
    fn loud_mix_is_limited_instead_of_hard_clipped() {
        let mixer = Mixer::new(48_000, 1);
        mixer.play_sound(
            "loud",
            mono(&vec![4.0; 64]),
            VoiceOptions {
                fade_in_ms: 0.0,
                fade_out_ms: 0.0,
                ..Default::default()
            },
        );
        let mut out = silence(64);
        mixer.process_frames(&[4.0; 64], &mut out);
        assert!(out.iter().all(|s| s.abs() <= 1.0));
        // The soft knee trims a hair off full scale instead of hard-clipping it.
        assert!(out[32] > 0.9, "mid level should stay hot: {}", out[32]);
    }

    #[test]
    fn finished_voices_are_reported_once() {
        let mixer = Mixer::new(48_000, 1);
        let id = mixer.play_sound("a", mono(&[1.0; 2]), VoiceOptions::default());
        mixer.process_frames(&[0.0; 4], &mut silence(4));
        assert_eq!(mixer.drain_finished(), vec![id]);
        assert!(mixer.drain_finished().is_empty());
    }

    #[test]
    fn meters_track_levels() {
        let mixer = Mixer::new(48_000, 1);
        mixer.process_frames(&[0.5; 100], &mut silence(100));
        let stats = mixer.stats();
        assert!((stats.mic_peak - 0.5).abs() < 0.01, "{stats:?}");
        assert!(stats.output_peak > 0.4);
    }

    #[test]
    fn underruns_and_drops_are_counted() {
        let mixer = Mixer::new(48_000, 1);
        mixer.note_underrun(3);
        mixer.note_dropped(7);
        let stats = mixer.stats();
        assert_eq!(stats.underruns, 3);
        assert_eq!(stats.dropped_samples, 7);
    }

    #[test]
    fn seek_moves_the_cursor() {
        let mixer = Mixer::new(48_000, 1);
        mixer.play_sound("a", mono(&vec![0.1; 1000]), VoiceOptions::default());
        mixer.seek(0.5);
        let (pos, len) = mixer.get_playback_state().unwrap();
        assert_eq!(len, 1000);
        assert!((480..=520).contains(&pos), "pos={pos}");
    }

    #[test]
    fn channels_are_forwarded_to_the_dsp_chain() {
        let mixer = Mixer::new(48_000, 6);
        assert_eq!(mixer.channels(), 6);
        mixer.set_channels(1);
        assert_eq!(mixer.channels(), 1);
    }

    #[test]
    fn empty_sound_is_rejected() {
        let mixer = Mixer::new(48_000, 1);
        assert_eq!(mixer.play_sound("a", mono(&[]), VoiceOptions::default()), 0);
    }

    #[test]
    fn stereo_positions_are_in_interleaved_samples() {
        let mixer = Mixer::new(48_000, 2);
        // 10 frames of stereo == 20 interleaved samples.
        mixer.play_sound("a", mono(&[1.0; 20]), VoiceOptions::default());
        mixer.process_frames(&[0.0; 4], &mut silence(4));
        // 4 interleaved samples == 2 frames consumed.
        assert_eq!(mixer.get_playback_state(), Some((4, 20)));
    }

    // ---------------------------------------------------------------- music

    #[test]
    fn music_plays_alongside_sound_effects() {
        let mixer = Mixer::new(48_000, 1);
        let music = mixer.play_music(
            "track",
            mono(&vec![0.2; 100]),
            VoiceOptions {
                fade_in_ms: 0.0,
                fade_out_ms: 0.0,
                ..Default::default()
            },
        );
        assert_ne!(music, 0);
        mixer.play_sound(
            "sfx",
            mono(&[0.3; 4]),
            VoiceOptions {
                fade_in_ms: 0.0,
                fade_out_ms: 0.0,
                ..Default::default()
            },
        );

        let mut out = silence(4);
        mixer.process_frames(&[0.0; 4], &mut out);
        for sample in &out {
            assert!((sample - 0.5).abs() < 1e-6, "got {sample}");
        }
        assert!(mixer.is_music_playing());
        assert_eq!(mixer.music_progress(), Some((4, 100)));
    }

    #[test]
    fn pausing_music_leaves_sound_effects_running() {
        let mixer = Mixer::new(48_000, 1);
        mixer.play_music(
            "track",
            mono(&vec![0.2; 100]),
            VoiceOptions {
                fade_in_ms: 0.0,
                fade_out_ms: 0.0,
                ..Default::default()
            },
        );
        mixer.set_music_paused(true);
        let mut out = silence(10);
        mixer.process_frames(&[0.0; 10], &mut out);
        assert!(out.iter().all(|s| *s == 0.0), "music must be silent");
        assert_eq!(
            mixer.music_progress(),
            Some((0, 100)),
            "a paused track must not advance"
        );

        mixer.set_music_paused(false);
        mixer.process_frames(&[0.0; 10], &mut out);
        assert!((out[0] - 0.2).abs() < 1e-6);
        assert_eq!(mixer.music_progress(), Some((10, 100)));
    }

    #[test]
    fn music_volume_is_independent_of_the_fx_slider() {
        let mixer = Mixer::new(48_000, 1);
        mixer.set_fx_volume(0.0);
        mixer.set_music_volume(0.5);
        mixer.play_music(
            "track",
            mono(&[1.0; 8]),
            VoiceOptions {
                fade_in_ms: 0.0,
                fade_out_ms: 0.0,
                ..Default::default()
            },
        );
        let mut out = silence(4);
        mixer.process_frames(&[0.0; 4], &mut out);
        assert!((out[0] - 0.5).abs() < 1e-6, "got {}", out[0]);
    }

    #[test]
    fn stopping_music_keeps_sound_effects_alive() {
        let mixer = Mixer::new(48_000, 1);
        mixer.play_music(
            "track",
            mono(&vec![0.2; 100]),
            VoiceOptions {
                fade_in_ms: 0.0,
                fade_out_ms: 0.0,
                ..Default::default()
            },
        );
        mixer.play_sound(
            "sfx",
            mono(&[0.3; 4]),
            VoiceOptions {
                fade_in_ms: 0.0,
                fade_out_ms: 0.0,
                ..Default::default()
            },
        );
        mixer.stop_music();
        assert!(!mixer.is_music_playing());
        assert_eq!(mixer.music_progress(), None);
        assert_eq!(mixer.active_voice_count(), 1);
    }

    #[test]
    fn looping_music_never_ends() {
        let mixer = Mixer::new(48_000, 1);
        mixer.play_music(
            "track",
            mono(&[0.2; 16]),
            VoiceOptions {
                loop_playback: true,
                fade_in_ms: 0.0,
                fade_out_ms: 0.0,
                ..Default::default()
            },
        );
        for _ in 0..50 {
            mixer.process_frames(&[0.0; 4], &mut silence(4));
        }
        assert!(mixer.is_music_playing());
        assert_eq!(mixer.music_sound_id(), Some("track".to_string()));
    }

    #[test]
    fn stats_report_the_music_channel() {
        let mixer = Mixer::new(48_000, 1);
        mixer.play_music("track", mono(&vec![0.2; 100]), VoiceOptions::default());
        let stats = mixer.stats();
        assert!(stats.music_playing);
        assert!(!stats.music_paused);
        mixer.set_music_paused(true);
        assert!(mixer.stats().music_paused);
    }

    #[test]
    fn music_trim_uses_the_same_rules_as_sounds() {
        let mixer = Mixer::new(48_000, 1);
        mixer.play_music(
            "track",
            mono(&vec![1.0; 48_000]),
            VoiceOptions {
                start_offset_ms: 1000,
                end_offset_ms: Some(2000),
                fade_in_ms: 0.0,
                fade_out_ms: 0.0,
                ..Default::default()
            },
        );
        let (_, length) = mixer.music_progress().unwrap();
        // 1 second of audio at 48 kHz.
        assert_eq!(length, 48_000);
    }

    #[test]
    fn hold_to_play_is_capped_so_a_missed_key_release_cannot_loop_forever() {
        let mixer = Mixer::new(48_000, 1);
        let data = mono(&vec![0.5; 48_000 * 300]);
        mixer.play_sound(
            "held",
            data,
            VoiceOptions {
                hold_to_play: true,
                loop_playback: true,
                fade_in_ms: 0.0,
                fade_out_ms: 0.0,
                ..Default::default()
            },
        );
        let (_, length) = mixer.get_playback_state().unwrap();
        assert_eq!(length, 48_000 * MAX_HOLD_SECONDS);
    }
}
