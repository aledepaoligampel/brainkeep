# Claude Code plugin

`plugins/brainkeep` adds one `PostToolUse` hook. It never blocks a tool and adds a
short reminder (about 85 tokens) to the model's context only when:

- a change document `<repo>/pcdd/changes/<name>.md`, edited with Edit/Write or by a
  shell command that writes it, reaches a milestone: its specs (`S#`), task lines
  (`T#`) or Log entries (`L#`) differ from the last reminder. Record the outcome in
  Brainkeep with project `<repo>` and topic `<name>`. Edits to evidence or the
  continuation section stay silent;
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

The last seen state of each change lives in `${CLAUDE_PLUGIN_DATA}/pcdd-milestones.json`
(kept across plugin updates; outside Claude Code, `~/.cache/brainkeep/`). The first edit
of a change after installing reminds once. If that file cannot be read or written, every
edit reminds, as in 0.1.0. Specs or Log entries written in another format (not `- S1`,
`- **S1`, `### S1` or `- L1`) do not count as milestones; task lines follow the PCDD CLI
syntax. To receive this version, run `claude plugin update brainkeep@brainkeep`.
