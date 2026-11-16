# Claude Code plugin

`plugins/brainkeep` adds one `PostToolUse` hook. It never blocks a tool and adds a
short reminder (about 85 tokens) to the model's context only when:

- a change document `<repo>/pcdd/changes/<name>.md` is edited with Edit/Write or by
  a shell command that writes it: record the outcome in Brainkeep with project
  `<repo>` and topic `<name>`;
- an artifact is published or opened: save its link (URL, what it is, who created
  it; never URLs carrying tokens) if it is not stored yet.

Install from this repository's marketplace:

```text
/plugin marketplace add aledepaoligampel/brainkeep
/plugin install brainkeep@brainkeep
```

Requirements: `python3` on `PATH` and the `brainkeep` MCP server already connected
(see [clients.md](clients.md)). Try it without installing:
`claude --plugin-dir plugins/brainkeep`.

Known limitation: a shell command that writes another file but mentions a change
document path may trigger an extra reminder.
