# B4 — Real audit_log

> **Status:** Design (approved to plan) · **Date:** 2026-07-11 · **Version:** 1.0
>
> **Parent:** [Effort B authorization design](../architecture/2026-07-10-effort-b-authorization-design.md) §5 (B4, the cross-cutting audit sub-epic) · **Builds on:** B1a–B1b (the Cedar PDP chokepoint + `Decision{allow, reasons}`), B2-flip (all grant mutations now flow through `access_grants` writers).
>
> The final Effort B increment. Records every PDP authorization decision (allow + deny) and every grant mutation into a dedicated `audit` schema, queryable by a platform admin. Closes the "real audit" cross-cutting concern the north-star reserved for B4.

---

## 1. Goal & scope

The Cedar PDP is the single enforcement chokepoint (`authorize(subject, action, resource) -> Decision{allow, reasons}`, reached by every `require`/`require_object` PEP), and its `reasons` already carry the contributing Cedar policy ids. B2-flip made `access_grants` the single grant store, so every grant change flows through a small set of writers. B4 turns both into an **append-only audit trail**: an `AuditingPdp` decorator captures every decision with zero PEP-call-site churn; the grant-mutation handlers emit a mutation event; both feed one async best-effort sink that batch-inserts into `audit.audit_log`. A platform-admin query API reads it back.

**In scope:** the `audit` schema + `audit_log` table; an async `AuditSink` (bounded channel + background batch writer, best-effort drop on overload); an `AuditingPdp` decorator around `CedarPdp` (captures decisions); mutation-event emission at the ~8 grant-mutation handlers; a background retention prune; a `GET /control-plane/audit` filtered/paginated query API (platform-admin only); config knobs.

**Out of scope:** a frontend audit-viewer page (backend + query API only — a page is a later increment); guaranteed/durable delivery (the user chose best-effort — dropped events under overload are acceptable); shipping to an external SIEM; auditing raw SQL / migration writes to `access_grants` (only app mutations are captured); data-plane query audit (Trino/Ranger has its own logs).

**Decisions locked in brainstorming:** capture **mutations + denies + allows** (full trail); **all events async best-effort** through one buffered channel + a background batch writer (never block or fail a request; drop on overflow); **backend + query API only** (no UI); mutations are **emitted at the grant-mutation handlers** (not a DB trigger) so one uniform channel carries both classes and the emit has the actor + before/after; a **system/non-request** event uses `actor_user_id = NULL` + `detail.source = "system"`.

## 2. Schema (`audit` schema)

New migration (repo is past 0075):

```sql
CREATE SCHEMA IF NOT EXISTS audit;

CREATE TABLE audit.audit_log (
    id            bigserial PRIMARY KEY,
    ts            timestamptz NOT NULL DEFAULT now(),
    kind          text NOT NULL CHECK (kind IN ('decision','mutation')),
    actor_user_id uuid,                       -- NULL for system/non-request events
    workspace_id  uuid,                       -- NULL for platform-scope
    action        text NOT NULL,              -- e.g. 'dashboard:read', 'grant:create'
    resource_type text NOT NULL,              -- 'platform'|'workspace'|'object'|'grant'|...
    resource_id   text,                       -- object id / scope id (text — object ids are "kind:uuid")
    decision      text CHECK (decision IN ('allow','deny')),  -- NULL for mutations
    reasons       text[],                     -- contributing Cedar policy ids (decisions)
    detail        jsonb NOT NULL DEFAULT '{}' -- before/after for mutations; context for decisions
);

CREATE INDEX audit_log_ts_idx     ON audit.audit_log (ts DESC);
CREATE INDEX audit_log_actor_idx  ON audit.audit_log (actor_user_id, ts DESC);
CREATE INDEX audit_log_action_idx ON audit.audit_log (action, ts DESC);
```

Append-only: nothing UPDATEs or DELETEs rows except the retention prune (§6). The dead `policy_audit` table (migration 0007) is left untouched (a separate legacy artifact).

## 3. The async sink (best-effort)

