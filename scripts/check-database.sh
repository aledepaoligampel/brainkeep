#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
./scripts/cargo.sh test --locked --test memory --test light_memory --test multiuser --test oauth_store
./scripts/cargo.sh build --locked
python3 tests/protocol.py
python3 tests/compact_protocol.py
