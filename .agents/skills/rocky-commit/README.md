# rocky-commit

Terse commit messages. Why over what.

## What it does

Generates Conventional Commits messages: <=50 char subject in imperative mood, body only when the "why" is not obvious from the diff. Drops filler, AI attribution, and file-name restating. Always includes body for breaking changes, security fixes, and data migrations.

## How to invoke

```
/rocky-commit         # generate from staged diff
/caveman-commit       # alias
```

## See also

- [`SKILL.md`](./SKILL.md) — full LLM-facing instructions
