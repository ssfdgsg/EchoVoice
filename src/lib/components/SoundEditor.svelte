<script lang="ts">
  import { untrack } from "svelte";
  import type { SoundItem } from "$lib/types";
  import { formatTime, prettyShortcut } from "$lib/types";

  interface Props {
    sound: SoundItem;
    categories: string[];
    labels: Record<string, string>;
    onPatch: (id: string, patch: Partial<SoundItem>) => void;
    onBindHotkey: (sound: SoundItem, event: KeyboardEvent) => void;
    onIsMusic: (sound: SoundItem, isMusic: boolean) => void;
    onRemove: (sound: SoundItem) => void;
    onClose: () => void;
  }

  let {
    sound,
    categories,
    labels,
    onPatch,
    onBindHotkey,
    onIsMusic,
    onRemove,
    onClose,
  }: Props = $props();

  // Local copies so dragging a slider never round-trips through the backend per pixel.
  // `untrack` keeps the "captures the initial value" warning away: these are deliberately
  // local editable copies, re-synced from the backend by the effect below.
  let name = $state(untrack(() => sound.name));
  let volume = $state(untrack(() => sound.volume));
  let startMs = $state(untrack(() => sound.start_offset_ms));
  let endMs = $state<string>(untrack(() => sound.end_offset_ms?.toString() ?? ""));

  $effect(() => {
    name = sound.name;
    volume = sound.volume;
    startMs = sound.start_offset_ms;
    endMs = sound.end_offset_ms?.toString() ?? "";
  });

  function commitName() {
    const trimmed = name.trim();
    if (trimmed && trimmed !== sound.name) {
      onPatch(sound.id, { name: trimmed });
    }
  }

  function commitTrim() {
    const start = Math.max(0, Math.round(Number(startMs) || 0));
    const raw = endMs.trim();
    const end = raw === "" ? null : Math.max(0, Math.round(Number(raw) || 0));
    if (start !== sound.start_offset_ms || end !== sound.end_offset_ms) {
      onPatch(sound.id, { start_offset_ms: start, end_offset_ms: end });
    }
  }
</script>

