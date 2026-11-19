# 🧠 Brainkeep

Shared, user-controlled memory for AI agents. Brainkeep is a small MCP server
(Rust + SQLite) that your assistants (Claude Code, claude.ai, Codex, Antigravity or
any MCP client) use to save and recall decisions, discoveries, progress and links
across sessions, tools and people.

- **One memory, many agents.** Every assistant reads and writes the same projects,
  with full revision history and verified authorship.
- **Built for teams.** Invite people by email, share projects read-only or
  read-write, revoke access instantly. Sign-in with Google over OAuth for claude.ai
  and other remote clients; fixed tokens for local agents.
- **Small context footprint.** Compact responses and a short memory protocol the
  server sends to clients, so agents save on their own without long prompts.
- **Safe by default.** Private files (0600), secret-shaped text (passwords, keys,
  tokens by pattern) is rejected before it is stored, per-user request limits,
  consistent backups, schema migrations with snapshots.
- **Web panel.** Search, filters, history, diffs and live updates, with Google
  sign-in per user; edit memories (with conflict handling), archive and restore,
  and purge for project owners.

> Status: early. Linux/macOS (Unix sockets); Windows is not supported yet.

## Quick start (local, single user)

macOS on Apple silicon, or Linux `aarch64`/`x86_64`:

```sh
curl -fsSL https://brainkeep.dev/install.sh | sh
```

The [script](site-src/install.sh) downloads the current
[release](https://github.com/aledepaoligampel/brainkeep/releases), checks it against the
release's `SHA256SUMS` (nothing is installed on a mismatch), puts the binary in
`~/.local/bin/brainkeep` and keeps `brainkeep serve` running as a user service: a
LaunchAgent (`~/Library/LaunchAgents/dev.brainkeep.serve.plist`) on macOS, a
`systemd --user` unit (`brainkeep.service`) on Linux. Then it connects Claude Code for all
your projects (`claude mcp add --scope user`), unless a server named `brainkeep` already
exists. Data lives in `~/.brainkeep`. No root needed. Run it again to upgrade; set
`BRAINKEEP_VERSION` for a specific release. Other agents: [docs/clients.md](docs/clients.md).

<details>
<summary>Manual install</summary>

```sh
v=0.1.1; t=brainkeep-$v-aarch64-macos    # Linux: t=brainkeep-$v-$(uname -m)-linux
base=https://github.com/aledepaoligampel/brainkeep/releases/download/v$v
curl -fLO $base/$t.tar.gz && curl -fLO $base/SHA256SUMS
grep " $t.tar.gz\$" SHA256SUMS | shasum -a 256 -c
tar -xzf $t.tar.gz && mkdir -p ~/.local/bin && cp $t/brainkeep ~/.local/bin/

~/.local/bin/brainkeep --data-dir ~/.brainkeep serve      # keep it running
claude mcp add --scope user brainkeep -- ~/.local/bin/brainkeep --data-dir ~/.brainkeep mcp
```

`serve` owns the database; `mcp` is the stdio bridge each local agent starts. The macOS
binary is not notarized: download it with `curl` as above (a browser download is
quarantined by Gatekeeper). To build from source instead: `cargo build --locked --release`.

</details>

<details>
<summary>Uninstall</summary>

```sh
# macOS
launchctl bootout gui/$(id -u)/dev.brainkeep.serve
rm ~/Library/LaunchAgents/dev.brainkeep.serve.plist
# Linux
systemctl --user disable --now brainkeep.service
rm "${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/brainkeep.service"
systemctl --user daemon-reload

claude mcp remove --scope user brainkeep
rm ~/.local/bin/brainkeep
rm -rf ~/.brainkeep    # deletes every memory
```

</details>

## Server for a team

Run it on any Linux host with systemd behind an HTTPS reverse proxy (Caddy, nginx,
a tunnel). See [docs/server.md](docs/server.md): install, tokens, users and sharing,
Google sign-in, the web panel, backups and upgrades.

Or deploy the container image: on Railway in one click, or on any Docker host. See
[docs/railway.md](docs/railway.md).

Then connect your clients: [docs/clients.md](docs/clients.md).

## Claude Code plugin

The `brainkeep` plugin reminds Claude Code to record work in Brainkeep: after
it edits a PCDD change document (`pcdd/changes/*.md`) and after
it publishes or opens an artifact (so the link is saved for the team).

```text
/plugin marketplace add aledepaoligampel/brainkeep
/plugin install brainkeep@brainkeep
```

The plugin does not configure the MCP server; connect it first. Details:
[docs/plugin.md](docs/plugin.md).

## Development

```sh
./scripts/check.sh            # format, clippy, tests, protocol tests
./scripts/check-database.sh   # storage and migration tests
cd panel-ui && npm ci --ignore-scripts && npx vite build   # rebuild the web panel
python3 scripts/build_site.py # regenerate the brainkeep.dev landing in site/
python3 scripts/og_images.py  # re-render its share images (needs Chrome)
```

The landing (`site/`, served as plain files) is generated from `site-src/` and reuses the
panel's tokens and fonts; commit its output after editing copy or markup.

The web panel (Svelte 5 + Bits UI) is compiled into `src/panel/` and embedded in the
binary, so servers never need Node.

## License

[MIT](LICENSE). Bundled fonts: SIL Open Font License, see [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
