#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
./scripts/cargo.sh fmt --all -- --check
./scripts/cargo.sh clippy --locked --all-targets -- -D warnings
./scripts/cargo.sh test --locked --all-targets
./scripts/cargo.sh build --locked
for test in protocol http_protocol oauth_protocol compact_protocol panel_protocol panel_google_protocol \
  panel_live_protocol panel_edit_protocol panel_archive_protocol \
  panel_activity_protocol panel_admin_protocol panel_share_protocol pcdd_hook; do
  python3 "tests/$test.py"
done
node tests/panel_render.mjs
python3 tests/site.py
