# Engine JWT Delegation Default-On — Design

> **Status:** Design approved (brainstorm). Next: implementation plan.
> **Builds on:** the STS effort (#304–308). egret's machine-IdP is already the
> single issuer of per-user engine tokens; this makes engines *use* it by default.
> *(This English spec is canonical; `docs/vi/...` is the Vietnamese reference.)*

**Goal:** When an admin provisions a Trino or StarRocks engine **without**
specifying a delegation mode, egret defaults to **per-user JWT delegation** (the
engine trusts egret's machine-IdP), instead of today's "open" service-user
engine. When per-user JWT isn't possible on the platform, egret **gracefully
degrades to `impersonate`** rather than failing or silently producing a broken
engine — so a default provision is always governed per-user and never hard-fails.

## 1. Decisions (settled)

1. **Where the default lives:** backend **default-when-omitted** — an omitted /
   blank `delegationMode` resolves to a concrete mode server-side (§3). The
   frontend wizard also reflects the default (pre-selects JWT, exposes the new
   opt-out). This is a true default-on, not just a UI nicety.
2. **Graceful degradation, not hard-fail, on the default path:** the default
   resolution *prefers* `jwt` but falls back to `impersonate` when `jwt` can't
   be guaranteed (no platform JWKS, or a StarRocks image that clearly predates
   `authentication_jwt`). `impersonate` needs no JWKS and no minimum engine
   version and works for every user (local + OIDC), so it is the safe universal
   fallback. The default path therefore **never hard-fails** and never provisions
   a broken engine.
3. **Hard-fail only on an *explicit* impossible request:** if an admin passes
   `delegationMode:"jwt"` **explicitly** but the platform has no machine-IdP JWKS
   (`catalog.public_url` unset), egret returns **400** — it honors the explicit
   intent rather than silently downgrading. (Trino's `resolve_auth` already does
   this; StarRocks gains the same guard.)
4. **Explicit opt-out:** `delegationMode:"none"` (new accepted value) provisions
   an **open** engine — the pre-change behavior — so deployments that want a
   service-user engine, or that have no `public_url`, retain an escape hatch.
5. **StarRocks TLS:** a resolved `jwt` mode auto-enables FE TLS on :9030 and
   generates a per-engine cert (StarRocks sends the JWT as a cleartext MySQL
   password, so TLS is mandatory — already fail-closed in the connector). A
   resolved `impersonate` mode keeps today's behavior (no cleartext JWT).

## 2. Existing seams (reused)

Both engines already key delegation off a `delegationMode` string and derive the
OIDC coordinates server-side (never from the request):

- **`st.engine_jwks_uri` / `st.engine_oidc_issuer`** (`egret-bin/main.rs:1518`) —
  egret's own machine-IdP JWKS + issuer, set unconditionally of `oidc.enabled`
  from `catalog.public_url`. **Empty iff `public_url` is unset** — this is the
  single reliable "can we do JWT?" signal at provision time.
- **StarRocks:** `StarRocksConfigBody.delegation_mode` (`engines.rs:198`);
  `validate_sr_delegation_mode` (`engines.rs:2478`); the create path
  `create_starrocks_engine` (`engines.rs:2714`) persists a `delegation {mode,
  tlsEnabled}` block; the provision-req builder (`engines.rs:~2314`) reads it back
  into `StarRocksDelegation` + `StarRocksDelegationBootstrap`;
  `starrocks_jwt_integration_sql` (`engines.rs:3099`) renders the
  `CREATE SECURITY INTEGRATION ... authentication_jwt` DDL.
- **Trino:** `trinoConfig.delegationMode` (`"jwt"|"impersonate"`) + the
  `trinoConfig.auth.jwt` block resolved by `resolve_auth` (`engines.rs:1341`,
  already defaults an empty `jwksUri` to `st.engine_jwks_uri` and audience to
  `"engine"`, and already 400s when JWT is requested with no JWKS);
  `inject_delegator_user` (`engines.rs:1456`) seeds the impersonate service user;
  `build_provision_req` (`engines.rs:544`) assembles the request.
- **Connector fail-closed:** `StarRocksConnector` refuses a bearer over a non-TLS
  socket (`egret-query/.../connector/starrocks.rs`) — so a jwt engine without TLS
  is already unusable; §1.5 makes provisioning enable TLS so this never triggers.

## 3. The default-resolution rule

A single helper decides the concrete mode at **create** time and the result is
**persisted** into the stored spec's delegation block. Because the resolved mode
is stored, the provision-req builder and the "UPDATE preserves stored value" path
need no change.

```
resolve_delegation(explicit: Option<&str>, has_jwks: bool, sr_image_tag: Option<&str>) -> Resolved
  match explicit {
    Some("none")                    => Open,                    // opt-out
    Some("impersonate")             => Impersonate,             // honored
    Some("jwt") if has_jwks         => Jwt,                     // honored
    Some("jwt")                     => Err(400 no-JWKS),        // explicit + impossible
    None | Some("")  =>                                          // DEFAULT (omitted)
      if has_jwks && !starrocks_image_is_pre_3_5(sr_image_tag)  => Jwt
      else                                                      => Impersonate
  }
```

- **`has_jwks`** = `!engine_jwks_uri.is_empty() && !engine_oidc_issuer.is_empty()`.
- **`starrocks_image_is_pre_3_5`** = best-effort parse of the `fe_image` tag
  (`starrocks/fe-ubuntu:<tag>`): if the tag clearly parses to `major.minor < 3.5`
  return true; an unparseable/absent tag returns **false** (assume capable — the
  default image is 4.1). This **only steers the default choice toward
  `impersonate`**; it is never a hard-fail. For Trino, `sr_image_tag` is always
  `None` (Trino's JWT authenticator has no such version floor), so Trino's default
  is purely `has_jwks ? jwt : impersonate`.

**Applied per engine:**
- **StarRocks** (`create_starrocks_engine`): resolve, then persist the resolved
  `mode` into the `delegation` block; set `tlsEnabled=true` when resolved `jwt`;
  when resolved `Open`, persist no delegation block (open engine, as today).
- **Trino** (`build_provision_req` / `resolve_auth`): resolve; a resolved `jwt`
  synthesizes the `auth.jwt` block (platform JWKS + aud `"engine"`) and sets
  `delegationMode="jwt"`; a resolved `impersonate` sets `delegationMode=
  "impersonate"` (→ `inject_delegator_user`); `Open` leaves auth/delegation unset.

## 4. Result matrix (admin sends **no** delegation field)

| Platform | Trino | StarRocks |
|---|---|---|
| `public_url` set, SR image ≥3.5 (default 4.1) | **jwt** | **jwt + FE TLS** |
| `public_url` set, SR image clearly <3.5 | **jwt** | **impersonate** |
| `public_url` unset | **impersonate** | **impersonate** |

Explicit `delegationMode`: `"jwt"` honored (400 if no JWKS), `"impersonate"` /
`"none"` honored verbatim.

## 5. Error handling

- Explicit `delegationMode:"jwt"` + no platform JWKS → **400 BadRequest** with a
  message naming both fixes: set `catalog.public_url`, or pass
  `delegationMode:"none"`/`"impersonate"`. (Trino already emits this; StarRocks
  gains it.)
- Default path (omitted) → never 400 on capability; degrades to `impersonate`.
- StarRocks resolved `impersonate` because of a pre-3.5 image → a `tracing::info`
  noting the degrade (observability; not an error).
- A genuinely-old StarRocks image passed with **explicit** `"jwt"` still can't be
  detected reliably; the `authentication_jwt` DDL then fails at bootstrap and
  surfaces as an engine error (unchanged from today, see #303).

## 6. Backward compatibility

- **Existing engines are untouched** until re-provisioned/updated; their stored
  delegation block already holds an explicit mode (or none), which the resolver
  passes through.
- **The default resolver is invoked ONLY by the create path; UPDATE never runs
  it.** An update with an omitted `delegationMode` preserves the stored delegation
  block verbatim — an existing open engine is not silently flipped to jwt on an
  unrelated config edit.
- **`"none"` reproduces the old open-engine behavior** for callers that relied on
  "omitted = open".
- OpenAPI: document the new default + the `"none"` value; regenerate `openapi.json`
  (the drift test enforces this).

## 7. Frontend (reflect the default)

The delegation selector gains an explicit **"Auto (JWT, else impersonate)"** entry
that is the **default selection** and **sends an omitted `delegationMode`** — so
the backend's graceful resolution (§3) applies. It must **not** send an explicit
`"jwt"` to represent the default: an explicit `"jwt"` on a platform without JWKS
is a 400 by design (§5), which would turn the intended graceful degrade into a
hard failure. The selector also exposes explicit **"JWT"**, **"Impersonate"**, and
**"None (open engine)"** (`delegationMode:"none"`) for admins who want to pin a
mode. No behavior beyond sending the value; the backend is authoritative.
(`useEngineConfigForm.ts`, the Security tab in `EngineDetail.tsx`.)

## 8. Testing

- **Unit (`engines.rs` tests):** StarRocks omitted+JWKS+4.1 ⇒ jwt & `tlsEnabled`;
  omitted+JWKS+3.4-tag ⇒ impersonate; omitted+no-JWKS ⇒ impersonate; explicit
  `"jwt"`+no-JWKS ⇒ 400; explicit `"none"` ⇒ open. Trino omitted+JWKS ⇒ jwt
  (auth.jwt synthesized, aud `"engine"`); omitted+no-JWKS ⇒ impersonate; explicit
  `"jwt"`+no-JWKS ⇒ 400. UPDATE omitted preserves stored mode. `starrocks_image_
  is_pre_3_5` parse: `3.4`/`3.4.10`→true, `4.1`/`latest`/`""`→false.
- **Live e2e (kind, opt-in):** provision a Trino **and** a StarRocks engine with
  **no** `delegationMode` on the `public_url` kind deploy ⇒ both come up
  jwt-delegated; a per-user query runs (admin-full vs member-masked), reusing the
  `sts-broker-kind` harness. Recipe `just e2e-kind-delegation-default`.

## 9. Out of scope

- No retro-migration of already-running engines (re-provision to adopt the default).
- No per-engine `jwksUri`/`issuer` override surface for StarRocks (unchanged).
- No change to the `impersonate` or `"none"` code paths beyond making them
  reachable as defaults/opt-out.
- Engines other than Trino/StarRocks.
