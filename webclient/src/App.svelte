<script lang="ts">
  import { MockConn, type Conn } from "./lib/connection";
  import type { Offer } from "./lib/protocol";
  import EntityTree from "./components/EntityTree.svelte";
  import Offers from "./components/Offers.svelte";
  import Log from "./components/Log.svelte";
  import CommandBar from "./components/CommandBar.svelte";

  const conn: Conn = new MockConn();

  // Bumped after every mutation so the derived reads recompute against the world
  // as it now is (late-bound, like the server would be).
  let version = $state(0);
  let selected = $state<number | null>(null);
  // A pending act awaiting a second role (the `put` object sub-pick).
  let pending = $state<{ offer: Offer; clicked: number } | null>(null);
  let lines = $state<string[]>(["Pointing graybox. Click a thing to see what you can do to it."]);

  const snap = $derived.by(() => {
    version;
    return conn.snapshot();
  });
  const offers = $derived.by(() => {
    version;
    return selected === null ? [] : conn.offersOn(selected);
  });
  const inventory = $derived.by(() => {
    version;
    return conn.inventory();
  });

  function say(...out: string[]) {
    lines = [...lines, ...out];
  }

  function select(id: number) {
    selected = id;
    pending = null;
  }

  function act(offer: Offer) {
    if (selected === null) return;
    if (offer.status.kind === "needsRole") {
      // Open the sub-pick instead of acting: the client resolves the missing role.
      pending = { offer, clicked: selected };
      return;
    }
    say(conn.perform(offer.name, conn.frameFor(offer.name, selected)));
    version++;
  }

  // Fill the pending act's missing role with `objectId` and run it.
  function pick(objectId: number) {
    if (!pending) return;
    const frame = { ...conn.frameFor(pending.offer.name, pending.clicked), object: objectId };
    say(conn.perform(pending.offer.name, frame));
    pending = null;
    version++;
  }

  const subjectName = $derived(selected === null ? "" : conn.name(selected));
</script>

<main>
  <header>
    <h1>MUSCE</h1>
    <p class="tag">thin pointing client (mock connection)</p>
  </header>

  <div class="panes">
    <nav aria-label="Here" class="tree">
      <h2>Here</h2>
      <ul>
        <EntityTree id={snap.root} {snap} {selected} onSelect={select} />
      </ul>
    </nav>

    <div class="detail">
      {#if selected === null}
        <p class="empty">Nothing selected.</p>
      {:else}
        <Offers subject={subjectName} {offers} onAct={act} />
        {#if pending}
          <section class="picker" aria-labelledby="picker-heading">
            <h2 id="picker-heading">
              {pending.offer.status.kind === "needsRole" ? pending.offer.status.role : "object"} for
              {pending.offer.name}
            </h2>
            {#if inventory.length}
              <ul>
                {#each inventory as item (item.id)}
                  <li><button onclick={() => pick(item.id)}>{item.name}</button></li>
                {/each}
              </ul>
            {:else}
              <p class="empty">You are carrying nothing.</p>
            {/if}
            <button class="cancel" onclick={() => (pending = null)}>Cancel</button>
          </section>
        {/if}
      {/if}
    </div>

    <div class="stream">
      <Log {lines} />
      <CommandBar onSubmit={(line) => say(...conn.send(line))} />
    </div>
  </div>
</main>

<style>
  main {
    max-width: 1100px;
    margin: 0 auto;
    padding: 1rem;
    display: flex;
    flex-direction: column;
    gap: 1rem;
    height: 100vh;
    box-sizing: border-box;
  }
  header {
    display: flex;
    align-items: baseline;
    gap: 0.75rem;
  }
  h1 {
    margin: 0;
    font-size: 1.25rem;
  }
  .tag {
    margin: 0;
    opacity: 0.55;
    font-size: 0.85rem;
  }
  .panes {
    display: grid;
    grid-template-columns: minmax(200px, 1fr) minmax(220px, 1fr) minmax(260px, 1.4fr);
    gap: 1rem;
    flex: 1;
    min-height: 0;
  }
  .tree,
  .detail,
  .stream {
    min-height: 0;
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }
  .stream {
    min-width: 0;
  }
  h2 {
    font-size: 0.9rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    opacity: 0.6;
    margin: 0;
  }
  .tree ul {
    list-style: none;
    margin: 0;
    padding: 0;
    overflow-y: auto;
  }
  .picker ul {
    list-style: none;
    margin: 0.5rem 0;
    padding: 0;
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem;
  }
  .picker button,
  .cancel {
    font: inherit;
    color: inherit;
    padding: 0.35rem 0.6rem;
    border-radius: 6px;
    border: 1px solid color-mix(in srgb, currentColor 30%, transparent);
    background: none;
    cursor: pointer;
  }
  .empty {
    opacity: 0.55;
  }
  /* Single-column on narrow viewports (mobile / PWA). */
  @media (max-width: 720px) {
    main {
      height: auto;
    }
    .panes {
      grid-template-columns: 1fr;
    }
  }
</style>
