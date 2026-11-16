#!/bin/sh
set -eu
umask 077
mkdir -p /var/lib/brainkeep/backups
STAMP=$(date -u +%Y%m%dT%H%M%S)
exec /opt/brainkeep/current/brainkeep --data-dir /var/lib/brainkeep/data backup "/var/lib/brainkeep/backups/brainkeep-$STAMP.sqlite"
