#!/bin/sh
# Container entrypoint for the Brainkeep server (Docker, Railway).
#
# Layout under BRAINKEEP_ROOT (default /data, a persistent volume):
#   store/   memories (SQLite) and the private local socket
#   config/  auth.json, panel.json, google.json (0600, never printed)
#
# Variables:
#   BRAINKEEP_HOST            public host name (Railway: RAILWAY_PUBLIC_DOMAIN by default)
#   PANEL_ADMIN_KEY           64 hex: admin key for the panel (required)
#   BRAINKEEP_OWNER           admin handle (default admin); fixed after the first start
#   BRAINKEEP_ADMIN_EMAIL     optional: lets the admin sign in with Google; fixed once invited
#   GOOGLE_CLIENT_ID / GOOGLE_CLIENT_SECRET   optional: enable Google sign-in (both)
#   BRAINKEEP_CLIENT_IP_HEADER  header the proxy overwrites with the client address
#                             (Railway: X-Real-IP by default; elsewhere CF-Connecting-IP)
#   BRAINKEEP_LISTEN          listen address (default 0.0.0.0)
#   PORT                      listen port (default 18765)
set -eu

ROOT=${BRAINKEEP_ROOT:-/data}
BIN=${BRAINKEEP_BIN:-brainkeep}
RUN_UID=${BRAINKEEP_UID:-10001}
STORE=$ROOT/store
CONFIG=$ROOT/config

