#!/bin/bash
# Snapshot the ZUL state DB. redb is crash-consistent on disk (every commit
# fsyncs), so a plain file copy yields a restorable, at-most-one-commit-stale
# snapshot — no need to stop the sequencer. Rotates local copies and, when
# ZUL_BACKUP_REMOTE is set, pushes off-box via rclone (the part that actually
# survives box loss — set it to a real remote for disaster recovery).
#
#   ./scripts/backup-redb.sh
#
# Restore (with the node STOPPED):
#   sudo systemctl stop zul-node
#   gunzip -c <backup>.redb.gz > /home/ubuntu/ZUL/data/zul.redb
#   sudo systemctl start zul-node
#
# Schedule nightly via cron, e.g.:
#   0 4 * * *  ZUL_BACKUP_REMOTE=b2:zul-backups /home/ubuntu/ZUL/scripts/backup-redb.sh
set -euo pipefail

DATA=${ZUL_DATA:-/home/ubuntu/ZUL/data}
DB="$DATA/zul.redb"
DEST=${ZUL_BACKUP_DIR:-/home/ubuntu/ZUL/backups}
KEEP=${ZUL_BACKUP_KEEP:-14}
REMOTE=${ZUL_BACKUP_REMOTE:-}

[ -f "$DB" ] || { echo "no DB at $DB" >&2; exit 1; }
mkdir -p "$DEST"

ts=$(date -u +%Y%m%dT%H%M%SZ)
out="$DEST/zul-$ts.redb.gz"
tmp="$DEST/.zul-$ts.tmp"

cp "$DB" "$tmp"
gzip -c "$tmp" > "$out"
rm -f "$tmp"
echo "backup: $out ($(du -h "$out" | cut -f1))"

# Rotate: keep the newest $KEEP local snapshots.
ls -1t "$DEST"/zul-*.redb.gz 2>/dev/null | tail -n +$((KEEP + 1)) | xargs -r rm -f

# Off-box replication (the real DR step). No-op until a remote is configured.
if [ -n "$REMOTE" ]; then
  if command -v rclone >/dev/null 2>&1; then
    rclone copy "$out" "$REMOTE" && echo "off-box: $REMOTE"
  else
    echo "ZUL_BACKUP_REMOTE set but rclone not installed" >&2
    exit 1
  fi
else
  echo "note: ZUL_BACKUP_REMOTE unset — local-only snapshot (does NOT survive box loss)"
fi
