# rockycrew

Compressed subagent delegation. Same results, ~60% smaller context cost.

## What it does

Three subagent presets (`rockycrew-investigator`, `rockycrew-builder`, `rockycrew-reviewer`) that return rocky-compressed output. Main context lasts longer across long sessions because each tool-result is ~1/3 the tokens of vanilla equivalents.

## When to use

Use rockycrew when you want the subagent's findings in minimal tokens. Use vanilla agents (`Explore`, `Code Reviewer`) when you want prose, architecture commentary, or full rationale.

## How to invoke

```
/rockycrew
/cavecrew          # alias
```

## See also

- [`SKILL.md`](./SKILL.md) — full decision guide with output contracts and chaining patterns
