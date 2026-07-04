# Trino authentication + Ingress exposure (Phase 1.6a, 2026-07-04)

VI reference: `docs/vi/specs/2026-07-04-trino-auth-ingress-design.md`.
Builds on Phase 1.5.2: `docs/en/specs/2026-07-04-trino-config-files-design.md` (shipped, #142).

**Goal:** Expose a provisioned Trino securely for real use:
1. **Authentication on Trino itself** — a **PASSWORD** authenticator (file-based, for
   external clients: BI/JDBC/CLI username+password) **and** a **JWT** authenticator
   (for egret's own querying via the existing per-user `EngineToken`, validated
   against the IdP's JWKS).
2. **Ingress exposure** — a standard Kubernetes `Ingress` (e.g. nginx) for the
   coordinator Service, so external clients reach Trino directly (TLS terminated at
   the ingress). **egret does NOT proxy queries** — that reverse-proxy idea was
   dropped; ingress does exposure, Trino does auth.

**Design pivot (why no proxy):** an earlier draft had egret reverse-proxy the Trino
query protocol. That is unnecessary complexity — standard k8s ingress exposes Trino,
and Trino's own PASSWORD/JWT auth secures it. egret's monitoring role (a native
query-monitor page over Trino's REST API) is **Phase 1.6b**, a separate spec.

**Non-goals (1.6a):** the egret query proxy (dropped); the monitoring page (1.6b);
autoscaling (1.5.4); observability (1.5.5); pod/scheduling (1.5.3); OAuth2/certificate
auth types (JWT + PASSWORD cover egret-internal + external).

## Chart-verified rendering
- `http-server.authentication.type={PASSWORD,JWT}` in config.properties.
- PASSWORD → `password-authenticator.properties` (`password-authenticator.name=file`,
  `file.password-file=/etc/trino/auth/password/password.db`) + a `password.db`
  (htpasswd format: `username:bcrypt-hash`, one per line).
- JWT → `http-server.authentication.jwt.key-file=<JWKS uri>` (+ optional
  `.jwt.required-audience`, `.jwt.principal-field`).
- Trino requires TLS for PASSWORD/JWT; since TLS terminates at the ingress and the
  coordinator stays HTTP internally, set
  `http-server.authentication.allow-insecure-over-http=true` +
  `http-server.process-forwarded=true`.
- Ingress: `networking.k8s.io/v1` `Ingress` (className, host, tls, annotations).

## Data model — engine config additions

Cluster-wide on `TrinoConfig` (auth is a coordinator concern):

```
auth?: {
  passwordUsers: [{ username: string, secretRef: string }]   // egret secret holding the plaintext password
  jwt?: { jwksUri: string, requiredAudience?: string }
}
ingress?: {
  enabled: boolean
  className?: string          // e.g. "nginx"
  host: string               // e.g. "trino-demo.egret.example.com"
  tlsSecretRef?: string      // k8s TLS secret name (optional)
  annotations?: Record<string,string>
}
```

`authentication.type` is derived: `PASSWORD` when `passwordUsers` is non-empty, `JWT`
when `jwt` is set, both (comma-joined, PASSWORD first) when both. Auth off ⇒ no auth
keys rendered (current behaviour preserved).

## Orchestrator rendering (`trino.rs`)

- **config.properties** (coordinator; workers don't authenticate clients): when auth
  is enabled, add `http-server.authentication.type=<derived>`,
  `http-server.authentication.allow-insecure-over-http=true`,
  `http-server.process-forwarded=true`; for JWT add
  `http-server.authentication.jwt.key-file={jwksUri}` (+ required-audience if set).
- **`password-authenticator.properties`** + **`password.db`** — when `passwordUsers`
  non-empty. `password.db` is `username:bcrypt(password)\n` per user; the orchestrator
  receives already-bcrypted entries (the control-plane hashes — see below) so the
  orchestrator crate needs no bcrypt dependency. SubPath-mount password.db at
  `/etc/trino/auth/password/password.db` and the properties file.
- **`trino_ingress(spec)`** — a new `networking.k8s.io/v1` `Ingress` object (name
  `{engine}-trino`, className, a rule for `host` → the `{engine}-trino` Service :8080,
  optional `tls: [{hosts:[host], secretName: tlsSecretRef}]`, annotations). The
  provisioner applies it when `ingress.enabled`, and deletes it otherwise / on engine
  delete.

## Control-plane

- Carriers gain `auth` + `ingress`. **Password hashing**: the control-plane resolves
  each `passwordUsers[].secretRef` via `st.secrets.resolve_plaintext` and **bcrypts**
  it (new `bcrypt` dep in egret-controlplane or egret-bin), passing
  `username:hash` entries to the orchestrator (so bcrypt lives control-plane-side,
  not in the kube layer). 400 if a referenced secret is missing.
- **JWKS default**: if `jwt.jwksUri` is omitted but JWT is enabled, default to the
  IdP JWKS egret already uses for `EngineToken` (Keycloak realm JWKS — a config value
  `auth.oidc.jwks_uri`, or the egret machine-IdP JWKS). Documented; the admin can
  override per engine.
- The provisioner gains an `Ingress` in its apply/delete set (guarded by
  `ingress.enabled`).

## Frontend (EngineDetail)

A new **Exposure & Auth** card (Cluster area):
- **Password users**: a list — username `<Input>` + a secret `<Select>` (`listSecrets`)
  for the password; add/remove.
- **JWT**: a toggle + an optional `jwksUri` `<Input>` (placeholder = the default IdP
  JWKS) + optional required-audience.
- **Ingress**: an "Expose via ingress" toggle → `host` `<Input>`, `className` `<Input>`
  (default nginx), an optional TLS-secret `<Input>`, and an annotations `KeyValueEditor`.
- Save → `updateEngine` → rolls the coordinator (auth) + applies/updates the Ingress.
  A note that clients then connect to `https://{host}` with their username+password
  (or the egret JWT).

## Test

- **Unit** (fake kube): config.properties has `authentication.type=PASSWORD,JWT` +
  `allow-insecure-over-http` + `process-forwarded` + the jwt key-file;
  `password.db` contains `bob:$2b$...` (a bcrypt hash, verified by `bcrypt::verify`);
  the `Ingress` object has the host rule → the Service + the tls block; nothing renders
  when auth/ingress are unset.
- **Live (kind)**: enable a password user (bob) → provision → `curl` the coordinator
  Service from inside the cluster with `-u bob:<pw>` on `/v1/statement` → 200; with a
  wrong password → 401; without auth header → 401. (The `Ingress` object is applied +
  its YAML verified; end-to-end ingress routing needs an nginx-ingress controller on
  kind — opt-in, documented, not required for the auth proof.)

## Compat
- All fields optional; existing engines keep no-auth (unchanged) until a user enables
  it. No DB migration.
- `allow-insecure-over-http=true` is safe here: TLS terminates at the ingress and the
  coordinator is in-cluster; the setting only permits auth over the trusted internal
  hop.
