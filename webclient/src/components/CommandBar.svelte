<script lang="ts">
  interface Props {
    onSubmit: (line: string) => void;
  }
  let { onSubmit }: Props = $props();
  let value = $state("");

  function submit(event: SubmitEvent) {
    event.preventDefault();
    onSubmit(value);
    value = "";
  }
</script>

<form class="bar" onsubmit={submit}>
  <label class="visually-hidden" for="command">Command</label>
  <input
    id="command"
    name="command"
    autocomplete="off"
    placeholder="type a command (try: look)"
    bind:value
  />
  <button type="submit">Send</button>
</form>

<style>
  .bar {
    display: flex;
    gap: 0.5rem;
  }
  input {
    flex: 1;
    font: inherit;
    color: inherit;
    padding: 0.5rem 0.6rem;
    border-radius: 6px;
    border: 1px solid color-mix(in srgb, currentColor 30%, transparent);
    background: none;
  }
  button {
    font: inherit;
    color: inherit;
    padding: 0.5rem 0.9rem;
    border-radius: 6px;
    border: 1px solid color-mix(in srgb, currentColor 30%, transparent);
    background: none;
    cursor: pointer;
  }
  .visually-hidden {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
    white-space: nowrap;
  }
</style>
