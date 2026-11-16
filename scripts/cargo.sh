#!/bin/sh
# Run cargo from PATH; the gate scripts call this wrapper.
set -eu
if command -v cargo >/dev/null 2>&1; then
  exec cargo "$@"
fi
printf '%s\n' 'Rust toolchain missing: install Rust (https://rustup.rs) or add cargo to PATH.' >&2
exit 1
