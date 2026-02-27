<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { open } from "@tauri-apps/plugin-dialog";
  import { onMount } from "svelte";

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

  let activeTab = $state<"soundboard" | "settings">("soundboard");
  let sounds = $state<SoundItem[]>([]);
  let currentPlayingId = $state<string | null>(null);

  // --- Progress Bar State ---
  let progressRatio = $state<number>(0);
  let isSeeking = $state<boolean>(false);
  let progressInterval: number | null = null;

  let inputDevices = $state<AudioDevice[]>([]);
  let outputDevices = $state<AudioDevice[]>([]);
  let selectedInput = $state<string>("");
  let selectedOutput = $state<string>("");

  let isBridgeRunning = $state(false);
  let errorMsg = $state("");

  let micVolume = $state(1.0);
  let fxVolume = $state(1.0);

  // --- Volume ---
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

  // --- Devices ---
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

  // --- Sounds ---
  async function loadSounds() {
    try {
      sounds = await invoke("get_sounds");
    } catch (err) {
      console.error("Failed to load sounds", err);
    }
  }

  async function addSoundFromFile() {
    try {
      const selected = await open({
        multiple: false,
        filters: [
          {
            name: "Audio Files",
            extensions: ["wav", "mp3", "ogg"],
          },
        ],
      });
      if (selected === null) return;

      const path = selected as string;
      const name = path.split("\\").pop()?.split("/").pop() || "Unknown Sound";
      const id = Date.now().toString();

      const newItem: SoundItem = { id, name, path, shortcut: null };
      await invoke("add_sound", { item: newItem });
      await loadSounds();
    } catch (err) {
      console.error("Failed to add sound", err);
      errorMsg = String(err);
    }
  }

  async function removeSoundItem(id: string) {
    try {
      await invoke("remove_sound", { id });
      if (currentPlayingId === id) {
        currentPlayingId = null;
      }
      await loadSounds();
    } catch (err) {
      console.error("Failed to remove sound", err);
    }
  }

  async function playSoundItem(id: string, path: string) {
    try {
      currentPlayingId = id;
      progressRatio = 0;
      await invoke("play_sound", { filePath: path });
      startProgressPolling();
    } catch (err) {
      console.error("Failed to play sound", err);
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
      console.error("Failed to stop sound", err);
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
          if (len > 0) {
            progressRatio = (pos / len) * 100;
          }
          if (pos >= len) {
            // finished playing
            currentPlayingId = null;
            stopProgressPolling();
          }
        } else {
          currentPlayingId = null;
          stopProgressPolling();
        }
      } catch (e) {
        console.error("Polling error", e);
      }
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
    } catch (err) {
      console.error("Failed to seek", err);
    }
  }

  async function updateSoundShortcut(id: string, shortcut: string | null) {
    try {
      await invoke("update_shortcut", { id, shortcut });
      await loadSounds();
    } catch (err) {
      console.error("Failed to update shortcut", err);
    }
  }

  // --- Bridge ---
  async function startBridge() {
    if (!selectedInput || !selectedOutput) {
      errorMsg = "Please select both input and output devices.";
      return;
    }

    try {
      errorMsg = "";
      await invoke("start_bridge", {
        inputDeviceId: selectedInput,
        outputDeviceId: selectedOutput,
      });
      isBridgeRunning = true;
    } catch (err) {
      console.error("Failed to start bridge", err);
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
      console.error("Failed to stop bridge", err);
      errorMsg = String(err);
    }
  }

  onMount(() => {
    loadDevices();
    loadSounds();

    listen("toggle-bridge", () => {
      if (isBridgeRunning) {
        stopBridge();
      } else {
        startBridge();
      }
    });

    listen<string>("shortcut-play", (event) => {
      currentPlayingId = event.payload;
      progressRatio = 0;
      startProgressPolling();
    });
  });
</script>

