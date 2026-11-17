#!/bin/sh
# Brainkeep installer for one user, on macOS (Apple silicon) or Linux (aarch64, x86_64):
#
#   curl -fsSL https://brainkeep.dev/install.sh | sh
#
# It downloads release @VERSION@ from GitHub, checks it against the release's SHA256SUMS,
# installs ~/.local/bin/brainkeep, keeps `brainkeep serve` running as a user service
# (launchd on macOS, systemd --user on Linux) and connects Claude Code. No root needed.
# Run it again to upgrade: data in ~/.brainkeep is kept. BRAINKEEP_VERSION picks another
# release. Manual steps and uninstall: https://github.com/aledepaoligampel/brainkeep#readme
#
# Everything runs from main(), called on the last line: a cut-off download runs nothing.
set -eu

say() { printf '%s\n' "$*"; }
fail() { printf 'brainkeep install: %s\n' "$*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || fail "$1 is required"; }

# A word to print in a command the user may paste: quoted only when it needs to be.
q() {
  case "$1" in
    *[!A-Za-z0-9_./~:=+-]*) printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")" ;;
    *) printf '%s' "$1" ;;
  esac
}

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

xml() { printf '%s' "$1" | sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g'; }

# systemd unit quoting: specifiers (%), variables ($), backslashes and quotes stay literal.
unit_word() { printf '"%s"' "$(printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e 's/%/%%/g' -e 's/\$/$$/g')"; }

socket_up() { [ -S "$data/brainkeep.sock" ]; }

wait_for() {
  # wait_for up|down: up to 10 s for the server socket to appear or go away.
  i=0
  while [ "$i" -lt 10 ]; do
    if [ "$1" = up ]; then socket_up && return 0; else socket_up || return 0; fi
    sleep 1
    i=$((i + 1))
  done
  return 1
}

other_server_note() {
  other=1
  say "Note: a \`brainkeep serve\` started by hand may be running for $data (or one crashed)."
  say "      Stop it so the service can take over: pkill -f $(q "brainkeep --data-dir $data serve")"
}

service_macos() {
  label=dev.brainkeep.serve
  domain=gui/$(id -u)
  agents=$HOME/Library/LaunchAgents
  plist=$agents/$label.plist
  mkdir -p "$agents"
  cat > "$plist.new" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>$label</string>
  <key>ProgramArguments</key>
  <array>
    <string>$(xml "$bin")</string>
    <string>--data-dir</string>
    <string>$(xml "$data")</string>
    <string>serve</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardOutPath</key>
  <string>$(xml "$data")/serve.log</string>
  <key>StandardErrorPath</key>
  <string>$(xml "$data")/serve.log</string>
</dict>
</plist>
EOF
  mv -f "$plist.new" "$plist"
  if launchctl print "$domain/$label" >/dev/null 2>&1 </dev/null; then
    # bootout stops the old server with SIGTERM, so it removes its socket and lock first.
    launchctl bootout "$domain/$label" </dev/null >/dev/null 2>&1 || true
    wait_for down || true
  elif socket_up; then
    other_server_note
  fi
  # bootout finishes asynchronously; bootstrap can fail for a moment right after it.
  tries=0
  until launchctl bootstrap "$domain" "$plist" </dev/null >/dev/null 2>&1; do
    tries=$((tries + 1))
    if [ "$tries" -ge 5 ]; then
      service="not loaded yet (no graphical login?); it starts at your next login, or run: launchctl bootstrap $domain $(q "$plist")"
      return 0
    fi
    sleep 1
  done
  if [ -n "$other" ]; then
    # KeepAlive retries every few seconds: the service takes over once the other server stops.
    service="waiting for the other server to stop (launchd: $label)"
  elif wait_for up && launchctl print "$domain/$label" 2>/dev/null </dev/null | grep -q 'state = running'; then
    service="running (launchd: $label)"
  else
    service="not running; see $(q "$data/serve.log")"
  fi
}

service_linux() {
  if ! command -v systemctl >/dev/null 2>&1 || ! systemctl --user show-environment >/dev/null 2>&1 </dev/null; then
    service="not installed (no systemd user session). Start it with:
    nohup $(q "$bin") --data-dir $(q "$data") serve >> $(q "$data/serve.log") 2>&1 &"
    return 0
  fi
  units=${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user
  mkdir -p "$units"
  cat > "$units/brainkeep.service" <<EOF
[Unit]
Description=Brainkeep memory server

[Service]
ExecStart=$(unit_word "$bin") --data-dir $(unit_word "$data") serve
Restart=on-failure

[Install]
WantedBy=default.target
EOF
  systemctl --user is-active --quiet brainkeep.service </dev/null 2>&1 || ! socket_up || other_server_note
  check="see: systemctl --user status brainkeep"
  if ! systemctl --user daemon-reload </dev/null >/dev/null 2>&1 ||
    ! systemctl --user enable brainkeep.service </dev/null >/dev/null 2>&1 ||
    ! systemctl --user restart brainkeep.service </dev/null >/dev/null 2>&1; then
    service="could not be started; $check"
  elif [ -n "$other" ]; then
    service="not started: another server holds $data; stop it, then: systemctl --user restart brainkeep"
  elif wait_for up && systemctl --user is-active --quiet brainkeep.service </dev/null 2>&1; then
    service="running (systemd --user: brainkeep.service)"
  else
    service="not running; $check"
  fi
  # Without lingering, user services stop at logout and do not start at boot.
  if command -v loginctl >/dev/null 2>&1 &&
    [ "$(loginctl show-user "$(id -un)" -p Linger --value 2>/dev/null </dev/null || true)" = no ]; then
    service="$service
           To keep it running after you log out: loginctl enable-linger $(id -un)"
  fi
}

connect_claude() {
  add="claude mcp add --scope user brainkeep -- $(q "$bin") --data-dir $(q "$data") mcp"
  if ! command -v claude >/dev/null 2>&1; then
    client="Claude Code not found. After installing it, run:
    $add"
  # From an empty directory, so a project-scoped server of the current folder does not count.
  elif (cd "$tmp" && claude mcp get brainkeep </dev/null >/dev/null 2>&1); then
    client="Claude Code already has a server named brainkeep; left unchanged"
  elif (cd "$tmp" && claude mcp add --scope user brainkeep -- "$bin" --data-dir "$data" mcp </dev/null >/dev/null 2>&1); then
    client="Claude Code connected (user scope: all projects)"
  else
    client="Claude Code could not be configured. Run:
    $add"
  fi
}

main() {
  version=${BRAINKEEP_VERSION:-@VERSION@}
  version=${version#v}
  repo=https://github.com/aledepaoligampel/brainkeep
  [ -n "${HOME:-}" ] || fail "HOME is not set"
  [ "$(id -u)" -ne 0 ] || fail "run it as your own user, without sudo: it installs into your home directory"
  bin_dir=$HOME/.local/bin
  bin=$bin_dir/brainkeep
  data=$HOME/.brainkeep
  other=

  case "$version" in
    [0-9]*.[0-9]*) ;;
    *) fail "invalid BRAINKEEP_VERSION: $version" ;;
  esac
  case "$version" in
    *[!0-9A-Za-z.-]* | *..*) fail "invalid BRAINKEEP_VERSION: $version" ;;
  esac
  os=$(uname -s)
  arch=$(uname -m)
  # A shell running under Rosetta reports x86_64 on Apple silicon.
  if [ "$os/$arch" = Darwin/x86_64 ] && [ "$(sysctl -n hw.optional.arm64 2>/dev/null || true)" = 1 ]; then
    arch=arm64
  fi
  case "$os/$arch" in
    Darwin/arm64) target=aarch64-macos ;;
    Linux/aarch64 | Linux/arm64) target=aarch64-linux ;;
    Linux/x86_64 | Linux/amd64) target=x86_64-linux ;;
    *) fail "no release for $os $arch; Brainkeep ships for macOS on Apple silicon and Linux aarch64/x86_64. See $repo#readme" ;;
  esac
  need curl
  need tar

  name=brainkeep-$version-$target
  base=$repo/releases/download/v$version
  tmp=$(mktemp -d "${TMPDIR:-/tmp}/brainkeep.XXXXXX")
  trap 'rm -rf "$tmp"; rm -f "$bin_dir/.brainkeep.new"' EXIT
  trap 'exit 130' INT TERM

  say "Downloading Brainkeep $version ($target)..."
  curl --proto '=https' --tlsv1.2 -fsSL -o "$tmp/$name.tar.gz" "$base/$name.tar.gz" || fail "download failed: $base/$name.tar.gz"
  curl --proto '=https' --tlsv1.2 -fsSL -o "$tmp/SHA256SUMS" "$base/SHA256SUMS" || fail "download failed: $base/SHA256SUMS"
  expected=$(awk -v f="$name.tar.gz" '$2 == f || $2 == "*" f {print $1; exit}' "$tmp/SHA256SUMS")
  case "$expected" in
    '' | *[!0-9a-f]*) fail "no checksum for $name.tar.gz in SHA256SUMS; nothing was installed" ;;
  esac
  actual=$(sha256 "$tmp/$name.tar.gz")
  [ "${#actual}" -eq 64 ] || fail "cannot compute SHA-256 (sha256sum or a working shasum is required); nothing was installed"
  [ "$actual" = "$expected" ] || fail "checksum mismatch for $name.tar.gz; nothing was installed"

  tar -xzf "$tmp/$name.tar.gz" -C "$tmp" 2>/dev/null || fail "cannot unpack $name.tar.gz; nothing was installed"
  [ -f "$tmp/$name/brainkeep" ] || fail "the release does not contain $name/brainkeep; nothing was installed"

  # Staged next to its destination (not in a temp dir that may be mounted noexec), tried, then moved.
  mkdir -p "$bin_dir"
  cp "$tmp/$name/brainkeep" "$bin_dir/.brainkeep.new"
  chmod 755 "$bin_dir/.brainkeep.new"
  "$bin_dir/.brainkeep.new" --version >/dev/null 2>&1 </dev/null || fail "the downloaded binary does not run on this system; nothing was installed"
  mv -f "$bin_dir/.brainkeep.new" "$bin"
  (umask 077 && mkdir -p "$data" && touch "$data/serve.log")
  chmod 700 "$data"

  case "$os" in
    Darwin) service_macos ;;
    *) service_linux ;;
  esac
  connect_claude

  say ""
  say "Brainkeep $version installed."
  say "  Binary:  $bin"
  say "  Data:    $data"
  say "  Server:  $service"
  say "  Client:  $client"
  say "  Other agents: $repo/blob/main/docs/clients.md"
  case ":${PATH:-}:" in
    *":$bin_dir:"*) ;;
    *) say ""
       say "Note: $bin_dir is not on your PATH. To run \`brainkeep\` by name, add to your shell profile:"
       say "  export PATH=\"\$HOME/.local/bin:\$PATH\"" ;;
  esac
}

main "$@"
