#!/bin/sh
# E2E ONLY (Task 21). Runs INSIDE a Trino 483 container (it has sh, curl and sed; no python, no
# jq): `docker exec -i <container> sh -c "$(cat trino-batch.sh)" trino-batch <admin> < jobs`.
#
# Each stdin line is one job: `label TAB sid TAB user TAB original-user TAB sql`, `-` for no
# original user (a tab is IFS whitespace: an empty field would collapse). The statement
# goes over Trino's client protocol to 127.0.0.1:8080 with `X-Trino-User: user` and, when
# original-user is set, `X-Trino-Original-User: original-user`. That second header is what makes
# Trino authorize an impersonation (ImpersonateUser): measured on both engines, the CLI's
# `--user A --session-user B` over insecure HTTP claims B outright and checks nothing.
#
# The sql `KILL` is the kill_query family: the admin ($1) starts a long query, the job's user
# runs `CALL system.runtime.kill_query` against it, and the admin kills it afterwards.
#
# Output, per job: `@@BEGIN label sid`, every protocol response on one line, `@@END label sid`.
set -u
ADMIN=$1
URL=http://127.0.0.1:8080/v1/statement

# $1 = user, $2 = original user or "", $3 = sql; prints each response on its own line.
run() {
  if [ -n "$2" ]; then
    r=$(curl -sS -X POST "$URL" -H "X-Trino-User: $1" -H "X-Trino-Original-User: $2" \
      -H "X-Trino-Source: egret-layer2" --data-binary "$3" | tr -d '\n')
  else
    r=$(curl -sS -X POST "$URL" -H "X-Trino-User: $1" -H "X-Trino-Source: egret-layer2" \
      --data-binary "$3" | tr -d '\n')
  fi
  while :; do
    printf '%s\n' "$r"
    n=$(printf '%s' "$r" | sed -n 's/.*"nextUri":"\([^"]*\)".*/\1/p')
    [ -z "$n" ] && break
    r=$(curl -sS "$n" -H "X-Trino-User: $1" | tr -d '\n')
  done
}

first_value() { # the first value of the first data row of a run's output
  sed -n 's/.*"data":\[\["\([^"]*\)".*/\1/p' | head -n 1
}

tab=$(printf '\t')
while IFS="$tab" read -r label sid user orig sql; do
  [ "$orig" = - ] && orig=""
  printf '@@BEGIN %s %s\n' "$label" "$sid"
  if [ "$sql" = KILL ]; then
    tag="l2kill$$$(date +%s%N)"
    # Followed in the background, so the query keeps running until it is killed.
    run "$ADMIN" "" "SELECT count(*) FROM UNNEST(sequence(1, 3000)) a(x) CROSS JOIN UNNEST(sequence(1, 3000)) b(y) CROSS JOIN UNNEST(sequence(1, 3000)) c(z) WHERE x + y + z > 0 /* $tag */" >/dev/null 2>&1 &
    bg=$!
    qid=""
    i=0
    while [ -z "$qid" ] && [ $i -lt 60 ]; do
      qid=$(run "$ADMIN" "" "SELECT query_id FROM system.runtime.queries WHERE state NOT IN ('FINISHED', 'FAILED') AND query LIKE '%/* $tag */%' AND query NOT LIKE '%system.runtime.queries%'" | first_value)
      i=$((i + 1))
      [ -z "$qid" ] && sleep 0.5
    done
    if [ -z "$qid" ]; then
      printf '{"error":{"message":"layer2 harness: the kill target never ran","errorName":"HARNESS"}}\n'
    else
      run "$user" "$orig" "CALL system.runtime.kill_query(query_id => '$qid', message => 'layer2')"
      run "$ADMIN" "" "CALL system.runtime.kill_query(query_id => '$qid', message => 'layer2 cleanup')" >/dev/null
    fi
    kill "$bg" 2>/dev/null
    wait "$bg" 2>/dev/null
  else
    run "$user" "$orig" "$sql"
  fi
  printf '@@END %s %s\n' "$label" "$sid"
done