<main class="container">
  <div class="card">
    <h1>EchoVoice</h1>

    <!-- Tab Bar -->
    <div class="tab-bar">
      <button
        class="tab-btn"
        class:active={activeTab === "soundboard"}
        onclick={() => (activeTab = "soundboard")}
      >
        🎵 Soundboard
      </button>
      <button
        class="tab-btn"
        class:active={activeTab === "settings"}
        onclick={() => (activeTab = "settings")}
      >
        ⚙️ Settings
      </button>
    </div>

    <!-- Error Banner -->
    {#if errorMsg}
      <div class="error-banner">
        {errorMsg}
      </div>
    {/if}

    <!-- Bridge Status Pill -->
    <div class="bridge-status" class:on={isBridgeRunning}>
      {isBridgeRunning ? "● Bridge ON" : "○ Bridge OFF"}
    </div>

    <!-- ==================== SOUNDBOARD TAB ==================== -->
    {#if activeTab === "soundboard"}
      <div class="tab-content">
        <div class="actions" style="margin-top: 0;">
          <button
            class="btn-start"
            style="width:100%;"
            onclick={addSoundFromFile}
          >
            + Add Audio File
          </button>
          {#if currentPlayingId}
            <button
              class="btn-stop"
              style="width:100%;"
              onclick={stopCurrentSound}
            >
              ■ Stop
            </button>
          {/if}
        </div>

        {#if sounds.length === 0}
          <p class="empty-hint">
            No sounds yet. Click "Add Audio File" to import .wav or .mp3 files.
          </p>
        {:else}
          <div class="sound-list">
            {#each sounds as sound}
              <div
                class="sound-item"
                class:playing={currentPlayingId === sound.id}
              >
                <div class="sound-info">
                  <strong>{sound.name}</strong>
                  <span class="path">{sound.path}</span>
                </div>
                <div class="sound-actions">
                  <input
                    type="text"
                    class="shortcut-input"
                    placeholder="Hotkey"
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
                  {#if currentPlayingId === sound.id}
                    <button
                      class="btn-sm btn-playing"
                      onclick={stopCurrentSound}>■</button
                    >
                  {:else}
                    <button
                      class="btn-sm"
                      onclick={() => playSoundItem(sound.id, sound.path)}
                      disabled={!isBridgeRunning}>▶</button
                    >
                  {/if}
                  <button
                    class="btn-sm btn-del"
                    onclick={() => removeSoundItem(sound.id)}>✕</button
                  >
                </div>
                <!-- Progress Bar (Only visible if this item is playing) -->
                {#if currentPlayingId === sound.id}
                  <div class="progress-container">
                    <input
                      type="range"
                      min="0"
                      max="100"
                      step="0.1"
                      bind:value={progressRatio}
                      onmousedown={() => {
                        isSeeking = true;
                      }}
                      onmouseup={(e) => {
                        isSeeking = false;
                        handleSeek(e);
                      }}
                      oninput={(e) => handleSeek(e)}
                      class="progress-bar"
                    />
                  </div>
                {/if}
              </div>
            {/each}
          </div>
        {/if}
      </div>

      <!-- ==================== SETTINGS TAB ==================== -->
    {:else}
      <div class="tab-content">
        <div class="form-group">
          <label for="input">Input (Microphone)</label>
          <select
            id="input"
            bind:value={selectedInput}
            disabled={isBridgeRunning}
          >
            {#each inputDevices as device}
              <option value={device.id}>{device.name}</option>
            {/each}
          </select>
        </div>

        <div class="form-group">
          <label for="output">Output (Virtual Cable / Speakers)</label>
          <select
            id="output"
            bind:value={selectedOutput}
            disabled={isBridgeRunning}
          >
            {#each outputDevices as device}
              <option value={device.id}>{device.name}</option>
            {/each}
          </select>
        </div>

        <div class="actions">
          <button
            class="btn-refresh"
            onclick={loadDevices}
            disabled={isBridgeRunning}
          >
            Refresh Devices
          </button>

          {#if isBridgeRunning}
            <button class="btn-stop" onclick={stopBridge}> Stop Bridge </button>
          {:else}
            <button class="btn-start" onclick={startBridge}>
              Start Bridge
            </button>
          {/if}
        </div>

        {#if isBridgeRunning}
          <hr style="border-color: rgba(255,255,255,0.1); margin: 25px 0;" />
          <p class="section-title">Volume Controls</p>

          <div class="form-group slide-group">
            <label for="micVol">Mic Volume: {micVolume.toFixed(2)}x</label>
            <input
              id="micVol"
              type="range"
              min="0"
              max="2"
              step="0.1"
              bind:value={micVolume}
              oninput={updateMicVolume}
            />
          </div>

          <div class="form-group slide-group">
            <label for="fxVol">Soundboard Volume: {fxVolume.toFixed(2)}x</label>
            <input
              id="fxVol"
              type="range"
              min="0"
              max="2"
              step="0.1"
              bind:value={fxVolume}
              oninput={updateFxVolume}
            />
          </div>
        {/if}
      </div>
    {/if}
  </div>
</main>

<style>
  :root {
    font-family:
      "Inter",
      system-ui,
      -apple-system,
      sans-serif;
    font-size: 16px;
    background: linear-gradient(135deg, #1e1e24 0%, #17181f 100%);
    color: #e2e8f0;
    margin: 0;
    padding: 0;
    height: 100vh;
    overflow: hidden;
  }

  .container {
    height: 100vh;
    display: flex;
    justify-content: center;
    align-items: center;
    padding: 20px;
    box-sizing: border-box;
  }

  .card {
    background: rgba(255, 255, 255, 0.05);
    backdrop-filter: blur(10px);
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 20px;
    padding: 30px;
    width: 100%;
    max-width: 480px;
    max-height: 88vh;
    overflow-y: auto;
    box-shadow: 0 25px 50px -12px rgba(0, 0, 0, 0.5);
  }

  .card::-webkit-scrollbar {
    width: 6px;
  }
  .card::-webkit-scrollbar-thumb {
    background: rgba(255, 255, 255, 0.15);
    border-radius: 3px;
  }

  h1 {
    margin: 0;
    font-size: 2rem;
    font-weight: 800;
    background: linear-gradient(to right, #60a5fa, #a78bfa);
    background-clip: text;
    -webkit-background-clip: text;
    -webkit-text-fill-color: transparent;
    text-align: center;
  }

  /* ---------- Tab Bar ---------- */
  .tab-bar {
    display: flex;
    gap: 4px;
    margin: 16px 0 12px;
    background: rgba(0, 0, 0, 0.25);
    border-radius: 10px;
    padding: 4px;
  }

  .tab-btn {
    flex: 1;
    padding: 10px 0;
    border-radius: 8px;
    border: none;
    background: transparent;
    color: #94a3b8;
    font-size: 0.9rem;
    font-weight: 600;
    cursor: pointer;
    transition: all 0.25s ease;
  }

  .tab-btn.active {
    background: rgba(255, 255, 255, 0.1);
    color: #f1f5f9;
    box-shadow: 0 2px 8px rgba(0, 0, 0, 0.3);
  }

  .tab-btn:hover:not(.active) {
    color: #cbd5e1;
  }

  .tab-content {
    animation: fadeIn 0.2s ease;
  }

  @keyframes fadeIn {
    from {
      opacity: 0;
      transform: translateY(4px);
    }
    to {
      opacity: 1;
      transform: translateY(0);
    }
  }

  /* ---------- Bridge Status ---------- */
  .bridge-status {
    text-align: center;
    font-size: 0.8rem;
    font-weight: 700;
    padding: 6px 0;
    margin-bottom: 12px;
    border-radius: 20px;
    background: rgba(239, 68, 68, 0.15);
    color: #fca5a5;
    letter-spacing: 0.5px;
  }

  .bridge-status.on {
    background: rgba(34, 197, 94, 0.15);
    color: #86efac;
  }

  /* ---------- Forms ---------- */
  .form-group {
    margin-bottom: 16px;
  }

  label {
    display: block;
    font-size: 0.85rem;
    font-weight: 600;
    color: #cbd5e1;
    margin-bottom: 6px;
  }

  select {
    width: 100%;
    padding: 10px 14px;
    border-radius: 10px;
    background: rgba(0, 0, 0, 0.2);
    border: 1px solid rgba(255, 255, 255, 0.1);
    color: #f8fafc;
    font-size: 0.95rem;
    outline: none;
    transition: all 0.3s ease;
    -webkit-appearance: none;
    box-sizing: border-box;
  }

  select:focus {
    border-color: #60a5fa;
    box-shadow: 0 0 0 3px rgba(96, 165, 250, 0.2);
  }

  select:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  select option {
    background: #1e293b;
    color: #f8fafc;
    padding: 10px;
  }

  .slide-group {
    margin-bottom: 12px;
  }

  input[type="range"] {
    width: 100%;
    accent-color: #60a5fa;
    cursor: pointer;
  }

  .section-title {
    text-align: center;
    color: #94a3b8;
    margin: 0 0 14px;
    font-size: 0.9rem;
    font-weight: 600;
  }

  /* ---------- Actions ---------- */
  .actions {
    display: flex;
    gap: 10px;
    margin-top: 20px;
  }

  button {
    flex: 1;
    padding: 12px 16px;
    border-radius: 10px;
    border: none;
    font-size: 0.9rem;
    font-weight: 700;
    cursor: pointer;
    transition: all 0.25s ease;
  }

  button:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .btn-refresh {
    background: rgba(255, 255, 255, 0.1);
    color: #e2e8f0;
  }

  .btn-refresh:hover:not(:disabled) {
    background: rgba(255, 255, 255, 0.2);
  }

  .btn-start {
    background: linear-gradient(to right, #3b82f6, #8b5cf6);
    color: white;
    box-shadow: 0 4px 14px 0 rgba(139, 92, 246, 0.39);
  }

  .btn-start:hover:not(:disabled) {
    transform: translateY(-1px);
    box-shadow: 0 6px 20px rgba(139, 92, 246, 0.5);
  }

  .btn-stop {
    background: linear-gradient(to right, #ef4444, #f43f5e);
    color: white;
    box-shadow: 0 4px 14px 0 rgba(239, 68, 68, 0.39);
  }

  .btn-stop:hover:not(:disabled) {
    transform: translateY(-1px);
    box-shadow: 0 6px 20px rgba(239, 68, 68, 0.5);
  }

  .error-banner {
    background: rgba(239, 68, 68, 0.1);
    border: 1px solid rgba(239, 68, 68, 0.2);
    color: #fca5a5;
    padding: 10px;
    border-radius: 8px;
    margin-bottom: 12px;
    font-size: 0.85rem;
    text-align: center;
  }

  .empty-hint {
    text-align: center;
    color: #64748b;
    font-size: 0.85rem;
    margin: 30px 0;
  }

  /* ---------- Sound List ---------- */
  .sound-list {
    margin-top: 16px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .sound-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    background: rgba(0, 0, 0, 0.2);
    padding: 9px 12px;
    border-radius: 8px;
    border: 1px solid rgba(255, 255, 255, 0.05);
    transition: all 0.2s ease;
  }

  .sound-item.playing {
    border-color: rgba(96, 165, 250, 0.4);
    background: rgba(96, 165, 250, 0.08);
  }

  .sound-info {
    display: flex;
    flex-direction: column;
    overflow: hidden;
    flex: 1;
    min-width: 0;
  }

  .sound-info strong {
    font-size: 0.88rem;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .sound-info .path {
    font-size: 0.7rem;
    color: #94a3b8;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .sound-actions {
    display: flex;
    align-items: center;
    gap: 5px;
    margin-left: 8px;
    flex-shrink: 0;
  }

  .shortcut-input {
    width: 80px;
    padding: 4px 6px !important;
    font-size: 0.7rem !important;
    text-align: center;
    border-radius: 6px !important;
    background: rgba(255, 255, 255, 0.08) !important;
    border: 1px solid transparent !important;
    color: #cbd5e1 !important;
    cursor: pointer;
    outline: none !important;
    box-sizing: border-box;
  }

  .shortcut-input:focus {
    background: rgba(96, 165, 250, 0.15) !important;
    border-color: #60a5fa !important;
    box-shadow: none !important;
  }

  .btn-sm {
    padding: 5px 10px;
    font-size: 0.8rem;
    border-radius: 6px;
    background: rgba(255, 255, 255, 0.1);
    border: none;
    color: #e2e8f0;
    cursor: pointer;
    transition: all 0.2s ease;
    flex: none;
  }

  .btn-sm:hover:not(:disabled) {
    background: rgba(255, 255, 255, 0.2);
  }

  .btn-sm:disabled {
    opacity: 0.3;
    cursor: not-allowed;
  }

  .btn-playing {
    background: rgba(239, 68, 68, 0.3);
    color: #fca5a5;
  }

  .btn-playing:hover {
    background: rgba(239, 68, 68, 0.5) !important;
  }

  .btn-del {
    background: rgba(239, 68, 68, 0.15);
    color: #fca5a5;
  }

  .btn-del:hover {
    background: rgba(239, 68, 68, 0.35);
  }

  /* ---------- Progress Bar ---------- */
  .progress-container {
    padding: 0 12px 10px 12px;
    background: rgba(96, 165, 250, 0.04);
    border-bottom-left-radius: 8px;
    border-bottom-right-radius: 8px;
    margin-top: -4px; /* visually connect to the item above */
    border: 1px solid rgba(96, 165, 250, 0.2);
    border-top: none;
  }

  .progress-bar {
    width: 100%;
    height: 6px;
    -webkit-appearance: none;
    appearance: none;
    background: rgba(255, 255, 255, 0.1);
    border-radius: 3px;
    outline: none;
    cursor: pointer;
  }

  .progress-bar::-webkit-slider-thumb {
    -webkit-appearance: none;
    appearance: none;
    width: 12px;
    height: 12px;
    border-radius: 50%;
    background: #60a5fa;
    cursor: pointer;
    box-shadow: 0 0 5px rgba(96, 165, 250, 0.5);
  }
</style>
