<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { onMount } from "svelte";

  interface AudioDevice {
    id: string;
    name: string;
  }

  let inputDevices = $state<AudioDevice[]>([]);
  let outputDevices = $state<AudioDevice[]>([]);

  let selectedInput = $state<string>("");
  let selectedOutput = $state<string>("");

  let isBridgeRunning = $state(false);
  let errorMsg = $state("");

  let testSoundPath = $state("");
  let micVolume = $state(1.0);
  let fxVolume = $state(1.0);

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

  onMount(() => {
    loadDevices();
  });

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
    } catch (err) {
      console.error("Failed to stop bridge", err);
      errorMsg = String(err);
    }
  }

  async function testPlaySound() {
    if (!testSoundPath) {
      errorMsg = "Please enter an absolute path to a sound file.";
      return;
    }
    try {
      errorMsg = "";
      await invoke("play_sound", { filePath: testSoundPath });
    } catch (err) {
      console.error("Failed to play sound", err);
      errorMsg = String(err);
    }
  }
</script>

<main class="container">
  <div class="card">
    <h1>EchoVoice</h1>
    <p class="subtitle">Direct Audio Bridge</p>

    <!-- Error Banner -->
    {#if errorMsg}
      <div class="error-banner">
        {errorMsg}
      </div>
    {/if}

    <div class="form-group">
      <label for="input">Input (Microphone)</label>
      <select id="input" bind:value={selectedInput} disabled={isBridgeRunning}>
        {#each inputDevices as device}
          <option value={device.id}>{device.name}</option>
        {/each}
      </select>
    </div>

    <div class="form-group">
      <label for="output">Output (Speakers)</label>
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
        <button class="btn-start" onclick={startBridge}> Start Bridge </button>
      {/if}
    </div>

    {#if isBridgeRunning}
      <hr style="border-color: rgba(255,255,255,0.1); margin: 30px 0;" />

      <p class="subtitle" style="margin-bottom: 10px;">Volume Controls</p>

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

      <hr style="border-color: rgba(255,255,255,0.1); margin: 30px 0;" />

      <p class="subtitle" style="margin-bottom: 10px;">Soundboard MVP Test</p>
      <div class="form-group">
        <label for="soundpath">Absolute Path to Audio File (.wav/.mp3)</label>
        <input
          id="soundpath"
          type="text"
          bind:value={testSoundPath}
          placeholder="C:\Users\Public\Music\test.wav"
        />
      </div>
      <button class="btn-start" style="width: 100%;" onclick={testPlaySound}>
        Mix & Play Sound
      </button>
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
    padding: 40px;
    width: 100%;
    max-width: 500px;
    box-shadow: 0 25px 50px -12px rgba(0, 0, 0, 0.5);
  }

  h1 {
    margin: 0;
    font-size: 2.5rem;
    font-weight: 800;
    background: linear-gradient(to right, #60a5fa, #a78bfa);
    -webkit-background-clip: text;
    -webkit-text-fill-color: transparent;
    text-align: center;
  }

  .subtitle {
    text-align: center;
    color: #94a3b8;
    margin-top: 5px;
    margin-bottom: 30px;
  }

  .form-group {
    margin-bottom: 20px;
  }

  label {
    display: block;
    font-size: 0.9rem;
    font-weight: 600;
    color: #cbd5e1;
    margin-bottom: 8px;
  }

  select {
    width: 100%;
    padding: 12px 16px;
    border-radius: 10px;
    background: rgba(0, 0, 0, 0.2);
    border: 1px solid rgba(255, 255, 255, 0.1);
    color: #f8fafc;
    font-size: 1rem;
    outline: none;
    transition: all 0.3s ease;
    cursor: pointer;
    appearance: none;
    background-image: url("data:image/svg+xml;charset=US-ASCII,%3Csvg%20xmlns%3D%22http%3A%2F%2Fwww.w3.org%2F2000%2Fsvg%22%20width%3D%22292.4%22%20height%3D%22292.4%22%3E%3Cpath%20fill%3D%22%2394a3b8%22%20d%3D%22M287%2069.4a17.6%2017.6%200%200%200-13-5.4H18.4c-5%200-9.3%201.8-12.9%205.4A17.6%2017.6%200%200%200%200%2082.2c0%205%201.8%209.3%205.4%2012.9l128%20127.9c3.6%203.6%207.8%205.4%2012.8%205.4s9.2-1.8%2012.8-5.4L287%2095c3.5-3.5%205.4-7.8%205.4-12.8%200-5-1.9-9.2-5.5-12.8z%22%2F%3E%3C%2Fsvg%3E");
    background-repeat: no-repeat;
    background-position: right 1rem top 50%;
    background-size: 0.65rem auto;
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

  input[type="text"] {
    width: 100%;
    padding: 12px 16px;
    border-radius: 10px;
    background: rgba(0, 0, 0, 0.2);
    border: 1px solid rgba(255, 255, 255, 0.1);
    color: #f8fafc;
    font-size: 1rem;
    outline: none;
    transition: all 0.3s ease;
    box-sizing: border-box;
  }

  input[type="text"]:focus {
    border-color: #60a5fa;
    box-shadow: 0 0 0 3px rgba(96, 165, 250, 0.2);
  }

  .slide-group {
    margin-bottom: 15px;
  }

  input[type="range"] {
    width: 100%;
    accent-color: #60a5fa;
    cursor: pointer;
  }

  .actions {
    display: flex;
    gap: 15px;
    margin-top: 30px;
  }

  button {
    flex: 1;
    padding: 14px 20px;
    border-radius: 12px;
    border: none;
    font-size: 1rem;
    font-weight: 700;
    cursor: pointer;
    transition: all 0.3s ease;
  }

  button:disabled {
    opacity: 0.5;
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
    transform: translateY(-2px);
    box-shadow: 0 6px 20px rgba(139, 92, 246, 0.5);
  }

  .btn-start:active:not(:disabled) {
    transform: translateY(0);
  }

  .btn-stop {
    background: linear-gradient(to right, #ef4444, #f43f5e);
    color: white;
    box-shadow: 0 4px 14px 0 rgba(239, 68, 68, 0.39);
  }

  .btn-stop:hover:not(:disabled) {
    transform: translateY(-2px);
    box-shadow: 0 6px 20px rgba(239, 68, 68, 0.5);
  }

  .btn-stop:active:not(:disabled) {
    transform: translateY(0);
  }

  .error-banner {
    background: rgba(239, 68, 68, 0.1);
    border: 1px solid rgba(239, 68, 68, 0.2);
    color: #fca5a5;
    padding: 12px;
    border-radius: 8px;
    margin-bottom: 20px;
    font-size: 0.9rem;
    text-align: center;
  }
</style>
