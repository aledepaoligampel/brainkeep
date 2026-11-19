# Running a Brainkeep server

One process (`brainkeep serve`) owns the SQLite database. It serves MCP over a
private Unix socket (local agents through `brainkeep mcp`) and, optionally, over
authenticated HTTP on loopback for remote clients, behind your HTTPS reverse proxy.

## Install on Linux (systemd)

Each [release](https://github.com/aledepaoligampel/brainkeep/releases) ships
`brainkeep-<version>-<aarch64|x86_64>-linux.tar.gz` (binary, `deploy/`, protocol tests),
`SHA256SUMS` and GitHub build provenance. Download and verify one:

```sh
v=0.1.0; a=$(uname -m)   # aarch64 or x86_64
curl -fLO https://github.com/aledepaoligampel/brainkeep/releases/download/v$v/brainkeep-$v-$a-linux.tar.gz
curl -fLO https://github.com/aledepaoligampel/brainkeep/releases/download/v$v/SHA256SUMS
sha256sum --ignore-missing -c SHA256SUMS
gh attestation verify brainkeep-$v-$a-linux.tar.gz --repo aledepaoligampel/brainkeep   # optional
tar -xzf brainkeep-$v-$a-linux.tar.gz && mv brainkeep-$v-$a-linux stage
```

Or build from source and stage the binary with `deploy/`:

```sh
cargo build --locked --release
mkdir stage && cp target/release/brainkeep stage/ && cp -r deploy stage/
```

Create `stage/auth.json` with one token per agent (see below), then:

```sh
cd stage && sudo BRAINKEEP_HOST=memory.example.com sh deploy/install.sh
```

The installer creates the `brainkeep` system user, releases under
`/opt/brainkeep/releases/<sha256>` (with `current` and `previous-release`), data in
`/var/lib/brainkeep/data`, configuration in `/etc/brainkeep/` (all private),
the hardened `brainkeep.service` and a daily backup timer. Updates keep data and
credentials. The service listens on `127.0.0.1:18765`; point your reverse proxy for
`https://memory.example.com` there. MCP is served at `/mcp`.

Rate limits for login and OAuth endpoints key on the `CF-Connecting-IP` header. Your
proxy must **set it to the real client address and overwrite any value sent by the
client**; otherwise every client shares one limit, or a client can spoof it.
Cloudflare does this at its edge. With Caddy: `header_up CF-Connecting-IP {remote_host}`;
with nginx: `proxy_set_header CF-Connecting-IP $remote_addr;`.

Without Cloudflare, name the header your proxy overwrites with
`--client-ip-header`, for example `--client-ip-header X-Real-IP`. `--http-listen`
changes the listen address (default `127.0.0.1`); use `0.0.0.0` or `::` only when the
proxy reaches the service over the network, as in a container. Railway and Docker:
see [railway.md](railway.md).

`/etc/brainkeep/env` holds `BRAINKEEP_HOST` and `BRAINKEEP_ARGS` (extra
flags, for example `--oauth-file /etc/brainkeep/google.json`). Restart the
service after changing it.

### Upgrade

`install.sh` copies `upgrade.sh` next to the binary. As a user with sudo:

```sh
sh /opt/brainkeep/current/upgrade.sh 0.2.0
```

It downloads that release, checks `SHA256SUMS` and, when `gh` is installed and signed in,
verifies that the tarball was built by this repository's release workflow from that tag.
It then runs the protocol suites against the downloaded binary (needs `python3`), takes a
backup and runs `deploy/install.sh`. If the service does not stay up, it switches back to
the previous release. Data, credentials and `/etc/brainkeep/env` are kept;
`/opt/brainkeep/previous-release` names the release to go back to. Installs older than
the first release have no `upgrade.sh` yet: run the one inside the release tarball.

Release binaries are built on Ubuntu 24.04 and need glibc 2.39 or newer (Debian 13,
Ubuntu 24.04 or later). On older systems, build from source.

## Fixed tokens (`auth.json`)

Each entry is `name: token` (64 hex characters) or an object with access rules:

```json
{
  "codex": "<64 hex>",
  "ana-laptop": {"token": "<64 hex>", "user": "ana", "read": ["*"], "write": ["*"]}
}
```

An entry without `user` acts as the admin. On a new database the admin's handle is
`owner`; to sign in with Google as the admin, invite your email with that handle
(`users invite you@example.org --handle owner`). `read`/`write` patterns apply to the
project names the assistant uses. Up to 16 entries; changes need a restart. Generate
tokens with `openssl rand -hex 32` and keep them out of chats and Git.

## Users and sharing

Each person is a user (handle + email). Projects belong to a user; people use short
names for their own projects and `owner/name` for shared ones. Other users' projects
that are not shared with you look exactly like missing ones.

```sh
sudo -u brainkeep /opt/brainkeep/current/brainkeep --data-dir /var/lib/brainkeep/data users invite ana@example.org --handle ana
sudo -u brainkeep /opt/brainkeep/current/brainkeep --data-dir /var/lib/brainkeep/data users list
sudo -u brainkeep /opt/brainkeep/current/brainkeep --data-dir /var/lib/brainkeep/data users disable ana
```

The admin can also do this from the panel's **Users** view (with or without Google):
invite, disable, enable and regenerate a **personal token** (`sbp_` + 64 hex, shown
once, stored only as SHA-256). A personal token works as `Authorization: Bearer` on
`/mcp` for that user, with full access to their own and shared projects, and signs in
to the panel; no `auth.json` change or restart is needed. Brainkeep does not send
email: the panel shows a message to copy with the panel URL, the MCP URL and the token.
The console does not issue personal tokens; use "Regenerate token" in the panel.

Changes apply without a restart. Disabling rejects all of that user's tokens and
sessions at once; enabling issues a new personal token and revives no session, but
`auth.json` tokens with that `user` work again (remove the entry and restart to cut
them for good). Owners share from their assistant with the `share` and `unshare`
tools, or from the panel's **Share** button on one of their projects (`read` or
`write` role).

