#!/usr/bin/env bash
# Block until the in-cluster egret deploy is ready to serve e2e:
#   - egret + keycloak pods Ready
#   - OIDC discovery converged (/api/auth/config returns 200 through the ingress)
# Run after `just deploy-kind-oidc`, before the Playwright e2e.
set -euo pipefail

NS="${EGRET_NS:-egret-system}"
BASE="${EGRET_E2E_BASE_URL:-http://egret.localtest.me}"

echo "waiting for egret pods in ${NS}..."
kubectl wait --for=condition=Ready pod -l app=egret -n "${NS}" --timeout=300s
# Keycloak may be absent on an OIDC-off deploy; don't fail if the label matches nothing.
kubectl wait --for=condition=Ready pod -l app=keycloak -n "${NS}" --timeout=300s || \
  echo "note: no keycloak pods matched (OIDC-off deploy?) — continuing"

echo "waiting for OIDC discovery at ${BASE}/api/auth/config ..."
for i in $(seq 1 60); do
  if curl -sf "${BASE}/api/auth/config" >/dev/null 2>&1; then
    echo "auth-config is up (attempt ${i})"
    exit 0
  fi
  sleep 5
done
echo "ERROR: ${BASE}/api/auth/config did not return 200 within ~5m" >&2
exit 1
