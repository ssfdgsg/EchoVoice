<script lang="ts">
  import type { SoundItem } from "$lib/types";
  import { formatTime, prettyShortcut } from "$lib/types";

  interface Props {
    sound: SoundItem;
    /** Progress 0..1 of the primary voice of this sound, if any. */
    progress: number;
    playing: boolean;
    bridgeRunning: boolean;
    labels: {
      volume: string;
      loop: string;
      hold: string;
      remove: string;
      bindHotkey: string;
      music: string;
      duration: string;
      edit: string;
      play: string;
      stop: string;
    };
    onPlay: (sound: SoundItem) => void;
    onStop: (sound: SoundItem) => void;
    onToggleFavorite: (sound: SoundItem) => void;
    onVolume: (sound: SoundItem, volume: number) => void;
    onEdit: (sound: SoundItem) => void;
    onRemove: (sound: SoundItem) => void;
    onBindHotkey: (sound: SoundItem, event: KeyboardEvent) => void;
  }

  let {
    sound,
    progress,
    playing,
    bridgeRunning,
    labels,
    onPlay,
    onStop,
    onToggleFavorite,
    onVolume,
    onEdit,
    onRemove,
    onBindHotkey,
  }: Props = $props();

  let hotkeyInput = $state<HTMLInputElement | null>(null);
  // Local draft so dragging the slider never round-trips through the backend per pixel.
  let draftVolume = $state<number | null>(null);
  let shownVolume = $derived(draftVolume ?? sound.volume);
</script>

<article
  class="sound-card"
  class:playing
  class:favorite={sound.favorite}
  data-sound-id={sound.id}
