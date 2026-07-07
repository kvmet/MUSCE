# rocky-compress

Compress `.md` files to Rocky prose. Save ~46% input tokens.

## What it does

Compresses natural language files (CLAUDE.md, todos, preferences) into Rocky-style prose. Preserves all code blocks, inline code, URLs, file paths, commands, and technical terms exactly. Backs up original as `FILE.original.md` before overwriting.

## How to invoke

```
/rocky-compress <filepath>
/caveman-compress <filepath>
```

## See also

- [`SKILL.md`](./SKILL.md) — full LLM-facing instructions
- [`SECURITY.md`](./SECURITY.md) — security considerations
