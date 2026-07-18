<script lang="ts">
  interface Props {
    lines: string[];
  }
  let { lines }: Props = $props();
</script>

<!-- A live region so a screenreader announces new output as it arrives; this is
     the accessible substitute for a terminal emulator. -->
<div class="log" role="log" aria-live="polite" aria-relevant="additions">
  {#each lines as line, i (i)}
    <p>{line}</p>
  {/each}
</div>

<style>
  .log {
    flex: 1;
    overflow-y: auto;
    padding: 0.75rem;
    border: 1px solid color-mix(in srgb, currentColor 20%, transparent);
    border-radius: 6px;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 0.85rem;
    line-height: 1.5;
  }
  .log p {
    margin: 0;
    white-space: pre-wrap;
  }
</style>
