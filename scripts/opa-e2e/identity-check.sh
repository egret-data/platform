#!/bin/sh
# E2E ONLY (port-8080 hardening, A5). Runs INSIDE a Trino 483 coordinator (sh, curl, sed):
#   docker exec -e TRINO_DELEGATOR_PASSWORD <container> sh -c "$(cat identity-check.sh)" identity-check <label>
# (scripts/opa-e2e/trino-auth.sh: identity_check). Proves that plain-HTTP identity spoofing is
# refused on this engine, and that the supported path still works:
#
#   1. POST /v1/statement on 8080, `X-Trino-User: bob`, no credential       -> 403 (insecure: refused
#      before any identity is read; at allow-insecure-over-http=true it is 200 and runs as bob)
#   2. as 1 plus `X-Forwarded-Proto: https`, no credential                  -> 401
#   3. as 2 with Basic egret_delegator, `X-Trino-Original-User: egret_delegator`,
#      `X-Trino-User: bob`, `SELECT current_user`                           -> 200, and the query
#      finishes with the single row bob (egret's own impersonate-mode query path)
#   4. GET /v1/info, no credential                                          -> 200 (probes need none)
#
# Statuses are asserted exactly, so a regression of the flag back to `true` fails the run.
# The measured table these mirror: docs/en/plans/2026-11-07-port-8080-hardening-live-result.md.
set -u
LABEL=$1
BASE=http://127.0.0.1:8080
DELEGATOR=egret_delegator # egret_core::engine_principals::DELEGATOR_PRINCIPAL
SQL='SELECT current_user'
FAILS=0
: "${TRINO_DELEGATOR_PASSWORD:?identity-check.sh: TRINO_DELEGATOR_PASSWORD is not set}"

verdict() { # $1 = check, $2 = want, $3 = got, $4 = detail
  if [ "$2" = "$3" ]; then
    printf 'PASS identity %s %s: %s\n' "$LABEL" "$1" "$3"
  else
    printf 'FAIL identity %s %s: want %s, got %s %s\n' "$LABEL" "$1" "$2" "$3" "$4"
    FAILS=$((FAILS + 1))
  fi
}
# post [curl args...]: POST the statement; prints `<status> <body on one line>`.
post() {
  curl -sS --max-time 30 -o /tmp/identity-check.body -w '%{http_code}' -X POST "$BASE/v1/statement" \
    -H 'X-Trino-User: bob' -H 'X-Trino-Source: egret-identity-check' "$@" --data-binary "$SQL" 2>/dev/null
  printf ' %s' "$(tr -d '\n' </tmp/identity-check.body 2>/dev/null | cut -c1-200)"
}

r=$(post)
verdict plain-http-no-credential 403 "${r%% *}" "${r#* }"

r=$(post -H 'X-Forwarded-Proto: https')
verdict forwarded-https-no-credential 401 "${r%% *}" "${r#* }"

# 3: the delegator, impersonating bob; every page of the query, nextUri followed on 8080.
auth() { curl -sS --max-time 30 -u "$DELEGATOR:$TRINO_DELEGATOR_PASSWORD" -H 'X-Forwarded-Proto: https' \
  -H "X-Trino-Original-User: $DELEGATOR" -H 'X-Trino-User: bob' "$@"; }
code=$(auth -o /tmp/identity-check.body -w '%{http_code}' -X POST "$BASE/v1/statement" \
  -H 'X-Trino-Source: egret-identity-check' --data-binary "$SQL" 2>/dev/null)
verdict delegator-impersonates-bob 200 "$code" "$(tr -d '\n' </tmp/identity-check.body | cut -c1-200)"
rows="" err="" i=0
r=$(tr -d '\n' </tmp/identity-check.body)
while [ "$code" = 200 ] && [ $i -lt 120 ]; do
  d=$(printf '%s' "$r" | sed -n 's/.*"data":\(\[\[[^]]*\]\]\).*/\1/p')
  [ -n "$d" ] && rows="$rows$d"
  printf '%s' "$r" | grep '"error"' >/dev/null && err=$(printf '%s' "$r" | cut -c1-300)
  # Behind `X-Forwarded-Proto: https` Trino writes nextUri as https://; follow its path on 8080.
  n=$(printf '%s' "$r" | sed -n 's/.*"nextUri":"\([^"]*\)".*/\1/p' | sed 's#^https\{0,1\}://[^/]*##')
  [ -z "$n" ] && break
  r=$(auth "$BASE$n" 2>/dev/null | tr -d '\n')
  i=$((i + 1))
done
verdict delegator-impersonates-bob-result '[["bob"]]' "$rows" "$err"

code=$(curl -sS --max-time 10 -o /dev/null -w '%{http_code}' "$BASE/v1/info" 2>/dev/null)
verdict info-no-credential 200 "$code" ""

rm -f /tmp/identity-check.body
[ "$FAILS" = 0 ]
