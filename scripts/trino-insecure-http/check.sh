#!/usr/bin/env bash
# Measures Trino 483 behaviour with allow-insecure-over-http true vs false.
# Own compose project; secrets/keystore are generated per run into a temp dir.
set -uo pipefail
cd "$(dirname "$0")"
export COMPOSE_PROJECT_NAME=trino-insecure-http
WORK=$(mktemp -d); export WORK
trap 'docker compose -p trino-insecure-http down -v >/dev/null 2>&1; rm -rf "$WORK"' EXIT
DELEG_PW=$(openssl rand -hex 8); MON_PW=$(openssl rand -hex 8)
export TRINO_INTERNAL_SECRET=$(openssl rand -hex 16)
{ for u in "egret_delegator:$DELEG_PW" "egret-monitor:$MON_PW"; do
    htpasswd -nbBC 10 "${u%%:*}" "${u#*:}"; done; } | sed 's/\$2[aby]\$/$2y$/' > "$WORK/password.db"
openssl req -x509 -newkey rsa:2048 -nodes -days 1 -subj /CN=coordinator \
  -addext subjectAltName=DNS:coordinator,DNS:localhost \
  -keyout "$WORK/k.pem" -out "$WORK/c.pem" 2>/dev/null
cat "$WORK/k.pem" "$WORK/c.pem" > "$WORK/keystore.pem"; chmod 644 "$WORK/keystore.pem" "$WORK/password.db"

H=http://localhost:18080; S=https://localhost:18443
run() { python3 -I ./query.py "$@"; }
cli() { docker exec trino-insecure-http-coordinator-1 trino --server "$1" "${@:2}" 2>&1 | tr '\n' ' ' | cut -c1-300; echo; }

