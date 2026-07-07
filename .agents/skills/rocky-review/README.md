# rocky-review

One-line PR comments. Location, problem, fix.

## What it does

Generates terse code review comments: `L<line>: <severity>: <problem>. <fix>.` Drops hedging, throat-clearing, and diff restating. Keeps exact line numbers, symbol names, and concrete fixes. Drops to normal prose for security findings and architectural disagreements.

## How to invoke

```
/rocky-review         # review current diff
/caveman-review       # alias
```

## See also

- [`SKILL.md`](./SKILL.md) — full LLM-facing instructions
