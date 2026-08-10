<script lang="ts">
  import { MockConn, WsConn, type Conn } from "./lib/connection";
  import { toSnapshot, type Snapshot } from "./lib/snapshot";
  import type { AffordanceValue } from "./lib/bindings/AffordanceValue";
  import type { Offer } from "./lib/bindings/Offer";
  import type { ParameterBinding } from "./lib/bindings/ParameterBinding";
  import type { ServerMsg } from "./lib/bindings/ServerMsg";
  import EntityTree from "./components/EntityTree.svelte";
  import Offers from "./components/Offers.svelte";
  import Log from "./components/Log.svelte";
  import CommandBar from "./components/CommandBar.svelte";

  // `?mock` runs the in-browser stand-in; otherwise connect to the server's WebSocket
  // (its default bind, overridable at build time). The UI is identical either way.
  const env = import.meta.env as unknown as { VITE_WS_URL?: string };
  const useMock = new URLSearchParams(location.search).has("mock");
  const conn: Conn = useMock
    ? new MockConn()
    : new WsConn(env.VITE_WS_URL ?? `ws://${location.hostname}:4001`);

  // Local state, updated by the inbound stream. The server pushes no state deltas, so
  // after any act the client re-reads (see `refresh`).
  let snap = $state<Snapshot | null>(null);
  let offersReply = $state<{ clicked: string; offers: Offer[] } | null>(null);
  let selected = $state<string | null>(null);
  // A partial app-defined grounding awaiting one or more typed input picks.
  let pending = $state<{
    offer: Offer;
    parameters: string[];
    inputs: ParameterBinding[];
  } | null>(null);
  let textInput = $state("");
  let lines = $state<string[]>(["Pointing client. Click a thing to see what you can do to it."]);

  // Offers reflect the current selection only: a reply for a since-changed selection
  // is stale and dropped.
  const offers = $derived(offersReply && offersReply.clicked === selected ? offersReply.offers : []);
  const subjectName = $derived(selected && snap ? (snap.entities.get(selected)?.name ?? "") : "");

  conn.subscribe((msg: ServerMsg) => {
    switch (msg.t) {
      case "event":
        lines = [...lines, msg.text];
        break;
      case "snapshot":
        snap = toSnapshot(msg);
        // Moving rooms (or any change) can drop the selected entity from view; a
        // stale selection would render offers for a thing no longer here.
        if (selected !== null && !snap.entities.has(selected)) {
          selected = null;
          offersReply = null;
          pending = null;
        }
        break;
      case "offers":
        offersReply = { clicked: msg.clicked, offers: msg.offers };
        break;
      case "performed":
        refresh();
        break;
    }
  });
  conn.onOpen(() => {
    // Embody (a bare `@play` seats a guest), then read the world.
    conn.send({ t: "line", line: "@play" });
    conn.send({ t: "query", q: "snapshot" });
  });
  conn.onClose(() => {
    lines = [...lines, "(disconnected)"];
  });

  // Re-read after a mutation: the tree, and the offers for the current selection.
  function refresh() {
    conn.send({ t: "query", q: "snapshot" });
    if (selected !== null) conn.send({ t: "query", q: "offers", clicked: selected });
  }

  function select(id: string) {
    selected = id;
    pending = null;
    textInput = "";
    conn.send({ t: "query", q: "offers", clicked: id });
  }

  function act(offer: Offer) {
    if (offer.status.kind === "needs") {
      pending = {
        offer,
        parameters: [...offer.status.parameters],
        inputs: [...offer.bindings],
      };
      textInput = "";
      return;
    }
    conn.send({ t: "perform", affordance: offer.affordance, inputs: offer.bindings });
  }

  function pick(value: AffordanceValue) {
    if (!pending) return;
    const [parameter, ...parameters] = pending.parameters;
    const inputs = [...pending.inputs, { parameter, value }];
    if (parameters.length) {
      pending = { ...pending, parameters, inputs };
      textInput = "";
    } else {
      conn.send({ t: "perform", affordance: pending.offer.affordance, inputs });
      pending = null;
    }
  }

  function candidates(offer: Offer, parameter: string): AffordanceValue[] {
    return offer.candidates.find((set) => set.parameter === parameter)?.values ?? [];
  }

  function pickText() {
    pick({ kind: "text", text: textInput });
  }

  function valueLabel(value: AffordanceValue): string {
    switch (value.kind) {
      case "entity":
        return snap?.entities.get(value.id)?.name ?? `#${value.id}`;
      case "text":
        return value.text;
      case "symbol":
        return value.value;
    }
  }

  function say(line: string) {
    conn.send({ t: "line", line });
    // A typed command can move the actor or change the world, and the server pushes
    // no state deltas, so re-read exactly as a clicked act does.
    refresh();
  }
</script>

<main>
  <header>
    <h1>MUSCE</h1>
    <p class="tag">thin pointing client ({useMock ? "mock" : "live"})</p>
  </header>

  <div class="panes">
    <nav aria-label="Here" class="tree">
      <h2>Here</h2>
      {#if snap}
        <ul>
          <EntityTree id={snap.root} {snap} {selected} onSelect={select} />
        </ul>
      {:else}
        <p class="empty">Connecting…</p>
      {/if}
    </nav>

    <div class="detail">
      {#if selected === null}
        <p class="empty">Nothing selected.</p>
      {:else}
        <Offers subject={subjectName} {offers} onAct={act} />
        {#if pending}
          {@const current = pending}
          {@const values = candidates(current.offer, current.parameters[0])}
          {@const declaration = current.offer.parameters.find(
            (parameter) => parameter.id === current.parameters[0],
          )}
          <section class="picker" aria-labelledby="picker-heading">
            <h2 id="picker-heading">
              {declaration?.label ?? current.parameters[0]}
              for {current.offer.display_name}
            </h2>
            {#if values.length}
              <ul>
                {#each values as value, index (`${value.kind}-${index}`)}
                  <li><button onclick={() => pick(value)}>{valueLabel(value)}</button></li>
                {/each}
              </ul>
            {:else if declaration?.sort.kind === "text"}
              <form
                onsubmit={(event) => {
                  event.preventDefault();
                  pickText();
                }}
              >
                <input aria-label={declaration.label} bind:value={textInput} />
                <button type="submit">Choose</button>
              </form>
            {:else}
              <p class="empty">No choices are available.</p>
            {/if}
            <button class="cancel" onclick={() => (pending = null)}>Cancel</button>
          </section>
        {/if}
      {/if}
    </div>

    <div class="stream">
      <Log {lines} />
      <CommandBar onSubmit={say} />
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