FAILED=0
CNAME=trino-insecure-http-coordinator-1; WNAME=trino-insecure-http-worker-1
wait_ready() { # fatal unless $1=soft
  for _ in $(seq 90); do curl -s $H/v1/info 2>/dev/null | grep -q '"starting":false' && return 0; sleep 2; done
  return 1
}
wait_nodes() { # $1.. = query.py auth args over $B
  for _ in $(seq 60); do
    case $(run "$@" 2>/dev/null) in *"[[2]]"*) return 0;; esac; sleep 2; done
  return 1
}
for cfg in tls notls; do
  conf=coordinator.config.properties; [ $cfg = notls ] && conf=coordinator-notls.config.properties
  for flag in true false; do
    echo "=== config=$cfg (https listener: $([ $cfg = tls ] && echo yes || echo NO)) allow-insecure-over-http=$flag ==="
    COORD_CONF=$conf ALLOW_INSECURE=$flag docker compose -p trino-insecure-http up -d --force-recreate >/dev/null 2>&1
    if ! wait_ready; then
      echo "COORDINATOR DID NOT REACH starting:false within 180s (state: $(docker inspect -f '{{.State.Status}} exit={{.State.ExitCode}}' $CNAME))"
      docker logs $CNAME 2>&1 | grep -E 'ERROR|Exception|must|not ' | head -8 | cut -c1-300
      [ $cfg = tls ] && FAILED=1
      continue
    fi
    if [ $cfg = tls ]; then
      wait_nodes $S "SELECT count(*) FROM system.runtime.nodes" -u "egret_delegator:$DELEG_PW" -H 'X-Trino-User: egret_delegator' || { echo "TIMEOUT waiting for 2 nodes"; FAILED=1; }
    else
      wait_nodes $H "SELECT count(*) FROM system.runtime.nodes" -u "egret-monitor:$MON_PW" -H 'X-Trino-User: egret-monitor' -H 'X-Forwarded-Proto: https' || echo "TIMEOUT waiting for 2 nodes (notls)"
    fi
    echo "1 plain http, X-Trino-User bob:           $(run $H 'SELECT current_user' -H 'X-Trino-User: bob')"
    [ $cfg = tls ] && echo "2 CLI --user ann --session-user bob:      $(cli http://localhost:8080 --user ann --session-user bob --execute 'SELECT current_user')"
    echo "3 fwd https, no credential:               $(run $H 'SELECT current_user' -H 'X-Trino-User: bob' -H 'X-Forwarded-Proto: https')"
    echo "4 fwd https, monitor as itself:           $(run $H 'SELECT current_user' -u "egret-monitor:$MON_PW" -H 'X-Trino-User: egret-monitor' -H 'X-Forwarded-Proto: https')"
    echo "5 fwd https, monitor claiming bob:        $(run $H 'SELECT current_user' -u "egret-monitor:$MON_PW" -H 'X-Trino-User: bob' -H 'X-Forwarded-Proto: https')"
    echo "6 coordinator GET /v1/info:               $(curl -s -w ' HTTP %{http_code}' $H/v1/info)"
    echo "7 coordinator GET /v1/info/state (admin): $(curl -s -w ' HTTP %{http_code}' -H 'X-Trino-User: admin' $H/v1/info/state)"
    if [ $cfg = tls ]; then
      echo "8 nodes over https as delegator->ann:     $(run $S 'SELECT count(*) FROM system.runtime.nodes' -u "egret_delegator:$DELEG_PW" -H 'X-Trino-Original-User: egret_delegator' -H 'X-Trino-User: ann')"
      echo "9 current_user over https delegator->ann: $(run $S 'SELECT current_user' -u "egret_delegator:$DELEG_PW" -H 'X-Trino-Original-User: egret_delegator' -H 'X-Trino-User: ann')"
    else
      echo "8 nodes over http as monitor (fwd https): $(run $H 'SELECT count(*) FROM system.runtime.nodes' -u "egret-monitor:$MON_PW" -H 'X-Trino-User: egret-monitor' -H 'X-Forwarded-Proto: https')"
    fi
    echo "worker log ERROR/Exception lines:         $(docker logs $WNAME 2>&1 | grep -cE ' ERROR |Exception')"
    docker logs $WNAME 2>&1 | grep -E ' ERROR |Exception' | head -5 | cut -c1-200
    echo "worker log lines matching 'error' (case-insens): $(docker logs $WNAME 2>&1 | grep -ci error), config-table lines among them: $(docker logs $WNAME 2>&1 | grep -i error | grep -c 'io.trino.bootstrap.engine')"
    # real preStop (crates/egret-provisioner/src/trino.rs): PUT on the WORKER's localhost:8080; run last, it shuts the worker down
    echo "10 worker preStop PUT SHUTTING_DOWN:      $(docker exec $WNAME curl -s -w ' HTTP %{http_code}' -X PUT -d '"SHUTTING_DOWN"' -H 'Content-type: application/json' -H 'X-Trino-User: admin' http://localhost:8080/v1/info/state)"
    echo "10b worker state afterwards:              $(docker exec $WNAME curl -s -w ' HTTP %{http_code}' http://localhost:8080/v1/info/state)"
  done
done
# Supplemental: the worker as rendered has no access-control config, so the preStop PUT is refused at
# BOTH flag values above. Re-measure it with a worker that has file access control allowing `admin`
# system_information read+write, to see whether the flag changes the PUT.
for flag in true false; do
  echo "=== supplemental: worker WITH access control (admin may write system information), allow-insecure-over-http=$flag ==="
  ALLOW_INSECURE=$flag docker compose -p trino-insecure-http -f docker-compose.yml -f docker-compose.worker-ac.yml up -d --force-recreate >/dev/null 2>&1
  wait_ready || { echo "TIMEOUT waiting for coordinator"; FAILED=1; continue; }
  wait_nodes $S "SELECT count(*) FROM system.runtime.nodes" -u "egret_delegator:$DELEG_PW" -H 'X-Trino-User: egret_delegator' || { echo "TIMEOUT waiting for 2 nodes"; FAILED=1; }
  echo "10 worker preStop PUT SHUTTING_DOWN:      $(docker exec $WNAME curl -s -w ' HTTP %{http_code}' -X PUT -d '"SHUTTING_DOWN"' -H 'Content-type: application/json' -H 'X-Trino-User: admin' http://localhost:8080/v1/info/state)"
  sleep 3
  echo "10b worker state afterwards:              $(docker exec $WNAME curl -s -w ' HTTP %{http_code}' http://localhost:8080/v1/info/state)"
done
exit $FAILED
