<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import { open } from "@tauri-apps/plugin-dialog";
  import { dict } from "$lib/i18n";
  import SoundTile from "$lib/components/SoundTile.svelte";
  import SoundEditor from "$lib/components/SoundEditor.svelte";
  import LevelMeter from "$lib/components/LevelMeter.svelte";
  import {
    formatTime,
    prettyShortcut,
    type AppConfig,
    type AudioDevice,
    type EngineStats,
    type HotkeyDiagnostics,
    type MusicState,
    type RoutingDiagnostics,
    type SoundItem,
    type VoiceInfo,
  } from "$lib/types";

  type Tab = "soundboard" | "routing" | "music" | "settings" | "guide";

  // ------------------------------------------------------------------ state

  let lang = $state<"en" | "zh">("zh");
  let t = $derived(dict[lang]);
  let theme = $state<"dark" | "light">("dark");
  let activeTab = $state<Tab>("soundboard");

  let sounds = $state<SoundItem[]>([]);
  let categories = $state<string[]>([]);
  let searchQuery = $state("");
  let categoryFilter = $state("");
  let favoritesOnly = $state(false);
  let editingId = $state<string | null>(null);

  let inputDevices = $state<AudioDevice[]>([]);
  let outputDevices = $state<AudioDevice[]>([]);
  let selectedInput = $state("");
  let selectedOutput = $state("");

  let isBridgeRunning = $state(false);
  let isBridgeStarting = $state(false);
  let errorMsg = $state("");
  let noticeMsg = $state("");

  // Audio settings mirror the persisted config so the UI stays authoritative while a
  // slider is dragged; every change is pushed to the backend (coalesced per frame).
  let audio = $state<AppConfig["audio"]>({
    mic_volume: 1,
    fx_volume: 1,
    music_volume: 1,
    noise_gate_threshold: 0,
    mic_muted: false,
    fx_muted: false,
    ducking_enabled: false,
    ducking_amount: 0.55,
    max_concurrent_sounds: 8,
    fade_ms: 6,
    eq: { enabled: false, low_db: 0, mid_db: 0, high_db: 0 },
    compressor: {
      enabled: false,
      threshold_db: -18,
      ratio: 3,
      attack_ms: 8,
      release_ms: 120,
      makeup_db: 0,
    },
    pitch: { enabled: false, ratio: 1 },
  });

  let globalStopShortcut = $state<string | null>(null);
  let micMuteShortcut = $state<string | null>(null);
  let fxMuteShortcut = $state<string | null>(null);
  let autoStartBridge = $state(false);
  let minimizeToTray = $state(true);
  let launchAtStartup = $state<boolean | null>(null);

  let stats = $state<EngineStats>({
    running: false,
    sample_rate: 48000,
    channels: 2,
    buffer_frames: 0,
    active_voices: 0,
    voices_started: 0,
    rejected_voices: 0,
    underruns: 0,
    dropped_samples: 0,
    mic_level: 0,
    output_level: 0,
    clipped_samples: 0,
    gate_open: true,
    ducking: false,
    paused: false,
    recording: false,
    recording_ms: 0,
    latency_ms: 0,
    reconnecting: false,
    reconnect_attempts: 0,
    music_playing: false,
    music_paused: false,
    music_track: null,
    music_position_ms: 0,
    music_length_ms: 0,
    last_error: null,
    voices: [],
  });

  let isDragging = $state(false);
  let hotkeyDiag = $state<HotkeyDiagnostics | null>(null);
  let lastHotkeyFlash = $state<string | null>(null);

  // Music player
  let musicPlaylist = $state<SoundItem[]>([]);
  let musicState = $state<MusicState>({
    playing: false,
    paused: false,
    track_id: null,
    track_name: null,
    position_ms: 0,
    length_ms: 0,
    index: null,
    playlist_len: 0,
  });
  let musicLoop = $state(true);
  let musicToggleKey = $state<string | null>(null);
  let musicNextKey = $state<string | null>(null);
  let musicPrevKey = $state<string | null>(null);
  let musicStopKey = $state<string | null>(null);
  let monitorEnabled = $state(false);
  let monitorDevice = $state<string>("");
  let auditionRunning = $state(false);
  let auditionLevel = $state(0);
  let routing = $state<RoutingDiagnostics | null>(null);

  let unlisteners: UnlistenFn[] = [];
  let noticeTimer: number | null = null;
  let hotkeyTimer: number | null = null;
  let flashTimer: number | null = null;
  let routingTimer: number | null = null;
  let pushTimer: number | null = null;

  // ----------------------------------------------------------------- derived

  let filteredSounds = $derived(
    sounds.filter((sound) => {
      if (favoritesOnly && !sound.favorite) return false;
      if (categoryFilter && sound.category !== categoryFilter) return false;
      if (!searchQuery.trim()) return true;
      return sound.name.toLowerCase().includes(searchQuery.trim().toLowerCase());
    }),
  );

  let editingSound = $derived(sounds.find((sound) => sound.id === editingId) ?? null);

  /** sound_id -> progress (0..1) of its most advanced voice. */
  let progressBySound = $derived.by(() => {
    const map = new Map<string, number>();
    for (const voice of stats.voices ?? []) {
      if (voice.length <= 0) continue;
      const ratio = Math.min(1, voice.position / voice.length);
      map.set(voice.sound_id, Math.max(map.get(voice.sound_id) ?? 0, ratio));
    }
    return map;
  });

  let playingIds = $derived(new Set((stats.voices ?? []).map((voice) => voice.sound_id)));
  let primaryVoice = $derived<VoiceInfo | null>((stats.voices ?? []).at(-1) ?? null);
  let primaryName = $derived(
    primaryVoice
      ? (sounds.find((sound) => sound.id === primaryVoice!.sound_id)?.name ??
        primaryVoice.sound_id)
      : "",
  );
  let extraVoices = $derived(Math.max(0, (stats.voices ?? []).length - 1));

  // ------------------------------------------------------------------ helpers

  function flash(message: string) {
    noticeMsg = message;
    if (noticeTimer) window.clearTimeout(noticeTimer);
    noticeTimer = window.setTimeout(() => (noticeMsg = ""), 4000);
  }

  function shortcutFromKeyboardEvent(
    event: KeyboardEvent,
  ): string | null | undefined {
    event.preventDefault();

    if (["Escape", "Backspace", "Delete"].includes(event.code)) {
      return null;
    }
    const MODIFIERS = [
      "ControlLeft",
      "ControlRight",
      "AltLeft",
      "AltRight",
      "ShiftLeft",
      "ShiftRight",
      "MetaLeft",
      "MetaRight",
    ];
    if (MODIFIERS.includes(event.code)) {
      return undefined;
    }

    const keys: string[] = [];
    if (event.ctrlKey) keys.push("Control");
    if (event.altKey) keys.push("Alt");
    if (event.shiftKey) keys.push("Shift");
    if (event.metaKey) keys.push("Super");
    keys.push(event.code || event.key);
    return keys.join("+");
  }

  async function refreshConfig() {
    try {
      const config: AppConfig = await invoke("get_app_config");
      sounds = config.sounds ?? [];
      globalStopShortcut = config.global_stop_shortcut;
      micMuteShortcut = config.mic_mute_shortcut;
      fxMuteShortcut = config.fx_mute_shortcut;
      musicToggleKey = config.music_toggle_shortcut;
      musicNextKey = config.music_next_shortcut;
      musicPrevKey = config.music_prev_shortcut;
      musicStopKey = config.music_stop_shortcut;
      musicLoop = config.music_loop_playlist;
      monitorEnabled = config.monitor_enabled;
      monitorDevice = config.monitor_device_id ?? "";
      autoStartBridge = config.auto_start_bridge;
      minimizeToTray = config.minimize_to_tray;
      theme = config.theme === "light" ? "light" : "dark";
      audio = config.audio;
      if (config.language === "en" || config.language === "zh") {
        lang = config.language;
      }
      if (config.default_input_id && !selectedInput) {
        selectedInput = config.default_input_id;
      }
      if (config.default_output_id && !selectedOutput) {
        selectedOutput = config.default_output_id;
      }
      categories = await invoke<string[]>("get_categories");
    } catch (err) {
      console.error("Failed to load config", err);
      errorMsg = String(err);
    }
  }

  async function loadDevices() {
    try {
      errorMsg = "";
      inputDevices = await invoke<AudioDevice[]>("get_input_devices");
      outputDevices = await invoke<AudioDevice[]>("get_output_devices");

      if (inputDevices.length > 0 && !selectedInput) {
        selectedInput = inputDevices[0].id;
      }
      if (outputDevices.length > 0 && !selectedOutput) {
        selectedOutput = outputDevices[0].id;
      }
    } catch (err) {
      console.error("Failed to load devices", err);
      errorMsg = String(err);
    }
  }

  async function saveDevices() {
    try {
      await invoke("set_default_devices", {
        inputId: selectedInput,
        outputId: selectedOutput,
      });
    } catch (err) {
      console.error(err);
    }
  }

  async function loadMusic() {
    try {
      musicPlaylist = await invoke<SoundItem[]>("get_music_playlist");
      musicState = await invoke<MusicState>("get_music_state");
    } catch (err) {
      console.error("Failed to load the music playlist", err);
    }
  }

  async function refreshStats() {
    try {
      stats = await invoke<EngineStats>("get_engine_stats");
    } catch {
      /* the bridge may not be running yet */
    }
  }

  function applyTheme(value: "dark" | "light") {
    theme = value;
    document.documentElement.dataset.theme = value;
    invoke("set_theme", { theme: value });
  }

  async function updateLanguage(newLang: "en" | "zh") {
    lang = newLang;
    try {
      await invoke("set_language", { lang: newLang });
    } catch (err) {
      console.error(err);
    }
  }

  // --------------------------------------------------------------- audio knobs

  function setAudio<K extends keyof AppConfig["audio"]>(
    key: K,
    value: AppConfig["audio"][K],
  ) {
    audio[key] = value;
    pushAudio();
  }

  function pushAudio() {
    // Slider drags fire many events; coalesce them into one IPC batch per frame.
    if (pushTimer) window.clearTimeout(pushTimer);
    pushTimer = window.setTimeout(flushAudio, 40);
  }

  function flushAudio() {
    if (pushTimer) {
      window.clearTimeout(pushTimer);
      pushTimer = null;
    }
    const snapshot = JSON.parse(JSON.stringify(audio)) as AppConfig["audio"];
    invoke("set_mic_volume", { volume: snapshot.mic_volume });
    invoke("set_fx_volume", { volume: snapshot.fx_volume });
    invoke("set_noise_gate_threshold", { threshold: snapshot.noise_gate_threshold });
    invoke("set_mic_muted", { muted: snapshot.mic_muted });
    invoke("set_fx_muted", { muted: snapshot.fx_muted });
    invoke("set_ducking", {
      enabled: snapshot.ducking_enabled,
      amount: snapshot.ducking_amount,
    });
    invoke("set_max_concurrent", { max: snapshot.max_concurrent_sounds });
    invoke("set_fade_ms", { fadeMs: snapshot.fade_ms });
    invoke("set_eq_enabled", { enabled: snapshot.eq.enabled });
    invoke("set_eq_gains", {
      low: snapshot.eq.low_db,
      mid: snapshot.eq.mid_db,
      high: snapshot.eq.high_db,
    });
    invoke("set_compressor_enabled", { enabled: snapshot.compressor.enabled });
    invoke("set_compressor_params", {
      threshold: snapshot.compressor.threshold_db,
      ratio: snapshot.compressor.ratio,
      attack: snapshot.compressor.attack_ms,
      release: snapshot.compressor.release_ms,
      gain: snapshot.compressor.makeup_db,
    });
    invoke("set_pitch_enabled", { enabled: snapshot.pitch.enabled });
    invoke("set_pitch_ratio", { ratio: snapshot.pitch.ratio });
  }

  // ------------------------------------------------------------------- sounds

  function newSoundItem(path: string): SoundItem {
    return {
      id: `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
      name: path.split("\\").pop()?.split("/").pop() || "Unknown Sound",
      path,
      shortcut: null,
      volume: 1,
      loop_playback: false,
      favorite: false,
      category: "Default",
      start_offset_ms: 0,
      end_offset_ms: null,
      retrigger: true,
      hold_to_play: false,
      duration_ms: null,
      is_music: false,
    };
  }

  async function addSoundFromFile() {
    try {
      errorMsg = "";
      const selected = await open({
        multiple: true,
        filters: [
          { name: "Audio Files", extensions: ["wav", "mp3", "ogg", "flac", "m4a"] },
        ],
      });
      if (!selected) return;

      const paths = (Array.isArray(selected) ? selected : [selected]) as string[];
      for (const path of paths) {
        await invoke("add_sound", { item: newSoundItem(path) });
      }
      await refreshConfig();
      flash(`${t.sb_add_sound}: +${paths.length}`);
    } catch (err) {
      errorMsg = String(err);
    }
  }

  async function importPath(path: string) {
    try {
      await invoke("add_sound", { item: newSoundItem(path) });
      await refreshConfig();
    } catch (err) {
      errorMsg = String(err);
    }
  }

  async function patchSound(id: string, patch: Partial<SoundItem>) {
    try {
      if (patch.name !== undefined) {
        await invoke("rename_sound", { id, name: patch.name });
      }
      if (patch.category !== undefined) {
        await invoke("set_sound_category", { id, category: patch.category });
      }
      if (patch.volume !== undefined) {
        await invoke("set_sound_volume", { id, volume: patch.volume });
      }
      if (patch.loop_playback !== undefined) {
        await invoke("set_sound_loop", { id, loopPlayback: patch.loop_playback });
      }
      if (patch.retrigger !== undefined) {
        await invoke("set_sound_retrigger", { id, retrigger: patch.retrigger });
      }
      if (patch.hold_to_play !== undefined) {
        await invoke("set_sound_hold_to_play", {
          id,
          holdToPlay: patch.hold_to_play,
        });
      }
      if (patch.favorite !== undefined) {
        await invoke("toggle_sound_favorite", { id });
      }
      if (patch.start_offset_ms !== undefined || patch.end_offset_ms !== undefined) {
        await invoke("set_sound_trim", {
          id,
          startOffsetMs: patch.start_offset_ms ?? 0,
          endOffsetMs: patch.end_offset_ms ?? null,
        });
      }
      await refreshConfig();
    } catch (err) {
      errorMsg = String(err);
    }
  }

  async function removeSound(sound: SoundItem) {
    try {
      await invoke("remove_sound", { id: sound.id });
      if (editingId === sound.id) editingId = null;
      await refreshConfig();
    } catch (err) {
      console.error("Remove err", err);
    }
  }

  async function bindHotkey(sound: SoundItem, event: KeyboardEvent) {
    const shortcut = shortcutFromKeyboardEvent(event);
    if (shortcut === undefined) return;
    try {
      errorMsg = "";
      await invoke("update_shortcut", { id: sound.id, shortcut });
      await refreshConfig();
    } catch (err) {
      errorMsg = String(err);
    }
  }

  async function bindGlobalShortcut(
    action: "stop" | "mic" | "fx",
    event: KeyboardEvent,
  ) {
    const shortcut = shortcutFromKeyboardEvent(event);
    if (shortcut === undefined) return;
    const command =
      action === "stop"
        ? "update_global_stop_shortcut"
        : action === "mic"
          ? "update_mic_mute_shortcut"
          : "update_fx_mute_shortcut";
    try {
      errorMsg = "";
      await invoke(command, { shortcut });
      await refreshConfig();
    } catch (err) {
      errorMsg = String(err);
    }
  }

  // ----------------------------------------------------------------- playback

  async function playSound(sound: SoundItem) {
    try {
      errorMsg = "";
      await invoke("play_sound_by_id", { id: sound.id });
    } catch (err) {
      errorMsg = String(err);
    }
  }

  async function stopSound(sound: SoundItem) {
    try {
      await invoke("stop_sound_by_id", { id: sound.id });
    } catch (err) {
      console.error(err);
    }
  }

  async function stopAll() {
    try {
      await invoke("stop_sound");
    } catch (err) {
      console.error(err);
    }
  }

  async function togglePause() {
    const next = !stats.paused;
    try {
      await invoke("set_paused", { paused: next });
      stats.paused = next;
    } catch (err) {
      console.error(err);
    }
  }

  // ----------------------------------------------------------------- recording

  async function musicPlay(index: number) {
    try {
      await invoke("play_music_at", { index });
      musicState = await invoke<MusicState>("get_music_state");
    } catch (err) {
      errorMsg = String(err);
    }
  }

  async function musicToggle() {
    try {
      await invoke("toggle_music");
      musicState = await invoke<MusicState>("get_music_state");
    } catch (err) {
      errorMsg = String(err);
    }
  }

  async function musicNext() {
    try {
      await invoke("next_music");
      musicState = await invoke<MusicState>("get_music_state");
    } catch (err) {
      errorMsg = String(err);
    }
  }

  async function musicPrev() {
    try {
      await invoke("prev_music");
      musicState = await invoke<MusicState>("get_music_state");
    } catch (err) {
      errorMsg = String(err);
    }
  }

  async function musicStop() {
    try {
      await invoke("stop_music");
      musicState = await invoke<MusicState>("get_music_state");
    } catch (err) {
      errorMsg = String(err);
    }
  }

  async function bindMusicHotkey(action: string, event: KeyboardEvent) {
    const shortcut = shortcutFromKeyboardEvent(event);
    if (shortcut === undefined) return;
    try {
      errorMsg = "";
      await invoke("update_music_shortcut", { action, shortcut });
      await refreshConfig();
    } catch (err) {
      errorMsg = String(err);
    }
  }

  async function toggleSoundIsMusic(sound: SoundItem) {
    try {
      await invoke("set_sound_is_music", { id: sound.id, isMusic: !sound.is_music });
      await refreshConfig();
      await loadMusic();
    } catch (err) {
      errorMsg = String(err);
    }
  }

  async function saveMonitor() {
    try {
      await invoke("set_monitor", {
        enabled: monitorEnabled,
        deviceId: monitorEnabled && monitorDevice ? monitorDevice : null,
      });
      flash(
        monitorEnabled
          ? `${t.mon_saved} · ${t.mon_needs_restart}`
          : t.mon_off,
      );
    } catch (err) {
      errorMsg = String(err);
    }
  }

  async function loadRouting() {
    try {
      routing = await invoke<RoutingDiagnostics>("get_routing_diagnostics");
    } catch (err) {
      routing = null;
    }
  }

  async function toggleAudition() {
    try {
      if (auditionRunning) {
        await invoke("stop_audition");
        auditionRunning = false;
        auditionLevel = 0;
      } else {
        const [input, output] = await invoke<[string, string]>(
          "resolve_audition_devices",
        );
        await invoke("start_audition", {
          inputDeviceId: input,
          outputDeviceId: output,
        });
        auditionRunning = true;
        flash(t.au_start);
      }
      await loadRouting();
    } catch (err) {
      errorMsg = String(err);
      auditionRunning = false;
    }
  }

  async function openSoundSettings() {
    try {
      const { openUrl } = await import("@tauri-apps/plugin-opener");
      await openUrl("ms-settings:sound");
    } catch (err) {
      errorMsg = String(err);
    }
  }

  async function restartAsAdmin() {
    try {
      await invoke("restart_as_administrator");
    } catch (err) {
      errorMsg = String(err);
    }
  }

  async function toggleRecording() {
    try {
      if (stats.recording) {
        const stamp = new Date().toISOString().slice(0, 19).replace(/[:T]/g, "-");
        const dir = await invoke<string>("get_recordings_dir");
        const path = `${dir}/echovoice-${stamp}.wav`;
        await invoke("stop_recording", { path });
        await importPath(path);
        flash(`${t.sb_stop_recording}: ${path}`);
      } else {
        await invoke("start_recording");
      }
      await refreshStats();
    } catch (err) {
      errorMsg = String(err);
    }
  }

  // -------------------------------------------------------------------- bridge

  async function startBridge() {
    if (isBridgeStarting) return;
    if (!selectedInput || !selectedOutput) {
      errorMsg = t.err_generic;
      return;
    }
    try {
      errorMsg = "";
      isBridgeStarting = true;
      await invoke("start_bridge", {
        inputDeviceId: selectedInput,
        outputDeviceId: selectedOutput,
      });
      isBridgeRunning = true;
      await invoke("set_default_devices", {
        inputId: selectedInput,
        outputId: selectedOutput,
      });
      await refreshStats();
    } catch (err) {
      errorMsg = String(err);
    } finally {
      isBridgeStarting = false;
    }
  }

  async function stopBridge() {
    try {
      errorMsg = "";
      await invoke("stop_bridge");
      isBridgeRunning = false;
      stats.voices = [];
      stats.active_voices = 0;
    } catch (err) {
      errorMsg = String(err);
    }
  }

  // ------------------------------------------------------------- drag and drop

  const AUDIO_EXTENSIONS = ["wav", "mp3", "ogg", "flac", "m4a", "aac", "wma", "opus"];

  function looksLikeAudio(path: string) {
    const ext = path.split(".").pop()?.toLowerCase() ?? "";
    return AUDIO_EXTENSIONS.includes(ext);
  }

  function setupDragAndDrop() {
    // Tauri hands over real filesystem paths, which the plain HTML5 File API does not.
    void (async () => {
      try {
        const { getCurrentWebview } = await import("@tauri-apps/api/webview");
        const unlisten = await getCurrentWebview().onDragDropEvent((event) => {
          if (event.payload.type === "enter" || event.payload.type === "over") {
            isDragging = true;
          } else if (event.payload.type === "drop") {
            isDragging = false;
            for (const path of event.payload.paths) {
              if (looksLikeAudio(path)) void importPath(path);
            }
          } else {
            isDragging = false;
          }
        });
        unlisteners.push(unlisten);
      } catch (err) {
        console.warn("Drag & drop unavailable", err);
      }
    })();
  }

  // ------------------------------------------------------------------- events

  async function setupEvents() {
    const on = <T>(name: string, handler: (payload: T) => void) =>
      listen<T>(name, (event) => handler(event.payload)).then((unlisten) => {
        unlisteners.push(unlisten);
      });

    await on<EngineStats>("engine-stats", (payload) => {
      stats = payload;
      isBridgeRunning = payload.running;
    });
    await on<string>("shortcut-play", (id) => {
      const sound = sounds.find((item) => item.id === id);
      if (sound) flash(`${t.pl_playing}: ${sound.name}`);
    });
    await on("hotkey-fired", (event) => {
      lastHotkeyFlash = String((event as { payload?: unknown }).payload ?? "");
      if (flashTimer) window.clearTimeout(flashTimer);
      flashTimer = window.setTimeout(() => (lastHotkeyFlash = null), 700);
    });
    await on<boolean>("global-stop", () => {
      stats.voices = [];
      stats.active_voices = 0;
    });
    await on<boolean>("mic-mute-changed", (muted) => {
      audio.mic_muted = muted;
    });
    await on<boolean>("fx-mute-changed", (muted) => {
      audio.fx_muted = muted;
    });
    await on<string>("playback-error", (message) => {
      errorMsg = message;
    });
    await on<string>("bridge-error", (message) => {
      errorMsg = `${t.err_bridge}: ${message}`;
      isBridgeRunning = false;
    });
    await on("bridge-stopped", () => {
      isBridgeRunning = false;
      stats.voices = [];
      stats.active_voices = 0;
    });
    await on("toggle-bridge", () => {
      if (isBridgeRunning) void stopBridge();
      else void startBridge();
    });
    await on<{ device: string }>("bridge-device-lost", (payload) => {
      stats.reconnecting = true;
      errorMsg = `${t.rt_device_lost} (${payload.device})`;
    });
    await on<{ device: string; attempt: number; retry_in_secs: number }>(
      "bridge-reconnecting",
      (payload) => {
        stats.reconnecting = true;
        stats.reconnect_attempts = payload.attempt;
        noticeMsg = `${t.rt_reconnecting} #${payload.attempt} · ${payload.retry_in_secs}s`;
      },
    );
    await on("bridge-reconnected", () => {
      stats.reconnecting = false;
      stats.reconnect_attempts = 0;
      errorMsg = "";
      flash(t.rt_reconnected);
    });
    await on("music-state-changed", async () => {
      musicState = await invoke<MusicState>("get_music_state");
    });
    await on("music-ended", async () => {
      musicState = await invoke<MusicState>("get_music_state");
    });
    await on<{ running: boolean; level: number }>("audition-level", (payload) => {
      auditionLevel = payload.level;
      auditionRunning = payload.running;
    });
  }

  onMount(async () => {
    applyTheme(theme);
    await refreshConfig();
    await loadDevices();
    await setupEvents();
    setupDragAndDrop();
    await refreshStats();

    try {
      launchAtStartup = await invoke<boolean>("get_launch_at_startup");
    } catch {
      launchAtStartup = null;
    }

    await loadMusic();
    await loadRouting();
    routingTimer = window.setInterval(loadRouting, 2000);

    if (autoStartBridge) {
      await startBridge();
    }

    // Refresh the hotkey panel every second: `keystrokes_seen` going up while a game is
    // focused is the proof that the listener is alive.
    hotkeyTimer = window.setInterval(async () => {
      try {
        hotkeyDiag = await invoke<HotkeyDiagnostics>("get_hotkey_diagnostics");
      } catch {
        /* backend not ready yet */
      }
    }, 1000);

    // Closing the window hides the app into the tray when the user asked for it.
    try {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      const appWindow = getCurrentWindow();
      await appWindow.onCloseRequested(async (event) => {
        if (!minimizeToTray) return;
        event.preventDefault();
        await appWindow.hide();
      });
    } catch (err) {
      console.warn("Window close hook unavailable", err);
    }
  });

  onDestroy(() => {
    for (const unlisten of unlisteners) unlisten();
    if (noticeTimer) window.clearTimeout(noticeTimer);
    if (pushTimer) window.clearTimeout(pushTimer);
    if (hotkeyTimer) window.clearInterval(hotkeyTimer);
    if (flashTimer) window.clearTimeout(flashTimer);
    if (routingTimer) window.clearInterval(routingTimer);
  });