## Google sign-in (OAuth for remote clients)

Brainkeep is its own OAuth authorization server (dynamic client registration,
PKCE); Google only proves the email. In Google Cloud Console create a project, an
External audience (scopes `openid`, `email`, `profile`), and a **Web application**
client with the redirect URI `https://memory.example.com/oauth/google/callback`.
Store its credentials on the server only:

```sh
sudo install -m 600 -o brainkeep -g brainkeep /dev/null /etc/brainkeep/google.json
sudoedit /etc/brainkeep/google.json   # {"client_id":"…","client_secret":"…"}
```

Add `--oauth-file /etc/brainkeep/google.json` to `BRAINKEEP_ARGS` and restart.
Only invited, enabled emails verified by Google can sign in; the first login binds the
email to that Google account.

Accepted OAuth redirect URIs are claude.ai, claude.com, ChatGPT connectors and HTTP
loopback (CLI clients); add others in `src/oauth.rs`.

## Web panel

`install.sh` creates `/etc/brainkeep/panel.json` with a read key, a session key
and an admin key (64 hex each, generated on the host and never printed); on an older
install it adds the admin key only to the exact one-line file it wrote before. The
panel is at `https://memory.example.com/panel` and has one sign-in field: a personal
token signs in as that user, the admin key as the admin with write access (users,
editing, sharing), and the read key as a read-only emergency admin login. With OAuth
enabled, people can also sign in with Google. Replacing `session_key`, `read` or
`admin` signs everyone out.

## Backups and restore

`brainkeep-backup.timer` writes a consistent snapshot daily to
`/var/lib/brainkeep/backups/`. Restore always goes into a **new** directory and
is checked before use:

```sh
sudo -u brainkeep /opt/brainkeep/current/brainkeep \
  --data-dir /var/lib/brainkeep/restore-check restore /var/lib/brainkeep/backups/brainkeep-<date>.sqlite
```

Backups are readable only by the `brainkeep` user. To switch to a restored copy,
stop the service, move the current `data` directory aside, move the restored
directory to `/var/lib/brainkeep/data`, and start the service again.

Schema migrations run automatically on start and leave a `pre-v<N>.sqlite`
snapshot. An older binary cannot open a newer schema: to roll back, restore that
snapshot into a new directory.

## Limits

- Panel writes (edit, archive, purge, share, users) need a user session (Google,
  admin key or personal token); the read key stays read-only. Personal tokens do not
  expire on their own: regenerate them or disable the user. Sharing from the panel
  shows every enabled user's handle, name and photo (not their email). A purge removes the memory from the live database; existing backups
  and `pre-v<N>.sqlite` snapshots keep it until you rotate or delete them. Users
  cannot be deleted yet.
- At most 16 fixed tokens. `/mcp` serves 32 concurrent requests in total and 6 per
  user, across all of that user's tokens and OAuth clients (more get `429` with
  `Retry-After`); a request body must arrive within 5 seconds.
- Secret detection is a conservative pattern filter (passwords, keys, tokens), not a
  DLP system: it does not recognize private or sensitive data by context, so keep
  that out of memories.
- `confirmed` and `source` are declared by the client that writes. They record what
  an agent claimed, not verified truth (`source_verified` is always `false`); do not
  treat them as human approval.
- Search is full-text (SQLite FTS), not semantic: it matches words, not synonyms or
  other languages, and listings favor recent memories. Use stable topics and the
  project's vocabulary; the server never calls a model.
