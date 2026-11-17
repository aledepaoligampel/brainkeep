#!/bin/sh
# Upgrade this server to a published release: download it, verify its checksum (and its
# build provenance when `gh` is installed and signed in), run the protocol suites against
# the binary, take a backup and install it with deploy/install.sh. If the new release does
# not stay up, the previous one is restored. Data, credentials and settings are kept.
# Usage, as a user with sudo: sh upgrade.sh 0.2.0
set -eu
REPO=${BRAINKEEP_REPO:-aledepaoligampel/brainkeep}
fail() { printf 'brainkeep upgrade: %s\n' "$1" >&2; exit 1; }

version=${1:-}
version=${version#v}
case $version in
  '' | *[!0-9A-Za-z.-]* | .* | *..*) fail 'usage: upgrade.sh <version>, e.g. 0.2.0' ;;
esac
case $(uname -m) in
  aarch64 | arm64) arch=aarch64 ;;
  x86_64 | amd64) arch=x86_64 ;;
  *) fail "no release build for $(uname -m)" ;;
esac
command -v python3 >/dev/null || fail 'python3 is needed to run the protocol suites'

name="brainkeep-$version-$arch-linux"
url="https://github.com/$REPO/releases/download/v$version"
# Staging and test data live in the home directory: /tmp may be RAM-backed.
work=$(mktemp -d "${BRAINKEEP_STAGE:-$HOME}/brainkeep-upgrade.XXXXXX")
trap 'rm -rf "$work"' EXIT
trap 'exit 1' HUP INT TERM

get() { curl -fsSL --proto '=https' --proto-redir '=https' --tlsv1.2 -o "$1" "$2"; }
get "$work/$name.tar.gz" "$url/$name.tar.gz"
get "$work/SHA256SUMS" "$url/SHA256SUMS"
expected=$(awk -v f="$name.tar.gz" '$2 == f { print $1 }' "$work/SHA256SUMS")
[ -n "$expected" ] || fail "SHA256SUMS has no entry for $name.tar.gz"
actual=$(sha256sum "$work/$name.tar.gz" | cut -d ' ' -f 1)
[ "$expected" = "$actual" ] || fail "checksum mismatch for $name.tar.gz"
# The checksum file ships with the tarball, so it only proves integrity. Provenance proves
# the tarball was built by this repository's release workflow from tag v$version.
if command -v gh >/dev/null 2>&1 && gh auth status >/dev/null 2>&1; then
  gh attestation verify "$work/$name.tar.gz" --repo "$REPO" \
    --signer-workflow "$REPO/.github/workflows/release.yml" --source-ref "refs/tags/v$version" >/dev/null \
    || fail 'build provenance did not verify'
  echo 'Checksum and build provenance verified.'
else
  echo 'Checksum verified. Build provenance not checked (install gh and run gh auth login to check it).'
fi

tar -xzf "$work/$name.tar.gz" -C "$work"
cd "$work/$name"
for t in protocol http_protocol oauth_protocol panel_protocol panel_google_protocol panel_live_protocol \
  panel_edit_protocol panel_archive_protocol panel_activity_protocol panel_admin_protocol; do
  TMPDIR="$work" BRAINKEEP_BINARY="$PWD/brainkeep" python3 "tests/$t.py" > "$work/$t.log" 2>&1 || {
    tail -n 20 "$work/$t.log" >&2
    fail "tests/$t.py failed against the downloaded binary"
  }
done
echo 'Protocol suites passed (smoke test on this host).'

if [ -e /etc/systemd/system/brainkeep-backup.service ]; then
  sudo systemctl start brainkeep-backup.service || fail 'backup failed; nothing was installed'
  echo 'Backup taken.'
fi
# From here on an interrupt would leave a half-finished install.
trap '' HUP INT TERM
sudo sh deploy/install.sh

# The installer checks the service once; give it time to show it stays up.
ok=1
for _ in 1 2 3 4 5 6 7 8 9 10; do
  sleep 1
  systemctl is-active --quiet brainkeep.service || { ok=0; break; }
done
if [ "$ok" = 0 ]; then
  previous=$(cat /opt/brainkeep/previous-release 2>/dev/null || true)
  [ -n "$previous" ] || fail "brainkeep.service is not running and there is no previous release"
  sudo sh -c 'rm -f /opt/brainkeep/next && ln -s "$1" /opt/brainkeep/next && mv -Tf /opt/brainkeep/next /opt/brainkeep/current \
    && systemctl restart brainkeep.service' sh "$previous"
  fail "brainkeep.service did not stay up; restored $previous"
fi
echo "Brainkeep $version installed."