fail() { printf 'brainkeep: %s\n' "$1" >&2; exit 1; }
is_hex64() { [ "${#1}" -eq 64 ] && printf '%s' "$1" | grep -Eqx '[0-9a-fA-F]{64}'; }
hex64() {
  h=$(od -An -N32 -tx1 /dev/urandom | tr -d ' \n') || h=
  is_hex64 "$h" || fail "cannot generate a random key"
  printf '%s' "$h"
}
# JSON string body: the values written here are validated or generated, but escape anyway.
json() { printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'; }

# As root (fresh volumes are root-owned): give the data to the service user, then drop.
if [ "$(id -u)" = 0 ] && [ "${BRAINKEEP_STAY_ROOT:-}" != 1 ]; then
  mkdir -p "$ROOT"
  chown "$RUN_UID:$RUN_UID" "$ROOT"
  chmod 0700 "$ROOT"
  for d in "$STORE" "$CONFIG"; do
    if [ -d "$d" ]; then chown -R "$RUN_UID:$RUN_UID" "$d"; fi
  done
  exec setpriv --reuid="$RUN_UID" --regid="$RUN_UID" --clear-groups --no-new-privs -- "$0" "$@"
fi

umask 077
HOST=${BRAINKEEP_HOST:-${RAILWAY_PUBLIC_DOMAIN:-}}
[ -n "$HOST" ] || fail "set BRAINKEEP_HOST to the public host name"
[ -n "${PANEL_ADMIN_KEY:-}" ] || fail "set PANEL_ADMIN_KEY (64 hex) to sign in to the panel as admin"
is_hex64 "$PANEL_ADMIN_KEY" || fail "PANEL_ADMIN_KEY must be 64 hex characters"
OWNER=${BRAINKEEP_OWNER:-admin}
if [ -n "${BRAINKEEP_CLIENT_IP_HEADER:-}" ]; then
  IP_HEADER=$BRAINKEEP_CLIENT_IP_HEADER
elif [ -n "${RAILWAY_PROJECT_ID:-}${RAILWAY_ENVIRONMENT_ID:-}${RAILWAY_ENVIRONMENT:-}" ]; then
  IP_HEADER=X-Real-IP
else
  IP_HEADER=CF-Connecting-IP
fi
if [ -n "${GOOGLE_CLIENT_ID:-}" ] && [ -n "${GOOGLE_CLIENT_SECRET:-}" ]; then
  GOOGLE=1
elif [ -n "${GOOGLE_CLIENT_ID:-}${GOOGLE_CLIENT_SECRET:-}" ]; then
  fail "set both GOOGLE_CLIENT_ID and GOOGLE_CLIENT_SECRET, or neither"
else
  GOOGLE=
fi

mkdir -p "$STORE" "$CONFIG"
chmod 0700 "$ROOT" "$STORE" "$CONFIG"

# Writes $1 atomically with the content on stdin.
put() {
  rm -f "$1.new"
  cat > "$1.new"
  chmod 0600 "$1.new"
  mv -f "$1.new" "$1"
}

# Agent token: generated once. Nobody needs to see it; people get personal tokens
# from the panel. Brainkeep requires at least one entry. An empty file (crash during
# the first write) is regenerated.
if [ ! -s "$CONFIG/auth.json" ]; then
  AGENT_TOKEN=$(hex64)
  printf '{"server":"%s"}\n' "$AGENT_TOKEN" | put "$CONFIG/auth.json"
fi

# Panel: read and session keys are generated once and kept; the admin key follows
# PANEL_ADMIN_KEY on every start, so rotating the variable rotates the key.
# A file that exists but cannot be read back stops the start instead of silently
# replacing the keys (which would sign everyone out).
key() { sed -n "s/.*\"$1\"[[:space:]]*:[[:space:]]*\"\([0-9a-fA-F]\{64\}\)\".*/\1/p" "$CONFIG/panel.json" | head -n 1; }
READ_KEY=
SESSION_KEY=
if [ -s "$CONFIG/panel.json" ]; then
  READ_KEY=$(key read)
  SESSION_KEY=$(key session_key)
  [ -n "$READ_KEY" ] && [ -n "$SESSION_KEY" ] \
    || fail "cannot read the read and session_key values of $CONFIG/panel.json; fix or delete it"
else
  READ_KEY=$(hex64)
  SESSION_KEY=$(hex64)
fi
printf '{"read":"%s","session_key":"%s","admin":"%s"}\n' "$READ_KEY" "$SESSION_KEY" "$PANEL_ADMIN_KEY" \
  | put "$CONFIG/panel.json"

set -- serve --http-port "${PORT:-18765}" --http-host "$HOST" \
  --http-listen "${BRAINKEEP_LISTEN:-0.0.0.0}" --client-ip-header "$IP_HEADER" \
  --auth-file "$CONFIG/auth.json" --panel-auth-file "$CONFIG/panel.json" --owner "$OWNER"
if [ -n "$GOOGLE" ]; then
  printf '{"client_id":"%s","client_secret":"%s"}\n' \
    "$(json "$GOOGLE_CLIENT_ID")" "$(json "$GOOGLE_CLIENT_SECRET")" | put "$CONFIG/google.json"
  set -- "$@" --oauth-file "$CONFIG/google.json"
else
  rm -f "$CONFIG/google.json"
fi

unset PANEL_ADMIN_KEY GOOGLE_CLIENT_ID GOOGLE_CLIENT_SECRET

# The admin's Google email is invited once the service answers (invite needs it
# running). The handle stays bound to that email, so a later change is reported, not
# applied. Double fork: tini, not brainkeep, reaps the helper.
INVITED=$(cat "$CONFIG/admin-invited" 2>/dev/null || true)
if [ -n "$INVITED" ] && [ -n "${BRAINKEEP_ADMIN_EMAIL:-}" ] && [ "$INVITED" != "$BRAINKEEP_ADMIN_EMAIL" ]; then
  printf 'brainkeep: BRAINKEEP_ADMIN_EMAIL changed, but the admin stays bound to %s; ignoring the new value\n' "$INVITED" >&2
elif [ -n "${BRAINKEEP_ADMIN_EMAIL:-}" ] && [ -z "$INVITED" ]; then
  ( (
    i=0
    while [ $i -lt 100 ]; do
      if "$BIN" --data-dir "$STORE" status >/dev/null 2>&1; then
        if "$BIN" --data-dir "$STORE" users invite "$BRAINKEEP_ADMIN_EMAIL" --handle "$OWNER" >/dev/null; then
          printf '%s' "$BRAINKEEP_ADMIN_EMAIL" > "$CONFIG/admin-invited"
        else
          printf 'brainkeep: could not invite %s as %s\n' "$BRAINKEEP_ADMIN_EMAIL" "$OWNER" >&2
        fi
        exit 0
      fi
      i=$((i + 1))
      sleep 0.2
    done
    printf 'brainkeep: service did not start; admin email not invited\n' >&2
  ) & )
fi

exec "$BIN" --data-dir "$STORE" "$@"