>
  <header class="card-head">
    <button
      class="star"
      class:on={sound.favorite}
      title={labels.edit}
      aria-label={labels.edit}
      onclick={() => onToggleFavorite(sound)}
    >
      {sound.favorite ? "★" : "☆"}
    </button>
    <div class="headings">
      <h3 title={sound.name}>{sound.name}</h3>
      <span class="meta">
        {#if sound.duration_ms != null}
          <span class="duration">{formatTime(sound.duration_ms)}</span>
        {/if}
        {#if sound.loop_playback}
          <span class="badge loop">{labels.loop}</span>
        {/if}
        {#if sound.hold_to_play}
          <span class="badge hold">{labels.hold}</span>
        {/if}
        {#if sound.is_music}
          <span class="badge music" title={labels.music}>🎵</span>
        {/if}
      </span>
    </div>
  </header>

  <div class="progress-track" aria-hidden="true">
    <div
      class="progress-fill"
      class:indeterminate={playing && sound.loop_playback}
      style="width: {sound.loop_playback && playing ? 100 : Math.round(progress * 100)}%"
    ></div>
  </div>

  <div class="card-body">
    <input
      bind:this={hotkeyInput}
      type="text"
      class="hotkey-input"
      class:bound={!!sound.shortcut}
      placeholder={labels.bindHotkey}
      value={sound.shortcut ? prettyShortcut(sound.shortcut) : ""}
      readonly
      onkeydown={(event) => onBindHotkey(sound, event)}
    />

    <label class="volume">
      <span class="sr-only">{labels.volume}</span>
      <input
        type="range"
        min="0"
        max="2"
        step="0.05"
        value={sound.volume}
        oninput={(event) =>
          onVolume(sound, parseFloat(event.currentTarget.value))}
      />
      <span class="volume-value">{Math.round(sound.volume * 100)}%</span>
    </label>
  </div>

  <footer class="card-foot">
    {#if playing}
      <button class="play-btn stop" onclick={() => onStop(sound)}>
        ■ {labels.stop}
      </button>
    {:else}
      <button
        class="play-btn play"
        disabled={!bridgeRunning}
        onclick={() => onPlay(sound)}
      >
        ▶ {labels.play}
      </button>
    {/if}
    <button class="icon-btn" title={labels.edit} onclick={() => onEdit(sound)}>
      ⚙
    </button>
    <button
      class="icon-btn danger"
      title={labels.remove}
      onclick={() => onRemove(sound)}
    >
      🗑
    </button>
  </footer>
</article>

<style>
  .sound-card {
    background: var(--card-bg);
    border: 1px solid var(--card-border);
    border-radius: 14px;
    padding: 14px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    box-shadow: var(--card-shadow);
    transition:
      transform 0.15s ease,
      box-shadow 0.15s ease,
      border-color 0.15s ease;
  }
  .sound-card:hover {
    transform: translateY(-2px);
    box-shadow: var(--card-shadow-hover);
  }
  .sound-card.playing {
    border-color: var(--accent);
    box-shadow: 0 0 0 1px var(--accent), var(--card-shadow-hover);
  }

  .card-head {
    display: flex;
    align-items: flex-start;
    gap: 8px;
  }
  .headings {
    min-width: 0;
    flex: 1;
  }
  .card-head h3 {
    margin: 0;
    font-size: 0.98rem;
    color: var(--text-strong);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .meta {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-top: 3px;
    font-size: 0.72rem;
    color: var(--text-muted);
  }
  .badge {
    padding: 1px 6px;
    border-radius: 999px;
    font-weight: 700;
    letter-spacing: 0.03em;
    font-size: 0.62rem;
  }
  .badge.loop {
    background: color-mix(in srgb, var(--accent) 18%, transparent);
    color: var(--accent);
  }
  .badge.hold {
    background: color-mix(in srgb, var(--warn) 20%, transparent);
    color: var(--warn);
  }
  .badge.music {
    background: color-mix(in srgb, #22c55e 20%, transparent);
  }

  .star {
    border: none;
    background: transparent;
    cursor: pointer;
    font-size: 1.1rem;
    line-height: 1;
    color: var(--text-faint);
    padding: 2px;
  }
  .star.on {
    color: #f5b301;
  }

  .progress-track {
    height: 4px;
    border-radius: 999px;
    background: var(--track-bg);
    overflow: hidden;
  }
  .progress-fill {
    height: 100%;
    background: var(--accent);
    border-radius: inherit;
    transition: width 0.12s linear;
  }
  .progress-fill.indeterminate {
    background: linear-gradient(
      90deg,
      var(--accent),
      color-mix(in srgb, var(--accent) 35%, transparent)
    );
    animation: pulse 1.4s ease-in-out infinite;
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

  .card-body {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .hotkey-input {
    width: 100%;
    box-sizing: border-box;
    border: 1px dashed var(--input-border);
    background: var(--input-bg);
    color: var(--text-strong);
    font-size: 0.78rem;
    font-weight: 600;
    padding: 7px 9px;
    border-radius: 8px;
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
    background: var(--input-bg-focus);
  }

  .volume {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .volume input[type="range"] {
    flex: 1;
    min-width: 0;
  }
  .volume-value {
    font-size: 0.72rem;
    color: var(--text-muted);
    width: 3.2em;
    text-align: right;
    font-variant-numeric: tabular-nums;
  }

  .card-foot {
    display: flex;
    gap: 6px;
    margin-top: auto;
  }
  .play-btn {
    flex: 1;
    border: none;
    padding: 8px;
    border-radius: 9px;
    font-weight: 700;
    font-size: 0.82rem;
    cursor: pointer;
    transition: filter 0.15s ease;
  }
  .play-btn.play {
    background: var(--btn-play-bg);
    color: var(--btn-play-fg);
  }
  .play-btn.play:hover:not(:disabled) {
    filter: brightness(1.08);
  }
  .play-btn.stop {
    background: var(--btn-stop-bg);
    color: var(--btn-stop-fg);
  }
  .play-btn:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }
  .icon-btn {
    width: 34px;
    border: none;
    border-radius: 9px;
    background: var(--btn-ghost-bg);
    color: var(--text-muted);
    cursor: pointer;
    font-size: 0.9rem;
  }
  .icon-btn:hover {
    background: var(--btn-ghost-bg-hover);
    color: var(--text-strong);
  }
  .icon-btn.danger:hover {
    background: color-mix(in srgb, var(--danger) 18%, transparent);
    color: var(--danger);
  }

  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    padding: 0;
    margin: -1px;
    overflow: hidden;
    clip: rect(0, 0, 0, 0);
    white-space: nowrap;
    border: 0;
  }
</style>
