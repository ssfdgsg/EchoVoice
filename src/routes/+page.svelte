<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { invoke, convertFileSrc } from "@tauri-apps/api/core";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import { open } from "@tauri-apps/plugin-dialog";
  import { dict } from "$lib/i18n";

  interface AudioDevice {
    id: string;
    name: string;
  }

  interface SoundItem {
    id: string;
    name: string;
    path: string;
    shortcut: string | null;
  }

  interface AppConfig {
    sounds: SoundItem[];
    default_input_id: string | null;
    default_output_id: string | null;
    global_stop_shortcut: string | null;
    language: string | null;
    noise_gate_threshold?: number;
  }

  // --- State ---
  let lang = $state<"en" | "zh">("zh");
  let t = $derived(dict[lang]);
  let activeTab = $state<"soundboard" | "settings" | "routing" | "help">(
    "soundboard",
  );
  let sounds = $state<SoundItem[]>([]);
  let currentPlayingId = $state<string | null>(null);
  let searchQuery = $state("");

  let progressRatio = $state<number>(0);
  let isSeeking = $state<boolean>(false);
  let progressInterval: number | null = null;
  let currentlyPlayingSound = $derived(
    sounds.find((s) => s.id === currentPlayingId),
  );

  let inputDevices = $state<AudioDevice[]>([]);
  let outputDevices = $state<AudioDevice[]>([]);
  let selectedInput = $state<string>("");
  let selectedOutput = $state<string>("");

  let isBridgeRunning = $state(false);
  let errorMsg = $state("");

  let micVolume = $state(1.0);
  let fxVolume = $state(1.0);
  let noiseGateThreshold = $state(0.0);

  let globalStopShortcut = $state<string | null>(null);

  // --- DSP Effect States ---
  let isEqEnabled = $state(false);
  let eqLow = $state(0.0);
  let eqMid = $state(0.0);
  let eqHigh = $state(0.0);

  let isCompEnabled = $state(false);
  let compThresh = $state(-20.0);
  let compRatio = $state(4.0);
  let compAttack = $state(10.0);
  let compRelease = $state(100.0);
  let compGain = $state(0.0);

  let isPitchEnabled = $state(false);
  let pitchRatio = $state(1.0);

  // --- App Config & Data ---
  async function loadConfig() {
    try {
      const config: AppConfig = await invoke("get_app_config");
      sounds = config.sounds || [];
      globalStopShortcut = config.global_stop_shortcut;
      if (config.default_input_id && !selectedInput)
        selectedInput = config.default_input_id;
      if (config.default_output_id && !selectedOutput)
        selectedOutput = config.default_output_id;
      if (config.language === "en" || config.language === "zh") {
        lang = config.language as "en" | "zh";
      }
      if (
        config.noise_gate_threshold !== null &&
        config.noise_gate_threshold !== undefined
      ) {
        noiseGateThreshold = config.noise_gate_threshold;
      }
    } catch (e) {
      console.error("Failed to load config", e);
    }
  }

  async function updateLanguage(newLang: "en" | "zh") {
    lang = newLang;
    try {
      await invoke("set_language", { lang: newLang });
    } catch (err) {
      console.error(err);
    }
  }

  async function loadDevices() {
    try {
      errorMsg = "";
      inputDevices = await invoke("get_input_devices");
      outputDevices = await invoke("get_output_devices");

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

  async function saveDefaultDevices() {
    try {
      await invoke("set_default_devices", {
        inputId: selectedInput,
        outputId: selectedOutput,
      });
    } catch (err) {
      console.error(err);
    }
  }

  // --- Volumes ---
  async function updateMicVolume() {
    try {
      await invoke("set_mic_volume", {
        volume: parseFloat(micVolume.toString()),
      });
    } catch (err) {
      console.error(err);
    }
  }

  async function updateFxVolume() {
    try {
      await invoke("set_fx_volume", {
        volume: parseFloat(fxVolume.toString()),
      });
    } catch (err) {
      console.error(err);
    }
  }

  // --- Noise Gate ---
  async function updateNoiseGate() {
    try {
      await invoke("set_noise_gate_threshold", {
        threshold: parseFloat(noiseGateThreshold.toString()),
      });
    } catch (err) {
      console.error(err);
    }
  }

  // --- DSP Sync ---
  function updateEq() {
    invoke("set_eq_enabled", { enabled: isEqEnabled });
    invoke("set_eq_gains", { low: eqLow, mid: eqMid, high: eqHigh });
  }

  function updateCompressor() {
    invoke("set_compressor_enabled", { enabled: isCompEnabled });
    invoke("set_compressor_params", {
      threshold: compThresh,
      ratio: compRatio,
      attack: compAttack,
      release: compRelease,
      gain: compGain,
    });
  }

  function updatePitch() {
    invoke("set_pitch_enabled", { enabled: isPitchEnabled });
    invoke("set_pitch_ratio", { ratio: pitchRatio });
  }

  // --- Sounds ---
  async function addSoundFromFile() {
    try {
      const selected = await open({
        multiple: false,
        filters: [{ name: "Audio Files", extensions: ["wav", "mp3", "ogg"] }],
      });
      if (!selected) return;

      const path = selected as string;
      const name = path.split("\\").pop()?.split("/").pop() || "Unknown Sound";
      const id = Date.now().toString();

      const newItem: SoundItem = { id, name, path, shortcut: null };
      await invoke("add_sound", { item: newItem });
      await loadConfig(); // Reload from backend
    } catch (err) {
      errorMsg = String(err);
    }
  }

  async function removeSoundItem(id: string) {
    try {
      await invoke("remove_sound", { id });
      if (currentPlayingId === id) currentPlayingId = null;
      await loadConfig();
    } catch (err) {
      console.error("Remove err", err);
    }
  }

  async function playSoundItem(id: string, path: string) {
    try {
      currentPlayingId = id;
      progressRatio = 0;
      await invoke("play_sound", { filePath: path });
      startProgressPolling();
    } catch (err) {
      errorMsg = String(err);
      currentPlayingId = null;
    }
  }

  async function stopCurrentSound() {
    try {
      await invoke("stop_sound");
      currentPlayingId = null;
      stopProgressPolling();
    } catch (err) {
      console.error("Stop err", err);
    }
  }

  // --- Progress Polling ---
  function startProgressPolling() {
    stopProgressPolling();
    progressInterval = window.setInterval(async () => {
      if (!currentPlayingId || isSeeking) return;
      try {
        const state: [number, number] | null = await invoke(
          "get_playback_progress",
        );
        if (state) {
          const [pos, len] = state;
          if (len > 0) progressRatio = (pos / len) * 100;
          if (pos >= len) {
            currentPlayingId = null;
            stopProgressPolling();
          }
        } else {
          currentPlayingId = null;
          stopProgressPolling();
        }
      } catch (e) {}
    }, 100);
  }

  function stopProgressPolling() {
    if (progressInterval) {
      clearInterval(progressInterval);
      progressInterval = null;
    }
    progressRatio = 0;
  }

  async function handleSeek(e: Event) {
    const target = e.target as HTMLInputElement;
    const ratio = parseFloat(target.value) / 100.0;
    try {
      await invoke("seek_sound", { positionRatio: ratio });
    } catch (err) {}
  }

  async function updateSoundShortcut(id: string, shortcut: string | null) {
    try {
      await invoke("update_shortcut", { id, shortcut });
      await loadConfig();
    } catch (err) {}
  }

  async function updateGlobalStopShortcut(shortcut: string | null) {
    try {
      await invoke("update_global_stop_shortcut", { shortcut });
      await loadConfig();
    } catch (err) {}
  }

  // --- Bridge ---
  async function startBridge() {
    if (!selectedInput || !selectedOutput) {
      errorMsg = "Select both input and output devices.";
      return;
    }
    try {
      errorMsg = "";
      await invoke("start_bridge", {
        inputDeviceId: selectedInput,
        outputDeviceId: selectedOutput,
      });
      isBridgeRunning = true;
      saveDefaultDevices();
    } catch (err) {
      errorMsg = String(err);
    }
  }

  async function stopBridge() {
    try {
      errorMsg = "";
      await invoke("stop_bridge");
      isBridgeRunning = false;
      currentPlayingId = null;
    } catch (err) {
      errorMsg = String(err);
    }
  }

  let unlistenToggleBridge: UnlistenFn | undefined;
  let unlistenShortcutPlay: UnlistenFn | undefined;
  let unlistenGlobalStop: UnlistenFn | undefined;

  onMount(async () => {
    await loadConfig();
    await loadDevices();

    unlistenToggleBridge = await listen("toggle-bridge", () => {
      if (isBridgeRunning) stopBridge();
      else startBridge();
    });

    unlistenShortcutPlay = await listen<string>("shortcut-play", (event) => {
      currentPlayingId = event.payload;
      progressRatio = 0;
      startProgressPolling();
    });

    unlistenGlobalStop = await listen("global-stop", () => {
      currentPlayingId = null;
      stopProgressPolling();
    });
  });

  onDestroy(() => {
    unlistenToggleBridge?.();
    unlistenShortcutPlay?.();
    unlistenGlobalStop?.();
  });

  // Derived filtered sounds
  let filteredSounds = $derived(
    sounds.filter((s) =>
      s.name.toLowerCase().includes(searchQuery.toLowerCase()),
    ),
  );
</script>

<div class="app-wrapper">
  <div class="main-window">
    <div class="main-content-wrapper">
      <!-- Sidebar -->
      <aside class="sidebar">
        <div class="logo">
          <span class="logo-icon">🔊</span>
          <h2>EchoVoice</h2>
        </div>

        <nav class="nav-menu">
          <button
            class="nav-item {activeTab === 'soundboard' ? 'active' : ''}"
            onclick={() => (activeTab = "soundboard")}
          >
            <span class="icon">🎧</span>
            {t.tab_soundboard}
          </button>
          <button
            class="nav-item {activeTab === 'routing' ? 'active' : ''}"
            onclick={() => (activeTab = "routing")}
          >
            <span class="icon">🎛️</span>
            {t.tab_routing}
          </button>
          <button
            class="nav-item {activeTab === 'settings' ? 'active' : ''}"
            onclick={() => (activeTab = "settings")}
          >
            <span class="icon">⚙️</span>
            {t.tab_settings}
          </button>
          <button
            class="nav-item {activeTab === 'help' ? 'active' : ''}"
            onclick={() => (activeTab = "help")}
          >
            <span class="icon">💡</span>
            {t.tab_guide}
          </button>
        </nav>

        <div class="bridge-status {isBridgeRunning ? 'on' : ''}">
          <div class="dot"></div>
          <span>{isBridgeRunning ? t.bridge_active : t.bridge_offline}</span>
        </div>
        <button
          class="toggle-btn"
          onclick={isBridgeRunning ? stopBridge : startBridge}
        >
          {isBridgeRunning ? t.btn_stop_bridge : t.btn_start_bridge}
        </button>
      </aside>

      <!-- Main Content -->
      <main class="content-area">
        {#if errorMsg}
          <div class="error-banner">{errorMsg}</div>
        {/if}

        {#if activeTab === "soundboard"}
          <header class="content-header">
            <div>
              <h1>{t.sb_title}</h1>
              <p class="subtitle">{sounds.length} {t.sb_subtitle}</p>
            </div>
            <div class="header-actions">
              <input
                type="text"
                class="search-bar"
                placeholder={t.sb_search_placeholder}
                bind:value={searchQuery}
              />
              <button class="btn-primary" onclick={addSoundFromFile}
                >{t.sb_add_sound}</button
              >
            </div>
          </header>

          <div class="sounds-grid">
            {#each filteredSounds as sound}
              <div
                class={`sound-card ${currentPlayingId === sound.id ? "playing" : ""}`}
              >
                <div class="card-icon">
                  {sound.name.charAt(0).toUpperCase()}
                </div>
                <div class="card-info">
                  <h3>{sound.name}</h3>
                  <input
                    type="text"
                    class="hotkey-input"
                    placeholder={t.sb_bind_hotkey_prompt}
                    value={sound.shortcut || ""}
                    onkeydown={(e) => {
                      e.preventDefault();
                      let keys: string[] = [];
                      if (e.ctrlKey) keys.push("CommandOrControl");
                      if (e.altKey) keys.push("Alt");
                      if (e.shiftKey) keys.push("Shift");
                      const nonModifiers = [
                        "Control",
                        "Alt",
                        "Shift",
                        "Meta",
                        "Escape",
                      ];
                      if (
                        e.key === "Escape" ||
                        e.key === "Backspace" ||
                        e.key === "Delete"
                      ) {
                        updateSoundShortcut(sound.id, null);
                        e.currentTarget.blur();
                        return;
                      }
                      if (!nonModifiers.includes(e.key)) {
                        keys.push(e.key.toUpperCase());
                        const combo = keys.join("+");
                        updateSoundShortcut(sound.id, combo);
                        e.currentTarget.blur();
                      }
                    }}
                    readonly
                  />
                </div>
                <div class="card-actions">
                  {#if currentPlayingId === sound.id}
                    <button class="play-btn stop" onclick={stopCurrentSound}
                      >■</button
                    >
                  {:else}
                    <button
                      class="play-btn play"
                      onclick={() => playSoundItem(sound.id, sound.path)}
                      disabled={!isBridgeRunning}>▶ {t.pl_playing}</button
                    >
                  {/if}
                  <button
                    class="menu-btn"
                    onclick={() => removeSoundItem(sound.id)}
                    title={t.sb_btn_remove}>🗑️</button
                  >
                </div>
              </div>
            {/each}
          </div>
        {/if}

        {#if activeTab === "routing"}
          <div class="settings-panel">
            <h1>{t.rt_title}</h1>
            <p class="subtitle" style="margin-bottom: 20px;">
              {t.rt_subtitle}
            </p>

            <div class="form-group">
              <label for="input-source">{t.rt_mic_input}</label>
              <select
                id="input-source"
                bind:value={selectedInput}
                onchange={saveDefaultDevices}
              >
                {#each inputDevices as dev}
                  <option value={dev.id}>{dev.name}</option>
                {/each}
              </select>
            </div>
            <div class="form-group">
              <label for="output-target">{t.rt_sb_output}</label>
              <select
                id="output-target"
                bind:value={selectedOutput}
                onchange={saveDefaultDevices}
              >
                {#each outputDevices as dev}
                  <option value={dev.id}>{dev.name}</option>
                {/each}
              </select>
              <small
                >Route this output into Discord/OBS as your microphone.</small
              >
            </div>

            <div class="sliders">
              <div class="slider-box">
                <label>{t.rt_mic_vol} ({Math.round(micVolume * 100)}%)</label>
                <input
                  type="range"
                  min="0"
                  max="2"
                  step="0.05"
                  bind:value={micVolume}
                  oninput={updateMicVolume}
                />
              </div>
              <div class="slider-box">
                <label>{t.rt_fx_vol} ({Math.round(fxVolume * 100)}%)</label>
                <input
                  type="range"
                  min="0"
                  max="2"
                  step="0.05"
                  bind:value={fxVolume}
                  oninput={updateFxVolume}
                />
              </div>
            </div>

            <div class="form-group" style="margin-top: 25px;">
              <label>{t.st_gate_label}</label>
              <p class="subtitle" style="margin-bottom: 10px;">
                {t.st_gate_desc}
              </p>
              <div class="slider-box" style="margin-top: 10px;">
                <label>
                  {#if noiseGateThreshold === 0}
                    {t.st_gate_off}
                  {:else}
                    {Math.round(noiseGateThreshold * 1000)}%
                  {/if}
                </label>
                <input
                  type="range"
                  min="0"
                  max="0.1"
                  step="0.001"
                  bind:value={noiseGateThreshold}
                  oninput={updateNoiseGate}
                />
              </div>
            </div>

            <!-- DSP Audio Effects Area -->
            <hr
              style="border: 0; border-top: 1px solid #f1f5f9; margin: 30px 0;"
            />
            <h2 style="margin-top: 0; font-size: 1.25rem;">Advanced Filters</h2>

            <!-- 1. Equalizer -->
            <div class="dsp-block">
              <label class="dsp-toggle">
                <input
                  type="checkbox"
                  bind:checked={isEqEnabled}
                  onchange={updateEq}
                />
                <strong>{t.fx_eq_enable}</strong>
              </label>
              {#if isEqEnabled}
                <div class="sliders dsp-sliders">
                  <div class="slider-box">
                    <label for="eqLow"
                      >{t.fx_eq_low} ({eqLow > 0 ? "+" : ""}{eqLow} dB)</label
                    >
                    <input
                      id="eqLow"
                      type="range"
                      min="-12"
                      max="12"
                      step="0.5"
                      bind:value={eqLow}
                      oninput={updateEq}
                    />
                  </div>
                  <div class="slider-box">
                    <label for="eqMid"
                      >{t.fx_eq_mid} ({eqMid > 0 ? "+" : ""}{eqMid} dB)</label
                    >
                    <input
                      id="eqMid"
                      type="range"
                      min="-12"
                      max="12"
                      step="0.5"
                      bind:value={eqMid}
                      oninput={updateEq}
                    />
                  </div>
                  <div class="slider-box">
                    <label for="eqHigh"
                      >{t.fx_eq_high} ({eqHigh > 0 ? "+" : ""}{eqHigh} dB)</label
                    >
                    <input
                      id="eqHigh"
                      type="range"
                      min="-12"
                      max="12"
                      step="0.5"
                      bind:value={eqHigh}
                      oninput={updateEq}
                    />
                  </div>
                </div>
              {/if}
            </div>

            <!-- 2. Compressor -->
            <div class="dsp-block">
              <label class="dsp-toggle">
                <input
                  type="checkbox"
                  bind:checked={isCompEnabled}
                  onchange={updateCompressor}
                />
                <strong>{t.fx_comp_enable}</strong>
              </label>
              {#if isCompEnabled}
                <div
                  class="sliders dsp-sliders"
                  style="display: grid; grid-template-columns: 1fr 1fr; gap: 15px;"
                >
                  <div class="slider-box">
                    <label for="compThresh"
                      >{t.fx_comp_thresh} ({compThresh} dB)</label
                    >
                    <input
                      id="compThresh"
                      type="range"
                      min="-60"
                      max="0"
                      step="1"
                      bind:value={compThresh}
                      oninput={updateCompressor}
                    />
                  </div>
                  <div class="slider-box">
                    <label for="compRatio"
                      >{t.fx_comp_ratio} ({compRatio}:1)</label
                    >
                    <input
                      id="compRatio"
                      type="range"
                      min="1"
                      max="20"
                      step="0.5"
                      bind:value={compRatio}
                      oninput={updateCompressor}
                    />
                  </div>
                  <div class="slider-box">
                    <label for="compAttack"
                      >{t.fx_comp_attack} ({compAttack} ms)</label
                    >
                    <input
                      id="compAttack"
                      type="range"
                      min="1"
                      max="200"
                      step="1"
                      bind:value={compAttack}
                      oninput={updateCompressor}
                    />
                  </div>
                  <div class="slider-box">
                    <label for="compRelease"
                      >{t.fx_comp_release} ({compRelease} ms)</label
                    >
                    <input
                      id="compRelease"
                      type="range"
                      min="10"
                      max="1000"
                      step="10"
                      bind:value={compRelease}
                      oninput={updateCompressor}
                    />
                  </div>
                  <div class="slider-box" style="grid-column: span 2;">
                    <label for="compGain"
                      >{t.fx_comp_makeup} ({compGain} dB)</label
                    >
                    <input
                      id="compGain"
                      type="range"
                      min="0"
                      max="24"
                      step="0.5"
                      bind:value={compGain}
                      oninput={updateCompressor}
                    />
                  </div>
                </div>
              {/if}
            </div>

            <!-- 3. Pitch Shifter -->
            <div class="dsp-block">
              <label class="dsp-toggle">
                <input
                  type="checkbox"
                  bind:checked={isPitchEnabled}
                  onchange={updatePitch}
                />
                <strong>{t.fx_pitch_enable}</strong>
              </label>
              {#if isPitchEnabled}
                <div class="sliders dsp-sliders">
                  <div class="slider-box">
                    <label for="pitchRatio"
                      >{t.fx_pitch_ratio} (x{pitchRatio})</label
                    >
                    <input
                      id="pitchRatio"
                      type="range"
                      min="0.5"
                      max="2.0"
                      step="0.05"
                      bind:value={pitchRatio}
                      oninput={updatePitch}
                    />
                  </div>
                </div>
              {/if}
            </div>
          </div>
        {/if}

        {#if activeTab === "settings"}
          <div class="settings-panel">
            <h1>{t.st_title}</h1>

            <div class="form-group">
              <label>{t.st_lang_label}</label>
              <p class="subtitle" style="margin-bottom: 10px;">
                {t.st_lang_desc}
              </p>
              <div style="display: flex; gap: 15px; margin-top: 5px;">
                <label
                  style="display: flex; align-items: center; gap: 5px; cursor: pointer;"
                >
                  <input
                    type="radio"
                    name="lang"
                    value="en"
                    checked={lang === "en"}
                    onchange={() => updateLanguage("en")}
                  />
                  <span style="font-weight: normal;">English</span>
                </label>
                <label
                  style="display: flex; align-items: center; gap: 5px; cursor: pointer;"
                >
                  <input
                    type="radio"
                    name="lang"
                    value="zh"
                    checked={lang === "zh"}
                    onchange={() => updateLanguage("zh")}
                  />
                  <span style="font-weight: normal;">简体中文</span>
                </label>
              </div>
            </div>

            <hr
              style="border: 0; border-top: 1px solid #f1f5f9; margin: 30px 0;"
            />

            <div class="form-group">
              <label>{t.st_global_controls}</label>
              <p class="subtitle" style="margin-bottom: 10px;">
                {t.st_global_desc}
              </p>
              <input
                type="text"
                class="hotkey-input"
                style="max-width: 300px; border: 1px solid #e2e8f0; padding: 10px;"
                placeholder={t.st_global_placeholder}
                value={globalStopShortcut || ""}
                onkeydown={(e) => {
                  e.preventDefault();
                  let keys: string[] = [];
                  if (e.ctrlKey) keys.push("CommandOrControl");
                  if (e.altKey) keys.push("Alt");
                  if (e.shiftKey) keys.push("Shift");
                  const nonModifiers = [
                    "Control",
                    "Alt",
                    "Shift",
                    "Meta",
                    "Escape",
                  ];
                  if (
                    e.key === "Escape" ||
                    e.key === "Backspace" ||
                    e.key === "Delete"
                  ) {
                    updateGlobalStopShortcut(null);
                    e.currentTarget.blur();
                    return;
                  }
                  if (!nonModifiers.includes(e.key)) {
                    keys.push(e.key.toUpperCase());
                    const combo = keys.join("+");
                    updateGlobalStopShortcut(combo);
                    e.currentTarget.blur();
                  }
                }}
                readonly
              />
            </div>
          </div>
        {/if}

        {#if activeTab === "help"}
          <div class="settings-panel help-panel">
            <h1>{t.gd_title}</h1>
            <p class="subtitle" style="margin-bottom: 20px;">
              {t.gd_subtitle}
            </p>

            <div class="guide-card">
              <div class="guide-step">Step 1</div>
              <h3>{t.gd_step1_title}</h3>
              <p>
                {t.gd_step1_desc}
              </p>
              <a
                href="https://vb-audio.com/Cable/"
                target="_blank"
                class="download-link"
              >
                {t.gd_step1_btn}
              </a>
              <p class="note">{t.gd_step1_note}</p>
            </div>

            <div class="guide-card">
              <div class="guide-step">Step 2</div>
              <h3>{t.gd_step2_title}</h3>
              <p>
                {t.gd_step2_desc}
              </p>
              <ul>
                <li>{t.gd_step2_l1}</li>
                <li>{t.gd_step2_l2}</li>
              </ul>
              <div
                class="note"
                style="margin-top: 10px; font-size: 0.9em; color: #64748b; background: #f8fafc; padding: 10px; border-radius: 6px;"
              >
                {t.gd_step2_tip}
              </div>
            </div>

            <div class="guide-card">
              <div class="guide-step">Step 3</div>
              <h3>{t.gd_step3_title}</h3>
              <p>
                {t.gd_step3_desc}
              </p>
              <ul>
                <li>{t.gd_step3_l1}</li>
                <li>{t.gd_step3_l2}</li>
              </ul>
              <div class="success-box">
                🎉 {t.gd_step3_success}
              </div>
            </div>

            <hr
              style="border: 0; border-top: 1px solid #e2e8f0; margin: 40px 0;"
            />

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
    </div>

    <!-- Bottom Player -->
    <div class="bottom-player">
      <div class="player-info">
        {#if currentlyPlayingSound}
          <div class="now-playing">
            <h4>{currentlyPlayingSound.name}</h4>
            <span>{t.pl_playing}</span>
          </div>
        {:else}
          <div class="now-playing empty">
            <h4>{t.pl_ready}</h4>
            <span>{t.pl_select}</span>
          </div>
        {/if}
      </div>

      <div class="player-controls">
        <div class="playback-buttons">
          {#if currentPlayingId}
            <button class="ctrl-btn stop-circle" onclick={stopCurrentSound}
              >■</button
            >
          {:else}
            <button class="ctrl-btn play-circle" disabled>▶</button>
          {/if}
        </div>
        <div class="progress-bar-container">
          <input
            type="range"
            min="0"
            max="100"
            step="0.1"
            class="progress-bar"
            bind:value={progressRatio}
            onmousedown={() => {
              isSeeking = true;
            }}
            onmouseup={(e) => {
              isSeeking = false;
              handleSeek(e);
            }}
            oninput={(e) => handleSeek(e)}
            disabled={!currentPlayingId}
          />
        </div>
      </div>

      <div class="player-settings">
        <div class="vol-control">
          <span>🔊 Volume</span>
          <input
            type="range"
            min="0"
            max="2"
            step="0.05"
            bind:value={fxVolume}
            oninput={updateFxVolume}
            class="mini-slider"
          />
        </div>
      </div>
    </div>
  </div>
</div>

<style>
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
    color: #333;
    overflow: hidden;
  }

  .app-wrapper {
    width: 100vw;
    height: 100vh;
    background: linear-gradient(135deg, #f0f4fd 0%, #e0e8f8 100%);
    background-size: cover;
    background-position: center;
    background-repeat: no-repeat;
    display: flex;
    align-items: center;
    justify-content: center;
    box-sizing: border-box;
  }

  .main-window {
    width: 100%;
    height: 100%;
    background: rgba(255, 255, 255, 0.85);
    backdrop-filter: blur(20px);
    box-shadow:
      0 20px 40px rgba(0, 0, 0, 0.1),
      0 1px 3px rgba(0, 0, 0, 0.05);
    display: flex;
    flex-direction: column;
    overflow: hidden;
    position: relative;
    border: 1px solid rgba(255, 255, 255, 0.5);
  }

  .main-content-wrapper {
    display: flex;
    flex: 1;
    min-height: 0; /* Important for scrollable children in flexbox */
  }

  /* --- Sidebar --- */
  .sidebar {
    width: 240px;
    flex-shrink: 0;
    background: rgba(250, 250, 250, 0.5);
    border-right: 1px solid rgba(0, 0, 0, 0.05);
    padding: 30px 20px;
    display: flex;
    flex-direction: column;
    overflow-y: auto;
  }

  .logo {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 40px;
  }

  .logo-icon {
    font-size: 24px;
  }

  .logo h2 {
    margin: 0;
    font-size: 1.2rem;
    font-weight: 700;
  }

  .nav-menu {
    display: flex;
    flex-direction: column;
    gap: 8px;
    flex-grow: 1;
  }

  .nav-item {
    display: flex;
    align-items: center;
    gap: 12px;
    width: 100%;
    padding: 12px 16px;
    border: none;
    background: transparent;
    border-radius: 10px;
    font-size: 0.95rem;
    font-weight: 600;
    color: #64748b;
    cursor: pointer;
    text-align: left;
    transition: all 0.2s ease;
  }

  .nav-item:hover {
    background: rgba(255, 255, 255, 0.6);
    color: #1e293b;
  }

  .nav-item.active {
    background: #ffffff;
    color: #1e293b;
    box-shadow: 0 2px 8px rgba(0, 0, 0, 0.04);
  }

  .bridge-status {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 0.85rem;
    color: #64748b;
    margin-bottom: 12px;
    padding: 0 10px;
  }

  .bridge-status .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: #ef4444;
  }
  .bridge-status.on .dot {
    background: #10b981;
    box-shadow: 0 0 8px #10b981;
  }

  .toggle-btn {
    width: 100%;
    padding: 12px;
    border-radius: 10px;
    border: none;
    background: #f1f5f9;
    color: #334155;
    font-weight: 600;
    cursor: pointer;
    transition: all 0.2s ease;
  }
  .toggle-btn:hover {
    background: #e2e8f0;
  }

  /* --- Content Area --- */
  .content-area {
    flex: 1;
    min-width: 0;
    padding: 40px;
    overflow-y: auto;
    overflow-x: hidden;
  }

  .content-header {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    margin-bottom: 30px;
    flex-wrap: wrap;
    gap: 15px;
  }

  .content-header h1 {
    margin: 0 0 4px 0;
    font-size: clamp(1.4rem, 2vw, 1.8rem);
    color: #0f172a;
  }

  .subtitle {
    margin: 0;
    color: #64748b;
    font-size: 0.95rem;
  }

  .header-actions {
    display: flex;
    gap: 15px;
  }

  .search-bar {
    padding: 10px 16px;
    border-radius: 20px;
    border: 1px solid #e2e8f0;
    background: #f8fafc;
    width: 100%;
    max-width: 250px;
    outline: none;
    font-family: inherit;
    transition: all 0.2s;
  }
  .search-bar:focus {
    background: #fff;
    border-color: #cbd5e1;
    box-shadow: 0 2px 10px rgba(0, 0, 0, 0.05);
  }

  .btn-primary {
    background: #3b82f6;
    color: white;
    border: none;
    padding: 10px 20px;
    border-radius: 20px;
    font-weight: 600;
    cursor: pointer;
    transition: background 0.2s;
  }
  .btn-primary:hover {
    background: #2563eb;
  }

  /* --- Sound Grid --- */
  .sounds-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
    gap: 20px;
    padding-bottom: 20px;
  }

  .sound-card {
    background: #ffffff;
    border-radius: 16px;
    padding: 16px;
    display: flex;
    flex-direction: column;
    box-shadow: 0 4px 15px rgba(0, 0, 0, 0.03);
    border: 1px solid #f1f5f9;
    transition:
      transform 0.2s,
      box-shadow 0.2s;
  }
  .sound-card:hover {
    transform: translateY(-2px);
    box-shadow: 0 8px 25px rgba(0, 0, 0, 0.06);
  }
  .sound-card.playing {
    border: 1px solid #3b82f6;
    box-shadow: 0 8px 25px rgba(59, 130, 246, 0.15);
  }

  .card-icon {
    width: 48px;
    height: 48px;
    border-radius: 12px;
    background: #eff6ff;
    color: #3b82f6;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 20px;
    font-weight: 700;
    margin-bottom: 12px;
  }

  .card-info h3 {
    margin: 0 0 6px 0;
    font-size: 1.05rem;
    color: #1e293b;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .hotkey-input {
    width: 100%;
    border: none;
    background: #f8fafc;
    color: #64748b;
    font-size: 0.8rem;
    padding: 4px 8px;
    border-radius: 6px;
    margin-bottom: 15px;
    cursor: text;
    outline: none;
  }
  .hotkey-input:focus {
    background: #ebf0f5;
  }

  .card-actions {
    display: flex;
    gap: 10px;
    margin-top: auto;
  }

  .play-btn {
    flex-grow: 1;
    border: none;
    padding: 8px;
    border-radius: 8px;
    font-weight: 600;
    cursor: pointer;
    background: #fdba74;
    color: #c2410c;
    transition: all 0.2s;
  }
  .play-btn.play:hover {
    background: #f97316;
    color: white;
  }
  .play-btn.stop {
    background: #fca5a5;
    color: #991b1b;
  }
  .play-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .menu-btn {
    width: 36px;
    background: #f1f5f9;
    border: none;
    border-radius: 8px;
    cursor: pointer;
    color: #64748b;
  }
  .menu-btn:hover {
    background: #e2e8f0;
  }

  /* --- Bottom Player --- */
  .bottom-player {
    height: 80px;
    flex-shrink: 0;
    background: #ffffff;
    border-top: 1px solid rgba(0, 0, 0, 0.05);
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0 30px;
    z-index: 10;
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
    font-size: 0.95rem;
    color: #1e293b;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .now-playing span {
    font-size: 0.8rem;
    color: #10b981;
  }
  .now-playing.empty span {
    color: #94a3b8;
  }

  .player-controls {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    flex: 2;
    min-width: 200px;
  }

  .ctrl-btn {
    width: 40px;
    height: 40px;
    border-radius: 50%;
    border: none;
    display: flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
    font-size: 16px;
    transition: all 0.2s;
  }
  .play-circle {
    background: #3b82f6;
    color: white;
  }
  .stop-circle {
    background: #ef4444;
    color: white;
  }
  .ctrl-btn:disabled {
    background: #cbd5e1;
    cursor: not-allowed;
  }

  .progress-bar-container {
    width: 100%;
    display: flex;
    align-items: center;
  }

  .progress-bar {
    width: 100%;
    height: 6px;
    -webkit-appearance: none;
    appearance: none;
    background: #e2e8f0;
    border-radius: 3px;
    outline: none;
  }

  .progress-bar::-webkit-slider-thumb {
    -webkit-appearance: none;
    appearance: none;
    width: 12px;
    height: 12px;
    border-radius: 50%;
    background: #3b82f6;
    cursor: pointer;
  }

  .player-settings {
    display: flex;
    justify-content: flex-end;
    flex: 1;
    min-width: 120px;
  }

  .vol-control {
    display: flex;
    align-items: center;
    gap: 10px;
    font-size: 0.9rem;
    color: #64748b;
  }

  .mini-slider {
    width: 80px;
  }

  /* --- Settings & Routing --- */
  .settings-panel {
    background: #fff;
    border-radius: 16px;
    padding: 30px;
    box-shadow: 0 4px 15px rgba(0, 0, 0, 0.02);
  }

  .form-group {
    margin-bottom: 25px;
  }

  .form-group label {
    display: block;
    margin-bottom: 8px;
    font-weight: 600;
    color: #334155;
  }

  select {
    width: 100%;
    padding: 12px;
    border-radius: 8px;
    border: 1px solid #e2e8f0;
    background: #f8fafc;
    font-family: inherit;
    font-size: 0.95rem;
    outline: none;
  }

  .sliders {
    display: flex;
    flex-direction: column;
    gap: 20px;
    margin-top: 30px;
  }

  .slider-box label {
    font-weight: 500;
    margin-bottom: 5px;
    display: block;
  }

  input[type="range"] {
    width: 100%;
  }

  .error-banner {
    background: #fee2e2;
    color: #b91c1c;
    padding: 15px;
    border-radius: 10px;
    margin-bottom: 20px;
    border: 1px solid #fecaca;
  }

  /* --- Responsive Design --- */
  @media (max-width: 900px) {
    .main-content-wrapper {
      flex-direction: column;
    }

    .sidebar {
      width: 100%;
      flex-direction: row;
      flex-wrap: wrap;
      padding: 15px;
      gap: 15px;
      align-items: center;
      border-right: none;
      border-bottom: 1px solid rgba(0, 0, 0, 0.05);
    }

    .logo {
      margin-bottom: 0;
    }

    .nav-menu {
      flex-direction: row;
      flex-wrap: wrap;
    }

    .nav-item {
      width: auto;
      padding: 8px 12px;
    }

    .bridge-status {
      margin-bottom: 0;
      margin-left: auto;
    }

    .toggle-btn {
      width: auto;
    }

    .content-area {
      padding: 20px;
    }
  }

  @media (max-width: 600px) {
    .bottom-player {
      padding: 15px;
      height: auto;
      flex-wrap: wrap;
      justify-content: center;
    }

    .player-info {
      width: 100%;
      text-align: center;
      justify-content: center;
    }

    .player-controls {
      width: 100%;
    }

    .player-settings {
      display: none; /* Hide volume on tiny screens to save space */
    }
  }
  /* ----- Help/Guide Panel ----- */
  .help-panel {
    max-width: 800px;
  }

  .guide-card {
    background: white;
    padding: 24px;
    border-radius: 12px;
    margin-bottom: 20px;
    border: 1px solid #e2e8f0;
    box-shadow: 0 4px 6px -1px rgb(0 0 0 / 0.05);
    position: relative;
    overflow: hidden;
  }

  .guide-step {
    position: absolute;
    top: -10px;
    right: -20px;
    font-size: 8rem;
    font-weight: 800;
    color: #f1f5f9;
    z-index: 0;
    line-height: 1;
    user-select: none;
    pointer-events: none;
  }

  .guide-card h3 {
    margin-top: 0;
    color: #1e293b;
    font-size: 1.25rem;
    position: relative;
    z-index: 1;
  }

  .guide-card p,
  .guide-card ul {
    color: #64748b;
    font-size: 1rem;
    line-height: 1.6;
    position: relative;
    z-index: 1;
  }

  .guide-card ul {
    padding-left: 20px;
    margin-bottom: 0;
  }

  .guide-card li {
    margin-bottom: 8px;
  }

  strong {
    color: #334155;
    font-weight: 600;
  }

  .download-link {
    display: inline-block;
    margin-top: 15px;
    padding: 10px 20px;
    background: #0ea5e9;
    color: white;
    text-decoration: none;
    border-radius: 8px;
    font-weight: 600;
    transition: background 0.2s;
    position: relative;
    z-index: 1;
  }

  .download-link:hover {
    background: #0284c7;
  }

  .note {
    font-size: 0.85rem;
    color: #94a3b8 !important;
    margin-top: 10px;
    font-style: italic;
  }

  .success-box {
    margin-top: 20px;
    padding: 16px;
    background: #f0fdf4;
    border: 1px solid #bbf7d0;
    border-radius: 8px;
    color: #166534;
    position: relative;
    z-index: 1;
  }

  /* --- DSP Blocks --- */
  .dsp-block {
    background: #f8fafc;
    border: 1px solid #e2e8f0;
    border-radius: 12px;
    padding: 16px;
    margin-bottom: 15px;
  }
  .dsp-toggle {
    display: flex;
    align-items: center;
    gap: 10px;
    cursor: pointer;
    font-size: 1.05rem;
    color: #1e293b;
    margin-bottom: 0;
  }
  .dsp-sliders {
    margin-top: 20px !important;
    padding-top: 20px;
    border-top: 1px dashed #cbd5e1;
  }

  .inline-btn {
    padding: 4px 10px;
    font-size: 0.85rem;
    margin: 0 5px;
  }
</style>
