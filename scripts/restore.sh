#!/usr/bin/env bash
# Restore a backup produced by scripts/backup.sh.
#   ./scripts/restore.sh backups/20261126-020000
set -euo pipefail

SRC="${1:?usage: restore.sh <backup-dir>}"
STACK_DIR="$(cd "$(dirname "$0")/.." && pwd)"
[[ -f "$SRC/hrms.sql.gz" ]] || { echo "missing $SRC/hrms.sql.gz"; exit 1; }

echo "This replaces the current database. Type 'restore' to continue:"
read -r CONFIRM
[[ "$CONFIRM" == "restore" ]] || { echo "aborted"; exit 1; }

echo "→ restoring postgres"
gunzip -c "$SRC/hrms.sql.gz" | docker compose -f "$STACK_DIR/docker-compose.yml" \
  exec -T postgres psql -v ON_ERROR_STOP=1 -U hrms -d hrms

if [[ -d "$SRC/minio" ]]; then
  echo "→ restoring minio objects"
  docker compose -f "$STACK_DIR/docker-compose.yml" run --rm --entrypoint sh \
    -v "$(cd "$SRC" && pwd)":/backup minio-init -c \
    'mc alias set local http://minio:9000 "$MINIO_ROOT_USER" "$MINIO_ROOT_PASSWORD" && mc mirror --overwrite /backup/minio local/"$S3_BUCKET"'
fi

echo "Restore complete. Restart the API: docker compose restart api"
