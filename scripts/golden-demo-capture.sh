#!/usr/bin/env bash
# Governance sub-project 1: capture every Trino/StarRocks profile's policy
# download, raw bytes, after a converging recompile. Run it twice against one
# stack: the second run must print 0 (nothing left to converge) and write the
# same SHA256SUMS as the first. Run it again after the refactor: the sums must
# still match.
#
# Usage:
#   EGRET_URL=http://localhost:18080 \
#   EGRET_EMAIL=admin@egret.local EGRET_PASSWORD=... \
#   EGRET_RANGER_TOKEN=<ranger.auth_token> \
#   scripts/golden-demo-capture.sh OUT_DIR
#
# The control-plane API takes a session cookie, and every POST must echo the
# `egret_csrf` cookie in `X-Egret-CSRF`; login is rate-limited (5 per 5
# minutes), so set EGRET_COOKIE_JAR to a jar from an earlier run to skip it.
# The Ranger download takes the operator token (`ranger.auth_token`).
set -euo pipefail

out="${1:?usage: $0 OUT_DIR}"
: "${EGRET_URL:?set EGRET_URL}"
: "${EGRET_RANGER_TOKEN:?set EGRET_RANGER_TOKEN}"
mkdir -p "$out"
rm -f "$out"/*.compile.json "$out"/*.download.json "$out/SHA256SUMS"

api="$EGRET_URL/api/control-plane"
jar="${EGRET_COOKIE_JAR:-$out/cookies.txt}"

if [[ -z "${EGRET_COOKIE_JAR:-}" ]]; then
  : "${EGRET_EMAIL:?set EGRET_EMAIL (or EGRET_COOKIE_JAR)}"
  : "${EGRET_PASSWORD:?set EGRET_PASSWORD (or EGRET_COOKIE_JAR)}"
  curl -fsS -c "$jar" -H 'Content-Type: application/json' \
    -d "$(jq -n --arg e "$EGRET_EMAIL" --arg p "$EGRET_PASSWORD" '{email: $e, password: $p}')" \
    "$EGRET_URL/api/auth/login" > /dev/null
fi
csrf="$(awk '$6 ~ /egret_csrf$/ { v = $7 } END { print v }' "$jar")"

if command -v sha256sum > /dev/null; then
  sha() { sha256sum "$@"; }
else
  sha() { shasum -a 256 "$@"; }
fi

curl -fsS -b "$jar" "$api/policies/services" \
  | jq -r '.[] | select(.type == "trino" or .type == "starrocks") | "\(.id) \(.name)"' \
  | sort -n \
  | while read -r id name; do
      curl -fsS -X POST -b "$jar" -H "X-Egret-CSRF: $csrf" \
        "$api/perm/compile/$id" > "$out/$name.compile.json"
      curl -fsS -H "Authorization: Bearer $EGRET_RANGER_TOKEN" \
        "$EGRET_URL/ranger/service/plugins/policies/download/$name" > "$out/$name.download.json"
      (cd "$out" && sha "$name.download.json") >> "$out/SHA256SUMS"
    done

# Everything the recompiles wrote; 0 once the stack has converged.
jq -s 'map(.created + .updated + .deleted) | add // 0' "$out"/*.compile.json