`egret-audit` (new crate, or a module in egret-core) defines:

```rust
pub enum AuditEvent {
    Decision { actor: Option<Uuid>, workspace_id: Option<Uuid>, action: String,
               resource_type: String, resource_id: Option<String>, allow: bool,
               reasons: Vec<String> },
    Mutation { actor: Option<Uuid>, workspace_id: Option<Uuid>, action: String,
               resource_type: String, resource_id: Option<String>, detail: serde_json::Value },
}

#[derive(Clone)]
pub struct AuditSink { tx: tokio::sync::mpsc::Sender<AuditEvent> }

impl AuditSink {
    /// Non-blocking: on a full channel the event is dropped (best-effort) and a
    /// counter/warn is incremented — never blocks or fails the caller.
    pub fn emit(&self, ev: AuditEvent) { let _ = self.tx.try_send(ev); }
}
```

A background writer task owns the `Receiver`, accumulates up to `batch_size` events (or `flush_interval_ms`), and does one multi-row INSERT into `audit.audit_log`. On shutdown it drains the channel. A dropped-event counter is logged periodically (observability for overload). `AuditSink::emit` is `try_send` — the request path never awaits a DB write.

## 4. Decisions — `AuditingPdp` decorator

`egret-auth` gains `AuditingPdp { inner: Arc<dyn Pdp>, sink: AuditSink }` implementing `Pdp`:

```rust
#[async_trait]
impl Pdp for AuditingPdp {
    async fn authorize(&self, subject: &Subject, action: &str, resource: &Resource) -> Result<Decision> {
        let decision = self.inner.authorize(subject, action, resource).await?;
        self.sink.emit(AuditEvent::Decision {
            actor: Some(subject.user_id.0),
            workspace_id: resource.workspace_id(),   // helper: Workspace/Object → Some, Platform → None
            action: action.to_string(),
            resource_type: resource.type_str(),      // "platform"|"workspace"|"object"
            resource_id: resource.id_str(),          // ws uuid / "kind:uuid" / None
            allow: decision.allow,
            reasons: decision.reasons.clone(),
        });
        Ok(decision)
    }
}
```

egret-bin wraps the real `CedarPdp` in `AuditingPdp` before putting it behind the shared `Arc<dyn Pdp>` that every state holds. Because every `require`/`require_object` calls `pdp.authorize`, this captures **every enforcement decision** (allow + deny) with **zero change to the 187 PEP call sites**. (Non-enforcement code — FE `/me` gating, `has_object_grant_in_workspace` admission — does not call `authorize`, so it is correctly not decision-audited.)

## 5. Mutations — emit at the grant-mutation handlers

The handlers that change `access_grants` (post-B2-flip) emit a `Mutation` event to the same sink after a successful mutation. The actor is `ctx.principal.user_id` (never client-supplied). Sites (~8):
- `egret-controlplane` access console: role `assign`/`unassign`, `replace_platform_roles`, (role create/update/delete + set_permissions if we audit role-def changes too), member `add`/`remove`.
- `egret-bi` shares: `grant_put` (allow/deny/Blocked), `grant_delete`.