</script>

<div class="app-wrapper">
  <div class="main-window">
    <div class="main-content-wrapper">
      <!-- ---------------------------------------------------------- sidebar -->
      <aside class="sidebar">
        <div class="logo">
          <span class="logo-icon">🔊</span>
          <h2>EchoVoice</h2>
        </div>

        <nav class="nav-menu">
          <button
            class="nav-item"
            class:active={activeTab === "soundboard"}
            onclick={() => (activeTab = "soundboard")}
          >
            <span class="icon">🎧</span>
            {t.tab_soundboard}
          </button>
          <button
            class="nav-item"
            class:active={activeTab === "routing"}
            onclick={() => (activeTab = "routing")}
          >
            <span class="icon">🎛️</span>
            {t.tab_routing}
          </button>
          <button
            class="nav-item"
            class:active={activeTab === "music"}
            onclick={() => (activeTab = "music")}
          >
            <span class="icon">🎵</span>
            {t.tab_music}
            {#if stats.music_playing}
              <span class="nav-badge" class:on={!stats.music_paused}></span>
            {/if}
          </button>
          <button
            class="nav-item"
            class:active={activeTab === "settings"}
            onclick={() => (activeTab = "settings")}
          >
            <span class="icon">⚙️</span>
            {t.tab_settings}
          </button>
          <button
            class="nav-item"
            class:active={activeTab === "guide"}
            onclick={() => (activeTab = "guide")}
          >
            <span class="icon">💡</span>
            {t.tab_guide}
          </button>
        </nav>

        <div class="side-meters">
          <LevelMeter
            level={stats.mic_level}
            label={t.rt_mic_level}
            muted={audio.mic_muted}
            compact
          />
          <LevelMeter level={stats.output_level} label={t.rt_out_level} compact />
        </div>

        <div
          class="bridge-status"
          class:on={isBridgeRunning}
          class:warn={stats.reconnecting}
        >
          <div class="dot"></div>
          <span>
            {#if stats.reconnecting}
              {t.rt_reconnecting}
              {#if stats.reconnect_attempts > 0}#{stats.reconnect_attempts}{/if}
            {:else if isBridgeRunning}
              {t.bridge_active}
            {:else}
              {t.bridge_offline}
            {/if}
          </span>
        </div>
        <button
          class="toggle-btn"
          onclick={isBridgeRunning ? stopBridge : startBridge}
          disabled={isBridgeStarting || stats.reconnecting}
        >
          {isBridgeRunning
            ? t.btn_stop_bridge
            : isBridgeStarting || stats.reconnecting
              ? t.btn_reconnecting
              : t.btn_start_bridge}
        </button>

        <div class="side-mutes">
          <button
            class="mute-btn"
            class:on={audio.mic_muted}
            onclick={() => {
              audio.mic_muted = !audio.mic_muted;
              pushAudio();
            }}
          >
            {audio.mic_muted ? "🔇" : "🎙"} {t.rt_mute_mic}
          </button>
          <button
            class="mute-btn"
            class:on={audio.fx_muted}
            onclick={() => {
              audio.fx_muted = !audio.fx_muted;
              pushAudio();
            }}
          >
            {audio.fx_muted ? "🔇" : "🔈"} {t.rt_mute_fx}
          </button>
        </div>
      </aside>

      <!-- ----------------------------------------------------------- content -->
      <main class="content-area">
        {#if errorMsg}
          <div class="error-banner">
            <span>{errorMsg}</span>
            <button class="banner-close" onclick={() => (errorMsg = "")}>✕</button>
          </div>
        {/if}
        {#if noticeMsg}
          <div class="notice-banner"><span>{noticeMsg}</span></div>
        {/if}

        {#if activeTab === "soundboard"}
          <header class="content-header">
            <div>
              <h1>{t.sb_title}</h1>
              <p class="subtitle">
                {sounds.length} {t.sb_subtitle}
                {#if stats.active_voices > 0}
                  · {stats.active_voices} {t.sb_playing_now}
                {/if}
              </p>
            </div>
            <div class="header-actions">
              <input
                type="text"
                class="search-bar"
                placeholder={t.sb_search_placeholder}
                bind:value={searchQuery}
              />
              <select class="filter-select" bind:value={categoryFilter}>
                <option value="">{t.sb_all_categories}</option>
                {#each categories as category}
                  <option value={category}>{category}</option>
                {/each}
              </select>
              <button
                class="chip"
                class:on={favoritesOnly}
                onclick={() => (favoritesOnly = !favoritesOnly)}
              >
                {favoritesOnly ? "★" : "☆"} {t.sb_favorites_only}
              </button>
              <button
                class="chip"
                class:rec={stats.recording}
                onclick={toggleRecording}
                disabled={!isBridgeRunning && !stats.recording}
              >
                {stats.recording ? `⏺ ${t.sb_recording}` : `⏺ ${t.sb_record}`}
              </button>
              <button class="chip" onclick={stopAll} disabled={!stats.active_voices}>
                ■ {t.sb_stop_all}
              </button>
              <button class="btn-primary" onclick={addSoundFromFile}>
                {t.sb_add_sound}
              </button>
            </div>
          </header>

          {#if filteredSounds.length === 0}
            <div class="empty-state">
              <div class="empty-icon">🎵</div>
              <h3>{sounds.length === 0 ? t.sb_empty_title : t.sb_no_results}</h3>
              <p>{sounds.length === 0 ? t.sb_empty_desc : ""}</p>
              {#if sounds.length === 0}
                <button class="btn-primary" onclick={addSoundFromFile}>
                  {t.sb_add_sound}
                </button>
              {/if}
            </div>
          {:else}
            <div class="sounds-grid">
              {#each filteredSounds as sound (sound.id)}
                <SoundTile
                  {sound}
                  progress={progressBySound.get(sound.id) ?? 0}
                  playing={playingIds.has(sound.id)}
                  bridgeRunning={isBridgeRunning}
                  labels={{
                    volume: t.sb_volume,
                    loop: t.sb_loop_badge,
                    hold: t.sb_hold_badge,
                    remove: t.sb_btn_remove,
                    bindHotkey: t.sb_bind_hotkey_prompt,
                    music: t.tab_music,
                    duration: t.sb_duration,
                    edit: t.se_title,
                    play: t.pl_playing,
                    stop: t.sb_stop_all,
                  }}
                  onPlay={playSound}
                  onStop={stopSound}
                  onToggleFavorite={(item) =>
                    patchSound(item.id, { favorite: !item.favorite })}
                  onVolume={(item, volume) => patchSound(item.id, { volume })}
                  onEdit={(item) => (editingId = item.id)}
                  onRemove={removeSound}
                  onBindHotkey={bindHotkey}
                />
              {/each}
            </div>
          {/if}
        {/if}

        {#if activeTab === "routing"}
          <div class="settings-panel">
            <h1>{t.rt_title}</h1>
            <p class="subtitle" style="margin-bottom: 20px;">{t.rt_subtitle}</p>

            <div class="form-group">
              <span class="field-label">{t.rt_mic_input}</span>
              <select id="input-source" bind:value={selectedInput} onchange={saveDevices}>
                {#each inputDevices as dev}
                  <option value={dev.id}>{dev.name}{dev.is_default ? " ★" : ""}</option>
                {/each}
              </select>
            </div>
            <div class="form-group">
              <span class="field-label">{t.rt_sb_output}</span>
              <select id="output-target" bind:value={selectedOutput} onchange={saveDevices}>
                {#each outputDevices as dev}
                  <option value={dev.id}>{dev.name}{dev.is_default ? " ★" : ""}</option>
                {/each}
              </select>
              <small>{t.gd_step2_tip}</small>
            </div>

            <div class="sliders">
              <div class="slider-box">
            <div class="monitor-box" class:on={monitorEnabled}>
              <label class="check monitor-head">
                <input
                  type="checkbox"
                  bind:checked={monitorEnabled}
                  onchange={saveMonitor}
                />
                <span>{t.mon_title}</span>
              </label>
              <p class="hint">{t.mon_desc}</p>
              {#if monitorEnabled}
                <div class="form-group" style="margin-top: 12px;">
                  <span class="field-label">{t.mon_device}</span>
                  <select bind:value={monitorDevice} onchange={saveMonitor}>
                    <option value="">{t.mon_default_device}</option>
                    {#each outputDevices.filter((d) => !d.name.includes("CABLE")) as dev}
                      <option value={dev.id}>{dev.name}</option>
                    {/each}
                  </select>
                  <p class="hint">{t.mon_needs_restart}</p>
                </div>
              {/if}
            </div>

                <span class="field-label">{t.rt_mic_vol} — {Math.round(audio.mic_volume * 100)}%</span>
                <input
                  type="range"
                  min="0"
                  max="2"
                  step="0.05"
                  value={audio.mic_volume}
                  oninput={(event) =>
                    setAudio("mic_volume", parseFloat(event.currentTarget.value))}
                />
              </div>
              <div class="slider-box">
                <span class="field-label">{t.rt_fx_vol} — {Math.round(audio.fx_volume * 100)}%</span>
                <input
                  type="range"
                  min="0"
                  max="2"
                  step="0.05"
                  value={audio.fx_volume}
                  oninput={(event) =>
                    setAudio("fx_volume", parseFloat(event.currentTarget.value))}
                />
              </div>
            </div>

            <div class="form-group">
              <span class="field-label">{t.st_gate_label}</span>
              <p class="subtitle">{t.st_gate_desc}</p>
              <div class="slider-box">
                <span class="field-label">
                  {#if audio.noise_gate_threshold === 0}
                    {t.st_gate_off}
                  {:else}
                    {Math.round(audio.noise_gate_threshold * 1000)}%
                  {/if}
                </span>
                <input
                  type="range"
                  min="0"
                  max="0.1"
                  step="0.001"
                  value={audio.noise_gate_threshold}
                  oninput={(event) =>
                    setAudio(
                      "noise_gate_threshold",
                      parseFloat(event.currentTarget.value),
                    )}
                />
              </div>
            </div>

            <!-- -------------------------------------------- routing self-check -->
            <div class="routing-box" class:bad={(routing?.problems.length ?? 0) > 0}>
              <div class="routing-head">
                <span class="field-label">{t.rt_check_title}</span>
                <button class="chip" onclick={loadRouting}>{t.rt_check_refresh}</button>
              </div>

              {#if routing}
                {#if !routing.cable_is_comms_default}
                  <button class="btn-primary fix-btn" onclick={openSoundSettings}>
                    {t.rt_fix_comms}
                  </button>
                {/if}
                <div class="check-grid">
                  <div class="check-cell" class:ok={routing.bridge_running}>
                    <span>{t.rt_check_bridge}</span>
                    <strong
                      >{routing.bridge_running ? t.bridge_active : t.bridge_offline}</strong
                    >
                  </div>
                  <div class="check-cell" class:ok={routing.output_is_cable}>
                    <span>{t.rt_check_output}</span>
                    <strong>{routing.output_is_cable ? t.hk_yes : t.hk_no}</strong>
                  </div>
                  <div class="check-cell" class:ok={routing.cable_output_present}>
                    <span>{t.rt_check_cable_present}</span>
                    <strong>{routing.cable_output_present ? t.hk_yes : t.hk_no}</strong>
                  </div>
                  <div class="check-cell" class:ok={routing.cable_is_comms_default}>
                    <span>{t.rt_check_comms_default}</span>
                    <strong>{routing.cable_is_comms_default ? t.hk_yes : t.hk_no}</strong>
                  </div>
                  <div class="check-cell" class:ok={routing.mic_device_present}>
                    <span>{t.rt_check_mic}</span>
                    <strong>{routing.mic_device_present ? t.hk_yes : t.hk_no}</strong>
                  </div>
                </div>

                <dl class="check-facts">
                  <dt>{t.rt_check_output}</dt>
                  <dd>{routing.output_device_name}</dd>
                  {#if routing.cable_output_name}
                    <dt>{t.rt_check_cable_name}</dt>
                    <dd>{routing.cable_output_name}</dd>
                  {/if}
                  <dt>{t.rt_check_comms_current}</dt>
                  <dd class:bad={!routing.cable_is_comms_default}>
                    {routing.default_comms_capture_name ?? "—"}
                  </dd>
                </dl>

                {#if routing.problems.length > 0}
                  <div class="problems">
                    {#each routing.problems as problem}
                      <p>⚠ {problem}</p>
                    {/each}
                  </div>
                {:else}
                  <div class="all-good">✓ {t.rt_check_all_good}</div>
                {/if}
              {/if}

              <!-- 试听: listen to exactly what the voice app receives -->
              <div class="audition-row">
                <button
                  class="chip"
                  class:on={auditionRunning}
                  class:rec={auditionRunning}
                  onclick={toggleAudition}
                >
                  {auditionRunning ? `⏹ ${t.au_stop}` : `🎧 ${t.au_start}`}
                </button>
                <span class="hint">{t.au_hint}</span>
              </div>
              {#if auditionRunning}
                <div class="audition-meter">
                  <LevelMeter level={auditionLevel} label={t.au_level} />
                  <span class="hint">
                    {auditionLevel > 0.001
                      ? `✓ ${t.au_signal}`
                      : `… ${t.au_no_signal}`}
                  </span>
                </div>
              {/if}
            </div>

            <hr class="rule" />

            <h2 class="section-title">{t.rt_diagnostics}</h2>
            <div class="stat-grid">
              <div class="stat">
                <span class="stat-label">{t.rt_sample_rate}</span>
                <span class="stat-value">{(stats.sample_rate / 1000).toFixed(1)} kHz</span>
              </div>
              <div class="stat">
                <span class="stat-label">{t.rt_channels}</span>
                <span class="stat-value">{stats.channels}</span>
              </div>
              <div class="stat">
                <span class="stat-label">{t.rt_buffer}</span>
                <span class="stat-value">{stats.buffer_frames}</span>
              </div>
              <div class="stat">
                <span class="stat-label">{t.rt_latency}</span>
                <span class="stat-value">{stats.latency_ms.toFixed(1)} ms</span>
              </div>
              <div class="stat">
                <span class="stat-label">{t.rt_underruns}</span>
                <span class="stat-value">{stats.underruns}</span>
              </div>
              <div class="stat">
                <span class="stat-label">{t.rt_dropped}</span>
                <span class="stat-value">{stats.dropped_samples}</span>
              </div>
              <div class="stat">
                <span class="stat-label">{t.rt_clipped}</span>
                <span class="stat-value">{stats.clipped_samples}</span>
              </div>
              <div class="stat">
                <span class="stat-label">{t.rt_active_voices}</span>
                <span class="stat-value">{stats.active_voices}</span>
              </div>
              <div class="stat">
                <span class="stat-label">{t.rt_reconnect}</span>
                <span class="stat-value">
                  {stats.reconnecting ? `#${stats.reconnect_attempts}` : t.st_gate_off}
                </span>
              </div>
            </div>

            <div class="meters-row">
              <LevelMeter
                level={stats.mic_level}
                label={t.rt_mic_level}
                muted={audio.mic_muted}
              />
              <LevelMeter level={stats.output_level} label={t.rt_out_level} />
            </div>
            <p class="hint">{t.rt_perf_note}</p>
            <p class="hint">{t.rt_autoreconnect_note}</p>

            <hr class="rule" />

            <h2 class="section-title">{t.st_behaviour}</h2>
            <label class="check">
              <input
                type="checkbox"
                checked={audio.ducking_enabled}
                onchange={(event) =>
                  setAudio("ducking_enabled", event.currentTarget.checked)}
              />
              <span>{t.rt_ducking}</span>
            </label>
            {#if audio.ducking_enabled}
              <div class="slider-box">
                <span class="field-label"
                  >{t.rt_ducking_amount} — {Math.round(audio.ducking_amount * 100)}%</span>
                <input
                  type="range"
                  min="0"
                  max="1"
                  step="0.05"
                  value={audio.ducking_amount}
                  oninput={(event) =>
                    setAudio("ducking_amount", parseFloat(event.currentTarget.value))}
                />
              </div>
            {/if}

            <div class="slider-box">
              <span class="field-label">{t.rt_max_concurrent} — {audio.max_concurrent_sounds}</span>
              <input
                type="range"
                min="1"
                max="16"
                step="1"
                value={audio.max_concurrent_sounds}
                oninput={(event) =>
                  setAudio(
                    "max_concurrent_sounds",
                    parseInt(event.currentTarget.value, 10),
                  )}
              />
            </div>

            <div class="slider-box">
              <span class="field-label">{t.rt_fade} — {audio.fade_ms} ms</span>
              <input
                type="range"
                min="0"
                max="30"
                step="1"
                value={audio.fade_ms}
                oninput={(event) =>
                  setAudio("fade_ms", parseFloat(event.currentTarget.value))}
              />
              <p class="hint">{t.rt_fade_hint}</p>
            </div>

            <hr class="rule" />
            <h2 class="section-title">{t.gd_dsp_title}</h2>

            <div class="dsp-block">
              <label class="dsp-toggle">
                <input
                  type="checkbox"
                  checked={audio.eq.enabled}
                  onchange={(event) => {
                    audio.eq.enabled = event.currentTarget.checked;
                    pushAudio();
                  }}
                />
                <strong>{t.fx_eq_enable}</strong>
              </label>
              {#if audio.eq.enabled}
                <div class="sliders dsp-sliders">
                  <div class="slider-box">
                    <span class="field-label"
                      >{t.fx_eq_low} ({audio.eq.low_db > 0 ? "+" : ""}{audio.eq.low_db} dB)</span>
                    <input
                      type="range"
                      min="-18"
                      max="18"
                      step="0.5"
                      value={audio.eq.low_db}
                      oninput={(event) => {
                        audio.eq.low_db = parseFloat(event.currentTarget.value);
                        pushAudio();
                      }}
                    />
                  </div>
                  <div class="slider-box">
                    <span class="field-label"
                      >{t.fx_eq_mid} ({audio.eq.mid_db > 0 ? "+" : ""}{audio.eq.mid_db} dB)</span>
                    <input
                      type="range"
                      min="-18"
                      max="18"
                      step="0.5"
                      value={audio.eq.mid_db}
                      oninput={(event) => {
                        audio.eq.mid_db = parseFloat(event.currentTarget.value);
                        pushAudio();
                      }}
                    />
                  </div>
                  <div class="slider-box">
                    <span class="field-label"
                      >{t.fx_eq_high} ({audio.eq.high_db > 0 ? "+" : ""}{audio.eq.high_db} dB)</span>
                    <input
                      type="range"
                      min="-18"
                      max="18"
                      step="0.5"
                      value={audio.eq.high_db}
                      oninput={(event) => {
                        audio.eq.high_db = parseFloat(event.currentTarget.value);
                        pushAudio();
                      }}
                    />
                  </div>
                </div>
              {/if}
            </div>

            <div class="dsp-block">
              <label class="dsp-toggle">
                <input
                  type="checkbox"
                  checked={audio.compressor.enabled}
                  onchange={(event) => {
                    audio.compressor.enabled = event.currentTarget.checked;
                    pushAudio();
                  }}
                />
                <strong>{t.fx_comp_enable}</strong>
              </label>
              {#if audio.compressor.enabled}
                <div class="dsp-grid">
                  <div class="slider-box">
                    <span class="field-label">{t.fx_comp_thresh} ({audio.compressor.threshold_db} dB)</span>
                    <input
                      type="range"
                      min="-60"
                      max="0"
                      step="1"
                      value={audio.compressor.threshold_db}
                      oninput={(event) => {
                        audio.compressor.threshold_db = parseFloat(event.currentTarget.value);
                        pushAudio();
                      }}
                    />
                  </div>
                  <div class="slider-box">
                    <span class="field-label">{t.fx_comp_ratio} ({audio.compressor.ratio}:1)</span>
                    <input
                      type="range"
                      min="1"
                      max="20"
                      step="0.5"
                      value={audio.compressor.ratio}
                      oninput={(event) => {
                        audio.compressor.ratio = parseFloat(event.currentTarget.value);
                        pushAudio();
                      }}
                    />
                  </div>
                  <div class="slider-box">
                    <span class="field-label">{t.fx_comp_attack} ({audio.compressor.attack_ms} ms)</span>
                    <input
                      type="range"
                      min="1"
                      max="200"
                      step="1"
                      value={audio.compressor.attack_ms}
                      oninput={(event) => {
                        audio.compressor.attack_ms = parseFloat(event.currentTarget.value);
                        pushAudio();
                      }}
                    />
                  </div>
                  <div class="slider-box">
                    <span class="field-label">{t.fx_comp_release} ({audio.compressor.release_ms} ms)</span>
                    <input
                      type="range"
                      min="10"
                      max="1000"
                      step="10"
                      value={audio.compressor.release_ms}
                      oninput={(event) => {
                        audio.compressor.release_ms = parseFloat(event.currentTarget.value);
                        pushAudio();
                      }}
                    />
                  </div>
                  <div class="slider-box">
                    <span class="field-label">{t.fx_comp_makeup} ({audio.compressor.makeup_db} dB)</span>
                    <input
                      type="range"
                      min="0"
                      max="24"
                      step="0.5"
                      value={audio.compressor.makeup_db}
                      oninput={(event) => {
                        audio.compressor.makeup_db = parseFloat(event.currentTarget.value);
                        pushAudio();
                      }}
                    />
                  </div>
                </div>
              {/if}
            </div>

            <div class="dsp-block">
              <label class="dsp-toggle">
                <input
                  type="checkbox"
                  checked={audio.pitch.enabled}
                  onchange={(event) => {
                    audio.pitch.enabled = event.currentTarget.checked;
                    pushAudio();
                  }}
                />
                <strong>{t.fx_pitch_enable}</strong>
              </label>
              {#if audio.pitch.enabled}
                <div class="sliders dsp-sliders">
                  <div class="slider-box">
                    <span class="field-label">{t.fx_pitch_ratio} (x{audio.pitch.ratio})</span>
                    <input
                      type="range"
                      min="0.5"
                      max="2"
                      step="0.05"
                      value={audio.pitch.ratio}
                      oninput={(event) => {
                        audio.pitch.ratio = parseFloat(event.currentTarget.value);
                        pushAudio();
                      }}
                    />
                  </div>
                </div>
              {/if}
            </div>
          </div>
        {/if}

        {#if activeTab === "music"}
          <div class="settings-panel music-panel">
            <header class="music-head">
              <div>
                <h1>{t.mu_title}</h1>
                <p class="subtitle">{t.mu_desc}</p>
              </div>
              <button class="chip" onclick={addSoundFromFile}>
                + {t.mu_add_track}
              </button>
            </header>

            <!-- transport -->
            <div class="transport">
              <div class="transport-info">
                <span class="transport-label">{t.mu_now_playing}</span>
                <strong class="transport-track">
                  {musicState.track_name ?? t.mu_nothing}
                </strong>
                <span class="transport-time">
                  {formatTime(musicState.position_ms)} / {formatTime(musicState.length_ms)}
                </span>
              </div>

              <div class="transport-buttons">
                <button
                  class="ctrl-btn"
                  disabled={musicPlaylist.length === 0}
                  onclick={musicPrev}
                  title={t.mu_prev}
                >
                  ⏮
                </button>
                <button
                  class="ctrl-btn play-circle"
                  disabled={musicPlaylist.length === 0}
                  onclick={musicToggle}
                  title={musicState.playing ? t.mu_pause : t.mu_play}
                >
                  {#if musicState.playing && !musicState.paused}⏸{:else}▶{/if}
                </button>
                <button
                  class="ctrl-btn stop-circle"
                  disabled={!musicState.playing}
                  onclick={musicStop}
                  title={t.mu_stop}
                >
                  ■
                </button>
                <button
                  class="ctrl-btn"
                  disabled={musicPlaylist.length === 0}
                  onclick={musicNext}
                  title={t.mu_next}
                >
                  ⏭
                </button>
              </div>
            </div>

            <div class="progress-bar-container music-progress">
              <div class="progress-bar">
                <div
                  class="progress-fill"
                  class:paused={musicState.paused}
                  style="width: {musicState.length_ms > 0
                    ? Math.round((musicState.position_ms / musicState.length_ms) * 100)
                    : 0}%"
                ></div>
              </div>
            </div>

            <div class="music-controls">
              <div class="slider-box">
                <span class="field-label"
                  >{t.mu_volume} — {Math.round(audio.music_volume * 100)}%</span
                >
                <input
                  type="range"
                  min="0"
                  max="2"
                  step="0.05"
                  value={audio.music_volume}
                  oninput={(event) =>
                    setAudio("music_volume", parseFloat(event.currentTarget.value))}
                />
              </div>
              <label class="check">
                <input
                  type="checkbox"
                  checked={musicLoop}
                  onchange={async (event) => {
                    musicLoop = event.currentTarget.checked;
                    await invoke("set_music_loop_playlist", { enabled: musicLoop });
                  }}
                />
                <span>{t.mu_loop_playlist}</span>
              </label>
            </div>

            <hr class="rule" />

            <h2 class="section-title">{t.mu_hotkeys}</h2>
            <div class="hotkey-rows">
              <div class="hotkey-row">
                <span class="hotkey-label">▶⏸ {t.mu_toggle}</span>
                <input
                  type="text"
                  class="hotkey-input"
                  class:bound={!!musicToggleKey}
                  placeholder={t.sb_bind_hotkey_prompt}
                  value={musicToggleKey ? prettyShortcut(musicToggleKey) : ""}
                  readonly
                  onkeydown={(event) => bindMusicHotkey("toggle", event)}
                />
              </div>
              <div class="hotkey-row">
                <span class="hotkey-label">⏭ {t.mu_next}</span>
                <input
                  type="text"
                  class="hotkey-input"
                  class:bound={!!musicNextKey}
                  placeholder={t.sb_bind_hotkey_prompt}
                  value={musicNextKey ? prettyShortcut(musicNextKey) : ""}
                  readonly
                  onkeydown={(event) => bindMusicHotkey("next", event)}
                />
              </div>
              <div class="hotkey-row">
                <span class="hotkey-label">⏮ {t.mu_prev}</span>
                <input
                  type="text"
                  class="hotkey-input"
                  class:bound={!!musicPrevKey}
                  placeholder={t.sb_bind_hotkey_prompt}
                  value={musicPrevKey ? prettyShortcut(musicPrevKey) : ""}
                  readonly
                  onkeydown={(event) => bindMusicHotkey("prev", event)}
                />
              </div>
              <div class="hotkey-row">
                <span class="hotkey-label">■ {t.mu_stop}</span>
                <input
                  type="text"
                  class="hotkey-input"
                  class:bound={!!musicStopKey}
                  placeholder={t.sb_bind_hotkey_prompt}
                  value={musicStopKey ? prettyShortcut(musicStopKey) : ""}
                  readonly
                  onkeydown={(event) => bindMusicHotkey("stop", event)}
                />
              </div>
            </div>
            <p class="hint">{t.mu_hotkey_hint}</p>

            <hr class="rule" />

            <h2 class="section-title">
              {t.mu_playlist} ({musicPlaylist.length})
            </h2>
            {#if musicPlaylist.length === 0}
              <div class="empty-state compact">
                <div class="empty-icon">🎵</div>
                <h3>{t.mu_empty_title}</h3>
                <p>{t.mu_empty_desc}</p>
              </div>
            {:else}
              <div class="track-list">
                {#each musicPlaylist as track, index (track.id)}
                  <div
                    class="track-row"
                    class:current={musicState.track_id === track.id}
                  >
                    <button
                      class="track-index"
                      onclick={() => musicPlay(index)}
                      disabled={!isBridgeRunning}
                      title={t.mu_play}
                    >
                      {musicState.track_id === track.id && musicState.playing
                        ? musicState.paused
                          ? "▶"
                          : "⏸"
                        : index + 1}
                    </button>
                    <div class="track-main">
                      <span class="track-name">{track.name}</span>
                      <span class="track-meta">
                        {#if track.duration_ms != null}
                          {formatTime(track.duration_ms)} ·
                        {/if}
                        {Math.round(track.volume * 100)}%
                      </span>
                    </div>
                    <button
                      class="icon-btn"
                      class:on={track.is_music}
                      title={t.mu_remove_from_playlist}
                      onclick={() => toggleSoundIsMusic(track)}
                    >
                      ✕
                    </button>
                  </div>
                {/each}
              </div>
            {/if}
          </div>
        {/if}

        {#if activeTab === "settings"}
          <div class="settings-panel">
            <h1>{t.st_title}</h1>

            <div class="form-group">
              <span class="field-label">{t.st_lang_label}</span>
              <p class="subtitle">{t.st_lang_desc}</p>
              <div class="radio-row">
                <label class="radio">
                  <input
                    type="radio"
                    name="lang"
                    checked={lang === "en"}
                    onchange={() => updateLanguage("en")}
                  />
                  <span>English</span>
                </label>
                <label class="radio">
                  <input
                    type="radio"
                    name="lang"
                    checked={lang === "zh"}
                    onchange={() => updateLanguage("zh")}
                  />
                  <span>简体中文</span>
                </label>
              </div>
            </div>

            <div class="form-group">
              <span class="field-label">{t.st_theme}</span>
              <div class="radio-row">
                <label class="radio">
                  <input
                    type="radio"
                    name="theme"
                    checked={theme === "dark"}
                    onchange={() => applyTheme("dark")}
                  />
                  <span>{t.st_theme_dark}</span>
                </label>
                <label class="radio">
                  <input
                    type="radio"
                    name="theme"
                    checked={theme === "light"}
                    onchange={() => applyTheme("light")}
                  />
                  <span>{t.st_theme_light}</span>
                </label>
              </div>
            </div>

            <hr class="rule" />

            <div class="form-group">
              <span class="field-label">{t.st_global_controls}</span>
              <p class="subtitle">{t.st_global_desc}</p>
              <div class="hotkey-rows">
                <div class="hotkey-row">
                  <span class="hotkey-label">■ {t.sb_stop_all}</span>
                  <input
                    type="text"
                    class="hotkey-input"
                    class:bound={!!globalStopShortcut}
                    placeholder={t.st_global_placeholder}
                    value={globalStopShortcut ? prettyShortcut(globalStopShortcut) : ""}
                    readonly
                    onkeydown={(event) => bindGlobalShortcut("stop", event)}
                  />
                </div>
                <div class="hotkey-row">
                  <span class="hotkey-label">🎙 {t.st_mic_mute_label}</span>
                  <input
                    type="text"
                    class="hotkey-input"
                    class:bound={!!micMuteShortcut}
                    placeholder={t.st_global_placeholder}
                    value={micMuteShortcut ? prettyShortcut(micMuteShortcut) : ""}
                    readonly
                    onkeydown={(event) => bindGlobalShortcut("mic", event)}
                  />
                </div>
                <div class="hotkey-row">
                  <span class="hotkey-label">🔈 {t.st_fx_mute_label}</span>
                  <input
                    type="text"
                    class="hotkey-input"
                    class:bound={!!fxMuteShortcut}
                    placeholder={t.st_global_placeholder}
                    value={fxMuteShortcut ? prettyShortcut(fxMuteShortcut) : ""}
                    readonly
                    onkeydown={(event) => bindGlobalShortcut("fx", event)}
                  />
                </div>
              </div>
              <p class="hint">{t.st_game_hotkey_hint}</p>
            </div>

            <hr class="rule" />

            <div class="form-group">
              <span class="field-label">{t.hk_title}</span>
              <p class="subtitle">{t.hk_desc}</p>

              {#if hotkeyDiag}
                <div class="diag-grid">
                  <div class="diag-cell" class:bad={!hotkeyDiag.listener_active}>
                    <span class="diag-label">{t.hk_listener}</span>
                    <span class="diag-value">
                      {hotkeyDiag.listener_active ? t.hk_ok : t.hk_dead}
                    </span>
                  </div>
                  <div class="diag-cell">
                    <span class="diag-label">{t.hk_channel}</span>
                    <span class="diag-value">{hotkeyDiag.channel}</span>
                  </div>
                  <div class="diag-cell" class:bad={!hotkeyDiag.elevated}>
                    <span class="diag-label">{t.hk_elevated}</span>
                    <span class="diag-value">
                      {hotkeyDiag.elevated ? t.hk_yes : t.hk_no}
                    </span>
                  </div>
                  <div class="diag-cell">
                    <span class="diag-label">{t.hk_keys_seen}</span>
                    <span class="diag-value">{hotkeyDiag.keystrokes_seen}</span>
                  </div>
                </div>

                {#if lastHotkeyFlash}
                  <div class="hotkey-flash">⌨ {lastHotkeyFlash}</div>
                {/if}

                {#if !hotkeyDiag.elevated}
                  <div class="warn-box">
                    <strong>{t.hk_elevation_title}</strong>
                    <p>{t.hk_elevation_body}</p>
                    <button class="btn-primary" onclick={restartAsAdmin}>
                      {t.hk_restart_admin}
                    </button>
                  </div>
                {/if}

                {#if hotkeyDiag.bound_hotkeys.length === 0}
                  <p class="hint">{t.hk_none_bound}</p>
                {:else}
                  <div class="bound-list">
                    {#each hotkeyDiag.bound_hotkeys as bound}
                      <div class="bound-row">
                        <code class="bound-key">{prettyShortcut(bound.accelerator)}</code>
                        <span class="bound-target">
                          {bound.target}
                          {#if bound.hold_to_play}
                            <span class="badge hold">{t.sb_hold_badge}</span>
                          {/if}
                        </span>
                      </div>
                    {/each}
                  </div>
                {/if}

                <p class="hint">{t.hk_test_hint}</p>
              {/if}
            </div>

            <hr class="rule" />

            <div class="form-group">
              <span class="field-label">{t.st_behaviour}</span>
              <label class="check">
                <input
                  type="checkbox"
                  checked={autoStartBridge}
                  onchange={async (event) => {
                    autoStartBridge = event.currentTarget.checked;
                    await invoke("set_auto_start_bridge", { enabled: autoStartBridge });
                  }}
                />
                <span>{t.st_auto_bridge}</span>
              </label>
              <label class="check">
                <input
                  type="checkbox"
                  checked={minimizeToTray}
                  onchange={async (event) => {
                    minimizeToTray = event.currentTarget.checked;
                    await invoke("set_minimize_to_tray", { enabled: minimizeToTray });
                  }}
                />
                <span>{t.st_minimize_tray}</span>
              </label>
              <label class="check">
                <input
                  type="checkbox"
                  checked={launchAtStartup ?? false}
                  onchange={async (event) => {
                    const next = event.currentTarget.checked;
                    try {
                      await invoke("set_launch_at_startup", { enabled: next });
                      launchAtStartup = next;
                    } catch (err) {
                      errorMsg = String(err);
                    }
                  }}
                />
                <span>{t.st_launch_startup}</span>
              </label>
            </div>
          </div>
        {/if}

        {#if activeTab === "guide"}
          <div class="settings-panel help-panel">
            <h1>{t.gd_title}</h1>
            <p class="subtitle" style="margin-bottom: 20px;">{t.gd_subtitle}</p>

            <div class="guide-card">
              <div class="guide-step">Step 1</div>
              <h3>{t.gd_step1_title}</h3>
              <p>{t.gd_step1_desc}</p>
              <a href="https://vb-audio.com/Cable/" target="_blank" class="download-link">
                {t.gd_step1_btn}
              </a>
              <p class="note">{t.gd_step1_note}</p>
            </div>

            <div class="guide-card">
              <div class="guide-step">Step 2</div>
              <h3>{t.gd_step2_title}</h3>
              <p>{t.gd_step2_desc}</p>
              <ul>
                <li>{t.gd_step2_l1}</li>
                <li>{t.gd_step2_l2}</li>
              </ul>
              <div class="note-box">{t.gd_step2_tip}</div>
            </div>

            <div class="guide-card">
              <div class="guide-step">Step 3</div>
              <h3>{t.gd_step3_title}</h3>
              <p>{t.gd_step3_desc}</p>
              <ul>
                <li>{t.gd_step3_l1}</li>
                <li>{t.gd_step3_l2}</li>
              </ul>
              <div class="success-box">🎉 {t.gd_step3_success}</div>
            </div>

            <hr class="rule" />

            <h2>{t.gd_sb_title}</h2>
            <div class="guide-card">
              <h3>{t.gd_sb_overlap_title}</h3>
              <p>{t.gd_sb_overlap_desc}</p>
            </div>
            <div class="guide-card">
              <h3>{t.gd_sb_vol_title}</h3>
              <p>{t.gd_sb_vol_desc}</p>
            </div>
            <div class="guide-card">
              <h3>{t.gd_sb_loop_title}</h3>
              <p>{t.gd_sb_loop_desc}</p>
            </div>
            <div class="guide-card">
              <h3>{t.gd_sb_trim_title}</h3>
              <p>{t.gd_sb_trim_desc}</p>
            </div>
            <div class="guide-card">
              <h3>{t.gd_sb_duck_title}</h3>
              <p>{t.gd_sb_duck_desc}</p>
            </div>
            <div class="guide-card">
              <h3>{t.gd_sb_rec_title}</h3>
              <p>{t.gd_sb_rec_desc}</p>
            </div>
            <div class="guide-card">
              <h3>{t.gd_sb_reconnect_title}</h3>
              <p>{t.gd_sb_reconnect_desc}</p>
            </div>
            <div class="guide-card">
              <h3>{t.gd_sb_hotkeys_title}</h3>
              <p>{t.gd_sb_hotkeys_desc}</p>
            </div>

            <hr class="rule" />

            <h2>{t.gd_dsp_title}</h2>
            <div class="guide-card">
              <h3>{t.gd_dsp_eq_title}</h3>
              <p>{t.gd_dsp_eq_desc}</p>
            </div>
            <div class="guide-card">
              <h3>{t.gd_dsp_comp_title}</h3>
              <p>{t.gd_dsp_comp_desc}</p>
            </div>
            <div class="guide-card">
              <h3>{t.gd_dsp_pitch_title}</h3>
              <p>{t.gd_dsp_pitch_desc}</p>
            </div>
          </div>
        {/if}
      </main>

      {#if editingSound}
        <SoundEditor
          sound={editingSound}
          {categories}
          labels={{
            title: t.se_title,
            name: t.se_name,
            category: t.se_category,
            volume: t.se_volume,
            loop: t.se_loop,
            retrigger: t.se_retrigger,
            hold: t.se_hold,
            favorite: t.se_favorite,
            trim: t.se_trim,
            trimStart: t.se_trim_start,
            trimEnd: t.se_trim_end,
            trimHint: t.se_trim_hint,
            hotkey: t.se_hotkey,
            hotkeyHint: t.se_hotkey_hint,
            bindHotkey: t.sb_bind_hotkey_prompt,
            useAsMusic: t.se_use_as_music,
            duration: t.sb_duration,
            file: t.se_file,
            save: t.se_save,
            delete: t.se_delete,
          }}
          onPatch={patchSound}
          onBindHotkey={bindHotkey}
          onIsMusic={(sound, isMusic) =>
            void invoke("set_sound_is_music", { id: sound.id, isMusic })
              .then(async () => {
                await refreshConfig();
                await loadMusic();
              })
              .catch((err) => (errorMsg = String(err)))}
          onRemove={(sound) => {
            void removeSound(sound);
          }}
          onClose={() => (editingId = null)}
        />
      {/if}
    </div>

    <!-- ------------------------------------------------------ bottom player -->
    <div class="bottom-player">
      <div class="player-info">
        {#if primaryVoice}
          <div class="now-playing">
            <h4>{primaryName}</h4>
            <span>
              {#if extraVoices > 0}+{extraVoices} ·{/if}
              {formatTime(
                (primaryVoice.position /
                  Math.max(1, stats.sample_rate * stats.channels)) *
                  1000,
              )}
            </span>
          </div>
        {:else}
          <div class="now-playing empty">
            <h4>{t.pl_nothing}</h4>
            <span>{t.pl_select}</span>
          </div>
        {/if}
      </div>

      <div class="player-controls">
        <div class="playback-buttons">
          <button
            class="ctrl-btn"
            disabled={!stats.active_voices}
            onclick={togglePause}
            title={stats.paused ? t.pl_resume : t.pl_pause}
          >
            {stats.paused ? "▶" : "⏸"}
          </button>
          <button
            class="ctrl-btn stop-circle"
            disabled={!stats.active_voices}
            onclick={stopAll}
            title={t.pl_stop}
          >
            ■
          </button>
        </div>
        <div class="progress-bar-container">
          <div class="progress-bar">
            <div
              class="progress-fill"
              style="width: {primaryVoice
                ? Math.round((primaryVoice.position / Math.max(1, primaryVoice.length)) * 100)
                : 0}%"
            ></div>
          </div>
        </div>
      </div>

      <div class="player-settings">
        <div class="vol-control">
          <span>🔈</span>
          <input
            type="range"
            min="0"
            max="2"
            step="0.05"
            value={audio.fx_volume}
            oninput={(event) =>
              setAudio("fx_volume", parseFloat(event.currentTarget.value))}
            class="mini-slider"
          />
          <span class="vol-readout">{Math.round(audio.fx_volume * 100)}%</span>
        </div>
      </div>
    </div>
  </div>

  {#if isDragging}
    <div class="drop-overlay">
      <div class="drop-card">
        <div class="drop-icon">🎵</div>
        <p>{t.sb_drop_hint}</p>
      </div>
    </div>
  {/if}
</div>

<style>
  :global(:root) {
    --accent: #3b82f6;
    --danger: #ef4444;
    --warn: #f59e0b;
    --card-bg: #ffffff;
    --card-border: rgba(15, 23, 42, 0.08);
    --card-shadow: 0 4px 14px rgba(15, 23, 42, 0.05);
    --card-shadow-hover: 0 10px 26px rgba(15, 23, 42, 0.1);
    --text-strong: #0f172a;
    --text-muted: #64748b;
    --text-faint: #cbd5e1;
    --input-bg: #f8fafc;
    --input-bg-focus: #ffffff;
    --input-border: #e2e8f0;
    --track-bg: #e2e8f0;
    --btn-play-bg: #fdba74;
    --btn-play-fg: #9a3412;
    --btn-stop-bg: #fca5a5;
    --btn-stop-fg: #7f1d1d;
    --btn-ghost-bg: #f1f5f9;
    --btn-ghost-bg-hover: #e2e8f0;
    --page-bg: linear-gradient(135deg, #f0f4fd 0%, #e0e8f8 100%);
    --window-bg: rgba(255, 255, 255, 0.86);
  }

  :global(:root[data-theme="dark"]) {
    --accent: #60a5fa;
    --danger: #f87171;
    --warn: #fbbf24;
    --card-bg: #161d2b;
    --card-border: rgba(148, 163, 184, 0.16);
    --card-shadow: 0 4px 14px rgba(0, 0, 0, 0.35);
    --card-shadow-hover: 0 12px 28px rgba(0, 0, 0, 0.45);
    --text-strong: #e8eefc;
    --text-muted: #94a3b8;
    --text-faint: #475569;
    --input-bg: #0f172a;
    --input-bg-focus: #131c2e;
    --input-border: #334155;
    --track-bg: #1e293b;
    --btn-play-bg: #b45309;
    --btn-play-fg: #fde3c8;
    --btn-stop-bg: #b91c1c;
    --btn-stop-fg: #fee2e2;
    --btn-ghost-bg: #1e293b;
    --btn-ghost-bg-hover: #334155;
    --page-bg: radial-gradient(
      1200px 700px at 15% -10%,
      #1b2740 0%,
      #0b1120 55%,
      #070b14 100%
    );
    --window-bg: rgba(11, 17, 32, 0.9);
  }

  :global(body) {
    margin: 0;
    font-family:
      "Inter",
      -apple-system,
      BlinkMacSystemFont,
      "Segoe UI",
      Roboto,
      Helvetica,
      Arial,
      sans-serif;
    color: var(--text-strong);
    overflow: hidden;
    background: #0b1120;
  }

  .app-wrapper {
    width: 100vw;
    height: 100vh;
    background: var(--page-bg);
    background-size: cover;
    background-position: center;
    display: flex;
    align-items: center;
    justify-content: center;
    box-sizing: border-box;
  }

  .main-window {
    width: 100%;
    height: 100%;
    background: var(--window-bg);
    backdrop-filter: blur(18px);
    box-shadow: 0 20px 40px rgba(0, 0, 0, 0.18);
    display: flex;
    flex-direction: column;
    overflow: hidden;
    position: relative;
  }

  .main-content-wrapper {
    display: flex;
    flex: 1;
    min-height: 0;
  }

  /* ------------------------------------------------------------- sidebar */
  .sidebar {
    width: 236px;
    flex-shrink: 0;
    background: rgba(127, 127, 127, 0.06);
    border-right: 1px solid var(--card-border);
    padding: 22px 16px;
    display: flex;
    flex-direction: column;
    gap: 14px;
    overflow-y: auto;
  }
  .logo {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .logo h2 {
    margin: 0;
    font-size: 1.15rem;
    font-weight: 800;
  }
  .nav-menu {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .nav-item {
    display: flex;
    align-items: center;
    gap: 11px;
    width: 100%;
    padding: 11px 14px;
    border: none;
    background: transparent;
    border-radius: 10px;
    font-size: 0.92rem;
    font-weight: 600;
    color: var(--text-muted);
    cursor: pointer;
    text-align: left;
    transition: all 0.15s ease;
  }
  .nav-item:hover {
    background: rgba(127, 127, 127, 0.1);
    color: var(--text-strong);
  }
  .nav-item.active {
    background: var(--card-bg);
    color: var(--text-strong);
    box-shadow: var(--card-shadow);
  }

  .side-meters {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 10px;
    border-radius: 10px;
    background: rgba(127, 127, 127, 0.07);
  }

  .bridge-status {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 0.82rem;
    color: var(--text-muted);
    padding: 0 4px;
  }
  .bridge-status .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--danger);
  }
  .bridge-status.on .dot {
    background: #22c55e;
    box-shadow: 0 0 8px #22c55e;
  }
  .bridge-status.warn {
    color: var(--warn);
  }
  .bridge-status.warn .dot {
    background: var(--warn);
    box-shadow: 0 0 8px var(--warn);
    animation: pulse 1s ease-in-out infinite;
  }

  .toggle-btn {
    width: 100%;
    padding: 11px;
    border-radius: 10px;
    border: none;
    background: var(--btn-ghost-bg);
    color: var(--text-strong);
    font-weight: 700;
    cursor: pointer;
    transition: filter 0.15s ease;
  }
  .toggle-btn:hover:not(:disabled) {
    filter: brightness(1.1);
  }
  .toggle-btn:disabled {
    opacity: 0.6;
    cursor: progress;
  }

  .side-mutes {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-top: auto;
  }
  .mute-btn {
    border: 1px solid var(--card-border);
    background: transparent;
    color: var(--text-muted);
    border-radius: 9px;
    padding: 8px 10px;
    font-size: 0.8rem;
    font-weight: 600;
    cursor: pointer;
    text-align: left;
  }
  .mute-btn:hover {
    color: var(--text-strong);
  }
  .mute-btn.on {
    border-color: color-mix(in srgb, var(--danger) 55%, transparent);
    color: var(--danger);
    background: color-mix(in srgb, var(--danger) 12%, transparent);
  }

  /* ------------------------------------------------------------- content */
  .content-area {
    flex: 1;
    min-width: 0;
    padding: 28px 32px;
    overflow-y: auto;
    overflow-x: hidden;
  }

  .error-banner,
  .notice-banner {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 12px 16px;
    border-radius: 11px;
    margin-bottom: 18px;
    font-size: 0.88rem;
    font-weight: 600;
  }
  .error-banner {
    background: color-mix(in srgb, var(--danger) 14%, transparent);
    color: var(--danger);
    border: 1px solid color-mix(in srgb, var(--danger) 35%, transparent);
  }
  .notice-banner {
    background: color-mix(in srgb, #22c55e 14%, transparent);
    color: #15803d;
    border: 1px solid color-mix(in srgb, #22c55e 35%, transparent);
  }
  :global(:root[data-theme="dark"]) .notice-banner {
    color: #4ade80;
  }
  .banner-close {
    border: none;
    background: transparent;
    color: inherit;
    cursor: pointer;
    font-size: 0.9rem;
  }

  .content-header {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    margin-bottom: 22px;
    flex-wrap: wrap;
    gap: 14px;
  }
  .content-header h1 {
    margin: 0 0 4px 0;
    font-size: 1.5rem;
  }
  .subtitle {
    margin: 0;
    color: var(--text-muted);
    font-size: 0.9rem;
  }
  .header-actions {
    display: flex;
    gap: 9px;
    flex-wrap: wrap;
    align-items: center;
  }
  .search-bar {
    padding: 9px 15px;
    border-radius: 999px;
    border: 1px solid var(--input-border);
    background: var(--input-bg);
    color: var(--text-strong);
    width: 180px;
    outline: none;
    font: inherit;
    font-size: 0.86rem;
  }
  .search-bar:focus {
    border-color: var(--accent);
    background: var(--input-bg-focus);
  }
  .filter-select {
    padding: 9px 12px;
    border-radius: 999px;
    border: 1px solid var(--input-border);
    background: var(--input-bg);
    color: var(--text-strong);
    font: inherit;
    font-size: 0.84rem;
    outline: none;
    max-width: 150px;
  }
  .chip {
    border: 1px solid var(--input-border);
    background: var(--input-bg);
    color: var(--text-muted);
    border-radius: 999px;
    padding: 9px 14px;
    font-size: 0.8rem;
    font-weight: 600;
    cursor: pointer;
    white-space: nowrap;
  }
  .chip:hover:not(:disabled) {
    color: var(--text-strong);
    border-color: var(--accent);
  }
  .chip:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }
  .chip.on {
    border-color: var(--accent);
    color: var(--accent);
    background: color-mix(in srgb, var(--accent) 12%, transparent);
  }
  .chip.rec {
    border-color: var(--danger);
    color: var(--danger);
    animation: pulse 1.3s ease-in-out infinite;
  }

  .btn-primary {
    background: var(--accent);
    color: #fff;
    border: none;
    padding: 9px 18px;
    border-radius: 999px;
    font-weight: 700;
    font-size: 0.86rem;
    cursor: pointer;
    white-space: nowrap;
  }
  .btn-primary:hover {
    filter: brightness(1.08);
  }

  .sounds-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(215px, 1fr));
    gap: 16px;
    padding-bottom: 20px;
  }

  .empty-state {
    text-align: center;
    padding: 60px 20px;
    color: var(--text-muted);
  }
  .empty-icon {
    font-size: 3rem;
    margin-bottom: 12px;
  }
  .empty-state h3 {
    margin: 0 0 8px;
    color: var(--text-strong);
  }

  /* ------------------------------------------------------------ settings */
  .settings-panel {
    background: var(--card-bg);
    border: 1px solid var(--card-border);
    border-radius: 16px;
    padding: 26px 28px;
    box-shadow: var(--card-shadow);
  }
  .settings-panel h1 {
    margin: 0;
    font-size: 1.4rem;
  }
  .section-title {
    font-size: 1.05rem;
    margin: 0 0 14px;
  }
  .rule {
    border: 0;
    border-top: 1px solid var(--card-border);
    margin: 26px 0;
  }
  .form-group {
    margin-bottom: 22px;
  }
  .field-label {
    display: block;
    margin-bottom: 8px;
    font-weight: 700;
    font-size: 0.92rem;
    color: var(--text-strong);
  }
  select {
    width: 100%;
    padding: 11px;
    border-radius: 9px;
    border: 1px solid var(--input-border);
    background: var(--input-bg);
    color: var(--text-strong);
    font: inherit;
    font-size: 0.9rem;
    outline: none;
  }
  .sliders {
    display: flex;
    flex-direction: column;
    gap: 18px;
    margin: 22px 0;
  }
  .slider-box .field-label {
    font-weight: 600;
    margin-bottom: 6px;
    display: block;
    font-size: 0.85rem;
    color: var(--text-muted);
  }
  input[type="range"] {
    width: 100%;
    accent-color: var(--accent);
  }
  .hint {
    font-size: 0.78rem;
    color: var(--text-muted);
    line-height: 1.5;
    margin: 6px 0 0;
  }
  .check {
    display: flex;
    align-items: flex-start;
    gap: 9px;
    font-size: 0.86rem;
    line-height: 1.4;
    cursor: pointer;
    margin-bottom: 10px;
  }
  .check input {
    margin-top: 2px;
    accent-color: var(--accent);
  }
  .radio-row {
    display: flex;
    gap: 18px;
    flex-wrap: wrap;
  }
  .radio {
    display: flex;
    align-items: center;
    gap: 6px;
    cursor: pointer;
    font-size: 0.88rem;
  }
  .radio input {
    accent-color: var(--accent);
  }

  .stat-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));
    gap: 10px;
    margin-bottom: 18px;
  }
  .stat {
    background: var(--input-bg);
    border: 1px solid var(--card-border);
    border-radius: 10px;
    padding: 10px 12px;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .stat-label {
    font-size: 0.7rem;
    color: var(--text-muted);
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.03em;
  }
  .stat-value {
    font-size: 1.02rem;
    font-weight: 700;
    font-variant-numeric: tabular-nums;
  }
  .meters-row {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 18px;
    margin-bottom: 12px;
  }

  .dsp-block {
    background: var(--input-bg);
    border: 1px solid var(--card-border);
    border-radius: 12px;
    padding: 16px;
    margin-bottom: 15px;
  }
  .dsp-toggle {
    display: flex;
    align-items: center;
    gap: 10px;
    cursor: pointer;
    font-size: 1rem;
  }
  .dsp-toggle input {
    accent-color: var(--accent);
  }
  .dsp-sliders {
    margin-top: 18px !important;
    padding-top: 18px;
    border-top: 1px dashed var(--card-border);
  }
  .dsp-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
    gap: 15px;
    margin-top: 18px;
    padding-top: 18px;
    border-top: 1px dashed var(--card-border);
  }

  .hotkey-rows {
    display: flex;
    flex-direction: column;
    gap: 10px;
    max-width: 460px;
  }
  .hotkey-row {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .hotkey-label {
    font-size: 0.82rem;
    font-weight: 600;
    color: var(--text-muted);
    min-width: 130px;
  }
  .hotkey-input {
    flex: 1;
    padding: 9px 12px;
    border-radius: 9px;
    border: 1px dashed var(--input-border);
    background: var(--input-bg);
    color: var(--text-strong);
    font-weight: 700;
    font-size: 0.85rem;
    text-align: center;
    cursor: pointer;
    outline: none;
  }
  .hotkey-input.bound {
    border-style: solid;
    border-color: color-mix(in srgb, var(--accent) 45%, transparent);
    color: var(--accent);
  }
  .hotkey-input:focus {
    border-color: var(--accent);
  }

  /* ---------------------------------------------------------- help panel */
  .help-panel {
    max-width: 860px;
  }
  .guide-card {
    background: var(--input-bg);
    padding: 24px;
    border-radius: 12px;
    margin-bottom: 18px;
    border: 1px solid var(--card-border);
    position: relative;
    overflow: hidden;
  }
  .guide-step {
    position: absolute;
    top: -10px;
    right: -16px;
    font-size: 7rem;
    font-weight: 800;
    color: var(--card-border);
    line-height: 1;
    user-select: none;
    pointer-events: none;
  }
  .guide-card h3 {
    margin: 0 0 8px;
    font-size: 1.12rem;
    position: relative;
  }
  .guide-card p,
  .guide-card ul {
    color: var(--text-muted);
    font-size: 0.95rem;
    line-height: 1.6;
    position: relative;
    margin: 0 0 8px;
  }
  .guide-card ul {
    padding-left: 20px;
  }
  .guide-card li {
    margin-bottom: 6px;
  }
  .download-link {
    display: inline-block;
    margin-top: 12px;
    padding: 10px 20px;
    background: var(--accent);
    color: #fff;
    text-decoration: none;
    border-radius: 8px;
    font-weight: 700;
    position: relative;
  }
  .note {
    font-size: 0.82rem;
    color: var(--text-muted);
    margin-top: 10px;
    font-style: italic;
  }
  .note-box {
    margin-top: 10px;
    font-size: 0.86rem;
    color: var(--text-muted);
    background: var(--card-bg);
    padding: 10px 12px;
    border-radius: 8px;
    border: 1px solid var(--card-border);
  }
  .success-box {
    margin-top: 16px;
    padding: 14px 16px;
    background: color-mix(in srgb, #22c55e 14%, transparent);
    border: 1px solid color-mix(in srgb, #22c55e 35%, transparent);
    border-radius: 8px;
    color: #15803d;
    position: relative;
  }
  :global(:root[data-theme="dark"]) .success-box {
    color: #4ade80;
  }

  /* ------------------------------------------------------ bottom player */
  .bottom-player {
    height: 78px;
    flex-shrink: 0;
    background: var(--card-bg);
    border-top: 1px solid var(--card-border);
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0 28px;
    gap: 15px;
  }
  .player-info {
    display: flex;
    align-items: center;
    flex: 1;
    min-width: 0;
  }
  .now-playing h4 {
    margin: 0;
    font-size: 0.92rem;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 260px;
  }
  .now-playing span {
    font-size: 0.78rem;
    color: #22c55e;
    font-variant-numeric: tabular-nums;
  }
  .now-playing.empty span {
    color: var(--text-faint);
  }
  .player-controls {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    flex: 2;
    min-width: 200px;
  }
  .playback-buttons {
    display: flex;
    gap: 10px;
  }
  .ctrl-btn {
    width: 38px;
    height: 38px;
    border-radius: 50%;
    border: none;
    display: flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
    font-size: 15px;
    background: var(--btn-ghost-bg);
    color: var(--text-strong);
  }
  .ctrl-btn:hover:not(:disabled) {
    background: var(--btn-ghost-bg-hover);
  }
  .ctrl-btn.stop-circle {
    background: var(--danger);
    color: #fff;
  }
  .ctrl-btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }
  .progress-bar-container {
    width: 100%;
  }
  .progress-bar {
    width: 100%;
    height: 6px;
    border-radius: 999px;
    background: var(--track-bg);
    overflow: hidden;
  }
  .progress-fill {
    height: 100%;
    background: var(--accent);
    border-radius: inherit;
    transition: width 0.1s linear;
  }
  .player-settings {
    display: flex;
    justify-content: flex-end;
    flex: 1;
    min-width: 130px;
  }
  .vol-control {
    display: flex;
    align-items: center;
    gap: 9px;
    font-size: 0.85rem;
    color: var(--text-muted);
  }
  .mini-slider {
    width: 90px;
  }
  .vol-readout {
    width: 3em;
    text-align: right;
    font-variant-numeric: tabular-nums;
  }



  /* ------------------------------------------------------------- music tab */
  .music-panel {
    max-width: 780px;
  }
  .music-head {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 16px;
    flex-wrap: wrap;
    margin-bottom: 22px;
  }
  .music-head h1 {
    margin: 0 0 4px;
    font-size: 1.4rem;
  }
  .nav-badge {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--text-faint);
    margin-left: auto;
  }
  .nav-badge.on {
    background: #22c55e;
    box-shadow: 0 0 7px #22c55e;
    animation: pulse 1.6s ease-in-out infinite;
  }

  .transport {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 18px;
    flex-wrap: wrap;
    padding: 16px 18px;
    border-radius: 13px;
    background: var(--input-bg);
    border: 1px solid var(--card-border);
  }
  .transport-info {
    display: flex;
    flex-direction: column;
    gap: 3px;
    min-width: 0;
  }
  .transport-label {
    font-size: 0.7rem;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--text-muted);
  }
  .transport-track {
    font-size: 1.08rem;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 320px;
  }
  .transport-time {
    font-size: 0.8rem;
    color: var(--text-muted);
    font-variant-numeric: tabular-nums;
  }
  .transport-buttons {
    display: flex;
    gap: 9px;
  }
  .music-progress {
    margin: 12px 0 18px;
  }
  .progress-fill.paused {
    background: var(--text-faint);
  }
  .music-controls {
    display: flex;
    align-items: flex-end;
    gap: 22px;
    flex-wrap: wrap;
  }
  .music-controls .slider-box {
    flex: 1;
    min-width: 200px;
  }

  .track-list {
    display: flex;
    flex-direction: column;
    gap: 7px;
  }
  .track-row {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px 10px;
    border-radius: 10px;
    background: var(--input-bg);
    border: 1px solid var(--card-border);
  }
  .track-row.current {
    border-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 10%, transparent);
  }
  .track-index {
    width: 32px;
    height: 32px;
    flex-shrink: 0;
    border: none;
    border-radius: 8px;
    background: var(--btn-ghost-bg);
    color: var(--text-strong);
    font-weight: 700;
    font-size: 0.84rem;
    cursor: pointer;
  }
  .track-row.current .track-index {
    background: var(--accent);
    color: #fff;
  }
  .track-index:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }
  .track-main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .track-name {
    font-size: 0.9rem;
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .track-meta {
    font-size: 0.74rem;
    color: var(--text-muted);
    font-variant-numeric: tabular-nums;
  }
  .empty-state.compact {
    padding: 34px 16px;
  }

  /* ------------------------------------------------- routing self-check */
  .routing-box {
    border: 1px solid var(--card-border);
    border-radius: 12px;
    padding: 14px 16px;
    margin: 16px 0;
    background: var(--input-bg);
  }
  .routing-box.bad {
    border-color: color-mix(in srgb, var(--danger) 55%, transparent);
    background: color-mix(in srgb, var(--danger) 8%, transparent);
  }
  .routing-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    margin-bottom: 12px;
  }
  .check-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
    gap: 8px;
    margin-bottom: 12px;
  }
  .check-cell {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 8px 11px;
    border-radius: 9px;
    background: var(--card-bg);
    border: 1px solid var(--card-border);
    font-size: 0.78rem;
    color: var(--text-muted);
  }
  .check-cell.ok {
    border-color: color-mix(in srgb, #22c55e 45%, transparent);
  }
  .check-cell.ok strong {
    color: #22c55e;
  }
  .check-cell:not(.ok) strong {
    color: var(--danger);
  }
  .check-facts {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 4px 12px;
    margin: 0 0 12px;
    font-size: 0.8rem;
  }
  .check-facts dt {
    color: var(--text-muted);
  }
  .check-facts dd {
    margin: 0;
    color: var(--text-strong);
    word-break: break-all;
  }
  .check-facts dd.bad {
    color: var(--danger);
    font-weight: 700;
  }
  .problems {
    border-left: 3px solid var(--danger);
    padding-left: 12px;
    margin: 4px 0 12px;
  }
  .problems p {
    margin: 0 0 8px;
    font-size: 0.82rem;
    line-height: 1.5;
    color: var(--text-strong);
  }
  .all-good {
    padding: 10px 12px;
    border-radius: 9px;
    background: color-mix(in srgb, #22c55e 12%, transparent);
    border: 1px solid color-mix(in srgb, #22c55e 35%, transparent);
    color: #15803d;
    font-weight: 700;
    font-size: 0.85rem;
    margin-bottom: 12px;
  }
  :global(:root[data-theme="dark"]) .all-good {
    color: #4ade80;
  }
  .fix-btn {
    margin-bottom: 12px;
  }
  .audition-row {
    display: flex;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
    padding-top: 12px;
    border-top: 1px dashed var(--card-border);
  }
  .audition-meter {
    display: flex;
    align-items: center;
    gap: 14px;
    margin-top: 10px;
    flex-wrap: wrap;
  }
  .audition-meter :global(.meter) {
    flex: 1;
    min-width: 160px;
  }

  /* ---------------------------------------------------- local monitor */
  .monitor-box {
    border: 1px solid var(--card-border);
    border-radius: 12px;
    padding: 14px 16px;
    margin: 16px 0;
    background: var(--input-bg);
  }
  .monitor-box.on {
    border-color: color-mix(in srgb, var(--accent) 55%, transparent);
    background: color-mix(in srgb, var(--accent) 8%, transparent);
  }
  .monitor-head {
    margin-bottom: 4px;
    font-weight: 700;
  }
  .monitor-head span {
    color: var(--text-strong);
  }

  /* ------------------------------------------------- hotkey diagnostics */
  .diag-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
    gap: 10px;
    margin: 14px 0;
  }
  .diag-cell {
    background: var(--input-bg);
    border: 1px solid var(--card-border);
    border-radius: 10px;
    padding: 10px 12px;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .diag-cell.bad {
    border-color: color-mix(in srgb, var(--danger) 55%, transparent);
    background: color-mix(in srgb, var(--danger) 10%, transparent);
  }
  .diag-label {
    font-size: 0.7rem;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--text-muted);
  }
  .diag-value {
    font-size: 0.98rem;
    font-weight: 700;
  }
  .diag-cell.bad .diag-value {
    color: var(--danger);
  }

  .warn-box {
    border: 1px solid color-mix(in srgb, var(--warn) 55%, transparent);
    background: color-mix(in srgb, var(--warn) 12%, transparent);
    border-radius: 11px;
    padding: 14px 16px;
    margin: 14px 0;
  }
  .warn-box strong {
    display: block;
    margin-bottom: 6px;
    color: var(--warn);
  }
  .warn-box p {
    margin: 0 0 12px;
    font-size: 0.86rem;
    line-height: 1.55;
    color: var(--text-muted);
  }

  .hotkey-flash {
    display: inline-block;
    margin: 4px 0 10px;
    padding: 6px 12px;
    border-radius: 999px;
    background: color-mix(in srgb, var(--accent) 18%, transparent);
    border: 1px solid color-mix(in srgb, var(--accent) 45%, transparent);
    color: var(--accent);
    font-weight: 700;
    font-size: 0.82rem;
    animation: pulse 0.7s ease-in-out;
  }

  .bound-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin: 10px 0;
  }
  .bound-row {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 7px 10px;
    border-radius: 8px;
    background: var(--input-bg);
    border: 1px solid var(--card-border);
    font-size: 0.84rem;
  }
  .bound-key {
    min-width: 120px;
    padding: 3px 8px;
    border-radius: 6px;
    background: color-mix(in srgb, var(--accent) 14%, transparent);
    color: var(--accent);
    font-weight: 700;
    text-align: center;
  }
  .bound-target {
    color: var(--text-muted);
    display: flex;
    align-items: center;
    gap: 8px;
  }

  /* --------------------------------------------------------- drop overlay */
  .drop-overlay {
    position: absolute;
    inset: 0;
    background: color-mix(in srgb, var(--accent) 18%, transparent);
    backdrop-filter: blur(3px);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 50;
    pointer-events: none;
  }
  .drop-card {
    background: var(--card-bg);
    border: 2px dashed var(--accent);
    border-radius: 18px;
    padding: 40px 60px;
    text-align: center;
    color: var(--text-strong);
    font-weight: 700;
  }
  .drop-icon {
    font-size: 2.6rem;
    margin-bottom: 10px;
  }

  @keyframes pulse {
    0%,
    100% {
      opacity: 0.55;
    }
    50% {
      opacity: 1;
    }
  }

  /* --------------------------------------------------------- responsive */
  @media (max-width: 980px) {
    .main-content-wrapper {
      flex-direction: column;
    }
    .sidebar {
      width: 100%;
      flex-direction: row;
      flex-wrap: wrap;
      align-items: center;
      gap: 10px;
      padding: 14px;
    }
    .logo {
      margin: 0;
    }
    .nav-menu {
      flex-direction: row;
      flex-wrap: wrap;
    }
    .nav-item {
      width: auto;
      padding: 8px 12px;
    }
    .side-meters,
    .side-mutes {
      flex-direction: row;
      margin: 0;
    }
    .bridge-status {
      margin-left: auto;
    }
    .toggle-btn {
      width: auto;
    }
    .content-area {
      padding: 20px;
    }
    .bottom-player {
      flex-wrap: wrap;
      height: auto;
      padding: 14px;
    }
    .player-info,
    .player-controls,
    .player-settings {
      width: 100%;
      flex: none;
    }
  }
</style>
