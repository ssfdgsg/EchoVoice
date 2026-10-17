export interface SoundItem {
  id: string;
  name: string;
  path: string;
  shortcut: string | null;
  volume: number;
  loop_playback: boolean;
  favorite: boolean;
  category: string;
  start_offset_ms: number;
  end_offset_ms: number | null;
  retrigger: boolean;
  hold_to_play: boolean;
  duration_ms: number | null;
  is_music: boolean;
}

export interface EqSettings {
  enabled: boolean;
  low_db: number;
  mid_db: number;
  high_db: number;
}

export interface CompressorSettings {
  enabled: boolean;
  threshold_db: number;
  ratio: number;
  attack_ms: number;
  release_ms: number;
  makeup_db: number;
}

export interface PitchSettings {
  enabled: boolean;
  ratio: number;
}

export interface AudioSettings {
  mic_volume: number;
  fx_volume: number;
  music_volume: number;
  noise_gate_threshold: number;
  mic_muted: boolean;
  fx_muted: boolean;
  ducking_enabled: boolean;
  ducking_amount: number;
  max_concurrent_sounds: number;
  fade_ms: number;
  eq: EqSettings;
  compressor: CompressorSettings;
  pitch: PitchSettings;
}

export interface AppConfig {
  sounds: SoundItem[];
  default_input_id: string | null;
  default_output_id: string | null;
  global_stop_shortcut: string | null;
  mic_mute_shortcut: string | null;
  fx_mute_shortcut: string | null;
  music_toggle_shortcut: string | null;
  music_next_shortcut: string | null;
  music_prev_shortcut: string | null;
  music_stop_shortcut: string | null;
  language: string | null;
  theme: string;
  auto_start_bridge: boolean;
  minimize_to_tray: boolean;
  launch_at_startup: boolean;
  audio: AudioSettings;
  monitor_enabled: boolean;
  monitor_device_id: string | null;
  music_loop_playlist: boolean;
  music_index: number | null;
}

export interface VoiceInfo {
  voice_id: number;
  sound_id: string;
  position: number;
  length: number;
  loop_playback: boolean;
  hold_to_play: boolean;
}

export interface EngineStats {
  running: boolean;
  sample_rate: number;
  channels: number;
  buffer_frames: number;
  active_voices: number;
  voices_started: number;
  rejected_voices: number;
  underruns: number;
  dropped_samples: number;
  mic_level: number;
  output_level: number;
  clipped_samples: number;
  gate_open: boolean;
  ducking: boolean;
  paused: boolean;
  recording: boolean;
  recording_ms: number;
  latency_ms: number;
  /** Non-zero while the supervisor retries a device that disappeared. */
  reconnecting: boolean;
  reconnect_attempts: number;
  music_playing: boolean;
  music_paused: boolean;
  music_track: string | null;
  music_position_ms: number;
  music_length_ms: number;
  last_error: string | null;
  voices?: VoiceInfo[];
}

export interface BoundHotkey {
  accelerator: string;
  target: string;
  hold_to_play: boolean;
}

export interface HotkeyDiagnostics {
  listener_active: boolean;
  keystrokes_seen: number;
  elevated: boolean;
  channel: string;
  bound_hotkeys: BoundHotkey[];
  sound_count: number;
}

export interface RoutingDiagnostics {
  bridge_running: boolean;
  output_device_name: string;
  output_is_cable: boolean;
  cable_output_present: boolean;
  cable_output_name: string | null;
  default_capture_name: string | null;
  default_comms_capture_name: string | null;
  cable_is_comms_default: boolean;
  mic_device_present: boolean;
  mic_device_name: string | null;
  problems: string[];
}

export interface AudioDevice {
  id: string;
  name: string;
  is_default: boolean;
}

/** `mm:ss` (or `h:mm:ss` for long clips) used all over the UI. */
export function formatTime(ms: number): string {
  if (!Number.isFinite(ms) || ms < 0) return "--:--";
  const totalSeconds = Math.floor(ms / 1000);
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;
  const pad = (value: number) => value.toString().padStart(2, "0");
  return hours > 0
    ? `${hours}:${pad(minutes)}:${pad(seconds)}`
    : `${minutes}:${pad(seconds)}`;
}

/** Linear 0..1 amplitude to a pleasant dB-ish label for meters. */
export function levelPercent(level: number): number {
  if (!Number.isFinite(level) || level <= 0) return 0;
  const db = 20 * Math.log10(level);
  // -54 dB .. 0 dB mapped onto 0..100%
  const percent = ((db + 54) / 54) * 100;
  return Math.max(0, Math.min(100, percent));
}

/** Renders an accelerator the way the OS would show it. */
export function prettyShortcut(shortcut: string | null): string {
  if (!shortcut) return "";
  return shortcut
    .split("+")
    .map((part) => {
      switch (part) {
        case "Control":
          return "Ctrl";
        case "Super":
          return "Win";
        default:
          return part.length === 1 ? part.toUpperCase() : part;
      }
    })
    .join(" + ");
}

export interface MusicState {
  playing: boolean;
  paused: boolean;
  track_id: string | null;
  track_name: string | null;
  position_ms: number;
  length_ms: number;
  index: number | null;
  playlist_len: number;
}