The `action` is a mutation verb (`grant:create`, `grant:revoke`, `role:assign`, `member:add`, …); `detail` carries the before/after (principal, role, scope, effect). The handler already holds `ctx`/`Auth(p)` for the actor. A thin helper `sink.emit(AuditEvent::Mutation{...})` at each site keeps it uniform. (Handlers, not repos, so the actor + HTTP context are in scope; a repo-level or DB-trigger approach can't see the request actor for a DELETE.)

## 6. Retention

A background prune task deletes rows older than `audit.retention_days` (default 90) on an interval (e.g. hourly): `DELETE FROM audit.audit_log WHERE ts < now() - ($1 || ' days')::interval`. Best-effort; logs the deleted count. Keeps the full-volume table bounded without partitioning (MVP; partition later if needed).

## 7. Query API (no UI)

`GET /api/control-plane/audit` — **platform-admin only** (gate via the existing admin-graph/`role:read`-class check used by the access console; confirm the precise gate during the plan). Query params: `actor` (uuid), `action` (string), `kind` (decision|mutation), `decision` (allow|deny), `from`/`to` (timestamptz), `workspace` (uuid), `limit` (default 100, max 500), `cursor` (keyset on `(ts, id)` for stable pagination). Returns newest-first `AuditEntryDto[]` + a `next_cursor`. utoipa + openapi + generated client (even without a page, the client stays current). No write endpoints (append-only; the writer + prune are the only mutators).

## 8. Config

`audit.enabled` (default true — false skips wrapping the PDP + emitting, for perf/testing), `audit.retention_days` (90), `audit.channel_capacity` (8192), `audit.batch_size` (256), `audit.flush_interval_ms` (1000). Sensible defaults so a stock deploy audits without tuning.

## 9. Security

- **Append-only + admin-only read:** no API updates/deletes rows; the query API is platform-admin-gated; the prune is the only deleter (retention).
- **Actor integrity:** the actor is the authenticated principal (decision: `subject.user_id`; mutation: `ctx.principal`) — never a request field. System events use `NULL` + `detail.source="system"`.
- **Best-effort is a stated tradeoff:** under overload the sink drops events (a dropped-count is logged) — acceptable per the user's choice; it never blocks or fails a request, and never weakens enforcement (audit is observational, orthogonal to the PDP decision).
- **No secret leakage:** `detail` carries grant metadata (principal/role/scope/effect) and decision context (action/resource/reasons) — never tokens, passwords, or query data.
- **PDP parity untouched:** `AuditingPdp` delegates the decision verbatim to `CedarPdp` and only observes it — the decision matrix is unchanged.

## 10. Testing

- **Decision capture:** `AuditingPdp` around a fake/real Pdp emits exactly one `Decision` event per `authorize` (both an allow and a deny), with the right actor/action/resource/allow/reasons.
- **Mutation capture:** a grant `assign`/`grant_put`/`grant_delete` through the handler emits a `Mutation` event with the actor + before/after.
- **Sink/writer:** events pushed to the channel are batch-inserted into `audit.audit_log`; a full channel drops without blocking (assert `emit` returns immediately + the drop counter increments).
- **Query API:** filters (actor/action/kind/decision/from-to/workspace) + keyset pagination + newest-first; platform-admin gate (non-admin → 403).
- **Retention:** the prune deletes rows older than the cutoff and keeps newer ones.
- **PDP parity:** the B1a regression matrix + object-scope suites pass unchanged (the decorator doesn't alter decisions).
- **Full gate:** BE `cargo test --workspace`, FE `lint/typecheck/test/build` (client regen for the audit DTOs), openapi current, adversarial security review.

## 11. Open questions (for the plan)

- **Crate placement:** `AuditEvent`/`AuditSink` in a new `egret-audit` crate vs a module in `egret-core`. Default: a small `egret-audit` crate (egret-core stays dependency-light; egret-auth + the handlers + egret-bin dep it). Confirm during the plan.
- **`Resource` helpers:** `AuditingPdp` needs `resource.workspace_id()/type_str()/id_str()` on `egret_core::authz::Resource` — add them (small, pure). Confirm none exist already.
- **Admin gate for the query API:** reuse the exact gate the access console uses (`require(&pdp, &p, "role:read")` or a dedicated `audit:read`). Default: reuse `role:read` (platform admin already holds it); a dedicated `audit:read` permission is cleaner but adds a catalog entry — decide in the plan.
- **Migration number:** next free after 0075; touch `egret-metastore/src/lib.rs`.

---

*Design v1.0 (2026-07-11). A real audit trail for Effort B: an `AuditingPdp` decorator captures every allow/deny at the PDP chokepoint (zero PEP churn) and the grant-mutation handlers emit change events; both feed one async best-effort sink that batch-inserts into an append-only `audit.audit_log`, with a retention prune and a platform-admin query API. Full-volume, best-effort, backend-only.*
