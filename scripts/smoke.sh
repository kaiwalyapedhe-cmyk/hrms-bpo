#!/usr/bin/env bash
# Smoke checklist for a running HRMS stack.
#
#   ./scripts/smoke.sh                       # http://localhost
#   BASE=http://localhost:8080 ./scripts/smoke.sh
#   HRMS_EMAIL=... HRMS_PASSWORD=... ./scripts/smoke.sh
#
# Requires: curl, python3 (used to read JSON without jq).
set -uo pipefail

BASE="${BASE:-http://localhost}"
EMAIL="${HRMS_EMAIL:-admin@hrms.local}"
PASSWORD="${HRMS_PASSWORD:-ChangeMe#2026}"
PASS=0
FAIL=0

ok()   { printf '  \033[32m✓\033[0m %s\n' "$1"; PASS=$((PASS + 1)); }
bad()  { printf '  \033[31m✗\033[0m %s — %s\n' "$1" "$2"; FAIL=$((FAIL + 1)); }

json() { python3 -c "import json,sys;d=json.load(sys.stdin);print(d$1)" 2>/dev/null; }

check() { # name, path, expected-status
  local name="$1" path="$2" want="${3:-200}" code
  code=$(curl -s -o /dev/null -w '%{http_code}' "$BASE$path")
  if [[ "$code" == "$want" ]]; then ok "$name ($code)"; else bad "$name" "expected $want, got $code"; fi
}

echo "HRMS smoke test — $BASE"
echo
echo "Liveness / readiness"
check "GET /health" "/health"
check "GET /health/ready" "/health/ready"
check "GET /swagger-ui/" "/swagger-ui/" "200"

echo
echo "Authentication"
LOGIN=$(curl -s -X POST "$BASE/api/auth/login" \
  -H 'Content-Type: application/json' \
  -d "{\"email\":\"$EMAIL\",\"password\":\"$PASSWORD\"}")
TOKEN=$(printf '%s' "$LOGIN" | json "['token']")
if [[ -n "${TOKEN:-}" ]]; then ok "POST /api/auth/login"; else bad "POST /api/auth/login" "no token in response: $LOGIN"; fi

auth_check() { # name, path
  local name="$1" path="$2" code
  code=$(curl -s -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $TOKEN" "$BASE$path")
  if [[ "$code" == "200" ]]; then ok "$name"; else bad "$name" "HTTP $code"; fi
}

if [[ -n "${TOKEN:-}" ]]; then
  echo
  echo "Authenticated reads"
  auth_check "GET /api/auth/me" "/api/auth/me"
  auth_check "GET /api/employees" "/api/employees"
  auth_check "GET /api/dashboard/snapshot" "/api/dashboard/snapshot"
  auth_check "GET /api/dashboard/history" "/api/dashboard/history?hours=4"
  auth_check "GET /api/payroll/payslips" "/api/payroll/payslips"
  auth_check "GET /api/leave/balances" "/api/leave/balances"
  auth_check "GET /api/quality/scores" "/api/quality/scores"
  auth_check "GET /api/admin/system" "/api/admin/system"

  echo
  echo "Authorization boundary"
  code=$(curl -s -o /dev/null -w '%{http_code}' "$BASE/api/employees")
  if [[ "$code" == "401" ]]; then ok "GET /api/employees without token → 401"; else bad "anonymous access blocked" "got $code"; fi
fi

echo
echo "Realtime dashboard"
WS_CODE=$(curl -s -o /dev/null -w '%{http_code}' --max-time 2 \
  -H 'Connection: Upgrade' -H 'Upgrade: websocket' \
  -H 'Sec-WebSocket-Version: 13' -H 'Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==' \
  "$BASE/ws/dashboard?token=${TOKEN:-invalid}")
if [[ "$WS_CODE" == "101" ]]; then ok "WS /ws/dashboard upgraded"; else bad "WS /ws/dashboard" "HTTP $WS_CODE"; fi

echo
printf 'Result: %d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
