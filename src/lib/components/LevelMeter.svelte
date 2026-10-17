<script lang="ts">
  import { levelPercent } from "$lib/types";

  interface Props {
    level: number;
    label: string;
    muted?: boolean;
    compact?: boolean;
  }

  let { level, label, muted = false, compact = false }: Props = $props();

  let percent = $derived(levelPercent(level));
</script>

<div class="meter" class:compact class:muted title="{label}: {Math.round(percent)}%">
  <div class="bar">
    <div class="fill" style="width: {percent}%"></div>
  </div>
  {#if !compact}
    <span class="caption">{muted ? "🔇" : "🎙"} {label}</span>
  {/if}
</div>

<style>
  .meter {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 0.76rem;
    color: var(--text-muted);
  }
  .bar {
    position: relative;
    flex: 1;
    height: 6px;
    min-width: 40px;
    border-radius: 999px;
    background: var(--track-bg);
    overflow: hidden;
  }
  .fill {
    height: 100%;
    border-radius: inherit;
    background: linear-gradient(90deg, #22c55e, #eab308 78%, #ef4444);
    transition: width 0.08s linear;
  }
  .meter.muted .fill {
    background: var(--text-faint);
  }
  .caption {
    white-space: nowrap;
  }
</style>
