#!/usr/bin/env bash
# Nightly logical backup: PostgreSQL dump + MinIO bucket mirror.
#   ./scripts/backup.sh [output-dir]
set -euo pipefail

OUT="${1:-backups/$(date +%Y%m%d-%H%M%S)}"
mkdir -p "$OUT"
STACK_DIR="$(cd "$(dirname "$0")/.." && pwd)"

echo "→ dumping postgres to $OUT/hrms.sql.gz"
docker compose -f "$STACK_DIR/docker-compose.yml" exec -T postgres \
  pg_dump --clean --if-exists -U hrms -d hrms | gzip > "$OUT/hrms.sql.gz"

echo "→ mirroring minio bucket to $OUT/minio"
docker compose -f "$STACK_DIR/docker-compose.yml" run --rm --entrypoint sh \
  -v "$(cd "$OUT" && pwd)":/backup minio-init -c \
  'mc alias set local http://minio:9000 "$MINIO_ROOT_USER" "$MINIO_ROOT_PASSWORD" && mc mirror --overwrite local/"$S3_BUCKET" /backup/minio'

echo "→ writing manifest"
{
  echo "created_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "database=hrms"
  echo "bucket=${S3_BUCKET:-hrms}"
  echo "git_commit=$(git -C "$STACK_DIR" rev-parse --short HEAD 2>/dev/null || echo unknown)"
} > "$OUT/MANIFEST"

echo "Backup complete: $OUT"
echo "Retention policy: keep 7 years for payroll artefacts (Payroll records retention, India)."
