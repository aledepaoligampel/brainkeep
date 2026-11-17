#!/usr/bin/env python3
"""Checks for the one-line local installer (site/install.sh, served at brainkeep.dev/install.sh).

The script runs against a fake GitHub release in a temporary HOME. curl, uname, launchctl,
systemctl and claude are stubs on PATH that log their arguments, so nothing leaves the machine
and no real service or client configuration is touched.
"""
import hashlib
import io
import os
import re
import shutil
import socket
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "site" / "install.sh"
VERSION = re.search(r'^version\s*=\s*"([^"]+)"', (ROOT / "Cargo.toml").read_text(encoding="utf-8"), re.M).group(1)
DOWNLOADS = "https://github.com/aledepaoligampel/brainkeep/releases/download/"
RELEASES = f"{DOWNLOADS}v{VERSION}/"
UPGRADE = "9.9.9"

# The fake brainkeep: `--version` prints it; `serve` leaves a Unix socket where the real one would.
FAKE_BINARY = f"""#!{sys.executable}
import os, socket, sys
if sys.argv[1:] == ["--version"]:
    print("brainkeep @VERSION@")
elif sys.argv[-1] == "serve":
    path = sys.argv[2] + "/brainkeep.sock"
    if os.path.exists(path):
        os.unlink(path)  # as the real server does with a stale socket
    socket.socket(socket.AF_UNIX).bind(path)
"""

STUBS = {
    # curl: serves $FAKE_RELEASE/v<version>/<file> for GitHub release URLs; logs the command line.
    "curl": f"""#!{sys.executable}
import os, shutil, sys
args = sys.argv[1:]
open(os.environ["FAKE_LOG"], "a").write("curl " + " ".join(args) + "\\n")
url, out = args[-1], args[args.index("-o") + 1]
if not url.startswith("{DOWNLOADS}"):
    sys.exit(22)
src = os.path.join(os.environ["FAKE_RELEASE"], *url[len("{DOWNLOADS}"):].split("/"))
if not os.path.isfile(src):
    sys.exit(22)
shutil.copyfile(src, out)
""",
    "uname": """#!/bin/sh
case "$1" in -s) echo "$FAKE_OS" ;; -m) echo "$FAKE_ARCH" ;; *) echo "$FAKE_OS" ;; esac
""",
    # launchctl: a loaded agent runs the installed binary; bootout stops it (the socket goes away).
    "launchctl": """#!/bin/sh
echo "launchctl $*" >> "$FAKE_LOG"
case "$1" in
  print) [ -e "$HOME/.fake-loaded" ] && echo "state = running" ;;
  bootout) rm -f "$HOME/.fake-loaded" "$HOME/.brainkeep/brainkeep.sock" ;;
  bootstrap) [ -z "${FAKE_BOOTSTRAP_FAILS:-}" ] || exit 5
    touch "$HOME/.fake-loaded"; "$HOME/.local/bin/brainkeep" --data-dir "$HOME/.brainkeep" serve ;;
  *) exit 1 ;;
esac
""",
    "systemctl": """#!/bin/sh
echo "systemctl $*" >> "$FAKE_LOG"
[ -z "${FAKE_NO_SYSTEMD:-}" ] || exit 1
[ "$1" = --user ] || exit 1
case "$2" in
  show-environment | daemon-reload | enable) ;;
  restart) [ "$3" != "${FAKE_SYSTEMCTL_FAILS:-}" ] || exit 1
    touch "$HOME/.fake-active"; "$HOME/.local/bin/brainkeep" --data-dir "$HOME/.brainkeep" serve ;;
  is-active) [ -e "$HOME/.fake-active" ] ;;
  *) exit 1 ;;
esac
""",
    "loginctl": """#!/bin/sh
echo "${FAKE_LINGER:-yes}"
""",
    "sysctl": """#!/bin/sh
[ -n "${FAKE_ARM64:-}" ] && echo 1 || exit 1
""",
    "id": """#!/bin/sh
case "$1" in -u) echo "${FAKE_UID:-$(/usr/bin/id -u)}" ;; *) /usr/bin/id "$@" ;; esac
""",
    # claude: logs its arguments and the directory it runs in.
    "claude": """#!/bin/sh
echo "claude $*" >> "$FAKE_LOG"
pwd >> "$FAKE_LOG.pwd"
case "$2" in get) [ -n "${FAKE_CLAUDE_HAS:-}" ] ;; add) [ -z "${FAKE_CLAUDE_FAILS:-}" ] ;; esac
""",
}


