#!/bin/sh
# E2E ONLY (Task 22). Runs INSIDE a Trino 483 container (sh, curl, date; no python):
#   docker exec <container> sh -c "$(cat latency-run.sh)" latency-run <mode> <args...>
#
# Modes:
#   once <user> <sql>
#     One statement over Trino's client protocol on 127.0.0.1:8080 as <user>. Prints
#     `@@T0 <ns>`, every protocol response on its own line, then `@@T1 <ns>`: the client's
#     wall time is T1 - T0, taken inside the container so `docker exec` costs nothing.
#   loop <user> <sql> <seconds> <interval>
#     Repeats the statement for <seconds>, sleeping <interval> seconds between the end of
#     one and the start of the next. Prints `<start ns> <ok|fail> <end ns>` per statement;
#     a statement is ok when no response carries an "error".
#
# Authentication (port-8080 hardening, A5): both engines refuse authentication over plain HTTP, so
# each request authenticates as the delegator (Basic, TRINO_DELEGATOR_PASSWORD from the
# environment, see trino-auth.sh) with `X-Forwarded-Proto: https`, and runs as <user> through
# `X-Trino-Original-User: egret_delegator` (egret's own query path). nextUri comes back as
# https://; its path is followed on 8080.
set -u
BASE=http://127.0.0.1:8080
URL=$BASE/v1/statement
DELEGATOR=egret_delegator # egret_core::engine_principals::DELEGATOR_PRINCIPAL

tcurl() { # $1 = user, then curl args; --fail: a refused request is a harness failure
  u=$1
  shift
  cred="$DELEGATOR:${TRINO_DELEGATOR_PASSWORD:-}"
  curl -sS --fail --max-time 60 -u "$cred" -H 'X-Forwarded-Proto: https' \
    -H "X-Trino-Original-User: $DELEGATOR" -H "X-Trino-User: $u" "$@"
}

# $1 = user, $2 = sql: every response on its own line.
run() {
  r=$(tcurl "$1" -X POST "$URL" -H "X-Trino-Source: egret-latency" \
    --data-binary "$2" 2>&1) || { printf '{"error":{"errorName":"HARNESS","message":"curl POST failed"}}\n'; return; }
  while :; do
    r=$(printf '%s' "$r" | tr -d '\n')
    [ -z "$r" ] && { printf '{"error":{"errorName":"HARNESS","message":"empty response"}}\n'; return; }
    printf '%s\n' "$r"
    n=$(printf '%s' "$r" | sed -n 's/.*"nextUri":"\([^"]*\)".*/\1/p' | sed 's#^https\{0,1\}://[^/]*##')
    [ -z "$n" ] && break
    r=$(tcurl "$1" "$BASE$n" 2>&1) \
      || { printf '{"error":{"errorName":"HARNESS","message":"curl GET failed"}}\n'; return; }
  done
}

mode=$1
shift
case "$mode" in
  once)
    printf '@@T0 %s\n' "$(date +%s%N)"
    run "$1" "$2"
    printf '@@T1 %s\n' "$(date +%s%N)"
    ;;
  loop)
    user=$1 sql=$2 seconds=$3 interval=$4
    end=$(( $(date +%s) + seconds ))
    while [ "$(date +%s)" -lt "$end" ]; do
      t0=$(date +%s%N)
      if run "$user" "$sql" | grep '"error"' >/dev/null; then s=fail; else s=ok; fi
      printf '%s %s %s\n' "$t0" "$s" "$(date +%s%N)"
      sleep "$interval"
    done
    ;;
  *)
    echo "latency-run.sh: unknown mode $mode" >&2
    exit 2
    ;;
esac
