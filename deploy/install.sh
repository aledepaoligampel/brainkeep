#!/bin/sh
# Install or update Brainkeep on a systemd Linux host. Run as root from a staging
# directory that contains the `brainkeep` binary and `deploy/`.
# First install: also put `auth.json` (agent tokens) there and set BRAINKEEP_HOST,
# e.g. `BRAINKEEP_HOST=memory.example.com sh deploy/install.sh`.
# Credentials are installed only once and never printed.
set -eu
test "$(id -u)" = 0
test -f brainkeep
test -d deploy
RELEASE=$(sha256sum brainkeep | cut -d ' ' -f 1)
if ! id brainkeep >/dev/null 2>&1; then
  useradd --system --home-dir /var/lib/brainkeep --shell /usr/sbin/nologin brainkeep
fi
install -d -m 0755 /opt/brainkeep /opt/brainkeep/releases
install -d -m 0700 -o brainkeep -g brainkeep /etc/brainkeep /var/lib/brainkeep
fail() { printf 'brainkeep install: %s\n' "$1" >&2; exit 1; }
if [ ! -e /etc/brainkeep/env ]; then
  [ -n "${BRAINKEEP_HOST:-}" ] || fail "first install needs BRAINKEEP_HOST=<public host name>"
  printf 'BRAINKEEP_HOST=%s\nBRAINKEEP_ARGS=\n' "$BRAINKEEP_HOST" > /etc/brainkeep/env
  chmod 0600 /etc/brainkeep/env
fi
if [ ! -e /etc/brainkeep/auth.json ]; then
  [ -f auth.json ] || fail "first install needs auth.json in the staging directory"
  install -m 0600 -o brainkeep -g brainkeep auth.json /etc/brainkeep/auth.json
fi
# Panel credentials are generated on the host and never leave it or get printed.
if [ ! -e /etc/brainkeep/panel.json ]; then
  (
    umask 077
    read_token=$(od -An -N32 -tx1 /dev/urandom | tr -d ' \n')
    session_key=$(od -An -N32 -tx1 /dev/urandom | tr -d ' \n')
    admin_key=$(od -An -N32 -tx1 /dev/urandom | tr -d ' \n')
    printf '{"read":"%s","session_key":"%s","admin":"%s"}\n' "$read_token" "$session_key" "$admin_key" > /etc/brainkeep/panel.json.new
  )
  chown brainkeep:brainkeep /etc/brainkeep/panel.json.new
  mv /etc/brainkeep/panel.json.new /etc/brainkeep/panel.json
elif ! grep -q '"admin"' /etc/brainkeep/panel.json; then
  # Older installs: add the admin key only to the exact one-line file this script wrote;
  # anything else is left alone. Adding it ends existing panel sessions once.
  if grep -Eqx '\{"read":"[0-9a-f]{64}","session_key":"[0-9a-f]{64}"\}' /etc/brainkeep/panel.json; then
    (
      umask 077
      admin_key=$(od -An -N32 -tx1 /dev/urandom | tr -d ' \n')
      sed "s/}\$/,\"admin\":\"$admin_key\"}/" /etc/brainkeep/panel.json > /etc/brainkeep/panel.json.new
    )
    chown brainkeep:brainkeep /etc/brainkeep/panel.json.new
    mv /etc/brainkeep/panel.json.new /etc/brainkeep/panel.json
  else
    printf 'brainkeep install: panel.json has no "admin" key; add one (64 hex) to manage users from the panel\n' >&2
  fi
fi
install -d -m 0755 "/opt/brainkeep/releases/$RELEASE"
install -m 0755 brainkeep "/opt/brainkeep/releases/$RELEASE/brainkeep"
install -m 0755 deploy/backup.sh "/opt/brainkeep/releases/$RELEASE/backup.sh"
if [ -L /opt/brainkeep/current ]; then
  readlink /opt/brainkeep/current > /opt/brainkeep/previous-release
fi
rm -f /opt/brainkeep/next
ln -s "releases/$RELEASE" /opt/brainkeep/next
mv -Tf /opt/brainkeep/next /opt/brainkeep/current
install -m 0644 deploy/brainkeep.service deploy/brainkeep-backup.service deploy/brainkeep-backup.timer /etc/systemd/system/
systemctl daemon-reload
systemctl enable brainkeep.service brainkeep-backup.timer
systemctl restart brainkeep.service
systemctl start brainkeep-backup.timer
sleep 1
systemctl is-active --quiet brainkeep.service
runuser -u brainkeep -- /opt/brainkeep/current/brainkeep --data-dir /var/lib/brainkeep/data status