<aside class="editor">
  <header class="editor-head">
    <h2>{labels.title}</h2>
    <button class="icon-btn" onclick={onClose} aria-label={labels.save}>✕</button>
  </header>

  <div class="editor-body">
    <label class="field">
      <span>{labels.name}</span>
      <input
        type="text"
        bind:value={name}
        onblur={commitName}
        onkeydown={(event) => {
          if (event.key === "Enter") event.currentTarget.blur();
        }}
      />
    </label>

    <label class="field">
      <span>{labels.category}</span>
      <input
        type="text"
        list="category-options"
        value={sound.category}
        onchange={(event) =>
          onPatch(sound.id, { category: event.currentTarget.value })}
      />
      <datalist id="category-options">
        {#each categories as category}
          <option value={category}></option>
        {/each}
      </datalist>
    </label>

    <label class="field">
      <span>{labels.volume} — {Math.round(volume * 100)}%</span>
      <input
        type="range"
        min="0"
        max="2"
        step="0.05"
        bind:value={volume}
        oninput={() => onPatch(sound.id, { volume })}
        onchange={() => onPatch(sound.id, { volume })}
      />
    </label>

    <div class="checks">
      <label class="check">
        <input
          type="checkbox"
          checked={sound.loop_playback}
          onchange={(event) =>
            onPatch(sound.id, { loop_playback: event.currentTarget.checked })}
        />
        <span>{labels.loop}</span>
      </label>
      <label class="check">
        <input
          type="checkbox"
          checked={sound.retrigger}
          onchange={(event) =>
            onPatch(sound.id, { retrigger: event.currentTarget.checked })}
        />
        <span>{labels.retrigger}</span>
      </label>
      <label class="check">
        <input
          type="checkbox"
          checked={sound.hold_to_play}
          onchange={(event) =>
            onPatch(sound.id, { hold_to_play: event.currentTarget.checked })}
        />
        <span>{labels.hold}</span>
      </label>
      <label class="check">
        <input
          type="checkbox"
          checked={sound.favorite}
          onchange={(event) =>
            onPatch(sound.id, { favorite: event.currentTarget.checked })}
        />
        <span>{labels.favorite}</span>
      </label>
      <label class="check">
        <input
          type="checkbox"
          checked={sound.is_music}
          onchange={(event) => onIsMusic(sound, event.currentTarget.checked)}
        />
        <span>{labels.useAsMusic}</span>
      </label>
    </div>

    <fieldset class="trim">
      <legend>{labels.trim}</legend>
      <div class="trim-row">
        <label class="field">
          <span>{labels.trimStart}</span>
          <input
            type="number"
            min="0"
            step="10"
            bind:value={startMs}
            onchange={commitTrim}
          />
        </label>
        <label class="field">
          <span>{labels.trimEnd}</span>
          <input
            type="number"
            min="0"
            step="10"
            placeholder="∞"
            bind:value={endMs}
            onchange={commitTrim}
          />
        </label>
      </div>
      <p class="hint">{labels.trimHint}</p>
      <p class="hint">
        {labels.duration}:
        <strong>{formatTime(sound.duration_ms ?? 0)}</strong>
      </p>
    </fieldset>

    <label class="field">
      <span>{labels.hotkey}</span>
      <input
        type="text"
        class="hotkey-input"
        class:bound={!!sound.shortcut}
        placeholder={labels.bindHotkey}
        value={sound.shortcut ? prettyShortcut(sound.shortcut) : ""}
        readonly
        onkeydown={(event) => onBindHotkey(sound, event)}
      />
      <p class="hint">{labels.hotkeyHint}</p>
    </label>

    <div class="field">
      <span>{labels.file}</span>
      <code class="path" title={sound.path}>{sound.path}</code>
    </div>
  </div>

  <footer class="editor-foot">
    <button class="btn-primary" onclick={onClose}>{labels.save}</button>
    <button class="btn-danger" onclick={() => onRemove(sound)}>
      {labels.delete}
    </button>
  </footer>
</aside>

<style>
  .editor {
    width: 320px;
    flex-shrink: 0;
    background: var(--panel-bg);
    border-left: 1px solid var(--card-border);
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  .editor-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 16px 18px;
    border-bottom: 1px solid var(--card-border);
  }
  .editor-head h2 {
    margin: 0;
    font-size: 1rem;
    color: var(--text-strong);
  }
  .editor-body {
    flex: 1;
    overflow-y: auto;
    padding: 18px;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  .editor-foot {
    display: flex;
    gap: 10px;
    padding: 14px 18px;
    border-top: 1px solid var(--card-border);
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .field > span {
    font-size: 0.8rem;
    font-weight: 600;
    color: var(--text-muted);
  }
  .field input[type="text"],
  .field input[type="number"],
  .field input[type="text"] {
    padding: 9px 11px;
    border-radius: 9px;
    border: 1px solid var(--input-border);
    background: var(--input-bg);
    color: var(--text-strong);
    font: inherit;
    font-size: 0.88rem;
    outline: none;
  }
  .field input:focus {
    border-color: var(--accent);
  }

  .checks {
    display: flex;
    flex-direction: column;
    gap: 9px;
  }
  .check {
    display: flex;
    align-items: flex-start;
    gap: 9px;
    font-size: 0.84rem;
    color: var(--text-strong);
    cursor: pointer;
    line-height: 1.35;
  }
  .check input {
    margin-top: 2px;
    accent-color: var(--accent);
  }

  .trim {
    border: 1px solid var(--card-border);
    border-radius: 11px;
    padding: 12px 14px;
    margin: 0;
  }
  .trim legend {
    padding: 0 6px;
    font-size: 0.78rem;
    font-weight: 700;
    color: var(--text-muted);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .trim-row {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 10px;
  }
  .hint {
    margin: 6px 0 0;
    font-size: 0.75rem;
    color: var(--text-muted);
    line-height: 1.45;
  }

  .hotkey-input {
    padding: 9px 11px;
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

  .path {
    display: block;
    padding: 8px 10px;
    border-radius: 8px;
    background: var(--input-bg);
    font-size: 0.72rem;
    color: var(--text-muted);
    word-break: break-all;
  }

  .btn-primary {
    flex: 1;
    border: none;
    padding: 10px;
    border-radius: 10px;
    background: var(--accent);
    color: #fff;
    font-weight: 700;
    cursor: pointer;
  }
  .btn-danger {
    border: 1px solid color-mix(in srgb, var(--danger) 45%, transparent);
    padding: 10px 14px;
    border-radius: 10px;
    background: transparent;
    color: var(--danger);
    font-weight: 700;
    cursor: pointer;
  }
  .icon-btn {
    border: none;
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
    font-size: 1rem;
    padding: 4px 8px;
    border-radius: 8px;
  }
  .icon-btn:hover {
    background: var(--btn-ghost-bg-hover);
    color: var(--text-strong);
  }
</style>
