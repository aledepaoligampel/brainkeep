# Railway and Docker

The repository's `Dockerfile` builds a server image that runs on
[Railway](https://railway.com) or any Docker host. One container serves MCP at
`/mcp` and the web panel at `/panel`. Memories and generated credentials live in
`/data`, which must be a persistent volume.

## Variables

| Variable | Required | Meaning |
| --- | --- | --- |
| `PANEL_ADMIN_KEY` | yes | 64 hex characters. Sign in to `/panel` with it as admin: invite people, issue personal tokens, share projects. Changing it and restarting rotates it. |
| `BRAINKEEP_HOST` | yes, except on Railway | Public host name, e.g. `memory.example.com`. On Railway it defaults to `RAILWAY_PUBLIC_DOMAIN` (the `*.up.railway.app` domain); with a custom domain, set it to that domain or the panel and OAuth reject requests. |
| `BRAINKEEP_OWNER` | no (`admin`) | Admin handle. Fixed after the first start. |
| `BRAINKEEP_ADMIN_EMAIL` | no | Admin's Google email, so the admin can also sign in with Google. Fixed once invited: a later change is only reported in the log. |
| `GOOGLE_CLIENT_ID`, `GOOGLE_CLIENT_SECRET` | no (both or neither) | Enable Google sign-in for claude.ai and the panel ([create the client](server.md#google-sign-in-oauth-for-remote-clients)). Redirect URI: `https://<host>/oauth/google/callback`. |
| `BRAINKEEP_CLIENT_IP_HEADER` | no | Header your proxy **overwrites** with the client address (one address, not a list), used for login rate limits. Default: `X-Real-IP` on Railway, `CF-Connecting-IP` elsewhere. |
| `PORT`, `BRAINKEEP_LISTEN` | no (`18765`, `0.0.0.0`) | Listen port and address. Railway sets `PORT`. |

On the first start the entrypoint generates the agent token (`auth.json`) and the
panel's read and session keys (`panel.json`) in `/data/config`, private and never
printed. Restarts keep them. Nobody needs the agent token: people connect with
Google or with the personal token the admin issues from the panel.

Generate a key with `openssl rand -hex 32`.

## Railway in one click

[![Deploy on Railway](https://railway.com/button.svg)](https://railway.com/deploy/brainkeep)

1. Optionally fill in `BRAINKEEP_ADMIN_EMAIL` and, for Google sign-in, the Google
   client; all three can be added later. The template generates `PANEL_ADMIN_KEY`.
2. When it is up, copy `PANEL_ADMIN_KEY` from the service's **Variables** tab and open
   `https://<your domain>/panel`. Sign in with that key.
3. **Users → Invite**: each person gets a message with the MCP URL and a personal
   token. Connect clients: [clients.md](clients.md).

Keep one replica: a single process owns the SQLite database.

### Updating

Railway does not redeploy a template service when Brainkeep changes. When a new
version lands on `main`, Railway shows that an update is available for the service;
apply it when it suits you ([Railway docs](https://docs.railway.com/templates/updates)).
If your GitHub account was connected when you deployed, Railway copied the repository
into it and offers the update as a branch you can test first.

Memories and credentials live in the `/data` volume, so an update keeps them. Schema
migrations run on start and leave a `pre-v<N>.sqlite` snapshot; an older version
cannot open a newer schema, so roll back by restoring that snapshot
([server.md](server.md)). To deploy changes automatically instead, eject to your own
copy of the repository (service **Settings → Source → Upstream Repo → Eject**): every
push to that copy deploys, and keeping it in sync with Brainkeep is up to you.

### Publishing the template (maintainers)

In Railway: **Workspace → Templates → New Template**, then:

- Service from this GitHub repository (Railway builds the `Dockerfile`).
- Volume mounted at `/data`.
- Public networking: generate a domain with target port `18765`.
- Variables:
  - `PANEL_ADMIN_KEY` = `${{secret(64, "abcdef0123456789")}}`
  - `PORT` = `18765`
  - `BRAINKEEP_CLIENT_IP_HEADER` = `X-Real-IP`
  - `BRAINKEEP_ADMIN_EMAIL`, `GOOGLE_CLIENT_ID`, `GOOGLE_CLIENT_SECRET`: empty,
    with a description, optional.
- Icon `https://brainkeep.dev/favicon.svg`, and an overview with the sections the
  marketplace requires.
- Publish (the published template is `https://railway.com/deploy/brainkeep`), and
  link the button from this page and the README:

```markdown
[![Deploy on Railway](https://railway.com/button.svg)](https://railway.com/deploy/brainkeep)
```

Template functions such as `secret()` run once, when someone deploys the template.

### Client IP on Railway

Railway's edge sets `X-Real-IP` to the client address and overwrites any value the
client sends. This comes from Railway staff answers, not a documented contract;
check it after deploying: with the wrong header every client shares one rate-limit
bucket, or a client can rotate the header to escape it. Requests over Railway's
private network (`*.railway.internal`) skip the edge and can set any header: do not
expose other services in the same project to untrusted code.

## Docker

```sh
docker build -t brainkeep .
KEY=$(openssl rand -hex 32); echo "Panel admin key: $KEY"   # keep it in your password manager
docker run -d --name brainkeep --restart unless-stopped \
  -v brainkeep-data:/data -p 127.0.0.1:18765:18765 \
  -e BRAINKEEP_HOST=memory.example.com \
  -e PANEL_ADMIN_KEY="$KEY" \
  -e BRAINKEEP_CLIENT_IP_HEADER=X-Real-IP \
  brainkeep
```

Put an HTTPS reverse proxy in front that sets the client IP header (Caddy:
`header_up X-Real-IP {remote_host}`; nginx: `proxy_set_header X-Real-IP $remote_addr;`).
Without the admin key you cannot sign in to the panel as admin; restarting with a new
`PANEL_ADMIN_KEY` replaces it.

The container starts as root only to take ownership of `/data`, then runs as the
unprivileged `brainkeep` user (uid 10001).

## Backups

The systemd backup timer of [server.md](server.md) does not exist in a container.

- **Railway:** enable scheduled backups in the volume's **Backups** tab (daily, weekly,
  monthly). They are volume snapshots, kept inside Railway; wiping the volume deletes
  them.
- **Off-site copy:** take a consistent backup with the running service and copy it out:

  ```sh
  railway ssh   # or: docker exec -it brainkeep sh
  setpriv --reuid=10001 --regid=10001 --clear-groups --no-new-privs \
    brainkeep --data-dir /data/store backup /data/backup-$(date +%F).sqlite
  ```

  Then copy it out and delete it from the volume. With Docker: `docker cp`. On Railway
  the file is binary, so send it through base64 rather than a raw terminal, e.g.
  `railway ssh -- base64 /data/backup-<date>.sqlite | base64 -d > backup.sqlite`, and
  check it opens before deleting the copy on the volume. Restore: [server.md](server.md#backups-and-restore).

Automatic off-site backups are not included.

## A team on Claude Code

Commit two files to each team repository so everyone gets the same setup. No
token goes into Git.

`.mcp.json` connects the server. With Google sign-in, each person runs `/mcp` once and
signs in:

```json
{
  "mcpServers": {
    "brainkeep": { "type": "http", "url": "https://memory.example.com/mcp" }
  }
}
```

Without Google, each person exports the personal token from their invitation
(`export BRAINKEEP_TOKEN=sbp_…` in their shell profile) and the file sends it:

```json
{
  "mcpServers": {
    "brainkeep": {
      "type": "http",
      "url": "https://memory.example.com/mcp",
      "headers": { "Authorization": "Bearer ${BRAINKEEP_TOKEN}" }
    }
  }
}
```

`.claude/settings.json` offers the [plugin](plugin.md) from this repository's
marketplace:

```json
{
  "extraKnownMarketplaces": {
    "brainkeep": { "source": { "source": "github", "repo": "aledepaoligampel/brainkeep" } }
  },
  "enabledPlugins": { "brainkeep@brainkeep": true }
}
```

Claude Code asks each person to trust the folder and to approve the project's MCP
server; until they trust it, `extraKnownMarketplaces` is ignored. If the plugin is not
installed after that, run `claude plugin install brainkeep@brainkeep --scope project`.

Organization-wide instead of per repository: on Claude for Teams or Enterprise, an
owner can put the same `extraKnownMarketplaces` and `enabledPlugins` in **Organization
settings → Claude Code → Managed settings** (or in `managed-settings.json` on each
machine); managed `enabledPlugins` cannot be turned off by users. For claude.ai web and
mobile, an owner adds the connector once under **Organization settings → Connectors**
with the URL `https://memory.example.com/mcp`; each member then connects it with their
own Google account.

Each person has their own memory; share team projects from the panel or with the
`share` tool, and teammates use them as `<owner>/<project>`
([users and sharing](server.md#users-and-sharing)).

References: [Claude Code settings](https://code.claude.com/docs/en/settings),
[plugins for organizations](https://code.claude.com/docs/en/plugins/org),
[MCP](https://code.claude.com/docs/en/mcp),
[server-managed settings](https://code.claude.com/docs/en/server-managed-settings).