def tarball(name, version):
    data = io.BytesIO()
    binary = FAKE_BINARY.replace("@VERSION@", version)
    with tarfile.open(fileobj=data, mode="w:gz") as tar:
        for path, body, mode in ((f"{name}/brainkeep", binary, 0o755), (f"{name}/LICENSE", "MIT\n", 0o644)):
            raw = body.encode()
            info = tarfile.TarInfo(path)
            info.size, info.mode = len(raw), mode
            tar.addfile(info, io.BytesIO(raw))
    return data.getvalue()


class Installer(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="bk-install-", dir="/tmp"))
        self.home = self.tmp / "home"
        self.home.mkdir()
        self.releases = self.tmp / "release"
        for version in (VERSION, UPGRADE):
            folder = self.releases / f"v{version}"
            folder.mkdir(parents=True)
            sums = []
            for target in ("aarch64-macos", "aarch64-linux", "x86_64-linux"):
                name = f"brainkeep-{version}-{target}.tar.gz"
                data = tarball(name[: -len(".tar.gz")], version)
                (folder / name).write_bytes(data)
                sums.append(f"{hashlib.sha256(data).hexdigest()}  {name}\n")
            (folder / "SHA256SUMS").write_text("".join(sums))
        self.release = self.releases / f"v{VERSION}"
        self.stubs = self.tmp / "stubs"
        self.stubs.mkdir()
        for name, body in STUBS.items():
            (self.stubs / name).write_text(body)
            (self.stubs / name).chmod(0o755)
        self.log = self.tmp / "log"
        self.log.touch()

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def run_installer(self, os_name="Darwin", arch="arm64", script=None, without=(), cwd=None, **env):
        stubs = self.stubs
        if without:
            stubs = self.tmp / "stubs-partial"
            shutil.rmtree(stubs, ignore_errors=True)
            shutil.copytree(self.stubs, stubs)
            for name in without:
                (stubs / name).unlink()
        environ = {
            "HOME": str(self.home),
            "PATH": f"{stubs}:/usr/bin:/bin",
            "FAKE_OS": os_name,
            "FAKE_ARCH": arch,
            "FAKE_RELEASE": str(self.releases),
            "FAKE_LOG": str(self.log),
            "TMPDIR": str(self.tmp),
            **env,
        }
        # The way users run it: the script arrives on stdin.
        source = (script if script is not None else SCRIPT.read_text(encoding="utf-8")).encode()
        result = subprocess.run([os.environ.get("INSTALL_SH", "/bin/sh")], input=source, env=environ, capture_output=True, timeout=60,
                                cwd=cwd or self.tmp)
        return result.returncode, result.stdout.decode() + result.stderr.decode()

    def calls(self, prefix):
        return [line for line in self.log.read_text().splitlines() if line.startswith(prefix + " ")]

    @property
    def binary(self):
        return self.home / ".local" / "bin" / "brainkeep"

    @property
    def plist(self):
        return self.home / "Library" / "LaunchAgents" / "dev.brainkeep.serve.plist"

    @property
    def unit(self):
        return self.home / ".config" / "systemd" / "user" / "brainkeep.service"

    def assert_nothing_installed(self):
        self.assertFalse(self.binary.exists())
        self.assertFalse(self.plist.exists())
        self.assertFalse(self.unit.exists())
        self.assertEqual(self.calls("claude"), [])
        self.assertEqual(self.calls("launchctl"), [])

    def assert_no_temporary_files(self):
        self.assertEqual(sorted(p.name for p in self.tmp.iterdir() if not p.name.startswith("log")),
                         ["home", "release", "stubs"], "temporary files are removed")
        bin_dir = self.home / ".local" / "bin"
        self.assertFalse((bin_dir / ".brainkeep.new").exists(), "the staged binary is removed")

    def test_macos_installs_verifies_starts_and_connects(self):
        code, out = self.run_installer()
        self.assertEqual(code, 0, out)
        self.assertTrue(os.access(self.binary, os.X_OK))
        self.assertEqual(subprocess.run([str(self.binary), "--version"], capture_output=True, text=True).stdout.strip(),
                         f"brainkeep {VERSION}")
        curls = self.calls("curl")
        self.assertEqual(len(curls), 2)
        for call in curls:
            self.assertIn("--proto =https", call)
            self.assertIn(RELEASES, call)
        self.assertTrue(any(c.endswith(f"brainkeep-{VERSION}-aarch64-macos.tar.gz") for c in curls))
        plist = self.plist.read_text()
        self.assertIn("<string>dev.brainkeep.serve</string>", plist)
        for arg in (str(self.binary), "--data-dir", str(self.home / ".brainkeep"), "serve"):
            self.assertIn(f"<string>{arg}</string>", plist)
        self.assertRegex(plist, r"<key>RunAtLoad</key>\s*<true/>")
        self.assertRegex(plist, r"<key>KeepAlive</key>\s*<true/>")
        self.assertIn(f"<string>{self.home}/.brainkeep/serve.log</string>", plist)
        self.assertIn(f"launchctl bootstrap gui/{os.getuid()} {self.plist}", self.calls("launchctl"))
        self.assertIn("Server:  running (launchd: dev.brainkeep.serve)", out)
        self.assertEqual(self.calls("claude")[-1],
                         f"claude mcp add --scope user brainkeep -- {self.binary} --data-dir {self.home}/.brainkeep mcp")
        self.assertEqual((self.home / ".brainkeep").stat().st_mode & 0o777, 0o700)
        self.assertEqual((self.home / ".brainkeep" / "serve.log").stat().st_mode & 0o777, 0o600)
        self.assertIn("not on your PATH", out)
        self.assertNotIn("started by hand", out)
        self.assert_no_temporary_files()

    def test_claude_is_checked_outside_the_current_project(self):
        project = self.tmp / "home" / "project"
        project.mkdir()
        code, out = self.run_installer(cwd=project)
        self.assertEqual(code, 0, out)
        dirs = Path(f"{self.log}.pwd").read_text().split()
        self.assertTrue(dirs)
        self.assertNotIn(str(project), [str(Path(d).resolve()) for d in dirs])

    def test_checksum_mismatch_installs_nothing(self):
        name = f"brainkeep-{VERSION}-aarch64-macos.tar.gz"
        (self.release / name).write_bytes((self.release / name).read_bytes() + b"tampered")
        code, out = self.run_installer()
        self.assertNotEqual(code, 0)
        self.assertIn("checksum mismatch", out)
        self.assert_nothing_installed()
        self.assert_no_temporary_files()

    def test_missing_checksum_entry_installs_nothing(self):
        (self.release / "SHA256SUMS").write_text("")
        code, out = self.run_installer()
        self.assertNotEqual(code, 0)
        self.assert_nothing_installed()

    def test_failed_download_installs_nothing(self):
        (self.release / f"brainkeep-{VERSION}-aarch64-macos.tar.gz").unlink()
        code, out = self.run_installer()
        self.assertNotEqual(code, 0)
        self.assertIn("download failed", out)
        self.assert_nothing_installed()
        self.assert_no_temporary_files()

    def test_unsupported_system_changes_nothing(self):
        for os_name, arch in (("Darwin", "x86_64"), ("FreeBSD", "amd64"), ("MINGW64_NT-10.0", "x86_64")):
            code, out = self.run_installer(os_name, arch)
            self.assertNotEqual(code, 0)
            self.assertIn("no release", out.lower())
            self.assert_nothing_installed()
            self.assertEqual(self.calls("curl"), [])

    def test_rosetta_shell_gets_the_apple_silicon_release(self):
        code, out = self.run_installer("Darwin", "x86_64", FAKE_ARM64="1")
        self.assertEqual(code, 0, out)
        self.assertTrue(any(c.endswith(f"brainkeep-{VERSION}-aarch64-macos.tar.gz") for c in self.calls("curl")))

    def test_root_is_refused(self):
        code, out = self.run_installer(FAKE_UID="0")
        self.assertNotEqual(code, 0)
        self.assertIn("without sudo", out)
        self.assertEqual(self.calls("curl"), [])

    def test_invalid_version_override_is_rejected(self):
        for bad in ("0.1.1/../../x", "..", "-rf", "0.1.1;id", "latest", ""):
            if bad == "":
                continue  # empty means the pinned version
            code, out = self.run_installer(BRAINKEEP_VERSION=bad)
            self.assertNotEqual(code, 0, bad)
            self.assertIn("invalid BRAINKEEP_VERSION", out)
            self.assertEqual(self.calls("curl"), [], bad)

    def test_version_override_downloads_that_release(self):
        for override in (UPGRADE, f"v{UPGRADE}"):
            code, out = self.run_installer(BRAINKEEP_VERSION=override)
            self.assertEqual(code, 0, out)
            self.assertTrue(self.calls("curl")[-1].endswith(f"{DOWNLOADS}v{UPGRADE}/SHA256SUMS"))
            self.assertIn(f"Brainkeep {UPGRADE} installed.", out)

    def test_truncated_download_runs_nothing(self):
        text = SCRIPT.read_text(encoding="utf-8").rstrip("\n")
        self.assertRegex(text.splitlines()[-1], r'^main "\$@"$')
        code, out = self.run_installer(script=text.rsplit("\n", 1)[0])
        self.assertEqual(self.log.read_text(), "")
        self.assertFalse(self.binary.exists())

    def test_linux_systemd_user_service_and_existing_client(self):
        code, out = self.run_installer("Linux", "x86_64", FAKE_CLAUDE_HAS="1")
        self.assertEqual(code, 0, out)
        self.assertTrue(any(c.endswith(f"brainkeep-{VERSION}-x86_64-linux.tar.gz") for c in self.calls("curl")))
        unit = self.unit.read_text()
        self.assertIn(f'ExecStart="{self.binary}" --data-dir "{self.home}/.brainkeep" serve', unit)
        self.assertIn("Restart=on-failure", unit)
        self.assertIn("WantedBy=default.target", unit)
        systemctl = self.calls("systemctl")
        for expected in ("systemctl --user daemon-reload", "systemctl --user enable brainkeep.service",
                         "systemctl --user restart brainkeep.service"):
            self.assertIn(expected, systemctl)
        self.assertIn("Server:  running (systemd --user: brainkeep.service)", out)
        self.assertNotIn("enable-linger", out)
        self.assertEqual(self.calls("claude"), ["claude mcp get brainkeep"], "an existing server is left alone")

    def test_linux_without_lingering_explains_it(self):
        code, out = self.run_installer("Linux", "x86_64", FAKE_LINGER="no")
        self.assertEqual(code, 0, out)
        self.assertIn("loginctl enable-linger", out)

    def test_linux_service_failure_still_connects_and_summarizes(self):
        code, out = self.run_installer("Linux", "x86_64", FAKE_SYSTEMCTL_FAILS="brainkeep.service")
        self.assertEqual(code, 0, out)
        self.assertIn("Server:  could not be started; see: systemctl --user status brainkeep", out)
        self.assertTrue(self.calls("claude"), "Claude Code is still connected")

    def test_linux_unit_keeps_special_characters_literal(self):
        self.home = self.tmp / "home" / "a b%h$x"
        self.home.mkdir()
        code, out = self.run_installer("Linux", "x86_64")
        self.assertEqual(code, 0, out)
        data = f"{self.tmp}/home/a b%%h$$x/.brainkeep"
        self.assertIn(f'--data-dir "{data}" serve', self.unit.read_text())

    def test_linux_arm_names(self):
        for arch in ("aarch64", "arm64"):
            code, out = self.run_installer("Linux", arch)
            self.assertEqual(code, 0, out)
            self.assertTrue(any(c.endswith(f"brainkeep-{VERSION}-aarch64-linux.tar.gz") for c in self.calls("curl")))

    def test_linux_without_systemd_or_claude_prints_the_commands(self):
        code, out = self.run_installer("Linux", "x86_64", without=("claude",), FAKE_NO_SYSTEMD="1")
        self.assertEqual(code, 0, out)
        self.assertFalse(self.unit.exists())
        self.assertIn(f"{self.binary} --data-dir {self.home}/.brainkeep serve", out)
        self.assertIn(f"claude mcp add --scope user brainkeep -- {self.binary} --data-dir {self.home}/.brainkeep mcp", out)

    def test_printed_commands_are_quoted_for_pasting(self):
        self.home = self.tmp / "home" / "with space"
        self.home.mkdir()
        code, out = self.run_installer("Linux", "x86_64", without=("claude",), FAKE_NO_SYSTEMD="1")
        self.assertEqual(code, 0, out)
        self.assertIn(f"claude mcp add --scope user brainkeep -- '{self.binary}' --data-dir '{self.home}/.brainkeep' mcp", out)
        self.assertIn(f"nohup '{self.binary}' --data-dir '{self.home}/.brainkeep' serve", out)

    def test_macos_bootstrap_failure_still_finishes(self):
        code, out = self.run_installer(FAKE_BOOTSTRAP_FAILS="1")
        self.assertEqual(code, 0, out)
        self.assertIn("starts at your next login", out)
        self.assertTrue(self.calls("claude"))

    def test_manual_server_already_running_is_reported(self):
        (self.home / ".brainkeep").mkdir(mode=0o700)
        sock = socket.socket(socket.AF_UNIX)
        sock.bind(str(self.home / ".brainkeep" / "brainkeep.sock"))
        try:
            code, out = self.run_installer()
        finally:
            sock.close()
        self.assertEqual(code, 0, out)
        self.assertIn("started by hand", out)
        self.assertIn(f"pkill -f 'brainkeep --data-dir {self.home}/.brainkeep serve'", out)
        self.assertIn("Server:  waiting for the other server to stop", out)
        self.assertNotIn("Server:  running", out)

    def test_rerun_upgrades_and_keeps_data(self):
        code, out = self.run_installer()
        self.assertEqual(code, 0, out)
        keep = self.home / ".brainkeep" / "memory.db"
        keep.write_text("data")
        self.log.write_text("")
        code, out = self.run_installer(FAKE_CLAUDE_HAS="1", BRAINKEEP_VERSION=UPGRADE)
        self.assertEqual(code, 0, out)
        self.assertEqual(keep.read_text(), "data")
        self.assertEqual(subprocess.run([str(self.binary), "--version"], capture_output=True, text=True).stdout.strip(),
                         f"brainkeep {UPGRADE}")
        launchctl = [c for c in self.calls("launchctl") if not c.startswith("launchctl print")]
        domain = f"gui/{os.getuid()}"
        self.assertEqual(launchctl, [f"launchctl bootout {domain}/dev.brainkeep.serve",
                                     f"launchctl bootstrap {domain} {self.plist}"])
        self.assertIn("Server:  running (launchd: dev.brainkeep.serve)", out)
        self.assertNotIn("started by hand", out)
        self.assertEqual(self.calls("claude"), ["claude mcp get brainkeep"])

    def test_path_note_is_skipped_when_bin_dir_is_on_path(self):
        code, out = self.run_installer(PATH=f"{self.stubs}:{self.home}/.local/bin:/usr/bin:/bin")
        self.assertEqual(code, 0, out)
        self.assertNotIn("not on your PATH", out)


if __name__ == "__main__":
    unittest.main(verbosity=1)
