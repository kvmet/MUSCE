<script lang="ts">
  import type { Entity } from "../lib/bindings/Entity";
  import type { Snapshot } from "../lib/snapshot";
  import Self from "./EntityTree.svelte";

  interface Props {
    id: string;
    snap: Snapshot;
    selected: string | null;
    onSelect: (id: string) => void;
  }
  let { id, snap, selected, onSelect }: Props = $props();

  const entity = $derived(snap.entities.get(id) as Entity | undefined);
  const children = $derived(entity?.contents ?? []);
</script>

{#if entity}
  <li>
    <button
      class="node"
      class:selected={selected === id}
      aria-pressed={selected === id}
      onclick={() => onSelect(id)}
    >
      {entity.name}
      {#if entity.kinds.length}<span class="kinds">{entity.kinds.join(" ")}</span>{/if}
    </button>
    {#if children.length}
      <ul>
        {#each children as childId (childId)}
          <Self id={childId} {snap} {selected} {onSelect} />
        {/each}
      </ul>
    {/if}
  </li>
{/if}

<style>
  ul {
    list-style: none;
    margin: 0;
    padding-left: 1rem;
  }
  .node {
    display: inline-flex;
    gap: 0.5rem;
    align-items: baseline;
    width: 100%;
    text-align: left;
    background: none;
    border: 1px solid transparent;
    border-radius: 4px;
    padding: 0.2rem 0.4rem;
    color: inherit;
    font: inherit;
    cursor: pointer;
  }
  .node:hover {
    background: color-mix(in srgb, currentColor 8%, transparent);
  }
  .node.selected {
    border-color: color-mix(in srgb, currentColor 40%, transparent);
    background: color-mix(in srgb, currentColor 12%, transparent);
  }
  .kinds {
    font-size: 0.75em;
    opacity: 0.55;
  }
</style>
