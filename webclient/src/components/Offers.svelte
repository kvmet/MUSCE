<script lang="ts">
  import type { Offer } from "../lib/bindings/Offer";

  interface Props {
    subject: string;
    offers: Offer[];
    onAct: (offer: Offer) => void;
  }
  let { subject, offers, onAct }: Props = $props();
</script>

<section aria-labelledby="offers-heading">
  <h2 id="offers-heading">Do to {subject}</h2>
  <ul class="offers">
    {#each offers as offer, index (`${offer.affordance}-${index}`)}
      <li>
        {#if offer.status.kind === "available"}
          <button class="offer available" onclick={() => onAct(offer)}>{offer.display_name}</button>
        {:else if offer.status.kind === "needs"}
          <button class="offer needs" onclick={() => onAct(offer)}>
            {offer.display_name}<span class="hint">choose {offer.status.parameters.join(", ")}…</span>
          </button>
        {:else}
          <button class="offer vetoed" disabled aria-disabled="true" title={offer.status.reason}>
            {offer.display_name}<span class="hint">{offer.status.reason}</span>
          </button>
        {/if}
      </li>
    {/each}
  </ul>
</section>

<style>
  h2 {
    font-size: 0.9rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    opacity: 0.6;
    margin: 0 0 0.5rem;
  }
  .offers {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem;
  }
  .offer {
    display: inline-flex;
    gap: 0.4rem;
    align-items: baseline;
    font: inherit;
    color: inherit;
    padding: 0.35rem 0.6rem;
    border-radius: 6px;
    border: 1px solid color-mix(in srgb, currentColor 30%, transparent);
    background: none;
    cursor: pointer;
  }
  .offer.available:hover,
  .offer.needs:hover {
    background: color-mix(in srgb, currentColor 12%, transparent);
  }
  .offer.vetoed {
    opacity: 0.45;
    cursor: not-allowed;
  }
  .hint {
    font-size: 0.75em;
    opacity: 0.7;
  }
</style>
