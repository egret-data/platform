#!/usr/bin/env bash
# Make egret.localtest.me resolve cluster-wide to the ingress-nginx controller, so
# in-cluster pods — including provisioned engine pods in egret-engines that fetch the
# OIDC JWKS at boot from http://egret.localtest.me/auth/realms/egret/... — can reach it.
#
# Idempotent: inserts a `rewrite name egret.localtest.me ...` line before the `ready`
# plugin inside the `.:53 { ... }` server block of the CoreDNS Corefile, then restarts
# CoreDNS. Safe to run repeatedly. Run AFTER `just ingress-install`.
set -euo pipefail

HOST="egret.localtest.me"
TARGET="ingress-nginx-controller.ingress-nginx.svc.cluster.local"
REWRITE="rewrite name ${HOST} ${TARGET}"

corefile="$(kubectl -n kube-system get configmap coredns -o jsonpath='{.data.Corefile}')"

if printf '%s' "$corefile" | grep -q "$HOST"; then
  echo "CoreDNS already rewrites ${HOST}; nothing to do."
  exit 0
fi

# Insert the rewrite line (matching the indentation of the `ready` line) right before
# the first `ready` plugin, which lives in the default `.:53` server block.
patched="$(printf '%s' "$corefile" | awk -v r="$REWRITE" '
  !done && $1 == "ready" {
    match($0, /^[[:space:]]*/)
    indent = substr($0, 1, RLENGTH)
    print indent r
    done = 1
  }
  { print }
')"

if ! printf '%s' "$patched" | grep -q "$HOST"; then
  echo "ERROR: could not find a 'ready' line in the CoreDNS Corefile to anchor the rewrite." >&2
  exit 1
fi

# Apply the patched Corefile.
kubectl -n kube-system create configmap coredns \
  --from-literal=Corefile="$patched" \
  --dry-run=client -o yaml | kubectl -n kube-system apply -f -

kubectl -n kube-system rollout restart deployment coredns
echo "CoreDNS patched: ${HOST} -> ${TARGET}"
