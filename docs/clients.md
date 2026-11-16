# Connecting clients

Use the MCP endpoint `https://memory.example.com/mcp` (your host). Clients that
support OAuth sign in with Google; others send a fixed token from `auth.json` as
`Authorization: Bearer <token>`. Name the server `brainkeep` in every client.

## claude.ai (web and mobile)

Settings → Connectors → Add custom connector, URL `https://memory.example.com/mcp`,
no client ID or secret (dynamic registration). Suggested name: «🧠 Brainkeep».
Connecting opens Google sign-in; the connector is then available on mobile too.

## Claude Code

OAuth:

```sh
claude mcp add --transport http brainkeep https://memory.example.com/mcp
```

Fixed token:

```sh
claude mcp add --transport http brainkeep https://memory.example.com/mcp \
  --header "Authorization: Bearer <token>"
```

Local, same machine as the server: `claude mcp add brainkeep -- brainkeep --data-dir <dir> mcp`.

## Other MCP clients (Codex, Antigravity, …)

Configure a Streamable HTTP server named `brainkeep` with the URL above and either
OAuth or the bearer header, following each client's MCP documentation.

## How agents use it

On connect, the server sends a short memory protocol (MCP `instructions`): at the
start of related work agents call `context` for the project, and they save decisions,
constraints, discoveries, progress, ideas under evaluation and links to shared
artifacts on their own, without narrating it. In Claude Code the core memory tools
(`context`, `recall`, `remember`, `correct`) load upfront instead of behind tool
search. You can always ask explicitly: “save this in Brainkeep”.
