# Trino data-plane + kind-deploy follow-ups

> Findings from the 2026-07-18 in-cluster end-to-end smoke of the `feat/deploy-egret-kind` work
> (egret deployed onto kind via `deploy/helm/egret`, OIDC single-origin, Trino provisioned + queried).
> The full path was proven end-to-end (**OIDC → engine JWKS → JWT → TLS:8443 → impersonation → query**;
> `SELECT current_user` returned the caller `admin`, not the connection principal `egret_delegator`).
> These three items were surfaced along the way and are **not** blockers for the deploy itself.

> Vietnamese reference: `docs/vi/plans/2026-07-18-trino-dataplane-deploy-followups.md`.

## 1. [BUG] Trino coordinator Service name mismatch — plain-provisioned Trino is unreachable

**Symptom:** any SQL Lab query against a Trino engine fails with
`connect: error sending request for url (https://{name}-coordinator.{ns}.svc:8443/v1/statement)`.

**Cause:** egret provisions Trino as plain Deployments (`{name}-coordinator`, `{name}-worker`) plus a
**single Service named `{name}-trino`** (ports `http:8080`, `https:8443`), but the connection layer
dials the host **`{name}-coordinator.{ns}.svc`** — for which no Service exists. The provisioner's
Service name and the connection's `coordinator_host`/endpoint disagree. This was almost certainly
masked historically by the Stackable `trino-operator`, which creates a `{name}-coordinator` Service;
the recreated kind cluster in this smoke had no Stackable operator, so the plain-Deployment path was
exercised and the mismatch showed.

**Fix (pick one, make them agree):**
- name the plain-provisioned coordinator Service `{name}-coordinator`, **or**
- have the connection dial `{name}-trino`.
Add a provisioning test asserting the Service name equals the dialed `coordinator_host`.

**Workaround used in the smoke:** created an alias Service `{name}-coordinator`
(selector `app: {name}-coordinator`, ports 8080/8443) so the dial resolved.

## 2. [GAP] Trino always uses the Ranger access-control plugin → delegated queries deny-all with no Ranger server

**Symptom:** after auth succeeds, an impersonated query fails with
`Access Denied: Principal egret_delegator cannot become user admin`.

**Cause:** egret writes `access-control.name=ranger` (`ranger.service.name={name}`,
`ranger-trino-security.xml`) into the coordinator config **regardless of `config.ranger.enabled`**.
With no Ranger server deployed the plugin fail-closes (deny-all), so egret's own service account
(`egret_delegator`) is not permitted to impersonate the caller. This is the documented
"ranger-configured-but-no-server = delegated-denied" trap.

**Fix (pick one):**
- when `ranger.enabled=false` (or no Ranger is reachable), provision Trino with a permissive/file-based
  access control that allows `egret_delegator` impersonation, **or**
- require a Ranger server and seed an impersonation policy (`egret_delegator` → any user) as part of
  provisioning.

**Workaround used in the smoke:** patched the coordinator config to `access-control.name=allow-all`
and restarted the coordinator → the impersonated query ran (`current_user` = the caller).

## 3. [DX] Deploy chart/image nits

- **Backend image cache-busting.** `deploy/docker/backend.Dockerfile` does `COPY . .` before
  `cargo build`, so **any** file change (a chart or docs edit) invalidates the cargo build layer and
  forces a full 20–40 min recompile on every `just deploy-kind`. Fix with cargo-chef, or copy the
  workspace `Cargo.toml`/`Cargo.lock` + crate manifests first and build deps in a cached layer, then
  copy sources.
- **`egret init` does not seed a default workspace.** Its help says "Bootstrap the initial admin user
  and default workspace", but a fresh deploy has **0 workspaces** (the platform admin is created, no
  workspace). Either create a default workspace in `init`, or correct the help text.
- **OIDC-off engine JWT is non-bootable (resolved by Milestone B, worth guarding).** With OIDC
  disabled the engine JWKS URI is empty (`engine_jwks_uri` is gated on `oidc.enabled`), so enabling
  "Accept egret JWT" on an engine writes a blank `http-server.authentication.jwt.key-file` and the
  Trino coordinator crashes (`FileSigningKeyLocator` / `SecurityException`). egret should either block
  enabling engine-JWT when no JWKS is available, or serve its own JWKS endpoint for egret-issued JWTs
  independent of external OIDC. (Enabling OIDC — Milestone B — populates the JWKS and fixes it.)
