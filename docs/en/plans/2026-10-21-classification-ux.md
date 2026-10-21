# Classification area usability (Package 1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A data steward can do "mask the email column for everyone except group data-team" in Governance > Classification in at most 15 clicks, with no guess. The tag page says what the classification does. Every classification can be removed where it is shown. Conflicts and overlaps are stated before the write, the preview speaks in effect, and one vocabulary is used throughout.

**Architecture:** Five small backend changes (Tasks 1–4), then seven frontend tasks on top of them (Tasks 5–11), then the full gate and a live walkthrough (Task 12).

- Backend:
  - stable error codes on classification writes (B1);
  - a structured table-tag/mask conflict (B2);
  - an `effect` block on rule preview (B3);
  - a read of the people a clearance admits (B4);
  - the warehouse id on Where used (B5).
- Frontend: a glossary module, shared helpers (`apiErrors.ts`, `ruleEffect.ts`, `ruleDraft.ts`, `ClassificationChip`), and changes to existing pages and dialogs. One new API route (B4), no new page route, no migration.

**Tech Stack:** Rust (axum 0.8, sqlx, utoipa), React 18 + TanStack Query + orval-generated client, Vitest + Testing Library, Playwright.

Spec: `docs/en/specs/2026-10-21-classification-ux-design.md`. Read §3 (decisions C1–C13) and §4 (glossary) first. Every `file:line` there was checked on `origin/main` `265c81c6`; re-check it with `grep -n` before editing, because line numbers drift.

## Global Constraints

- **Glossary (exact UI terms)**:
  - Classification / Classifications; Key; Attach; Remove.
  - Rule, of kind Mask or Row filter. Mask type labels: Redact, Partial mask: show last 4, Partial mask: show first 4, Hash, Nullify, Unmasked (retain original value), Date: show only year, Custom.
  - Exempt; Applies to (Classification | Path); Clearance; Where used.
  - Engine policies (tab) / Engine policy; Who can read?
  - Delete (type the name); Remove (confirm, no typing).
  - Enforced by N engines / Not enforced yet.
- **Retired strings** that must not appear in this area's UI:
  - tab and section names: "Mask & filter", "Masks & filters", "Masking rules", "Mask and filter rules", "Hand-written";
  - picker and field text: "Pick a tag", "Selects by", "Reaches this tag", "Carries it";
  - other: "egret rule", "perm.rules", "Not gated".
  - "Tags" as a list label or column header in Governance Data / Classification.
- **Error codes** (body `{error, code, …fields}`; `error` keeps a full sentence):
  - `tag_key_invalid` (400) `{key, allowed: "a-z 0-9 : -"}`
  - `tag_key_taken` (409) `{key, existing: {id, name}}`
  - `tag_name_taken` (409) `{name, existing: {id, key, name}}`
  - `tag_in_use` (409) `{rules: [{id, name}]}`
  - `table_tag_mask_conflict` (400) `{tag, tagId, warehouseId|null, schema, table, mask, maskOrigin: "rule"|"policy", moreTables}`
  - existing: `rule_overlap` (409) `{rules: [{id, name}], coordinate, kind}`.
- **Key charset**: `^[a-z0-9:-]+$`, the server's `validate_key` (`crates/egret-bi/src/tags.rs:52-65`). `_` is NOT allowed.
- **Rulebook** (`docs/en/frontend-rules.md`):
  - FE-ACTION-4: every disabled control says why. Here that means `aria-disabled` plus `aria-describedby` on a visible reason; never `disabled` on Create/Preview/Delete/Add.
  - FE-ACTION-2/3/5; FE-FORM-2/4; FE-STATE-1/2/3.
  - FE-VISUAL-1: tokens only. No raw Tailwind palette (`bg-yellow-600`) and no `bg-[#…]`/`text-[oklch(…)]`. The classification colour is user data and goes through `style={{ backgroundColor }}` exactly as `TagSwatch` does.
  - FE-COPY-1/2/3/4; FE-TEST-1/2/3/4.
  - FE-DATA-1: `npm run gen:api`, never bare `orval`.
  - FE-DATA-3: no server data in `localStorage`; rule drafts live in module memory.
- **a11y**:
  - Required fields show "(required)" in the label and carry `aria-required="true"`.
  - Icon-only buttons have an `aria-label` naming the action and the object.
  - Swatches are `aria-hidden`.
  - Warnings that change Create's meaning are tied to Create by `aria-describedby`.
- **Copy**: UI text is English only (FE-COPY-1; there is no i18n layer). The Vietnamese glossary column is for docs (spec §12.1).
- **OpenAPI**: after any change to a route's schema or `utoipa` description, run `cargo run -q -p egret-bin --bin egret -- openapi > openapi.json` from the repo root. Then run `npm run gen:api` in `frontend/` and commit both: `committed_openapi_json_is_current` (`crates/egret-server/src/lib.rs:775`) and CI's client-drift step fail otherwise.
- **DB-gated tests must run, not skip.** Every `cargo test` below runs with `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test` (the `justfile`'s `test_database_url`; start Postgres with `just up`) and **without** `EGRET_ALLOW_DB_SKIP`.
  - After each run: `grep -c "skip: DATABASE_URL"` on the captured output must print `0`.
  - Never point tests at the dev DB `egret`.
- **Logs and scratch files** go to the session scratchpad: `export SCRATCH=<the session's scratchpad directory>` once per shell; every `tee` below writes there, never into the repo or `/tmp`.
- **One cargo command at a time.** Run long suites in the background. Commit before a long suite.
- **Stacks**: the demo stack on `127.0.0.1:18080` belongs to another session. Never stop, reset or seed it. Task 12 brings up its own stack under project `egret-classux` on port 18280.
- **Commits**: Conventional Commits, scope `classification` unless stated, trailer exactly `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`. No `Closes #N` in commits. No scratch files in git. Do not push until the user says so.
- **Docs**: `egret-data/docs` is NOT updated by this plan (spec §15, deferred pending the user's decision).

## File structure

| File | Responsibility |
|---|---|
| `crates/egret-bi/src/error.rs` | `BiError::Coded { status, body }` plus constructors |
| `crates/egret-bi/src/tags.rs` | coded key errors, `name_taken`, `ExistingTag`, `tag_in_use` |
| `crates/egret-controlplane/src/{error.rs,tags.rs}` | `CpError::Coded`; name check in `create_tag`/`update_tag` |
| `crates/egret-perm-compile/src/routes/error.rs` | `PermHttpError::Coded` |
| `crates/egret-policy/src/unreadable_table_mask.rs` | `tag_id`, `warehouse_id`, `more_tables`, `refusal_body()` |
| `crates/egret-perm-compile/src/effect.rs` (Create) | `RuleEffect` and `rule_effect()` |
| `crates/egret-perm-compile/src/routes/people.rs` | `cleared_people` handler and `ClearedPeople` |
| `crates/egret-controlplane/src/governance_tag_usage.rs` | `TableUsage.warehouse_id` |
| `frontend/src/routes/governance/glossary.ts` (Create) | the terms and `MASK_TYPE_LABEL` |
| `frontend/src/routes/governance/apiErrors.ts` (Create) | code narrowers |
| `frontend/src/routes/governance/ruleEffect.ts` (Create) | `effectSentence`, `effectivePerPrincipal` |
| `frontend/src/routes/governance/ruleDraft.ts` (Create) | `RuleEntry`, `seedRule`, the draft store |
| `frontend/src/routes/governance/ClassificationChip.tsx` (Create) | the chip |
| `frontend/src/routes/governance/RemoveClassificationDialog.tsx` (Create) | the Remove confirm |
| `frontend/src/routes/governance/classifications/EffectStrip.tsx` (Create) | the tag page summary |

---

## Task 1: Coded errors for classification writes (B1)

**Files:**
- Modify: `crates/egret-bi/src/error.rs`, `crates/egret-bi/src/tags.rs` (`map_conflict_err` :40-47, `validate_key` :52-65, `create` :72-96, `delete` ~:370-410)
- Modify: `crates/egret-controlplane/src/error.rs` (:13-40 enum, :110-124 `From<BiError>`, :148-170 `IntoResponse`), `crates/egret-controlplane/src/tags.rs` (`create_tag` :167, `update_tag` :201, the `utoipa` 409 descriptions)
- Modify: `crates/egret-automation/src/render/payload.rs` (:347-361, the exhaustive `BiError` match)
- Test: `crates/egret-bi/tests/tags_db.rs`, `crates/egret-controlplane/tests/tags_counts.rs`

**Interfaces:**
- Produces:
  - `egret_bi::BiError::Coded { status: axum::http::StatusCode, body: serde_json::Value }`;
  - `BiError::coded(status, code: &str, error: impl Into<String>, fields: serde_json::Value) -> BiError`;
  - `BiError::code(&self) -> Option<&str>`;
  - `egret_bi::tags::ExistingTag { id: Uuid, key: String, name: String }`;
  - `TagRepo::name_taken(&self, name: &str, except: Option<Uuid>) -> Result<Option<ExistingTag>, BiError>`;
  - `egret_controlplane::CpError::Coded(StatusCode, serde_json::Value)`.
- Codes: `tag_key_invalid`, `tag_key_taken`, `tag_name_taken`, `tag_in_use`.

- [ ] **Step 1: Write the failing repo tests**

In `crates/egret-bi/tests/tags_db.rs`:

1. In `create_dup_conflict_count_assign_cascade`, replace the two assertions on `dup` and `bad`:

```rust
    // a second create with the SAME key -> tag_key_taken, naming the holder.
    let dup = repo
        .create(&key, "PII 2", "", "", Some(by))
        .await
        .expect_err("dup key must conflict");
    assert_eq!(dup.code(), Some("tag_key_taken"), "got {dup:?}");
    let egret_bi::BiError::Coded { status, body } = &dup else { panic!("{dup:?}") };
    assert_eq!(*status, axum::http::StatusCode::CONFLICT);
    assert_eq!(body["key"], key);
    assert_eq!(body["existing"]["id"], tag.id.to_string());
    assert_eq!(body["existing"]["name"], "PII");

    // an invalid key -> tag_key_invalid (not a DB error), `_` included.
    for bad_key in ["Has Spaces!", "a_b"] {
        let bad = repo
            .create(bad_key, "x", "", "", Some(by))
            .await
            .expect_err("invalid key must be rejected");
        assert_eq!(bad.code(), Some("tag_key_invalid"), "{bad_key}: {bad:?}");
    }
```

2. Add:

```rust
#[tokio::test]
async fn name_taken_is_case_insensitive_and_skips_the_tag_itself() {
    let Some(url) = egret_test_db::url().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let repo = TagRepo::new(db.clone());
    let by = seed_user(&db).await;
    let suffix = Uuid::new_v4().simple().to_string();
    let name = format!("Public {suffix}");
    let t = repo
        .create(&format!("pub-{suffix}"), &name, "", "", Some(by))
        .await
        .expect("create");

    let hit = repo
        .name_taken(&name.to_uppercase(), None)
        .await
        .expect("query")
        .expect("an upper-case spelling is the same name");
    assert_eq!((hit.id, hit.key.as_str()), (t.id, t.key.as_str()));
    assert!(repo.name_taken(&format!("  {name}  "), None).await.unwrap().is_some(), "trimmed");
    assert!(
        repo.name_taken(&name.to_lowercase(), Some(t.id)).await.unwrap().is_none(),
        "renaming a classification to its own name in another case is not a duplicate"
    );
    assert!(repo.name_taken(&format!("Other {suffix}"), None).await.unwrap().is_none());
}
```

3. In `deleting_a_tag_a_live_rule_selects_is_refused_not_silently_cascaded` (~:146), replace the assertion on the refusal with:

```rust
    assert_eq!(err.code(), Some("tag_in_use"), "got {err:?}");
    let egret_bi::BiError::Coded { body, .. } = &err else { panic!("{err:?}") };
    let ids: Vec<&str> = body["rules"].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec![rule_id.to_string().as_str()]);
    assert!(!body["error"].as_str().unwrap().contains("perm.rules"), "no internal table names: {body}");
```

(`rule_id` is the id the test already inserts. If the test inserts without `RETURNING id`, add `RETURNING id` to that `INSERT` and bind it to `rule_id`.)

- [ ] **Step 2: Write the failing HTTP tests**

In `crates/egret-controlplane/tests/tags_counts.rs`, add a `send` helper next to `get` (:161):

```rust
async fn send(st: &CpState, token: &str, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
    let req = HttpRequest::builder()
        .method(method)
        .uri(uri)
        .header("Cookie", format!("egret_session={token}"))
        .header("Content-Type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = control_plane_router().with_state(st.clone()).oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
async fn classification_writes_answer_stable_codes() {
    let f = setup().await;
    let st = state(&f).await;
    let token = session(&f, &["tag:read", "tag:manage"]).await;
    let s = unique("cx");
    let (status, first) = send(&st, &token, "POST", "/control-plane/tags",
        json!({"key": format!("pii-{s}"), "name": format!("PII {s}")})).await;
    assert_eq!(status, StatusCode::CREATED, "{first}");

    // Same name, other case, other key: tag_name_taken naming the holder.
    let (status, body) = send(&st, &token, "POST", "/control-plane/tags",
        json!({"key": format!("pii2-{s}"), "name": format!("pii {s}")})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "tag_name_taken");
    assert_eq!(body["existing"]["key"], format!("pii-{s}"));
    assert!(body["error"].as_str().unwrap().contains("already exists"));

    // Same key: tag_key_taken.
    let (status, body) = send(&st, &token, "POST", "/control-plane/tags",
        json!({"key": format!("pii-{s}"), "name": format!("Other {s}")})).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::CONFLICT, Some("tag_key_taken")), "{body}");

    // Bad key: tag_key_invalid.
    let (status, body) = send(&st, &token, "POST", "/control-plane/tags",
        json!({"key": "bad_key", "name": format!("Bad {s}")})).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::BAD_REQUEST, Some("tag_key_invalid")), "{body}");

    // Rename another classification INTO the taken name: refused.
    let (_, second) = send(&st, &token, "POST", "/control-plane/tags",
        json!({"key": format!("sec-{s}"), "name": format!("Secret {s}")})).await;
    let second_id = second["id"].as_str().unwrap();
    let (status, body) = send(&st, &token, "PATCH", &format!("/control-plane/tags/{second_id}"),
        json!({"name": format!("PII {s}")})).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::CONFLICT, Some("tag_name_taken")), "{body}");

    // Rename a classification to its own name in another case: allowed.
    let first_id = first["id"].as_str().unwrap();
    let (status, body) = send(&st, &token, "PATCH", &format!("/control-plane/tags/{first_id}"),
        json!({"name": format!("PII {}", s.to_uppercase())})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}
```

- [ ] **Step 3: Run them to confirm they fail**

```bash
just up
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test cargo test -p egret-bi --test tags_db 2>&1 | tee "$SCRATCH"/classux-t1a.log | tail -30
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test cargo test -p egret-controlplane --test tags_counts classification_writes_answer_stable_codes 2>&1 | tee "$SCRATCH"/classux-t1b.log | tail -30
grep -c "skip: DATABASE_URL" "$SCRATCH"/classux-t1a.log "$SCRATCH"/classux-t1b.log
```

Expected: compile errors (`code`, `name_taken` and `BiError::Coded` do not exist). The grep prints `0` for both.

- [ ] **Step 4: Implement**

1. `crates/egret-bi/src/error.rs`: add the variant and helpers.

```rust
    /// A refusal with a stable `code` for the UI (classification UX spec
    /// C5/C10/C11). `body` is `{error, code, …fields}`, served as is with
    /// `status`; `error` is always a full sentence, so a client that reads only
    /// `error` keeps working.
    #[error("{}", .body["error"].as_str().unwrap_or("refused"))]
    Coded {
        status: StatusCode,
        body: serde_json::Value,
    },
```

```rust
impl BiError {
    /// `{error, code}` merged with `fields` (an object; anything else is ignored).
    pub fn coded(
        status: StatusCode,
        code: &str,
        error: impl Into<String>,
        fields: serde_json::Value,
    ) -> Self {
        let mut body = serde_json::json!({ "error": error.into(), "code": code });
        if let (Some(b), serde_json::Value::Object(extra)) = (body.as_object_mut(), fields) {
            b.extend(extra);
        }
        BiError::Coded { status, body }
    }

    /// The stable code of a coded or overlap refusal.
    pub fn code(&self) -> Option<&str> {
        match self {
            BiError::Coded { body, .. } | BiError::Overlap { body } => body["code"].as_str(),
            _ => None,
        }
    }
}
```

   In `IntoResponse for BiError` add `BiError::Coded { status, body } => (status, body),`. Make sure `StatusCode` is imported at the top (`use axum::http::StatusCode;`).

2. `crates/egret-bi/src/tags.rs`:
   - `validate_key` returns `BiError::coded(StatusCode::BAD_REQUEST, "tag_key_invalid", format!("A classification key may hold lowercase letters, digits, ':' and '-' only: {key:?}"), json!({"key": key, "allowed": "a-z 0-9 : -"}))`. The empty case uses the same code with the error "A classification key must not be empty.".
   - In `create`, replace `.map_err(map_conflict_err)?` with a match. On `sqlx::Error::Database(dbe)` with `dbe.is_unique_violation()`, read the holder with `sqlx::query_as::<_, (Uuid, String)>("SELECT id, name FROM tags WHERE key = $1")` and return:

```rust
BiError::coded(
    StatusCode::CONFLICT,
    "tag_key_taken",
    format!("A classification with the key {key:?} already exists ({name})."),
    json!({"key": key, "existing": {"id": id, "name": name}}),
)
```

     Any other error stays `BiError::Db`. Delete `map_conflict_err` if nothing else calls it (`grep -n map_conflict_err crates/egret-bi/src/tags.rs`).
   - Add:

```rust
/// A classification already holding a name (C10): the name check's answer.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow, serde::Serialize)]
pub struct ExistingTag {
    pub id: Uuid,
    pub key: String,
    pub name: String,
}

impl TagRepo {
    /// Another classification whose name equals `name` ignoring case and
    /// surrounding spaces; `except` is the classification being renamed.
    /// Engines match a classification rule by NAME, so two classifications
    /// with one name share every rule written on either (spec C10).
    pub async fn name_taken(&self, name: &str, except: Option<Uuid>) -> Result<Option<ExistingTag>, BiError> {
        Ok(sqlx::query_as::<_, ExistingTag>(
            "SELECT id, key, name FROM tags \
              WHERE lower(btrim(name)) = lower(btrim($1)) \
                AND ($2::uuid IS NULL OR id <> $2) \
              ORDER BY key LIMIT 1",
        )
        .bind(name)
        .bind(except)
        .fetch_optional(&self.db.pool)
        .await?)
    }
}

impl BiError {
    pub fn tag_name_taken(name: &str, existing: &ExistingTag) -> Self {
        BiError::coded(
            StatusCode::CONFLICT,
            "tag_name_taken",
            format!("A classification named {:?} already exists (key {}).", existing.name, existing.key),
            json!({"name": name, "existing": existing}),
        )
    }
}
```

   - In `delete`, inside the transaction and **before** `DELETE FROM tags`, read the rules that use the key:

```rust
        let rules: Vec<(Uuid, String)> = sqlx::query_as(
            "SELECT r.id, r.name FROM perm.rules r JOIN tags t ON t.key = r.tag_key \
              WHERE t.id = $1 ORDER BY r.name",
        )
        .bind(id)
        .fetch_all(&mut *tx)
        .await?;
        if !rules.is_empty() {
            return Err(tag_in_use(&rules));
        }
```

     Use `fn tag_in_use(rules: &[(Uuid, String)]) -> BiError`, which produces 409 `tag_in_use` with the error "Rules still use this classification: <names joined by ', '>. Delete them or change what they apply to first." and the fields `{"rules": [{"id", "name"}]}`. The foreign-key mapping at the `DELETE` (:392-400) becomes `tag_in_use(&[])`, the fallback for a race.

3. `crates/egret-controlplane/src/error.rs`:
   - add `#[error("{}", .1["error"].as_str().unwrap_or("refused"))] Coded(StatusCode, serde_json::Value),`;
   - in `From<egret_bi::BiError>` add `E::Coded { status, body } => CpError::Coded(status, body),`;
   - in `IntoResponse`, next to the `RuleOverlap` early return, add `if let CpError::Coded(status, body) = self { return (status, Json(body)).into_response(); }`, and add `CpError::Coded(..) => unreachable!("answered above")` to the match.
4. `crates/egret-controlplane/src/tags.rs`:
   - In `create_tag`, before `st.tags.create(...)`:

```rust
    if let Some(existing) = st.tags.name_taken(&body.name, None).await? {
        return Err(egret_bi::BiError::tag_name_taken(&body.name, &existing).into());
    }
```

   - In `update_tag`, do the same when `body.name` is `Some(n)`, with `Some(id)`.
   - Update the `utoipa` 409/400 descriptions to name the codes ("409: `code` is `tag_key_taken` or `tag_name_taken`…").
5. `crates/egret-automation/src/render/payload.rs`: add an arm.

```rust
        egret_bi::BiError::Coded { status, body } => {
            let msg = body["error"].as_str().unwrap_or("refused").to_string();
            if status == axum::http::StatusCode::BAD_REQUEST {
                AutomationError::BadRequest(msg)
            } else {
                AutomationError::Conflict(msg)
            }
        }
```

6. Find any other exhaustive match: `grep -rn "BiError::Overlap" crates --include='*.rs' | grep -v /tests/`. Add a `Coded` arm wherever `Overlap` has one.

- [ ] **Step 5: Run the tests to confirm they pass, then the crates**

```bash
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test cargo test -p egret-bi --test tags_db 2>&1 | tee "$SCRATCH"/classux-t1a.log | tail -5
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test cargo test -p egret-controlplane --test tags_counts 2>&1 | tee "$SCRATCH"/classux-t1b.log | tail -5
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test cargo test -p egret-bi -p egret-controlplane -p egret-automation 2>&1 | tee "$SCRATCH"/classux-t1c.log | tail -15
grep -c "skip: DATABASE_URL" "$SCRATCH"/classux-t1a.log "$SCRATCH"/classux-t1b.log "$SCRATCH"/classux-t1c.log
```

Expected: `test result: ok` for each, and the grep prints `0` three times. If another test asserted `BiError::Conflict(_)` for a duplicate key or `BiError::BadRequest(_)` for a bad key, update it to `.code()` in the same way (find them with `grep -rn "Conflict(_)\|BadRequest(_)" crates/egret-bi/tests crates/egret-controlplane/tests`).

- [ ] **Step 6: Regenerate the OpenAPI document and the client**

```bash
cargo run -q -p egret-bin --bin egret -- openapi > openapi.json
cd frontend && npm run gen:api && cd ..
cargo test -p egret-server committed_openapi_json_is_current
git status --short frontend/src/api/generated openapi.json
```

Expected: the test passes. Only description text changed in `openapi.json`; the generated client may be unchanged.

- [ ] **Step 7: Commit**

```bash
cargo fmt --all
cargo clippy -p egret-bi -p egret-controlplane -p egret-automation --all-targets -- -D warnings
git add crates/egret-bi crates/egret-controlplane crates/egret-automation openapi.json frontend/src/api
git commit -m "feat(classification): stable error codes for classification writes, refuse duplicate names

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

## Task 2: Structured table-classification/mask conflict (B2)

**Files:**
- Modify: `crates/egret-policy/src/unreadable_table_mask.rs` (struct :208-217, `refusal` :257-275, `table_level_attachment_of` :403-428, `unreadable_table_mask_for` :525-594, unit tests)
- Modify: `crates/egret-perm-compile/src/routes/error.rs` (enum, `IntoResponse`), `crates/egret-perm-compile/src/routes.rs` (:1477-1491)
- Modify: `crates/egret-bi/src/catalog_tags.rs` (:119-136), `crates/egret-bi/src/tags.rs` (:198-207)
- Test: `crates/egret-perm-compile/tests/routes.rs`, `crates/egret-bi/tests/catalog_tags_routes.rs`

**Interfaces:**
- Consumes: `BiError::Coded` (Task 1).
- Produces:
  - `UnreadableTableMask { mask_origin, mask, tag, tag_id: Uuid, warehouse_id: Option<Uuid>, schema, table, more_tables: bool }`;
  - `UnreadableTableMask::refusal_body(&self) -> serde_json::Value` (code `table_tag_mask_conflict`);
  - `PermHttpError::Coded { status: StatusCode, body: serde_json::Value }`.
- `warehouse_id` is `Some` for the Mask and Rename arms (read from the attachment) and `None` for the Attachment arm (the write in flight IS that attachment).

- [ ] **Step 1: Write the failing unit test**

In the `tests` module of `crates/egret-policy/src/unreadable_table_mask.rs`:

```rust
    #[test]
    fn the_refusal_body_carries_the_sentence_and_every_field_the_ui_acts_on() {
        let tag_id = Uuid::new_v4();
        let wh = Uuid::new_v4();
        let pair = UnreadableTableMask {
            mask_origin: MaskOrigin::Rule,
            mask: "hash email".into(),
            tag: "PII".into(),
            tag_id,
            warehouse_id: Some(wh),
            schema: "analytics".into(),
            table: "orders".into(),
            more_tables: true,
        };
        let body = pair.refusal_body();
        assert_eq!(body["error"], pair.refusal());
        assert_eq!(body["code"], "table_tag_mask_conflict");
        assert_eq!(body["tag"], "PII");
        assert_eq!(body["tagId"], tag_id.to_string());
        assert_eq!(body["warehouseId"], wh.to_string());
        assert_eq!((body["schema"].as_str(), body["table"].as_str()), (Some("analytics"), Some("orders")));
        assert_eq!(body["mask"], "hash email");
        assert_eq!(body["maskOrigin"], "rule");
        assert_eq!(body["moreTables"], true);
    }
```

- [ ] **Step 2: Write the failing route tests**

In `crates/egret-perm-compile/tests/routes.rs` (the fixture's tag is on `sales.orders.email`; this adds a table-level attachment of the same tag to `sales.orders`):

```rust
/// Spec C5: a classification mask over a classification that sits on a whole
/// table is refused with a stable code and the ids the UI needs to offer
/// "Remove from table" — on preview, before anything is created.
#[tokio::test]
async fn a_tag_mask_over_a_table_level_attachment_answers_table_tag_mask_conflict() {
    let Some(f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let token = admin_session(&f).await;
    let (tag_id, wh): (Uuid, Uuid) = sqlx::query_as(
        "SELECT a.tag_id, a.warehouse_id FROM catalog_tag_assignments a JOIN tags t ON t.id = a.tag_id \
          WHERE t.key = $1 LIMIT 1",
    )
    .bind(&f.tag_key)
    .fetch_one(&f.db.pool)
    .await
    .expect("the fixture's assignment");
    sqlx::query(
        "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, column_name) \
         VALUES ($1, $2, 'sales', 'orders', '')",
    )
    .bind(tag_id)
    .bind(wh)
    .execute(&f.db.pool)
    .await
    .expect("table-level attachment");

    let rule = json!({
        "name": unique("mask-on-table-tag"),
        "kind": { "type": "mask", "mask_type": "MASK_HASH", "value_expr": null },
        "selector": { "type": "tag", "key": f.tag_key },
        "exempt": { "include": [], "exclude": [] }
    });
    let (status, body) = into_json(
        f.app()
            .oneshot(req("POST", "/perm/preview", &token, Some(json!({ "rule": rule }))))
            .await
            .unwrap(),
    )
    .await;

    sqlx::query("DELETE FROM catalog_tag_assignments WHERE tag_id = $1 AND column_name = ''")
        .bind(tag_id)
        .execute(&f.db.pool)
        .await
        .expect("clean up");

    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "table_tag_mask_conflict");
    assert_eq!(body["tagId"], tag_id.to_string());
    assert_eq!(body["warehouseId"], wh.to_string());
    assert_eq!((body["schema"].as_str(), body["table"].as_str()), (Some("sales"), Some("orders")));
    assert_eq!(body["maskOrigin"], "rule");
    assert!(body["error"].as_str().unwrap().starts_with("tag \""), "the sentence is kept: {body}");
}
```

In `crates/egret-bi/tests/catalog_tags_routes.rs`, in `a_table_level_assignment_is_refused_when_a_tag_mask_names_the_tag`, after the existing status assertion for each surface, add:

```rust
        let body: serde_json::Value = serde_json::from_str(&text).expect("json body");
        assert_eq!(body["code"], "table_tag_mask_conflict", "{surface}: {body}");
        assert_eq!(body["maskOrigin"], if surface == "rule" { "rule" } else { "policy" });
        assert_eq!(body["tagId"], tag.id.to_string());
        assert!(body["warehouseId"].is_null(), "the attachment in flight is the table-level one");
```

(`text` is the response body string the test already reads with `body_string`. If it names it differently, use that binding.)

- [ ] **Step 3: Run to confirm they fail**

```bash
cargo test -p egret-policy --lib the_refusal_body_carries 2>&1 | tail -5
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test cargo test -p egret-perm-compile --test routes table_tag_mask_conflict 2>&1 | tee "$SCRATCH"/classux-t2a.log | tail -10
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test cargo test -p egret-bi --test catalog_tags_routes 2>&1 | tee "$SCRATCH"/classux-t2b.log | tail -10
grep -c "skip: DATABASE_URL" "$SCRATCH"/classux-t2a.log "$SCRATCH"/classux-t2b.log
```

Expected: a compile error on the new fields, then `code` missing. The grep prints `0`.

- [ ] **Step 4: Implement**

1. The struct gains `pub tag_id: Uuid, pub warehouse_id: Option<Uuid>, pub more_tables: bool`.
2. `table_level_attachment_of` returns `Option<(TableLevel, bool)>`, where `struct TableLevel { tag_id: Uuid, tag: String, warehouse_id: Uuid, schema: String, table: String }` derives `sqlx::FromRow`. SQL: `SELECT t.id AS tag_id, t.name AS tag, a.warehouse_id, a.schema_name AS schema, a.table_name AS table …` with `LIMIT 2` and `fetch_all`. The bool is `rows.len() > 1`.
3. Arms:
   - Mask: fills every field from the row, with `warehouse_id: Some(row.warehouse_id)`.
   - Attachment: `tag_id` (already in scope), `warehouse_id: None`, `more_tables: false`.
   - Rename: `tag_id`, `warehouse_id: Some(..)`, `more_tables` from the query.
4. `refusal_body()`:

```rust
    pub fn refusal_body(&self) -> serde_json::Value {
        serde_json::json!({
            "error": self.refusal(),
            "code": "table_tag_mask_conflict",
            "tag": self.tag,
            "tagId": self.tag_id,
            "warehouseId": self.warehouse_id,
            "schema": self.schema,
            "table": self.table,
            "mask": self.mask,
            "maskOrigin": match self.mask_origin { MaskOrigin::Rule => "rule", MaskOrigin::Policy => "policy" },
            "moreTables": self.more_tables,
        })
    }
```

   (Check the `MaskOrigin` variant names at :186-202 and use them exactly.)
5. `PermHttpError`:
   - add `#[error("{}", .body["error"].as_str().unwrap_or("refused"))] Coded { status: StatusCode, body: serde_json::Value },`;
   - `IntoResponse` arm: `PermHttpError::Coded { status, body } => (status, body),`.
6. `routes.rs:1490`: `return Err(PermHttpError::Coded { status: StatusCode::BAD_REQUEST, body: pair.refusal_body() });`.
7. `catalog_tags.rs:135`: `return Err(crate::error::BiError::Coded { status: StatusCode::BAD_REQUEST, body: pair.refusal_body() }.into());`. Make the same change at `tags.rs:198-207`.
8. `crates/egret-policy/src/admin/mod.rs:1826` keeps `PolicyHttpError::BadRequest(pair.refusal())` (spec C5: the editor is out of scope).
9. Fix the struct literals in this file's unit tests (add the three fields) and any in `crates/egret-policy/tests/unreadable_table_mask_db.rs`. The DB test compares `.refusal()` strings, which are unchanged.

- [ ] **Step 5: Run to confirm they pass, then the crates**

```bash
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test cargo test -p egret-policy -p egret-bi -p egret-perm-compile 2>&1 | tee "$SCRATCH"/classux-t2c.log | tail -20
grep -c "skip: DATABASE_URL" "$SCRATCH"/classux-t2c.log
```

Expected: all `ok`; grep `0`. Run it in the background: it is long.

- [ ] **Step 6: Regenerate OpenAPI and the client** (the commands of Task 1 Step 6; update the 400 descriptions on `/perm/rules`, `/perm/preview` and `PUT …/resource-tags` to name `table_tag_mask_conflict`).

- [ ] **Step 7: Commit**

```bash
cargo fmt --all && cargo clippy -p egret-policy -p egret-bi -p egret-perm-compile --all-targets -- -D warnings
git add crates openapi.json frontend/src/api
git commit -m "feat(classification): answer the table-level classification/mask conflict with a stable code and its ids

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

## Task 3: Rule preview reports its effect (B3)

**Files:**
- Create: `crates/egret-perm-compile/src/effect.rs`
- Modify: `crates/egret-perm-compile/src/lib.rs` (`pub mod effect;`), `crates/egret-perm-compile/src/routes.rs` (`PreviewResponse` :962, `preview` :2061-2085, the OpenAPI component list)
- Test: `crates/egret-perm-compile/tests/routes.rs`

**Interfaces:**
- Consumes: `egret_perm::eval::{glob_match, resource_level_matches}`, `egret_perm::list_rules_via` (`crates/egret-perm/src/store.rs:285`), `crate::members::{group_members_via, workspace_members_via, role_holders_via}`.
- Produces:
  - `pub struct RuleEffect { kind: String, catalogs: Vec<EffectCatalog>, table_count: Option<u32>, column_count: Option<u32>, enforced_by: u32, exempt_members: u32, exempt_everyone: bool, already_masked: Vec<ExistingMask> }`;
  - `pub struct EffectCatalog { catalog_id: Uuid, catalog_name: String, engines: Vec<EffectEngine>, not_enforced: Option<String> }`;
  - `pub struct EffectEngine { id: Uuid, name: String, engine_type: String }`;
  - `pub struct ExistingMask { rule_id: Uuid, rule_name: String, mask_type: String, coordinate: String, exempt: PrincipalSet }`.
  - All are `#[serde(rename_all = "camelCase")]` and `utoipa::ToSchema`.
  - `pub async fn rule_effect(conn: &mut sqlx::PgConnection, editing: Option<Uuid>, rule: &NewRule) -> Result<RuleEffect, PermHttpError>`.
  - `PreviewResponse { affected, compile, effect: Option<RuleEffect> }`.

- [ ] **Step 1: Write the failing tests**

In `crates/egret-perm-compile/tests/routes.rs`:

```rust
async fn preview_rule(f: &Fixture, token: &str, rule: Value) -> (StatusCode, Value) {
    into_json(
        f.app()
            .oneshot(req("POST", "/perm/preview", token, Some(json!({ "rule": rule }))))
            .await
            .unwrap(),
    )
    .await
}

fn mask_rule(name: &str, selector: Value, exempt_include: Value) -> Value {
    json!({
        "name": unique(name),
        "kind": { "type": "mask", "mask_type": "MASK_HASH", "value_expr": null },
        "selector": selector,
        "exempt": { "include": exempt_include, "exclude": [] }
    })
}

/// Spec C7 (a)+(d): the fixture's catalog has one engine bound to a profile, and
/// its tag sits on one column, so a tag mask reaches 1 table and 1 column and is
/// enforced by 1 engine.
#[tokio::test]
async fn a_tag_mask_preview_states_its_reach_and_its_engines() {
    let Some(f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let token = admin_session(&f).await;
    let (status, body) =
        preview_rule(&f, &token, mask_rule("eff-tag", json!({"type": "tag", "key": f.tag_key}), json!([]))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let e = &body["effect"];
    assert_eq!(e["kind"], "mask");
    assert_eq!((e["tableCount"].as_u64(), e["columnCount"].as_u64()), (Some(1), Some(1)), "{e}");
    assert_eq!(e["enforcedBy"], 1, "{e}");
    let cat = &e["catalogs"][0];
    assert_eq!(cat["catalogId"], f.catalog_id.to_string());
    assert!(cat["notEnforced"].is_null(), "{cat}");
    assert_eq!(cat["engines"].as_array().unwrap().len(), 1);
}

/// Spec C7 (b): a catalog no engine serves is reported as not enforced.
#[tokio::test]
async fn a_path_mask_on_a_catalog_without_an_engine_is_not_enforced() {
    let Some(f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let token = admin_session(&f).await;
    let orphan = orphan_catalog(&f, "eff-orphan").await;
    let selector = json!({"type": "path", "catalog_id": orphan, "schema": "s", "table": "t", "column": "email"});
    let (status, body) = preview_rule(&f, &token, mask_rule("eff-orphan", selector, json!([]))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let e = &body["effect"];
    assert_eq!(e["enforcedBy"], 0, "{e}");
    assert_eq!(e["catalogs"][0]["notEnforced"], "no_engine", "{e}");
    assert_eq!((e["tableCount"].as_u64(), e["columnCount"].as_u64()), (Some(1), Some(1)));
    // A path with no column masks every column: the count is not a number.
    let every = json!({"type": "path", "catalog_id": orphan, "schema": "s", "table": "t"});
    let (_, body) = preview_rule(&f, &token, mask_rule("eff-every", every, json!([]))).await;
    assert!(body["effect"]["columnCount"].is_null(), "{body}");
    // A pattern level has no count either.
    let pattern = json!({"type": "path", "catalog_id": orphan, "schema": "s", "table": "ord*", "column": "email"});
    let (_, body) = preview_rule(&f, &token, mask_rule("eff-pattern", pattern, json!([]))).await;
    assert!(body["effect"]["tableCount"].is_null(), "{body}");
}

/// Spec C7 (c): exemptions count distinct people — a group of two plus one of
/// them named directly is two.
#[tokio::test]
async fn exempt_members_counts_distinct_people() {
    let Some(f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let token = admin_session(&f).await;
    let a = known_user(&f, unique("effa")).await;
    let b = known_user(&f, unique("effb")).await;
    let group = unique("eff_group");
    let gid: Uuid = sqlx::query_scalar("INSERT INTO groups (name) VALUES ($1) RETURNING id")
        .bind(&group)
        .fetch_one(&f.db.pool)
        .await
        .unwrap();
    for u in [&a, &b] {
        sqlx::query("INSERT INTO user_groups (user_id, group_id) SELECT id, $2 FROM users WHERE username = $1")
            .bind(u)
            .bind(gid)
            .execute(&f.db.pool)
            .await
            .unwrap();
    }
    let include = json!([{"kind": "group", "ref": group}, {"kind": "user", "ref": a}]);
    let (status, body) =
        preview_rule(&f, &token, mask_rule("eff-exempt", json!({"type": "tag", "key": f.tag_key}), include)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["effect"]["exemptMembers"], 2, "{body}");
    assert_eq!(body["effect"]["exemptEveryone"], false);
}

/// Spec C6: on a catalog no engine serves (nothing compiles, so the compiler's
/// overlap check sees nothing), a second mask on one column is still reported.
#[tokio::test]
async fn a_second_mask_on_a_column_is_reported_without_any_engine() {
    let Some(f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let token = admin_session(&f).await;
    let orphan = orphan_catalog(&f, "eff-overlap").await;
    let path = json!({"type": "path", "catalog_id": orphan, "schema": "s", "table": "t", "column": "email"});
    let first = mask_rule("eff-first", path.clone(), json!([]));
    let (status, created) = into_json(
        f.app().oneshot(req("POST", "/perm/rules", &token, Some(first.clone()))).await.unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let first_id = created["id"].as_str().unwrap().to_string();

    // The same column spelled in another case: resource levels match without regard to case.
    let same_column = json!({"type": "path", "catalog_id": orphan, "schema": "S", "table": "T", "column": "EMAIL"});
    let (status, body) = preview_rule(&f, &token, mask_rule("eff-second", same_column, json!([]))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let hits = body["effect"]["alreadyMasked"].as_array().unwrap();
    assert_eq!(hits.len(), 1, "{body}");
    assert_eq!(hits[0]["ruleId"], first_id);
    assert_eq!(hits[0]["maskType"], "MASK_HASH");

    // Previewing a PATCH of the first rule does not report the rule against itself.
    let (status, body) = into_json(
        f.app()
            .oneshot(req("POST", "/perm/preview", &token, Some(json!({ "id": first_id, "rule": first }))))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["effect"]["alreadyMasked"], json!([]), "{body}");
}

/// A grant preview carries no effect.
#[tokio::test]
async fn a_grant_preview_has_no_effect_block() {
    let Some(f) = setup().await else { return };
    let _lock = exclusive(&f).await;
    let token = admin_session(&f).await;
    let user = known_user(&f, unique("effg")).await;
    let (status, body) = into_json(
        f.app()
            .oneshot(req("POST", "/perm/preview", &token, Some(json!({ "grant": grant_body(f.catalog_id, &user) }))))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.get("effect").is_none_or(|e| e.is_null()), "{body}");
}
```

(`known_user`, `orphan_catalog`, `grant_body`, `unique`, `exclusive`, `admin_session`, `into_json` and `req` are the file's existing helpers at :396, :882, :633, :52, :133, :440, :406 and :414.)

- [ ] **Step 2: Run to confirm they fail**

```bash
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test cargo test -p egret-perm-compile --test routes eff 2>&1 | tee "$SCRATCH"/classux-t3a.log | tail -20
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test cargo test -p egret-perm-compile --test routes a_grant_preview_has_no_effect_block a_second_mask exempt_members 2>&1 | tail -5
grep -c "skip: DATABASE_URL" "$SCRATCH"/classux-t3a.log
```

Expected: FAIL (`effect` is absent, so `body["effect"]["kind"]` is null). The grep prints `0`.

- [ ] **Step 3: Implement `effect.rs`**

```rust
//! What a rule will do, in effect terms (classification UX spec C6, C7).
//!
//! Computed from the MODEL — rules, classification assignments, catalogs and
//! engine attachments — never from compiled downloads, so it answers on a
//! catalog no engine serves, which is exactly where the compiler's overlap
//! check (`crate::overlap`) sees nothing. Read-only; runs inside the preview's
//! rolled-back transaction, before `write_rule`.

use std::collections::{BTreeMap, BTreeSet};

use egret_perm::eval::{glob_match, resource_level_matches};
use egret_perm::model::{Principal, PrincipalKind, PrincipalSet, RuleKind, Selector};
use serde::Serialize;
use uuid::Uuid;

use crate::routes::{NewRule, PermHttpError};

/// At most this many overlapping masks are listed (spec §10).
pub const ALREADY_MASKED_CAP: usize = 20;

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RuleEffect {
    /// `mask` or `row_filter`.
    pub kind: String,
    pub catalogs: Vec<EffectCatalog>,
    /// `None` when a path level is a pattern.
    pub table_count: Option<u32>,
    /// Masks only; `None` for a pattern, for "every column" (a path mask without a column) and for row filters.
    pub column_count: Option<u32>,
    /// Distinct engines, bound to a profile, attached to a reached catalog.
    pub enforced_by: u32,
    /// Distinct users in `exempt.include`, groups, roles and workspaces expanded.
    pub exempt_members: u32,
    /// `exempt.include` names the `public` group: nobody is masked.
    pub exempt_everyone: bool,
    /// Other mask rules that reach a column this rule reaches (≤ [`ALREADY_MASKED_CAP`]).
    pub already_masked: Vec<ExistingMask>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EffectCatalog {
    pub catalog_id: Uuid,
    pub catalog_name: String,
    pub engines: Vec<EffectEngine>,
    /// `no_engine` (nothing attached) or `no_profile` (engines attached, none bound to a profile).
    pub not_enforced: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct EffectEngine {
    pub id: Uuid,
    pub name: String,
    pub engine_type: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExistingMask {
    pub rule_id: Uuid,
    pub rule_name: String,
    pub mask_type: String,
    /// `catalog.schema.table.column`.
    pub coordinate: String,
    pub exempt: PrincipalSet,
}

/// One column (or, for a table-level classification, `column = None`) a rule reaches.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Coord {
    catalog_id: Uuid,
    catalog_name: String,
    schema: String,
    table: String,
    column: Option<String>,
}

fn is_pattern(s: &str) -> bool {
    s.contains('*') || s.contains('?')
}

pub async fn rule_effect(
    conn: &mut sqlx::PgConnection,
    editing: Option<Uuid>,
    rule: &NewRule,
) -> Result<RuleEffect, PermHttpError> {
    let db = PermHttpError::db;
    let is_mask = matches!(rule.kind, RuleKind::Mask { .. });

    // 1. Reach: the catalogs, and (when exact) the coordinates.
    let (catalog_ids, coords, table_count, column_count): (BTreeSet<Uuid>, Vec<Coord>, Option<u32>, Option<u32>) =
        match &rule.selector {
            Selector::Tag { key } => {
                let rows: Vec<(Uuid, String, String, String, String)> = sqlx::query_as(
                    "SELECT c.id, c.name, a.schema_name, a.table_name, a.column_name \
                       FROM catalog_tag_assignments a \
                       JOIN tags t ON t.id = a.tag_id \
                       JOIN registry.catalogs c ON c.warehouse_id = a.warehouse_id \
                      WHERE t.key = $1",
                )
                .bind(key)
                .fetch_all(&mut *conn)
                .await
                .map_err(db)?;
                let coords: Vec<Coord> = rows
                    .into_iter()
                    .map(|(id, name, s, t, c)| Coord {
                        catalog_id: id,
                        catalog_name: name,
                        schema: s,
                        table: t,
                        column: (!c.is_empty()).then_some(c),
                    })
                    .collect();
                let tables: BTreeSet<_> =
                    coords.iter().map(|c| (c.catalog_id, c.schema.clone(), c.table.clone())).collect();
                let columns = coords.iter().filter(|c| c.column.is_some()).count() as u32;
                let any_table_level = coords.iter().any(|c| c.column.is_none());
                (
                    coords.iter().map(|c| c.catalog_id).collect(),
                    coords.clone(),
                    Some(tables.len() as u32),
                    (is_mask && !any_table_level).then_some(columns),
                )
            }
            Selector::Path(scope) => {
                let name: String = sqlx::query_scalar("SELECT name FROM registry.catalogs WHERE id = $1")
                    .bind(scope.catalog_id)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(db)?
                    .unwrap_or_default();
                let exact = |v: &Option<String>| v.as_deref().is_some_and(|s| !is_pattern(s));
                let table_exact = exact(&scope.schema) && exact(&scope.table);
                let column_exact = table_exact && exact(&scope.column);
                let coord = Coord {
                    catalog_id: scope.catalog_id,
                    catalog_name: name,
                    schema: scope.schema.clone().unwrap_or_else(|| "*".into()),
                    table: scope.table.clone().unwrap_or_else(|| "*".into()),
                    column: scope.column.clone(),
                };
                (
                    BTreeSet::from([scope.catalog_id]),
                    vec![coord],
                    table_exact.then_some(1),
                    (is_mask && column_exact).then_some(1),
                )
            }
        };

    // 2. Engines per catalog.
    let mut catalogs = Vec::new();
    let mut engine_ids = BTreeSet::new();
    for id in &catalog_ids {
        let name: String = sqlx::query_scalar("SELECT name FROM registry.catalogs WHERE id = $1")
            .bind(id)
            .fetch_one(&mut *conn)
            .await
            .map_err(db)?;
        let attached: Vec<(Uuid, String, String, bool)> = sqlx::query_as(
            "SELECT e.id, e.name, e.type::text, e.authz_service_id IS NOT NULL \
               FROM registry.engine_catalog_attachments a \
               JOIN engine_mgmt.engines e ON e.id = a.engine_id \
              WHERE a.catalog_id = $1 ORDER BY e.name",
        )
        .bind(id)
        .fetch_all(&mut *conn)
        .await
        .map_err(db)?;
        let engines: Vec<EffectEngine> = attached
            .iter()
            .filter(|(_, _, _, bound)| *bound)
            .map(|(id, name, ty, _)| EffectEngine { id: *id, name: name.clone(), engine_type: ty.clone() })
            .collect();
        engine_ids.extend(engines.iter().map(|e| e.id));
        let not_enforced = if attached.is_empty() {
            Some("no_engine".to_string())
        } else if engines.is_empty() {
            Some("no_profile".to_string())
        } else {
            None
        };
        catalogs.push(EffectCatalog { catalog_id: *id, catalog_name: name, engines, not_enforced });
    }

    // 3. Exempt people.
    let (exempt_members, exempt_everyone) = count_exempt(conn, &rule.exempt).await?;

    // 4. Other masks on the same columns (masks only, exact columns only).
    let already_masked = if is_mask {
        already_masked(conn, editing, &coords).await?
    } else {
        Vec::new()
    };

    Ok(RuleEffect {
        kind: if is_mask { "mask" } else { "row_filter" }.into(),
        catalogs,
        table_count,
        column_count,
        enforced_by: engine_ids.len() as u32,
        exempt_members,
        exempt_everyone,
        already_masked,
    })
}
```

Write `count_exempt` and `already_masked` in the same file:

- **`count_exempt`.**
  - Iterate `exempt.include`.
  - A `Group` named `public` sets `exempt_everyone = true` and adds nobody.
  - Otherwise collect group names, role names, workspace ids (`WorkspaceMembers` ref parsed as `Uuid`; skip one that does not parse) and usernames.
  - Call `crate::members::group_members_via`, `role_holders_via` and `workspace_members_via` (each `pub(crate)`), map each row to its `user_id`, and resolve direct users with `SELECT id FROM users WHERE username = ANY($1)`.
  - Return the size of the union of `user_id`s. (Spec C7: `exclude` is not subtracted; a non-empty `exclude` cannot compile anyway, per the model doc at `crates/egret-perm/src/model.rs:197-208`.)
- **`already_masked`.**
  - Return empty unless some coordinate has a `column` without pattern characters, and stop after `ALREADY_MASKED_CAP` entries.
  - Load all rules with `egret_perm::list_rules_via(conn)` (map its error with `PermHttpError::db`). Keep `RuleKind::Mask` rules whose `id != editing`.
  - Load the classification names on each exact coordinate in one query per coordinate:

```sql
SELECT t.name FROM catalog_tag_assignments a JOIN tags t ON t.id = a.tag_id
  JOIN registry.catalogs c ON c.warehouse_id = a.warehouse_id
 WHERE c.id = $1 AND lower(a.schema_name) = lower($2) AND lower(a.table_name) = lower($3)
   AND (a.column_name = '' OR lower(a.column_name) = lower($4))
```

  - A rule reaches the coordinate when:
    - `Selector::Path(s)` with `s.catalog_id == coord.catalog_id`, and each of `schema`/`table`/`column` either absent or `resource_level_matches(level, coord_level)`; or
    - `Selector::Tag { key }` whose `tags.name` (load `key → name` once with `SELECT key, name FROM tags`) `glob_match`es one of the coordinate's classification names. Case-sensitive, as the engines match.
  - Push `ExistingMask { rule_id, rule_name, mask_type, coordinate: format!("{catalog}.{schema}.{table}.{column}"), exempt }`, deduplicated by `(rule_id, coordinate)`.

In `routes.rs`:

1. `#[derive(…)] pub struct PreviewResponse { pub affected: …, pub compile: …, #[serde(skip_serializing_if = "Option::is_none")] pub effect: Option<crate::effect::RuleEffect> }`.
2. In `preview`, compute `let effect = match (&rule, ..) { (Some(r), ..) => Some(crate::effect::rule_effect(&mut tx, id, r).await?), _ => None };` **before** the `match (grant, rule)` that moves `r`.
3. Register the four new schemas in the `perm` OpenAPI `components(schemas(...))` list.
4. Make `NewRule` and `PermHttpError` reachable from `effect.rs` (`pub(crate) use` if they are private).

- [ ] **Step 4: Run to confirm they pass, then the crate**

```bash
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test cargo test -p egret-perm-compile 2>&1 | tee "$SCRATCH"/classux-t3b.log | tail -20
grep -c "skip: DATABASE_URL" "$SCRATCH"/classux-t3b.log
```

Expected: all `ok`, grep `0`. (In the background: the crate's suite is long.)

- [ ] **Step 5: Regenerate OpenAPI and the client** (Task 1 Step 6). Expected: `frontend/src/api/generated/model/` gains `ruleEffect.ts`, `effectCatalog.ts`, `effectEngine.ts`, `existingMask.ts`, and `previewResponse.ts` gains `effect?: RuleEffect`.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all && cargo clippy -p egret-perm-compile --all-targets -- -D warnings
(cd frontend && npx tsc -b)
git add crates/egret-perm-compile openapi.json frontend/src/api
git commit -m "feat(perm): rule preview reports reach, enforcing engines, exempt people and masks already on the column

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

## Task 4: Who a clearance admits (B4), and the warehouse on Where used (B5)

**Files:**
- Modify: `crates/egret-perm-compile/src/routes/people.rs` (new handler and DTOs), `crates/egret-perm-compile/src/routes.rs` (route next to `/perm/people/roles/{name}` at ~:163, OpenAPI paths)
- Modify: `crates/egret-controlplane/src/governance_tag_usage.rs` (`TableUsage` :56-65, its query)
- Test: `crates/egret-perm-compile/tests/people.rs`, `crates/egret-controlplane/tests/governance_tag_usage.rs`

**Interfaces:**
- Produces:
  - `GET /api/control-plane/perm/clearances/{tag_id}/people` (`policy:read`) → `ClearedPeople { count: u32, people: Vec<ClearedPerson>, truncated: bool, engines_enforce: bool, on_engines_count: u32 }`;
  - `ClearedPerson { user_id: Uuid, label: String, username: Option<String>, status: String, service_account: bool, via: Vec<ClearedVia>, on_engines: bool }`;
  - (fix round 1) `ClearedVia { kind: ClearedViaKind /* role|group */, name: String, scope: Option<ClearedScope /* platform|workspace|object */>, scope_id: Option<Uuid>, through_group: Option<String> }`; role holders through a group are listed too. The spec C9 table is authoritative for what each population means.
  - `ClearedVia { kind: String /* role|group */, name: String }`. All camelCase.
  - `TableUsage.warehouse_id: Uuid` (wire `warehouseId`).
  - Generated hook `usePermClearedPeople(tagId)`.

- [ ] **Step 1: Write the failing tests**

In `crates/egret-perm-compile/tests/people.rs`:

```rust
async fn insert_tag_with_clearance(pool: &sqlx::PgPool, roles: &[String], groups: &[Uuid]) -> Uuid {
    let t = tag();
    let id: Uuid = sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, $2) RETURNING id")
        .bind(format!("clr-{t}"))
        .bind(format!("Clr {t}"))
        .fetch_one(pool)
        .await
        .expect("tag");
    sqlx::query(
        "INSERT INTO classification_policies (tag_id, required_roles, required_groups) VALUES ($1, $2, $3)",
    )
    .bind(id)
    .bind(roles)
    .bind(groups)
    .execute(pool)
    .await
    .expect("clearance");
    id
}

/// Spec C9: role holders and group members, each person once, an expired grant
/// not counted, and `onEngines` only through a required role.
#[tokio::test]
async fn cleared_people_lists_role_holders_and_group_members_once() {
    let Some(e) = env().await else { return };
    let p = &e.db.pool;
    let t = tag();
    let both = insert_user(p, &format!("clr_both_{t}@egret.local"), "Both", Some(&format!("clr_both_{t}"))).await;
    let role_only = insert_user(p, &format!("clr_role_{t}@egret.local"), "Role only", Some(&format!("clr_role_{t}"))).await;
    let group_only = insert_user(p, &format!("clr_grp_{t}@egret.local"), "Group only", None).await;
    let expired = insert_user(p, &format!("clr_exp_{t}@egret.local"), "Expired", Some(&format!("clr_exp_{t}"))).await;
    let role_name = format!("clr_role_{t}");
    let role = insert_role(p, "platform", &role_name).await;
    insert_grant(p, "user", both, role, "platform", None, "allow", false).await;
    insert_grant(p, "user", role_only, role, "platform", None, "allow", false).await;
    insert_grant(p, "user", expired, role, "platform", None, "allow", true).await;
    let group = insert_group(p, &format!("clr_group_{t}"), &[both, group_only]).await;
    let tag_id = insert_tag_with_clearance(p, &[role_name.clone()], &[group]).await;

    let body = principal(&e, &format!("/perm/clearances/{tag_id}/people")).await;
    assert_eq!(body["count"], 3, "{body}");
    assert_eq!(body["truncated"], false);
    let people = body["people"].as_array().unwrap();
    let labels: Vec<&str> = people.iter().map(|p| p["label"].as_str().unwrap()).collect();
    assert_eq!(labels, vec!["Both", "Group only", "Role only"], "sorted by label: {body}");
    let both_row = &people[0];
    assert_eq!(both_row["via"].as_array().unwrap().len(), 2, "{both_row}");
    assert_eq!(both_row["onEngines"], true);
    assert_eq!(people[1]["onEngines"], false, "a group member is not named on engines");
    assert!(people[1]["username"].is_null());
}

#[tokio::test]
async fn cleared_people_is_gated_and_answers_404_and_empty() {
    let Some(e) = env().await else { return };
    let tag_id = insert_tag_with_clearance(&e.db.pool, &[], &[]).await;
    let path = format!("/perm/clearances/{tag_id}/people");
    let (status, _) = get_path(&e, &e.nobody, &path).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = get_path(&e, &e.reader, &path).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!({"count": 0, "people": [], "truncated": false}));
    let (status, _) = get_path(&e, &e.reader, &format!("/perm/clearances/{}/people", Uuid::new_v4())).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
```

In `crates/egret-controlplane/tests/governance_tag_usage.rs`, extend the test that asserts a table row: add `assert_eq!(row["warehouseId"], wh.to_string());`. `wh` is the warehouse id the test's fixture inserts; use its existing binding.

- [ ] **Step 2: Run to confirm they fail**

```bash
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test cargo test -p egret-perm-compile --test people cleared_people 2>&1 | tee "$SCRATCH"/classux-t4a.log | tail -10
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test cargo test -p egret-controlplane --test governance_tag_usage 2>&1 | tee "$SCRATCH"/classux-t4b.log | tail -10
grep -c "skip: DATABASE_URL" "$SCRATCH"/classux-t4a.log "$SCRATCH"/classux-t4b.log
```

Expected: a 404 from the router (no route), and a null `warehouseId`. grep `0`.

- [ ] **Step 3: Implement**

1. `people.rs`, `cleared_people`:
   - `require(policy:read)`.
   - `SELECT required_roles, required_groups FROM classification_policies WHERE tag_id = $1`. When there is no row, check `SELECT EXISTS(SELECT 1 FROM tags WHERE id = $1)`: return 404 (`PermHttpError::NotFound`) when the tag does not exist, else the empty answer.
   - Role holders: `crate::members::role_holders_via(&mut conn, &roles)`. These rows also mark `on_engines = true`.
   - Group members:

```sql
SELECT g.name, ug.user_id FROM user_groups ug JOIN groups g ON g.id = ug.group_id WHERE g.id = ANY($1)
```

   - Group by `user_id` into `via` entries. Load `label` as `COALESCE(NULLIF(name,''), email)` and `username` from `users`.
   - Sort by `label`. `count` = number of distinct people. Truncate `people` at 200 (`truncated = count > 200`).
   - Use a read-only transaction, as `list_overlaps` does (`routes.rs:1769-1783`).
2. Route: `.route("/perm/clearances/{tag_id}/people", get(people::cleared_people))`. Add the `utoipa::path` (tag `perm`; 200/401/403/404/500) to the OpenAPI `paths(...)` list, and the three schemas to `components`.
3. `governance_tag_usage.rs`: add `pub warehouse_id: Uuid` to `TableUsage` and select it where the rows are grouped (it is the assignment's `warehouse_id`, already joined to `registry.catalogs`).

- [ ] **Step 4: Run to confirm they pass, then the two crates**

```bash
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test cargo test -p egret-perm-compile --test people -p egret-controlplane 2>&1 | tee "$SCRATCH"/classux-t4c.log | tail -10
grep -c "skip: DATABASE_URL" "$SCRATCH"/classux-t4c.log
```

- [ ] **Step 5: Regenerate OpenAPI and the client** (Task 1 Step 6). Expected new exports: `usePermClearedPeople`, `getPermClearedPeopleQueryKey`, `ClearedPeople`, `ClearedPerson`, `ClearedVia`; `TableUsage.warehouseId`.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all && cargo clippy -p egret-perm-compile -p egret-controlplane --all-targets -- -D warnings
(cd frontend && npx tsc -b)
git add crates openapi.json frontend/src/api
git commit -m "feat(perm): read the people a classification's clearance admits; name the warehouse on Where used

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

## Task 5: One vocabulary (C1)

**Files:**
- Create: `frontend/src/routes/governance/glossary.ts`, `frontend/src/routes/governance/apiErrors.ts`, `frontend/src/routes/governance/__tests__/glossary.test.ts`, `frontend/src/routes/governance/__tests__/apiErrors.test.ts`
- Modify:
  - `frontend/src/routes/governance/permFormat.ts` (`formatRuleKind` :162-169);
  - `RuleDialog.tsx` (move `MASK_TYPES` :41-50 to the glossary; labels "Selects by" → "Applies to", "Tag" → "Classification");
  - `classifications/TagPage.tsx` (:23-29);
  - `TagRulesSection.tsx` (:40, :45-48, :78-82);
  - `classifications/tabs/WhereUsedTab.tsx` (:134, :144);
  - `classifications/tabs/HandWrittenTagTab.tsx` (headings);
  - `data/ObjectHeader.tsx` (:39 action label, :164, :175);
  - `data/ColumnPanel.tsx` (:207 "Add tag", :219 "Tags", :234 "Masking rules");
  - `data/tabs/OverviewTab.tsx` (:305-306);
  - `data/ObjectTabs.tsx` (the "Masks & filters" tab label);
  - `data/tabs/MasksFiltersTab.tsx` (:194 empty text);
  - `data/AddTagDialog.tsx` (:128, :160-161, :169, :178);
  - `openInData.tsx` (`ExternalLink` → `ArrowRight`);
  - `people/itemParts.tsx` (`ItemHeader` gains `back`);
  - `people/RulePage.tsx` (reads `?from=`);
  - `tagDeleteFailure.ts` (copy).
- Test (update to the new names): `__tests__/RuleDialog.test.tsx`, `__tests__/TagRulesSection.test.tsx`, `classifications/__tests__/{TagPage,WhereUsedTab,PolicyTabs}.test.tsx`, `data/__tests__/{ObjectPage,ColumnPanel}.test.tsx`, `people/__tests__/RulePage.test.tsx`, `__tests__/permFormat.test.ts`

**Interfaces:**
- Produces: `glossary.ts`:
  - `TERM` (object of the §4 terms);
  - `MASK_TYPE_LABEL: Record<string, string>`;
  - `MASK_TYPES: {value: string; label: string}[]`;
  - `TAG_TAB_LABEL` values `Where used | Clearance | Rules | Views | Engine policies`.
- Produces: `apiErrors.ts`:
  - `type TableTagMaskConflict`, `type RuleOverlapBody`, `type TagInUse`, `type TagNameTaken`, `type TagKeyTaken`;
  - narrowers `tableTagMaskConflict(e)`, `ruleOverlap(e)`, `tagInUse(e)`, `tagNameTaken(e)`, `tagKeyTaken(e)`, `tagKeyInvalid(e)`. Each returns the typed body or `null`.
- Produces: `ItemHeader({kind, title, actions, back?: {to: string; label: string}})`; `RulePage` accepts `?from=classification:<id>` and `?from=data:<url-encoded path>`.

- [ ] **Step 1: Write the failing tests**

`frontend/src/routes/governance/__tests__/glossary.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { MASK_TYPE_LABEL, MASK_TYPES } from "@/routes/governance/glossary";
import { formatRuleKind } from "@/routes/governance/permFormat";
import type { Rule } from "@/api/generated/model";

const rule = (kind: Rule["kind"]): Rule => ({
  id: "r",
  name: "r",
  kind,
  selector: { type: "tag", key: "pii" },
  exempt: { include: [], exclude: [] },
});

describe("the glossary's mask types", () => {
  it("labels every type the dialog offers, and formatRuleKind uses the label", () => {
    expect(MASK_TYPES.map((m) => m.value)).toEqual([
      "MASK", "MASK_SHOW_LAST_4", "MASK_SHOW_FIRST_4", "MASK_HASH", "MASK_NULL", "MASK_NONE", "MASK_DATE_SHOW_YEAR", "CUSTOM",
    ]);
    expect(formatRuleKind(rule({ type: "mask", mask_type: "MASK_HASH" }))).toBe("Mask · Hash");
    expect(formatRuleKind(rule({ type: "mask", mask_type: "CUSTOM", value_expr: "upper(x)" }))).toBe("Mask · Custom (upper(x))");
    expect(formatRuleKind(rule({ type: "row_filter", filter_expr: "region = 'us'" }))).toBe("Row filter · region = 'us'");
    // An unknown type the API may still hold is shown as stored, never dropped.
    expect(formatRuleKind(rule({ type: "mask", mask_type: "MASK_FUTURE" }))).toBe("Mask · MASK_FUTURE");
    expect(MASK_TYPE_LABEL.MASK).toBe("Redact");
  });
});
```

`frontend/src/routes/governance/__tests__/apiErrors.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { ApiError } from "@/api/client";
import { ruleOverlap, tableTagMaskConflict, tagInUse, tagKeyTaken, tagNameTaken } from "@/routes/governance/apiErrors";

const err = (status: number, body: Record<string, unknown>) =>
  new ApiError(status, String(body.error), body.code as string | undefined, body);

describe("apiErrors", () => {
  it("narrows each code to its body and nothing else", () => {
    const conflict = err(400, {
      error: "tag \"PII\" is attached…", code: "table_tag_mask_conflict", tag: "PII", tagId: "t1",
      warehouseId: "w1", schema: "analytics", table: "orders", mask: "m", maskOrigin: "rule", moreTables: false,
    });
    expect(tableTagMaskConflict(conflict)).toMatchObject({ tag: "PII", warehouseId: "w1", table: "orders" });
    expect(ruleOverlap(conflict)).toBeNull();
    expect(tableTagMaskConflict(new ApiError(400, "bad request: x"))).toBeNull();
    expect(tableTagMaskConflict(new Error("x"))).toBeNull();

    const overlap = err(409, { error: "…", code: "rule_overlap", rules: [{ id: "a", name: "A" }, { id: null, name: "P" }], coordinate: "c.s.t.email", kind: "mask" });
    expect(ruleOverlap(overlap)?.rules.map((r) => r.name)).toEqual(["A", "P"]);
    expect(tagInUse(err(409, { error: "…", code: "tag_in_use", rules: [{ id: "r1", name: "Hash" }] }))?.rules).toEqual([{ id: "r1", name: "Hash" }]);
    expect(tagNameTaken(err(409, { error: "…", code: "tag_name_taken", name: "pii", existing: { id: "t", key: "pii", name: "PII" } }))?.existing.key).toBe("pii");
    expect(tagKeyTaken(err(409, { error: "…", code: "tag_key_taken", key: "pii", existing: { id: "t", name: "PII" } }))?.existing.name).toBe("PII");
  });
});
```

Update the existing tests to the new names, and add to each one an assertion that the retired name is gone. For example, in `data/__tests__/ObjectPage.test.tsx` the header test becomes:

```ts
    const tags = await within(header()).findByRole("list", { name: "Classifications" });
    expect(within(tags).getAllByRole("listitem").map((li) => li.textContent)).toEqual(["CONFIDENTIAL"]);
    expect(within(header()).queryByRole("list", { name: "Tags" })).not.toBeInTheDocument();
```

and the actions test expects `["Grant access", "Add classification", "Who can read?"]`. In `classifications/__tests__/TagPage.test.tsx`:

```ts
  it("names the tabs with the glossary", async () => {
    serve({ "/tags": ok([PII]) });
    openArea(URL_PII, TAG_READER);
    await screen.findByRole("heading", { level: 1, name: "PII" }, T);
    expect(tabNames()).toEqual(["Where used", "Clearance", "Rules", "Views", "Engine policies"]);
  });
```

In `people/__tests__/RulePage.test.tsx` (it renders `RulePage` under a `MemoryRouter`; use the file's existing render helper with an initial entry):

```ts
  test("the way back names the classification the rule was opened from", async () => {
    vi.mocked(tagsApi.useTags).mockReturnValue({ data: [{ id: "t-pii", key: "pii_v2", name: "PII Data" }] } as never);
    renderAt("/governance/rules/r1?from=classification:t-pii", WRITER);
    const back = await screen.findByRole("link", { name: "← PII Data" });
    expect(back).toHaveAttribute("href", "/governance/classifications/t-pii?tab=rules");
    expect(screen.queryByRole("link", { name: "People & groups" })).not.toBeInTheDocument();
  });
```

(If the file's helper is not called `renderAt`, add one that renders `<MemoryRouter initialEntries={[url]}><Routes><Route path="/governance/rules/:id" element={<RulePage />} /></Routes></MemoryRouter>` inside the providers the file already uses.)

- [ ] **Step 2: Run to confirm they fail**

```bash
cd frontend && npx vitest run src/routes/governance 2>&1 | tail -30
```

Expected: the new tests fail (no `glossary.ts`, old labels); the existing tests that you renamed fail on the new names.

- [ ] **Step 3: Implement**

1. Write `glossary.ts`:

```ts
/**
 * The Classification area's vocabulary (spec 2026-10-21-classification-ux-design §4):
 * one word per thing. A string in this area that names one of these concepts uses
 * the constant, so a rename happens here once.
 */
export const TERM = {
  classification: "Classification",
  classifications: "Classifications",
  key: "Key",
  attach: "Attach",
  remove: "Remove",
  rule: "Rule",
  rules: "Rules",
  mask: "Mask",
  rowFilter: "Row filter",
  exempt: "Exempt",
  appliesTo: "Applies to",
  clearance: "Clearance",
  whereUsed: "Where used",
  enginePolicies: "Engine policies",
  whoCanRead: "Who can read?",
  addClassification: "Add classification",
  addRule: "Add rule",
} as const;

/** Ranger's mask types (`crates/egret-policy/src/service-defs/trino.json`, `maskTypes`) with the dialog's labels. */
export const MASK_TYPES: { value: string; label: string }[] = [
  { value: "MASK", label: "Redact" },
  { value: "MASK_SHOW_LAST_4", label: "Partial mask: show last 4" },
  { value: "MASK_SHOW_FIRST_4", label: "Partial mask: show first 4" },
  { value: "MASK_HASH", label: "Hash" },
  { value: "MASK_NULL", label: "Nullify" },
  { value: "MASK_NONE", label: "Unmasked (retain original value)" },
  { value: "MASK_DATE_SHOW_YEAR", label: "Date: show only year" },
  { value: "CUSTOM", label: "Custom" },
];

export const MASK_TYPE_LABEL: Record<string, string> = Object.fromEntries(MASK_TYPES.map((m) => [m.value, m.label]));
```

2. `formatRuleKind`: `Mask · ${MASK_TYPE_LABEL[t] ?? t}`, `Mask · Custom (${expr})`, `Row filter · ${expr}`.
3. Apply the renames listed under Files. Use `TERM.*` for the exact strings:
   - Data header list `aria-label="Classifications"`;
   - column headers "Classifications" and "Rules";
   - section titles "Classifications" and "Rules";
   - tab "Rules";
   - empty texts "No rule applies to this <noun>." / "No rule uses this classification yet.";
   - the Rules table headers "Rule | Kind | Applies through | Exempt", where `reachText` reads "This classification" / "Classification <key>, which has the same name";
   - the section intro "Rules that mask or filter data carrying this classification. Engines match a rule by the classification's name, so a rule written on another classification with the same name applies here too.";
   - the Where used column "Attached to".
4. `apiErrors.ts`: one private `bodyOf(e, code)` reading `ApiError.code`/`ApiError.body` (`frontend/src/api/client.ts:4-27`) and one exported narrower per code, typed per the Global Constraints list.
5. `OpenInDataLink`: import `ArrowRight` instead of `ExternalLink` (lucide). `ItemHeader`: `back ?? { to: PEOPLE_BASE, label: "People & groups" }`, rendered as `← {label}`.
6. `RulePage`:
   - read `useSearchParams().get("from")`.
   - `classification:<id>` → `{to: tagUrl(id, "rules"), label: name}`, with the name from `useTags()`, or "Classification" while loading or without `tag:read`.
   - `data:<path>` → `{to: decodeURIComponent(path), label: last path segment}`.
   - Anything else → the default.
   - The Rules tab (`TagRulesSection`) links rules with `?from=classification:<tag.id>`; give it a `tagId` prop.
7. `tagDeleteFailure.ts` copy: `The "Rules" tab on ${where} lists the rules that use this classification.` (the code-based rewrite is Task 11).

- [ ] **Step 4: Run to confirm they pass**

```bash
cd frontend && npx vitest run src/routes/governance src/components/perm 2>&1 | tail -15
grep -rn "Mask & filter\|Masks & filters\|Masking rules\|Mask and filter rules\|Hand-written\"\|Selects by\|Reaches this tag\|Carries it\|egret rule\|Pick a tag" src/routes/governance --include='*.tsx' --include='*.ts' | grep -v __tests__
```

Expected: tests pass. The grep prints nothing; any hit left is a missed string to rename, unless it is a code comment, which may name the old term for history.

- [ ] **Step 5: Lint, types, commit**

```bash
cd frontend && npm run lint && npx tsc -b && cd ..
git add frontend/src
git commit -m "feat(classification): one vocabulary across Classification, Data and the rule pages

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

## Task 6: Classification chips, remove in place, a richer picker, the table warning (C4, C5-attach)

**Files:**
- Create: `frontend/src/routes/governance/ClassificationChip.tsx`, `frontend/src/routes/governance/RemoveClassificationDialog.tsx`, `frontend/src/routes/governance/__tests__/ClassificationChip.test.tsx`, `frontend/src/routes/governance/data/__tests__/AddTagDialog.test.tsx`
- Modify: `frontend/src/routes/governance/data/{ObjectHeader.tsx,ColumnPanel.tsx,tabs/OverviewTab.tsx,AddTagDialog.tsx,ObjectPage.tsx}`, `frontend/src/routes/governance/classifications/tabs/WhereUsedTab.tsx`, `frontend/src/routes/governance/data/__tests__/fakeDataServer.ts` (record DELETE)
- Test: `data/__tests__/ObjectPage.test.tsx`, `classifications/__tests__/WhereUsedTab.test.tsx`

**Interfaces:**
- Consumes: `TableUsage.warehouseId` (Task 4); `useControlPlaneCatalogUnassignResourceTag`; `tableTagMaskConflict` (Task 5); `rulesOnTag` (`ruleMatch.ts:62`).
- Produces:
  - `ClassificationChip({ tag: {id, key, name, color}, inherited?: boolean, onRemove?: () => void, objectLabel: string })`;
  - `RemoveClassificationDialog({ open, onOpenChange, tag: {id, name, key}, target: {warehouseId, schema, table, column?: string, label: string} })`;
  - `useCanEditCatalogTags(): boolean` (`catalog_registry:update` or platform admin), exported from `ClassificationChip.tsx`;
  - `TagTarget.columns?: string[]`.

- [ ] **Step 1: Write the failing tests**

`frontend/src/routes/governance/__tests__/ClassificationChip.test.tsx`:

```tsx
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";
import type { Me } from "@/api/hooks";
import { AuthContext } from "@/auth/useAuth";

vi.mock("@/api/tags", async () => {
  const actual = await vi.importActual<typeof import("@/api/tags")>("@/api/tags");
  return {
    ...actual,
    useTags: () => ({ data: [{ id: "t-sec", key: "sec", name: "Secret", required_roles: ["cleared"], required_groups: [] }] }),
  };
});
import { ClassificationChip } from "@/routes/governance/ClassificationChip";

const ME = (permissions: string[]): Me => ({ user_id: "u", email: "u@x", platform_roles: [], permissions });
const SECRET = { id: "t-sec", key: "sec", name: "Secret", color: "#e11d48" };

function show(me: Me, props: Partial<React.ComponentProps<typeof ClassificationChip>> = {}) {
  return render(
    <QueryClientProvider client={new QueryClient()}>
      <AuthContext.Provider value={{ me, isLoading: false }}>
        <MemoryRouter>
          <ClassificationChip tag={SECRET} objectLabel="sales.orders.email" {...props} />
        </MemoryRouter>
      </AuthContext.Provider>
    </QueryClientProvider>,
  );
}

describe("ClassificationChip", () => {
  it("shows the colour, links to the classification and says it requires clearance", () => {
    show(ME(["tag:read"]));
    const link = screen.getByRole("link", { name: /Secret/ });
    expect(link).toHaveAttribute("href", "/governance/classifications/t-sec");
    expect(link.parentElement?.querySelector('[aria-hidden="true"][style]')).toHaveStyle({ backgroundColor: "#e11d48" });
    expect(screen.getByText("requires clearance")).toHaveClass("sr-only");
  });

  it("is plain text without tag:read", () => {
    show(ME([]));
    expect(screen.queryByRole("link")).not.toBeInTheDocument();
    expect(screen.getByText("Secret")).toBeInTheDocument();
  });

  it("offers Remove only when given a handler, and names what it removes from where", async () => {
    const onRemove = vi.fn();
    const { unmount } = show(ME(["tag:read"]));
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
    unmount();
    show(ME(["tag:read"]), { onRemove });
    await userEvent.click(screen.getByRole("button", { name: "Remove Secret from sales.orders.email" }));
    expect(onRemove).toHaveBeenCalledOnce();
  });

  it("marks an inherited chip and never offers Remove on it", () => {
    show(ME(["tag:read"]), { inherited: true, onRemove: vi.fn() });
    expect(screen.getByText("from table")).toBeInTheDocument();
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });
});
```

In `data/__tests__/fakeDataServer.ts`, record DELETEs too. Change the resource-tags branch to:

```ts
      if (w && ["PUT", "DELETE"].includes((init?.method ?? "GET").toUpperCase())) {
        const method = (init?.method ?? "GET").toUpperCase();
        tagWrites.push({ method, warehouseId: decodeURIComponent(w[1]), body: JSON.parse(String(init?.body)) });
        return reply(data.tagWrite ?? { status: 200, body: {} });
      }
```

Then add `method: string` to `DataCalls["tagWrites"]`'s element type, and in existing assertions on `tagWrites` add `method: "PUT"`.

In `data/__tests__/ObjectPage.test.tsx`:

```ts
  it("removes a column's own classification after a confirm, and refetches", async () => {
    const user = userEvent.setup();
    const { tagWrites, tagCalls } = serveData(fixture({ tagCatalogue: ok([]) }));
    renderPage("/governance/data/ice/sales/orders", { ...STEWARD, permissions: ["policy:read", "tag:read", "catalog_registry:update"] });
    const columns = await within(panel()).findByRole("table", { name: "Columns" });
    await user.click(await within(columns).findByRole("button", { name: "Remove PII from ice.sales.orders.email" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Remove PII from ice.sales.orders.email?" });
    const before = tagCalls.length;
    await user.click(within(dialog).getByRole("button", { name: "Remove" }));
    await waitFor(() =>
      expect(tagWrites).toEqual([
        { method: "DELETE", warehouseId: "wh-c-ice", body: { schema: "sales", table: "orders", column: "email", tag_id: "t-pii" } },
      ]),
    );
    await waitFor(() => expect(tagCalls.length).toBeGreaterThan(before));
    expect(toast.success).toHaveBeenCalledWith("Removed PII from ice.sales.orders.email.");
  });

  it("offers no Remove without catalog_registry:update", async () => {
    serveData(fixture());
    renderPage("/governance/data/ice/sales/orders");
    const columns = await within(panel()).findByRole("table", { name: "Columns" });
    await within(columns).findByText("PII");
    expect(within(columns).queryByRole("button", { name: /^Remove / })).not.toBeInTheDocument();
  });
```

`frontend/src/routes/governance/data/__tests__/AddTagDialog.test.tsx`:

```tsx
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Me } from "@/api/hooks";
import { AuthContext } from "@/auth/useAuth";
import { AddTagDialog, type TagTarget } from "@/routes/governance/data/AddTagDialog";
import { serve, ok, fail } from "@/routes/governance/engines/__tests__/engineHarness";

afterEach(() => vi.unstubAllGlobals());

const PII = { id: "t-pii", key: "pii", name: "PII", description: "Personal data", color: "#e11d48", assignment_count: 0, created_at: "", required_roles: [], required_groups: [] };
const HASH = { id: "r1", name: "Hash PII", kind: { type: "mask", mask_type: "MASK_HASH" }, selector: { type: "tag", key: "pii" }, exempt: { include: [], exclude: [] }, tag_name: "PII" };
const TABLE: TagTarget = { catalog: "ice", catalogId: "c-ice", warehouseId: "wh-1", schema: "sales", table: "orders", assigned: [], columns: ["id", "email"] };
const me = (permissions: string[]): Me => ({ user_id: "u", email: "u@x", platform_roles: [], permissions });

function open(target: TagTarget, as: Me) {
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <AuthContext.Provider value={{ me: as, isLoading: false }}>
        <MemoryRouter>
          <AddTagDialog target={target} onOpenChange={() => {}} />
        </MemoryRouter>
      </AuthContext.Provider>
    </QueryClientProvider>,
  );
}

async function pick(name: string) {
  await userEvent.click(await screen.findByRole("combobox", { name: "Classification" }));
  await userEvent.click(await screen.findByRole("option", { name: new RegExp(name) }));
}

describe("Add classification on a whole table", () => {
  it("warns before submit when a mask uses the classification, and attaches to columns instead", async () => {
    const served = serve({ "/tags": ok([PII]), "/perm/rules": ok([HASH]), "PUT /warehouses/wh-1/resource-tags": ok({ ok: true }) });
    open(TABLE, me(["tag:read", "policy:read", "catalog_registry:update"]));
    await pick("PII");
    expect(await screen.findByText(/would make sales\.orders unreadable/)).toBeInTheDocument();
    const add = screen.getByRole("button", { name: "Add classification" });
    expect(add).toHaveAttribute("aria-disabled", "true");
    expect(add).toHaveAccessibleDescription(/Hash PII masks by PII/);
    await userEvent.click(screen.getByRole("button", { name: "Attach to columns instead" }));
    await userEvent.click(screen.getByRole("checkbox", { name: "email" }));
    await userEvent.click(screen.getByRole("button", { name: "Attach to 1 column" }));
    await waitFor(() =>
      expect(served.bodies["PUT /warehouses/wh-1/resource-tags"]).toEqual([{ schema: "sales", table: "orders", column: "email", tag_id: "t-pii" }]),
    );
  });

  it("without policy:read, says what a table-level classification does and lets the server decide", async () => {
    serve({ "/tags": ok([PII]), "PUT /warehouses/wh-1/resource-tags": { status: 400, body: { error: "tag \"PII\" is attached…", code: "table_tag_mask_conflict", tag: "PII", tagId: "t-pii", warehouseId: null, schema: "sales", table: "orders", mask: "Hash PII", maskOrigin: "rule", moreTables: false } } });
    open(TABLE, me(["tag:read", "catalog_registry:update"]));
    await pick("PII");
    expect(screen.getByText(/A rule that masks by classification needs it on columns/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Add classification" }));
    const alert = await screen.findByRole("alert");
    expect(within(alert).getByText(/Hash PII masks by PII, so PII on the whole table sales\.orders would make it unreadable/)).toBeInTheDocument();
  });

  it("shows the swatch, key, description and clearance in each option", async () => {
    serve({ "/tags": ok([{ ...PII, required_roles: ["cleared"] }]), "/perm/rules": ok([]) });
    open({ ...TABLE, column: "email" }, me(["tag:read", "policy:read", "catalog_registry:update"]));
    await userEvent.click(await screen.findByRole("combobox", { name: "Classification" }));
    const option = await screen.findByRole("option", { name: /PII/ });
    expect(option).toHaveTextContent("pii");
    expect(option).toHaveTextContent("Personal data");
    expect(option).toHaveTextContent("Clearance");
  });

  it("keeps a failed write's server sentence beside the field", async () => {
    serve({ "/tags": ok([PII]), "/perm/rules": ok([]), "PUT /warehouses/wh-1/resource-tags": fail(500, "internal error") });
    open({ ...TABLE, column: "email" }, me(["tag:read", "policy:read", "catalog_registry:update"]));
    await pick("PII");
    await userEvent.click(screen.getByRole("button", { name: "Add classification" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("internal error");
  });
});
```

(`serve` strips `/api/control-plane`, so `/perm/rules` and `/tags` are the keys; see `engines/__tests__/engineHarness.tsx:46-64`.)

In `classifications/__tests__/WhereUsedTab.test.tsx`, add a test that serves a usage with `tables: [{ catalogId: "c1", catalogName: "ice", warehouseId: "wh-1", schema: "sales", table: "orders", onTable: false, columns: ["email"] }]`. As `ADMIN`, it clicks "Remove PII from ice.sales.orders.email", confirms, and asserts `served.bodies["DELETE /warehouses/wh-1/resource-tags"]` equals `[{ schema: "sales", table: "orders", column: "email", tag_id: "t-pii" }]`.

- [ ] **Step 2: Run to confirm they fail**

```bash
cd frontend && npx vitest run src/routes/governance/__tests__/ClassificationChip.test.tsx src/routes/governance/data src/routes/governance/classifications 2>&1 | tail -30
```

- [ ] **Step 3: Implement**

1. **`ClassificationChip.tsx`**:
   - a swatch span (`aria-hidden`, `style={{ backgroundColor: tag.color || DEFAULT_COLOR }}`; import `DEFAULT_COLOR` from `classifications/tagParts.tsx`, statically importable since it has no lazy-area dependency);
   - the name as a `Link` to `tagUrl(tag.id)` when `tag:read` (or admin), else a `span`;
   - `Lock` (`aria-hidden`) plus `<span className="sr-only">requires clearance</span>` when `useTags().data` shows that id with clearance;
   - for an inherited chip, "from table" in `text-muted-foreground`;
   - when `onRemove` is given and the chip is not inherited, `IconButton`/`Button size="icon" variant="ghost"` with `aria-label={`Remove ${tag.name} from ${objectLabel}`}` and an `X` icon.
   - The outer element is a `Badge variant="outline"` (not `secondary`), so the swatch carries the colour.
2. **`RemoveClassificationDialog.tsx`**: `AlertDialog`, title `Remove ${tag.name} from ${target.label}?`. The body text is from spec C4; it reads `usePermListRules` only when `usePolicyPermissions()("policy:read")`, and lists `rulesOnTag(rules, tag)` names. **Remove** calls `useControlPlaneCatalogUnassignResourceTag().mutateAsync({ warehouseId, data: { schema, table, column: column ?? "", tag_id } })`. On success it runs the same invalidations as `AddTagDialog.tsx:80-93` and `toast.success(`Removed ${tag.name} from ${target.label}.`)`. On failure it keeps the dialog open and shows `refusalMessage(e, "catalog_registry:update", "The classification could not be removed. Try again.")` in an `Alert` (FE-ACTION-3).
3. **Use the chip in the three places**: `ObjectHeader.tsx:175-181`, `ColumnPanel.tsx:266-279`, `OverviewTab.tsx:364-375`. Pass `onRemove` when `useCanEditCatalogTags()` and the object is in an egret catalog (the pages already know `warehouseId`: `TagTarget.warehouseId`).
4. **`WhereUsedTab.tsx`**: an actions column (`ACTIONS_CELL`) with one Remove button per assignment, via the same dialog, `target.label = `${catalogName}.${schema}.${table}${column ? "." + column : ""}``.
5. **`AddTagDialog.tsx`**:
   - The label is "Classification" and the title is "Add classification".
   - Each `SelectItem` holds the swatch, the name, `<span className="font-mono text-xs">{key}</span>`, the description's first line, "Clearance" with a `Lock` when gated, and "N rules" with `policy:read`.
   - For a table target:
     - when `policy:read`, compute `masks = rulesOnTag(rules, picked).filter((r) => r.kind.type === "mask")`;
     - if non-empty, render the conflict sentence (`${masks[0].name} masks by ${picked.name}, so ${picked.name} on the whole table ${schema}.${table} would make it unreadable.`), set Add to aria-disabled with `aria-describedby` on that sentence, and render **Attach to columns instead**, which switches to a checkbox list of `target.columns`;
     - the submit becomes `Attach to N column(s)` and sends one `assign.mutateAsync` per checked column in order. Stop at the first failure, show it, and keep the remaining ticks.
     - When there are no masks or no `policy:read`, render the note from spec C5.
   - Server errors: `tableTagMaskConflict(assign.error)` → the short sentence; else `refusalMessage(...)` as today.
6. **`ObjectPage.tsx`**: pass `columns` (the browse result's column names) into `TagTarget` for a table.

- [ ] **Step 4: Run, lint, types**

```bash
cd frontend && npx vitest run src/routes/governance 2>&1 | tail -15 && npm run lint && npx tsc -b && npm run test:bundle:governance-data && npm run test:bundle:governance-classification
```

Expected: pass. The bundle guards stay green: `ClassificationChip.tsx` and `RemoveClassificationDialog.tsx` live in `routes/governance/`, not under `data/`, and import nothing from `data/`.

- [ ] **Step 5: Commit**

```bash
git add frontend/src
git commit -m "feat(classification): coloured, linked classification chips with remove in place; warn on a table-level classification a mask uses

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

## Task 7: The rule dialog: reasons, required fields, defaults, drafts, errors in view (C8)

**Files:**
- Create: `frontend/src/routes/governance/ruleDraft.ts`, `frontend/src/routes/governance/__tests__/ruleDraft.test.ts`
- Modify: `frontend/src/routes/governance/RuleDialog.tsx`, `frontend/src/routes/governance/data/tabs/MasksFiltersTab.tsx` (:239-246), `frontend/src/routes/governance/data/ColumnPanel.tsx` (new "Add rule"), `frontend/src/routes/governance/people/RulePage.tsx` (:71), `frontend/src/components/perm/PrincipalPicker.tsx` (exempt mode: the suggestions list after the help line)
- Test: `frontend/src/routes/governance/__tests__/RuleDialog.test.tsx`

**Interfaces:**
- Produces:
  - `type RuleEntry`, exactly:

```ts
type RuleEntry =
  | { kind: "classification"; tagKey: string; tagName: string }
  | { kind: "table"; catalogId: string; schema: string; table: string }
  | { kind: "column"; catalogId: string; schema: string; table: string; column: string; classifications: { key: string; name: string }[] }
  | { kind: "edit"; rule: Rule };
```

  - `seedRule(entry: RuleEntry): NewRule`;
  - `ruleDraftKey(userId: string, entry: RuleEntry): string`;
  - `loadRuleDraft(key): NewRule | undefined`, `saveRuleDraft(key, draft: NewRule): void`, `clearRuleDraft(key): void`;
  - `RuleDialog({ open, onOpenChange, entry: RuleEntry })`, which replaces `existing`/`initialSelector`/`initialKind`.

- [ ] **Step 1: Write the failing tests**

`frontend/src/routes/governance/__tests__/ruleDraft.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { clearRuleDraft, loadRuleDraft, ruleDraftKey, saveRuleDraft, seedRule } from "@/routes/governance/ruleDraft";

describe("seedRule: defaults per entry point (spec C8)", () => {
  it("from a classification: a Redact mask on that classification, named after it", () => {
    expect(seedRule({ kind: "classification", tagKey: "pii", tagName: "PII" })).toEqual({
      name: "Mask PII",
      kind: { type: "mask", mask_type: "MASK" },
      selector: { type: "tag", key: "pii" },
      exempt: { include: [], exclude: [] },
    });
  });
  it("from a table: a mask on the table's path, column left empty", () => {
    expect(seedRule({ kind: "table", catalogId: "c", schema: "s", table: "t" })).toEqual({
      name: "",
      kind: { type: "mask", mask_type: "MASK" },
      selector: { type: "path", catalog_id: "c", schema: "s", table: "t" },
      exempt: { include: [], exclude: [] },
    });
  });
  it("from a column: a mask on that column", () => {
    const r = seedRule({ kind: "column", catalogId: "c", schema: "s", table: "t", column: "email", classifications: [] });
    expect(r.selector).toEqual({ type: "path", catalog_id: "c", schema: "s", table: "t", column: "email" });
  });
});

describe("the draft store", () => {
  it("keys drafts by user and entry, and clears one without touching another", () => {
    const a = ruleDraftKey("u1", { kind: "classification", tagKey: "pii", tagName: "PII" });
    const b = ruleDraftKey("u2", { kind: "classification", tagKey: "pii", tagName: "PII" });
    expect(a).not.toBe(b);
    const draft = seedRule({ kind: "classification", tagKey: "pii", tagName: "PII" });
    saveRuleDraft(a, { ...draft, name: "kept" });
    expect(loadRuleDraft(a)?.name).toBe("kept");
    expect(loadRuleDraft(b)).toBeUndefined();
    clearRuleDraft(a);
    expect(loadRuleDraft(a)).toBeUndefined();
  });
});
```

In `__tests__/RuleDialog.test.tsx`, first convert the existing `renderDialog` callers: `existing={r}` → `entry={{ kind: "edit", rule: r }}`; the default render → `entry={{ kind: "classification", tagKey: "pii_v2", tagName: "PII Data" }}`; tests that relied on an empty tag → `entry={{ kind: "table", catalogId: "cat-1", schema: "sales", table: "orders" }}` plus a switch of Applies to. Wrap the render in `AuthContext.Provider` with `{ me: { user_id: "u1", email: "", platform_roles: ["platform_admin"], permissions: [] }, isLoading: false }`. Then add:

```tsx
describe("RuleDialog — Create always says why it is not available (C8.2)", () => {
  test("Create stays focusable and is described by the reason in the footer", async () => {
    const user = userEvent.setup();
    renderDialog({ entry: { kind: "table", catalogId: "cat-1", schema: "sales", table: "orders" } });
    const create = screen.getByRole("button", { name: "Create" });
    expect(create).toHaveAttribute("aria-disabled", "true");
    expect(create).not.toBeDisabled();
    expect(create).toHaveAccessibleDescription("Name the rule.");
    await user.type(screen.getByLabelText(/^Name/), "mask email");
    expect(create).toHaveAccessibleDescription("Preview the rule to enable Create.");
    await user.click(create);
    expect(createMutateAsync).not.toHaveBeenCalled();
  });

  test("required labels say so", () => {
    renderDialog({ entry: { kind: "classification", tagKey: "pii_v2", tagName: "PII Data" } });
    expect(screen.getByText("Name").closest("label")).toHaveTextContent("Name (required)");
    expect(screen.getByLabelText(/^Name/)).toHaveAttribute("aria-required", "true");
  });

  test("a path mask says that an empty column masks every column", () => {
    renderDialog({ entry: { kind: "table", catalogId: "cat-1", schema: "sales", table: "orders" } });
    expect(screen.getByText("Leave Column empty to mask every column of the table.")).toBeInTheDocument();
  });
});

describe("RuleDialog — defaults per entry point (C8.3)", () => {
  test("from a classification: Mask, Redact, Applies to = that classification, named after it", () => {
    renderDialog({ entry: { kind: "classification", tagKey: "pii_v2", tagName: "PII Data" } });
    expect(screen.getByLabelText(/^Name/)).toHaveValue("Mask PII Data");
    expect(screen.getByRole("combobox", { name: "Kind" })).toHaveTextContent("Mask a column");
    expect(screen.getByRole("combobox", { name: "Applies to" })).toHaveTextContent("Classification");
    expect(screen.getByText("PII Data")).toBeInTheDocument();
  });

  test("from a table: Mask on the table's path, not a row filter", () => {
    renderDialog({ entry: { kind: "table", catalogId: "cat-1", schema: "sales", table: "orders" } });
    expect(screen.getByRole("combobox", { name: "Kind" })).toHaveTextContent("Mask a column");
    expect(JSON.parse(screen.getByTestId("scope-value").textContent ?? "{}")).toEqual({ catalog_id: "cat-1", schema: "sales", table: "orders" });
  });
});

describe("RuleDialog — the draft survives Cancel (C8.4)", () => {
  test("Cancel on a dirty form asks first; Keep editing keeps everything; reopening restores it", async () => {
    const user = userEvent.setup();
    const entry = { kind: "table", catalogId: "cat-1", schema: "sales", table: "orders" } as const;
    const { onOpenChange, unmount } = renderDialog({ entry });
    await user.type(screen.getByLabelText(/^Name/), "mask email");
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    const ask = await screen.findByRole("alertdialog", { name: "Discard this rule?" });
    await user.click(within(ask).getByRole("button", { name: "Keep editing" }));
    expect(screen.getByLabelText(/^Name/)).toHaveValue("mask email");
    expect(onOpenChange).not.toHaveBeenCalledWith(false);
    unmount(); // the user navigates away (a detour) with the dialog open
    renderDialog({ entry });
    expect(screen.getByLabelText(/^Name/)).toHaveValue("mask email");
    expect(screen.getByText(/Restored your unsaved rule/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Start over" }));
    expect(screen.getByLabelText(/^Name/)).toHaveValue("");
  });
});

describe("RuleDialog — errors stay with the form (C8.5)", () => {
  test("a preview error renders in the footer, takes focus, and clears no field", async () => {
    const user = userEvent.setup();
    previewMutateAsync.mockRejectedValueOnce(new ApiError(400, "bad request: nope"));
    renderDialog({ entry: { kind: "classification", tagKey: "pii_v2", tagName: "PII Data" } });
    await user.click(screen.getByRole("button", { name: "Preview" }));
    const footer = screen.getByRole("group", { name: "Rule actions" });
    const err = await within(footer).findByRole("alert");
    expect(err).toHaveTextContent("nope");
    expect(err.contains(document.activeElement) || err === document.activeElement).toBe(true);
    expect(screen.getByLabelText(/^Name/)).toHaveValue("Mask PII Data");
  });
});
```

(Add `within` to the file's `@testing-library/react` import.)

- [ ] **Step 2: Run to confirm they fail**

```bash
cd frontend && npx vitest run src/routes/governance/__tests__/RuleDialog.test.tsx src/routes/governance/__tests__/ruleDraft.test.ts 2>&1 | tail -30
```

- [ ] **Step 3: Implement**

1. **`ruleDraft.ts`**: a module-level `const drafts = new Map<string, NewRule>()`.
   - `ruleDraftKey(userId, entry)` is `${userId}|classification:${tagKey}`, `${userId}|table:${catalogId}/${schema}/${table}`, `${userId}|column:${…}/${column}` or `${userId}|edit:${rule.id}`.
   - `seedRule` per spec C8.3 (edit → the rule's own fields).
2. **`RuleDialog.tsx`**:
   - `RuleDialog({ open, onOpenChange, entry })` renders `<RuleForm entry={entry} …/>` while open. The form seeds from `loadRuleDraft(key) ?? seedRule(entry)` and saves on every change (`useEffect` on the body snapshot) unless the body equals the seed.
   - `dirty = snapshot !== seedSnapshot`. `useUnsavedChangesWarning(dirty)`.
   - The dialog's `onOpenChange(false)` (Cancel, Escape, outside click) goes through `requestClose()`. When dirty it opens an `AlertDialog` "Discard this rule?" with **Keep editing** and **Discard**; Discard calls `clearRuleDraft(key)` and closes.
   - On successful save: `clearRuleDraft(key)`.
   - A restored draft shows `Restored your unsaved rule · <Button variant="link">Start over</Button>`.
   - **Labels**:
     - `Name (required)`, with `(required)` as `<span className="text-muted-foreground">`;
     - `Applies to` with options **Classification** / **Path**; the Classification picker label is "Classification (required)";
     - "Custom expression (required)", "Filter expression (required)";
     - `aria-required="true"` on those inputs.
   - Path mask help under `ScopePicker`: "Leave Column empty to mask every column of the table." when kind is mask and Applies to is Path.
   - **Footer**: `<div role="group" aria-label="Rule actions" className="sticky bottom-0 -mx-6 border-t bg-background px-6 py-3">`. It holds the reason line (`id={reasonId}`, the first of `blockedReason`, `"Preview the rule to enable Create."` when `!preview || previewStale`, and the preview/save error), the error `Alert` (`tabIndex={-1}`, focused via a ref after it renders), **Cancel**, and **Create** / **Save changes** rendered with `aria-disabled={!canSave || saving}`, `aria-describedby={canSave ? undefined : reasonId}`, and `onClick={() => canSave && !saving && void save()}`. Preview gets the same pattern for `blockedReason`.
   - Remove the old in-body `saveError` (:347-352) and `blockedReason` (:447-451) alerts. The preview box keeps only the preview result.
3. **Callers**:
   - `MasksFiltersTab.tsx:239-246`: `entry={{ kind: "table", catalogId: catalog.id, schema, table }}` for a table, and the column entry for a column.
   - `RulePage.tsx:71`: `entry={{ kind: "edit", rule: found }}`.
   - `ColumnPanel.tsx`: an **Add rule** button (shown with `policy:update`) beside Who can read, opening the column entry. The dialog shows a link button "Or apply it to <name>" per classification on the column, which switches Applies to = Classification with that key.
4. **`PrincipalPicker.tsx`**, exempt mode: render the suggestions list after the help line, not absolutely over it. Assert in `components/perm/__tests__/PrincipalPicker.test.tsx` with `compareDocumentPosition` that the help text precedes the listbox.

- [ ] **Step 4: Run, lint, types**

```bash
cd frontend && npx vitest run src/routes/governance src/components/perm 2>&1 | tail -15 && npm run lint && npx tsc -b
```

- [ ] **Step 5: Commit**

```bash
git add frontend/src
git commit -m "feat(classification): rule dialog says why Create waits, marks required fields, starts from where you are and keeps your draft

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

## Task 8: Preview in effect terms, overlaps, and the table conflict in the rule dialog (C5-rule, C6, C7)

**Files:**
- Create: `frontend/src/routes/governance/ruleEffect.ts`, `frontend/src/routes/governance/__tests__/ruleEffect.test.ts`
- Modify: `frontend/src/routes/governance/RuleDialog.tsx` (preview block :453-518)
- Test: `frontend/src/routes/governance/__tests__/RuleDialog.test.tsx`

**Interfaces:**
- Consumes:
  - `PreviewResponse.effect: RuleEffect`, `ExistingMask` (Task 3);
  - `tableTagMaskConflict`, `ruleOverlap` (Task 5);
  - `useControlPlaneCatalogUnassignResourceTag`.
- Produces:
  - `effectSentence(e: RuleEffect): string`;
  - `type PrincipalLine = { principal: string; verdict: "real" | "masked" | "undecided"; text: string }`;
  - `effectivePerPrincipal(rule: { maskType: string; exempt: PrincipalSet }, existing: ExistingMask[]): PrincipalLine[]`.

- [ ] **Step 1: Write the failing tests**

`frontend/src/routes/governance/__tests__/ruleEffect.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import type { ExistingMask, RuleEffect } from "@/api/generated/model";
import { effectSentence, effectivePerPrincipal } from "@/routes/governance/ruleEffect";

const base: RuleEffect = {
  kind: "mask", catalogs: [], tableCount: 1, columnCount: 1, enforcedBy: 2, exemptMembers: 3, exemptEveryone: false, alreadyMasked: [],
};

describe("effectSentence", () => {
  it("says what is masked, by how many engines, for how many people", () => {
    expect(effectSentence(base)).toBe("Masks 1 column on 1 table · enforced by 2 engines · 3 people exempt");
    expect(effectSentence({ ...base, columnCount: 12, tableCount: 5, enforcedBy: 1, exemptMembers: 1 }))
      .toBe("Masks 12 columns on 5 tables · enforced by 1 engine · 1 person exempt");
    expect(effectSentence({ ...base, columnCount: null })).toBe("Masks every column of 1 table · enforced by 2 engines · 3 people exempt");
    expect(effectSentence({ ...base, tableCount: null, columnCount: null })).toBe("Masks the columns matching the path · enforced by 2 engines · 3 people exempt");
    expect(effectSentence({ ...base, kind: "row_filter", tableCount: 4, columnCount: null, exemptMembers: 0 }))
      .toBe("Filters rows of 4 tables · enforced by 2 engines · nobody exempt");
    expect(effectSentence({ ...base, exemptEveryone: true })).toBe("Masks 1 column on 1 table · enforced by 2 engines · everyone exempt");
  });

  it("leaves the engine count out when nothing enforces it (the warning says it)", () => {
    expect(effectSentence({ ...base, enforcedBy: 0 })).toBe("Masks 1 column on 1 table · 3 people exempt");
  });
});

const g = (ref: string) => ({ kind: "group" as const, ref });
const other = (name: string, maskType: string, include: ReturnType<typeof g>[]): ExistingMask => ({
  ruleId: name, ruleName: name, maskType, coordinate: "c.s.t.email", exempt: { include, exclude: [] },
});

describe("effectivePerPrincipal (spec C6)", () => {
  it("exempt from every mask sees the value; from none is masked; from some is undecided", () => {
    const lines = effectivePerPrincipal(
      { maskType: "MASK", exempt: { include: [g("data-team"), g("auditors")], exclude: [] } },
      [other("Hash personal data", "MASK_HASH", [g("data-stewards"), g("auditors")])],
    );
    expect(lines.map((l) => [l.principal, l.verdict])).toEqual([
      ["group auditors", "real"],
      ["group data-stewards", "undecided"],
      ["group data-team", "undecided"],
      ["Everyone else", "masked"],
    ]);
    expect(lines.find((l) => l.principal === "Everyone else")?.text).toBe("masked; which mask applies is decided by engine order");
  });

  it("names the mask type when every mask agrees", () => {
    const lines = effectivePerPrincipal({ maskType: "MASK_HASH", exempt: { include: [], exclude: [] } }, [other("A", "MASK_HASH", [])]);
    expect(lines).toEqual([{ principal: "Everyone else", verdict: "masked", text: "masked (Hash)" }]);
  });
});
```

In `__tests__/RuleDialog.test.tsx`:

```tsx
const EFFECT = { kind: "mask", catalogs: [{ catalogId: "cat-1", catalogName: "wh", engines: [], notEnforced: "no_engine" }], tableCount: 1, columnCount: 1, enforcedBy: 0, exemptMembers: 2, exemptEveryone: false, alreadyMasked: [] };

describe("RuleDialog — the preview speaks in effect (C7)", () => {
  test("leads with the effect, warns when no engine enforces it, keeps profiles under details", async () => {
    const user = userEvent.setup();
    previewMutateAsync.mockResolvedValueOnce({ status: 200, data: { ...OK_PREVIEW_ONE.data, effect: EFFECT } });
    renderDialog({ entry: { kind: "classification", tagKey: "pii_v2", tagName: "PII Data" } });
    await user.click(screen.getByRole("button", { name: "Preview" }));
    expect(await screen.findByText("Masks 1 column on 1 table · 2 people exempt")).toBeInTheDocument();
    const warn = screen.getByRole("status");
    expect(warn).toHaveTextContent("Not enforced yet. wh has no engine attached");
    expect(screen.getByRole("button", { name: "Create" })).toHaveAccessibleDescription(/Not enforced yet/);
    const details = screen.getByText("What egret will recompile").closest("details");
    expect(details).not.toHaveAttribute("open");
    expect(details).toHaveTextContent("trino-prod");
  });
});

describe("RuleDialog — a second mask on a column is never silent (C6)", () => {
  test("names the mask already there, gives the per-group result, and asks for a tick", async () => {
    const user = userEvent.setup();
    const already = [{ ruleId: "r9", ruleName: "Hash personal data", maskType: "MASK_HASH", coordinate: "wh.analytics.customers.email", exempt: { include: [{ kind: "group", ref: "data-stewards" }], exclude: [] } }];
    previewMutateAsync.mockResolvedValueOnce({ status: 200, data: { ...OK_PREVIEW_ONE.data, effect: { ...EFFECT, enforcedBy: 1, catalogs: [], alreadyMasked: already } } });
    renderDialog({ entry: { kind: "classification", tagKey: "pii_v2", tagName: "PII Data" } });
    await user.click(screen.getByRole("button", { name: "Preview" }));
    expect(await screen.findByText(/wh\.analytics\.customers\.email is already masked by/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Hash personal data" })).toHaveAttribute("href", "/governance/rules/r9");
    expect(screen.getByRole("list", { name: "Effective result" })).toHaveTextContent("group data-stewards: undecided");
    const create = screen.getByRole("button", { name: "Create" });
    expect(create).toHaveAttribute("aria-disabled", "true");
    await user.click(screen.getByRole("checkbox", { name: /Create anyway/ }));
    expect(create).not.toHaveAttribute("aria-disabled", "true");
  });

  test("a 409 rule_overlap names both rules and offers no tick", async () => {
    const user = userEvent.setup();
    previewMutateAsync.mockRejectedValueOnce(new ApiError(409, "recompiling profile …", "rule_overlap", {
      error: "recompiling profile …", code: "rule_overlap", rules: [{ id: "a", name: "Hash A" }, { id: "b", name: "Null B" }], coordinate: "wh.s.t.email", kind: "mask",
    }));
    renderDialog({ entry: { kind: "classification", tagKey: "pii_v2", tagName: "PII Data" } });
    await user.click(screen.getByRole("button", { name: "Preview" }));
    expect(await screen.findByText(/would make two masks decide wh\.s\.t\.email differently/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Hash A" })).toBeInTheDocument();
    expect(screen.queryByRole("checkbox", { name: /Create anyway/ })).not.toBeInTheDocument();
  });
});

describe("RuleDialog — the table-level classification conflict, said short (C5)", () => {
  test("one sentence, Remove from table, then a fresh preview", async () => {
    const user = userEvent.setup();
    const unassign = vi.fn().mockResolvedValue({ status: 200, data: { ok: true } });
    vi.mocked(catalogApi.useControlPlaneCatalogUnassignResourceTag).mockReturnValue({ mutateAsync: unassign, isPending: false } as never);
    previewMutateAsync
      .mockRejectedValueOnce(new ApiError(400, "tag \"PII Data\" is attached to the whole table…", "table_tag_mask_conflict", {
        error: "tag \"PII Data\" is attached to the whole table…", code: "table_tag_mask_conflict", tag: "PII Data", tagId: "t1",
        warehouseId: "wh-1", schema: "analytics", table: "orders", mask: "Mask PII Data", maskOrigin: "rule", moreTables: false,
      }))
      .mockResolvedValueOnce({ status: 200, data: { ...OK_PREVIEW_ONE.data, effect: { ...EFFECT, enforcedBy: 2, catalogs: [] } } });
    renderDialog({ entry: { kind: "classification", tagKey: "pii_v2", tagName: "PII Data" } });
    await user.click(screen.getByRole("button", { name: "Preview" }));
    expect(await screen.findByText("PII Data is on the whole table analytics.orders, so masking by PII Data would make that table unreadable.")).toBeInTheDocument();
    expect(screen.getByText("Details").closest("details")).not.toHaveAttribute("open");
    await user.click(screen.getByRole("button", { name: "Remove from table" }));
    expect(unassign).toHaveBeenCalledWith({ warehouseId: "wh-1", data: { schema: "analytics", table: "orders", column: "", tag_id: "t1" } });
    expect(await screen.findByText("Masks 1 column on 1 table · enforced by 2 engines · 2 people exempt")).toBeInTheDocument();
    expect(previewMutateAsync).toHaveBeenCalledTimes(2);
  });
});
```

(Add `vi.mock("@/api/generated/control-plane-catalog/control-plane-catalog");` and `import * as catalogApi from "@/api/generated/control-plane-catalog/control-plane-catalog";` to the file's mocks. The render's `me` is platform admin, so Remove from table is offered. The second preview's `enforcedBy: 2` differs from the first fixture's on purpose, so the assertion can tell the fresh preview from a stale one.)

- [ ] **Step 2: Run to confirm they fail**

```bash
cd frontend && npx vitest run src/routes/governance/__tests__/ruleEffect.test.ts src/routes/governance/__tests__/RuleDialog.test.tsx 2>&1 | tail -30
```

- [ ] **Step 3: Implement**

1. **`ruleEffect.ts`**:
   - `effectSentence` per the tests:
     - plurals written out (FE-COPY-4);
     - `everyone exempt` when `exemptEveryone`;
     - `nobody exempt` at 0;
     - the engine segment only when `enforcedBy > 0`.
   - `effectivePerPrincipal`:
     - `masks = [{ name: "this rule", maskType, exempt }, …existing]`;
     - principals = the union of `exempt.include` across masks, keyed `${kind} ${ref}` (labels `group x`, `user x`, `role x`, `workspace x`), sorted;
     - for each, `from = masks.filter((m) => m.exempt.include.some(same))`:
       - all → `{verdict: "real", text: "sees the real value"}`;
       - none → masked;
       - else → `{verdict: "undecided", text: "undecided: the engine applies whichever mask it evaluates first"}`;
     - "masked" text: when every mask's type is equal → `masked (${MASK_TYPE_LABEL[t] ?? t})`, else `masked; which mask applies is decided by engine order`;
     - finally `{principal: "Everyone else", verdict: "masked", …}`.
2. **`RuleDialog.tsx`** preview block:
   - (a) `effectSentence` as the lead `<p className="font-medium">`.
   - (b) When `enforcedBy === 0`, a `<div role="status" id={warnId}>`. For `no_engine`: "**Not enforced yet.** <names of catalogs with notEnforced> has no engine attached, so this rule is stored but no query is masked. Attach an engine in Control Plane > Engines." For `no_profile`: the variant from spec C7. Add `warnId` to Create's `aria-describedby` even when Create is enabled.
   - (c) When `effect.alreadyMasked.length > 0`:
     - the sentence per spec C6, with `Link`s to `/governance/rules/${ruleId}?from=…`;
     - a `<ul aria-label="Effective result">` of `${principal}: ${text}`;
     - a checkbox `Create anyway: I understand <column> will have two masks`. Unticked adds the reason "Tick 'Create anyway' to create a second mask on this column." to Create's reason.
   - (d) A `<details><summary>What egret will recompile</summary>` holding the old profile line, the counts and `skipped`.
   - (e) Errors:
     - `tableTagMaskConflict(e)` → the short sentence, **Remove from table** (when `warehouseId` and the caller can edit catalog tags: `catalog_registry:update` or admin, via `useCanEditCatalogTags` from Task 6), **Apply by path instead** (sets Applies to = Path with an empty scope and focuses the catalog picker: the conflict body names a warehouse, not a catalog, so the dialog does not guess one), `moreTables` text, and `<details><summary>Details</summary>{e.message}</details>`. After a successful Remove, invalidate the classification queries as Task 6 does and call `runPreview()` again.
     - `ruleOverlap(e)` → the sentence plus two links, with the server sentence under Details.
     - Anything else → the server's message (FE-ACTION-3).

- [ ] **Step 4: Run, lint, types**

```bash
cd frontend && npx vitest run src/routes/governance 2>&1 | tail -15 && npm run lint && npx tsc -b
```

- [ ] **Step 5: Commit**

```bash
git add frontend/src
git commit -m "feat(classification): rule preview speaks in effect, names masks already on the column, and explains the table-level conflict in one sentence

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

## Task 9: The classification page says what it does; Clearance says its scope and who is cleared (C2, C3, C9)

**Files:**
- Create: `frontend/src/routes/governance/classifications/EffectStrip.tsx`, `frontend/src/routes/governance/classifications/__tests__/EffectStrip.test.tsx`
- Modify:
  - `frontend/src/routes/governance/classifications/TagPage.tsx` (render the strip between the header and the tabs);
  - `tabs/WhereUsedTab.tsx` (:134 empty state);
  - `frontend/src/routes/governance/TagRulesSection.tsx` (empty state and an Add rule toolbar button);
  - `tabs/ClearanceTab.tsx` (:145-148, :163-196, :357-361);
  - `ClassificationsIndex.tsx` (:194 empty state).
- Test: `classifications/__tests__/{ClearanceTab,ClassificationsIndex,WhereUsedTab,TagPage}.test.tsx`

**Interfaces:**
- Consumes: `usePermClearedPeople` (Task 4), `RuleDialog` with `entry` (Task 7), `rulesOnTag`, `useControlPlaneGovernanceTagUsage`.
- Produces:
  - `EffectStrip({ tag })`;
  - `ClearanceTab` text constants `CLEARANCE_SCOPE` and `CLEARED_NEEDS_POLICY_READ = "Seeing who is cleared needs the policy:read permission."`;
  - `NO_CLASSIFICATION_YET` is kept as the empty-state title.

- [ ] **Step 1: Write the failing tests**

`frontend/src/routes/governance/classifications/__tests__/EffectStrip.test.tsx`:

```tsx
import { afterEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ADMIN, ok, openArea, serve, STEWARD, tag, TAG_READER, where } from "./harness";

vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn(), warning: vi.fn() } }));
afterEach(() => vi.unstubAllGlobals());

const T = { timeout: 3000 };
const PII = tag({ id: "t-pii", key: "pii", name: "PII" });
const USAGE_NONE = { tag: { id: "t-pii", key: "pii", name: "PII" }, data: { tables: [], tableCount: 0, columnCount: 0, orphaned: 0 }, bi: { readable: [], unreadable: { count: 0 }, deleted: 0, checked: 0, remaining: 0 } };
const USAGE_TWO = { ...USAGE_NONE, data: { ...USAGE_NONE.data, tableCount: 2, columnCount: 3 } };
const HASH = { id: "r1", name: "Hash email", kind: { type: "mask", mask_type: "MASK_HASH" }, selector: { type: "tag", key: "pii" }, exempt: { include: [], exclude: [] }, tag_name: "PII" };
const strip = () => screen.getByRole("region", { name: "Effect" });

describe("the Effect strip", () => {
  it("says a new classification does nothing yet, and how to change that", async () => {
    serve({ "/tags": ok([PII]), "/classifications/t-pii/usage": ok(USAGE_NONE), "/perm/rules": ok([]) });
    openArea("/governance/classifications/t-pii", ADMIN);
    await screen.findByRole("heading", { level: 1, name: "PII" }, T);
    await waitFor(() => expect(strip()).toHaveTextContent("Does nothing yet."));
    expect(within(strip()).getByText("No rule uses this classification yet.")).toBeInTheDocument();
    expect(within(strip()).getByText("None: anyone who can read the data can open it.")).toBeInTheDocument();
    expect(within(strip()).getByRole("link", { name: "Attach in Data" })).toHaveAttribute("href", "/governance/data");
    await userEvent.click(within(strip()).getByRole("button", { name: "Set clearance" }));
    expect(where()).toBe("/governance/classifications/t-pii?tab=clearance");
  });

  it("states rules, clearance and usage when they exist", async () => {
    serve({ "/tags": ok([{ ...PII, required_groups: ["g1", "g2"] }]), "/classifications/t-pii/usage": ok(USAGE_TWO), "/perm/rules": ok([HASH]) });
    openArea("/governance/classifications/t-pii", ADMIN);
    await waitFor(() => expect(strip()).toHaveTextContent("Mask · Hash: Hash email"));
    expect(strip()).toHaveTextContent("Only 2 groups may open data with it");
    expect(strip()).toHaveTextContent("2 tables · 3 columns · 0 BI objects");
    expect(strip()).not.toHaveTextContent("Does nothing yet.");
  });

  it("Add rule opens the rule dialog on this classification", async () => {
    serve({ "/tags": ok([PII]), "/classifications/t-pii/usage": ok(USAGE_NONE), "/perm/rules": ok([]), "/governance/catalogs": ok([]) });
    openArea("/governance/classifications/t-pii", ADMIN);
    await userEvent.click(await within(await screen.findByRole("region", { name: "Effect" }, T)).findByRole("button", { name: "Add rule" }));
    const dialog = await screen.findByRole("dialog", { name: "New rule" });
    expect(within(dialog).getByLabelText(/^Name/)).toHaveValue("Mask PII");
  });

  it("says which permission is missing instead of hiding facts", async () => {
    serve({ "/tags": ok([PII]), "/classifications/t-pii/usage": ok(USAGE_NONE) });
    openArea("/governance/classifications/t-pii", TAG_READER);
    await waitFor(() => expect(strip()).toHaveTextContent("Reading rules needs the policy:read permission."));
    expect(within(strip()).queryByRole("button", { name: "Add rule" })).not.toBeInTheDocument();
    expect(within(strip()).queryByRole("button", { name: "Set clearance" })).not.toBeInTheDocument();
  });

  it("offers Add rule only with policy:update", async () => {
    serve({ "/tags": ok([PII]), "/classifications/t-pii/usage": ok(USAGE_NONE), "/perm/rules": ok([]) });
    openArea("/governance/classifications/t-pii", STEWARD);
    await waitFor(() => expect(strip()).toHaveTextContent("No rule uses this classification yet."));
    expect(within(strip()).queryByRole("button", { name: "Add rule" })).not.toBeInTheDocument();
    expect(within(strip()).getByText("Adding a rule needs the policy:update permission.")).toBeInTheDocument();
  });
});
```

(`serve` keys strip `/api/control-plane`, so the usage route `GET /api/control-plane/classifications/{id}/usage` is `/classifications/t-pii/usage`.)

In `classifications/__tests__/ClearanceTab.test.tsx`:

```tsx
  it("states what clearance does before anything is added, and who is cleared", async () => {
    serve({
      "/tags": ok([PII]),
      "/access/roles": ok([]),
      "/access/groups": ok([]),
      "/perm/clearances/t-pii/people": ok({ count: 0, people: [], truncated: false, enginesEnforce: false, onEnginesCount: 0 }),
    });
    openArea("/governance/classifications/t-pii?tab=clearance", ADMIN);
    const scope = await screen.findByText(/Clearance decides who may open data with this classification/, undefined, T);
    expect(scope).toBeInTheDocument();
    expect(screen.getByText(/A required group is enforced when the data is opened in egret, not in SQL Lab or notebooks/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "add a mask rule" })).toBeInTheDocument();
    expect(screen.getByText("None: anyone who can read the data can open it.")).toBeInTheDocument();
    expect(screen.queryByText(/Not gated/)).not.toBeInTheDocument();
  });

  it("lists the people a clearance lists, each once, with qualified copy", async () => {
    const via = (kind: "role" | "group", name: string, extra = {}) =>
      ({ kind, name, scope: kind === "role" ? "platform" : null, scopeId: null, throughGroup: null, ...extra });
    serve({
      "/tags": ok([{ ...PII, required_roles: ["cleared"] }]),
      "/access/roles": ok([]),
      "/access/groups": ok([]),
      "/perm/clearances/t-pii/people": ok({
        count: 3, truncated: false, enginesEnforce: true, onEnginesCount: 1,
        people: [
          { userId: "u1", label: "Ann", username: "ann", status: "active", serviceAccount: false,
            via: [via("role", "cleared")], onEngines: true },
          { userId: "u2", label: "Bo", username: null, status: "active", serviceAccount: false,
            via: [via("group", "data-team")], onEngines: false },
          { userId: "u3", label: "Cy", username: "cy", status: "active", serviceAccount: false,
            via: [via("role", "cleared", { throughGroup: "ops" })], onEngines: false },
        ],
      }),
    });
    openArea("/governance/classifications/t-pii?tab=clearance", ADMIN);
    await userEvent.click(await screen.findByText("3 people hold a required role or belong to a required group", undefined, T));
    expect(screen.getByText("Platform admins can always open it in egret.")).toBeInTheDocument();
    expect(screen.queryByText(/Engines do not enforce this clearance/)).not.toBeInTheDocument();
    const list = screen.getByRole("list", { name: "Cleared people" });
    expect(within(list).getAllByRole("listitem").map((li) => li.textContent)).toEqual([
      "Ann · ann · role cleared · engines",
      "Bo · group data-team",
      "Cy · cy · role cleared through group ops · Cleared in egret, refused on engines",
    ]);
  });

  it("says when engines do not enforce a group-only clearance", async () => {
    serve({
      "/tags": ok([{ ...PII, required_groups: ["g-1"] }]),
      "/access/roles": ok([]),
      "/access/groups": ok([]),
      "/perm/clearances/t-pii/people": ok({ count: 0, people: [], truncated: false, enginesEnforce: false, onEnginesCount: 0 }),
    });
    openArea("/governance/classifications/t-pii?tab=clearance", ADMIN);
    expect(await screen.findByText("Engines do not enforce this clearance: anyone with SQL access can read the data.", undefined, T)).toBeInTheDocument();
  });

  it("without policy:read, says what reading the cleared people needs", async () => {
    serve({ "/tags": ok([{ ...PII, required_roles: ["cleared"] }]), "/access/roles": ok([]), "/access/groups": ok([]) });
    openArea("/governance/classifications/t-pii?tab=clearance", TAG_MANAGER);
    expect(await screen.findByText("Seeing who is cleared needs the policy:read permission.", undefined, T)).toBeInTheDocument();
  });

  it("draws its warnings with tokens, never the raw-palette badge", async () => {
    serve({ "/tags": ok([{ ...PII, required_groups: ["g1"] }]), "/access/roles": ok([]), "/access/groups": ok([{ id: "g1", name: "data-team" }]), "/perm/clearances/t-pii/people": ok({ count: 0, people: [], truncated: false }) });
    const { container } = openArea("/governance/classifications/t-pii?tab=clearance", ADMIN);
    await screen.findByText(/Clearance decides/, undefined, T);
    expect(container.querySelector(".bg-yellow-600")).toBeNull();
  });
```

(Check the roles and groups list keys against `frontend/src/api/access.ts` `listRoles`/`listGroups`. The file's existing tests already serve them; reuse their exact keys.)

In `classifications/__tests__/ClassificationsIndex.test.tsx`:

```tsx
  it("explains the three steps when there is no classification yet", async () => {
    serve({ "/tags": ok([]) });
    openArea("/governance/classifications", STEWARD);
    const steps = await screen.findByRole("list", { name: "How classification works" }, T);
    expect(within(steps).getAllByRole("listitem").map((li) => li.textContent)).toEqual([
      "Create a classification.",
      "Attach it to tables or columns in Data.",
      "Add a rule (mask or row filter) or set clearance on the classification's page.",
    ]);
    expect(within(steps).getByRole("link", { name: "Data" })).toHaveAttribute("href", "/governance/data");
  });
```

- [ ] **Step 2: Run to confirm they fail**

```bash
cd frontend && npx vitest run src/routes/governance/classifications 2>&1 | tail -30
```

- [ ] **Step 3: Implement**

1. **`EffectStrip.tsx`**: `<section aria-label="Effect" className="grid grid-cols-1 gap-3 rounded-md border p-3 sm:grid-cols-3">` with three `<div>`s, each a `<dl>` (`dt` the fact name, `dd` the text and its action), following spec C2's table.
   - The rules fact uses `formatRuleKind(r)` + `: ` + `r.name` for the first rule, plus ` and N more` when there are more.
   - **Add rule** is a `Button` that opens `RuleDialog` with `entry={{ kind: "classification", tagKey: tag.key, tagName: tag.name }}`.
   - **Set clearance** sets `?tab=clearance` through `useSearchParams`.
   - **Attach in Data** is a `Link to="/governance/data"`, followed by the sentence "Open a table or a column and use Add classification."
   - The status line "Does nothing yet." appears when there are no rules (known), no clearance and zero usage.
   - Each fact has its own loading skeleton and its own error line (FE-STATE-1).
2. **`TagPage.tsx`**: `<EffectStrip tag={tag} />` after `TagHeader`.
3. **`TagRulesSection.tsx`**: an **Add rule** toolbar button (policy:update) and an empty state with the same action.
4. **`WhereUsedTab.tsx:134`**: `EmptyState` gains `action={<Link to="/governance/data">Attach in Data</Link>}` and the description "Open a table or a column and use Add classification."
5. **`ClearanceTab.tsx`**:
   - (a) `CLEARANCE_SCOPE` (spec C9.1 text) in a local `Callout`: `<div role="note" className="flex gap-2 rounded-md border border-highlight bg-highlight/15 p-3 text-sm text-foreground">` with `AlertTriangle` `aria-hidden`. It is always rendered. Inside it, a link-styled `Button` "add a mask rule" opens `RuleDialog` with the classification entry.
   - (b) The four measured warnings move into `<details><summary>Engine details</summary>…</details>`, as `Callout`s. Remove `Badge variant="warning"` from this file.
   - (c) "Not gated — …" becomes "None: anyone who can read the data can open it.".
   - (d) Cleared people:
     - With `policy:read`, `usePermClearedPeople(tag.id)` renders `<details><summary>{count} {count === 1 ? "person holds a required role or belongs to a required group" : "people hold a required role or belong to a required group"}</summary><ul aria-label="Cleared people">…</ul></details>`. Never say "can open it" for this count (spec C9 table: it is neither egret's nor the engines' cleared set).
     - Each item is `label · username? · via · markers`. A via is `kind name`, plus ` in workspace <name or id> only` / ` on one object only` for a scoped role, plus ` through group <g>` when `throughGroup`; vias are joined ", ". Markers: "service account", "disabled", "engines" when `onEngines`, and "Cleared in egret, refused on engines" when every via is through a group. "and N more" when `truncated`.
     - Fixed lines: always "Platform admins can always open it in egret."; when `!enginesEnforce`, "Engines do not enforce this clearance: anyone with SQL access can read the data."
     - Without `policy:read`, it shows `CLEARED_NEEDS_POLICY_READ`.
     - Invalidate `getPermClearedPeopleQueryKey(tag.id)` in `persist`'s success.
6. **`ClassificationsIndex.tsx:194`**: the `EmptyState` gains a description with `<ol aria-label="How classification works">` holding the three steps, the middle one linking "Data". Keep the New button for `tag:manage`; without it, show "Creating needs the tag:manage permission."

- [ ] **Step 4: Run, lint, types, bundle guard**

```bash
cd frontend && npx vitest run src/routes/governance 2>&1 | tail -15 && npm run lint && npx tsc -b && npm run test:bundle:governance-classification
```

- [ ] **Step 5: Commit**

```bash
git add frontend/src
git commit -m "feat(classification): the classification page states its effect, clearance states its scope and who is cleared

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

## Task 10: Create, edit and delete flows (C10, C11)

**Files:**
- Modify:
  - `frontend/src/routes/governance/classifications/tagParts.tsx` (`slugifyTagKey` :8-16, add `TAG_KEY_PATTERN`);
  - `ClassificationsIndex.tsx` (the create dialog :277-424);
  - `TagHeader.tsx` (edit form :83-157);
  - `DeleteTagDialog.tsx`;
  - `frontend/src/routes/governance/tagDeleteFailure.ts`;
  - `frontend/src/routes/governance/people/RulePage.tsx` (:39-48, :72-81).
- Test: `classifications/__tests__/{ClassificationsIndex,TagPage,DeleteTagDialog}.test.tsx`, `people/__tests__/RulePage.test.tsx`

**Interfaces:**
- Consumes: `tagNameTaken`, `tagKeyTaken`, `tagKeyInvalid`, `tagInUse`, `tableTagMaskConflict` (Task 5); the server codes (Tasks 1, 2).
- Produces: `TAG_KEY_PATTERN = /^[a-z0-9:-]+$/`; `slugifyTagKey` maps `_` to `-`.

- [ ] **Step 1: Write the failing tests**

In `ClassificationsIndex.test.tsx`:

```tsx
describe("New classification: rules said inline (C10)", () => {
  it("slugs without underscores and flags a key the server would refuse as you type", async () => {
    const user = userEvent.setup();
    serve({ "/tags": ok([]) });
    openArea("/governance/classifications", STEWARD);
    await user.click(await screen.findByRole("button", { name: "New classification" }, T));
    await user.type(screen.getByLabelText(/^Name/), "Card_no data");
    expect(screen.getByLabelText(/^Key/)).toHaveValue("card-no-data");
    await user.clear(screen.getByLabelText(/^Key/));
    await user.type(screen.getByLabelText(/^Key/), "a_b");
    expect(screen.getByLabelText(/^Key/)).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByLabelText(/^Key/)).toHaveAccessibleDescription(/Use lowercase letters, digits, - and : only/);
    expect(screen.getByRole("button", { name: "Create classification" })).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByText(/Lowercase letters, digits, - and :\./)).toBeInTheDocument();
  });

  it("warns about a duplicate name before submit, case-insensitively, with a link to it", async () => {
    const user = userEvent.setup();
    serve({ "/tags": ok([tag({ id: "t-pub", key: "public", name: "Public" })]) });
    openArea("/governance/classifications", STEWARD);
    await user.click(await screen.findByRole("button", { name: "New classification" }, T));
    await user.type(screen.getByLabelText(/^Name/), "public");
    expect(screen.getByText("A classification named Public already exists.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Public" })).toHaveAttribute("href", "/governance/classifications/t-pub");
  });

  it("puts each server code beside its own field", async () => {
    const user = userEvent.setup();
    serve({
      "/tags": ok([]),
      "POST /tags": { status: 409, body: { error: "A classification with the key \"pii\" already exists (PII).", code: "tag_key_taken", key: "pii", existing: { id: "t1", name: "PII" } } },
    });
    openArea("/governance/classifications", STEWARD);
    await user.click(await screen.findByRole("button", { name: "New classification" }, T));
    await user.type(screen.getByLabelText(/^Name/), "pii");
    await user.click(screen.getByRole("button", { name: "Create classification" }));
    expect(await screen.findByText("Key pii is taken by PII.")).toBeInTheDocument();
    expect(screen.getByLabelText(/^Key/)).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByLabelText(/^Name/)).toHaveValue("pii");
  });

  it("labels the colour and states its value as text", async () => {
    serve({ "/tags": ok([]) });
    openArea("/governance/classifications", STEWARD);
    await userEvent.click(await screen.findByRole("button", { name: "New classification" }, T));
    const colour = screen.getByLabelText("Colour");
    expect(colour).toHaveAccessibleDescription(/Colour of the swatch shown next to the name\. #6366f1/);
  });
});
```

In `DeleteTagDialog.test.tsx`:

```tsx
  it("cannot delete while rules use it: Delete says why and links each rule", async () => {
    serve({ "/tags": ok([PII]), "/classifications/t-pii/usage": ok(USAGE), "/perm/rules": ok([HASH_RULE]) });
    openArea("/governance/classifications/t-pii", ADMIN);
    await userEvent.click(await screen.findByRole("button", { name: "Delete" }, T));
    const dialog = await screen.findByRole("alertdialog");
    const del = within(dialog).getByRole("button", { name: "Delete" });
    expect(del).toHaveAttribute("aria-disabled", "true");
    expect(del).toHaveAccessibleDescription(/Delete or change these rules first/);
    expect(within(dialog).getByRole("link", { name: "Hash email" })).toHaveAttribute("href", "/governance/rules/r1?from=classification:t-pii");
    expect(within(dialog).queryByRole("textbox")).not.toBeInTheDocument();
  });

  it("asks for the classification's NAME, not its key", async () => {
    serve({ "/tags": ok([PII]), "/classifications/t-pii/usage": ok(USAGE), "/perm/rules": ok([]) });
    openArea("/governance/classifications/t-pii", ADMIN);
    await userEvent.click(await screen.findByRole("button", { name: "Delete" }, T));
    expect(await screen.findByText(/Type PII to confirm/)).toBeInTheDocument();
  });

  it("a tag_in_use refusal without policy:read toasts the count and no table names", async () => {
    serve({
      "/tags": ok([PII]),
      "/classifications/t-pii/usage": ok(USAGE),
      "DELETE /tags/t-pii": { status: 409, body: { error: "Rules still use this classification: Hash email. …", code: "tag_in_use", rules: [{ id: "r1", name: "Hash email" }] } },
    });
    openArea("/governance/classifications", TAG_MANAGER);
    // open the row's delete from the index, type the name, confirm
    await userEvent.click(await screen.findByRole("button", { name: "Actions for PII" }, T));
    await userEvent.click(await screen.findByRole("menuitem", { name: "Delete" }));
    await userEvent.type(await screen.findByRole("textbox"), "PII");
    await userEvent.click(screen.getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(toast.error).toHaveBeenCalledWith("PII is still used by 1 rule.", expect.anything()));
    expect(JSON.stringify(vi.mocked(toast.error).mock.calls)).not.toContain("perm.rules");
  });
```

(Define `USAGE` and `HASH_RULE` at the top of the file if absent, as in Task 9's test.)

In `people/__tests__/RulePage.test.tsx`:

```tsx
  test("deleting asks for the rule's name and returns to where the page was opened from", async () => {
    const user = userEvent.setup();
    const del = vi.fn((_v, opts?: { onSuccess?: () => void }) => opts?.onSuccess?.());
    vi.mocked(permApi.usePermDeleteRule).mockReturnValue({ mutate: del, mutateAsync: vi.fn().mockResolvedValue({}), isPending: false } as never);
    renderAt("/governance/rules/r1?from=classification:t-pii", WRITER);
    await user.click(await screen.findByRole("button", { name: "Delete" }));
    await user.type(await screen.findByRole("textbox"), "mask-ssn");
    await user.click(screen.getByRole("button", { name: "Delete" }));
    expect(await screen.findByTestId("where")).toHaveTextContent("/governance/classifications/t-pii?tab=rules");
  });
```

(Include a `Where` output inside the `MemoryRouter` the helper renders, and the target route, as `classifications/__tests__/harness.tsx:29-32` does.)

- [ ] **Step 2: Run to confirm they fail**

```bash
cd frontend && npx vitest run src/routes/governance/classifications src/routes/governance/people/__tests__/RulePage.test.tsx 2>&1 | tail -30
```

- [ ] **Step 3: Implement**

1. **`tagParts.tsx`**: `export const TAG_KEY_PATTERN = /^[a-z0-9:-]+$/;`. In `slugifyTagKey` the class becomes `[^a-z0-9:-]+`.
2. **Create dialog**:
   - `Name (required)`, `Key (required)`.
   - Key validation on change: `keyError = key && !TAG_KEY_PATTERN.test(key) ? "Use lowercase letters, digits, - and : only." : null`.
   - Name duplicate: `useTags().data?.find((t) => t.name.trim().toLowerCase() === name.trim().toLowerCase())` shows "A classification named <Link>{t.name}</Link> already exists.".
   - Create is `aria-disabled` while either error, an empty name or an empty key holds, with the reason tied.
   - Server errors by code:
     - `tagKeyTaken` → `Key ${key} is taken by ${existing.name}.` under Key;
     - `tagNameTaken` → the name sentence under Name;
     - `tagKeyInvalid` → under Key;
     - else → the top alert.
   - Hint: "Fixed once created. Follows the name until you edit it. Lowercase letters, digits, - and :."
   - Colour: `aria-describedby` on a `<p id>` reading "Colour of the swatch shown next to the name. {color}".
3. **`TagHeader` edit form**: the same name check (except its own id), plus `tagNameTaken` and `tableTagMaskConflict` (the rename case: "<new name> is masked by <mask> while it sits on the whole table <schema>.<table>; attach it to columns first.").
4. **`DeleteTagDialog`**:
   - `blocking = rules where selector.key === tag.key`. When `blocking.length > 0` (and `policy:read`), render the reason "Delete or change these rules first:" plus links `/governance/rules/${id}?from=classification:${tag.id}`, pass `confirmDisabled` to `ConfirmDeleteDialog`, and hide the input.
   - `ConfirmDeleteDialog` takes `confirmDisabled` already (`frontend/src/components/ui/confirm-delete-dialog.tsx:50-51`). If it renders the button `disabled`, add an additive `confirmDisabledReason?: string` prop that renders the button `aria-disabled` with the reason tied (FE-TEST-3: additive, default unchanged), plus a `hideInput` prop if needed, and test it in `components/ui/__tests__/`.
   - `confirmText={tag.name}`, case-insensitive (drop `caseSensitive`).
5. **`tagDeleteFailure.ts`**: `tagInUse(e)` → `toast.error(`${tag.name} is still used by ${n} ${n === 1 ? "rule" : "rules"}.`, { action: showRules ? { label: "Show rules", onClick: showRules } : undefined })`. Pass the tag name in. Other errors keep the server's message.
6. **`RulePage`**:
   - `ConfirmDeleteDialog confirmText={found.name}`.
   - On success: `navigate(backTo)`, using the `back.to` computed in Task 5, defaulting to `/governance/classifications`, then `toast.success(`Rule ${found.name} deleted.`)`.
   - Remove the comment at :41 that says the page stays.

- [ ] **Step 4: Run, lint, types**

```bash
cd frontend && npx vitest run src/routes/governance src/components/ui 2>&1 | tail -15 && npm run lint && npx tsc -b
```

- [ ] **Step 5: Commit**

```bash
git add frontend/src
git commit -m "feat(classification): inline key and duplicate-name checks, one delete convention, no dead end after deleting a rule

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

## Task 11: Explain prose, 403 with a way back, 390 px tables (C12, C13)

**Files:**
- Modify:
  - `frontend/src/components/perm/ExplainResult.tsx` (:1209-1374);
  - `frontend/src/components/perm/MembersExplainView.tsx` (:208);
  - `frontend/src/routes/Forbidden.tsx`;
  - `frontend/src/auth/guards.tsx` (`RequireGovernanceAccess` :91-94, `RequirePermission`);
  - `frontend/src/routes/governance/classifications/ClassificationsIndex.tsx` (column `className`s);
  - `frontend/src/routes/governance/data/tabs/OverviewTab.tsx` (:300-352, `STACKED_TABLE` and `data-label`).
- Test: `frontend/src/components/perm/__tests__/ExplainResult.test.tsx`, `frontend/src/__tests__/governanceRouting.test.tsx`, `classifications/__tests__/ClassificationsIndex.test.tsx`, `data/__tests__/ObjectPage.test.tsx`

**Interfaces:**
- Produces: `Forbidden({ reason?: string; back?: { to: string; label: string } })`. The default back link is `{ to: "/", label: "Go to the home page" }`.

- [ ] **Step 1: Write the failing tests**

In `ExplainResult.test.tsx` (it renders `ExplainResultView` with fixtures; reuse one with `outcome: "per_profile"`):

```tsx
  it("leads with the verdict, keeps one visible limit line, and folds the rest", () => {
    render(<ExplainResultView result={PER_PROFILE} />);
    expect(screen.getByText(/Masks on Trino and row filters are not shown here/)).toBeVisible();
    const details = screen.getByText("How this answer was worked out").closest("details");
    expect(details).not.toHaveAttribute("open");
    expect(within(details as HTMLElement).getByText(/egret serves no roles download and runs no usersync/)).toBeInTheDocument();
  });

  it("says nothing is enforced in one short paragraph", () => {
    render(<ExplainResultView result={{ outcome: "no_profile_serves_the_catalog", profiles: [] }} />);
    expect(screen.getByText("No engine with a policy profile serves this catalog, so nothing is enforced here (this is not a refusal). Attach an engine to govern it.")).toBeInTheDocument();
  });
```

(Use the file's existing per-profile fixture name in place of `PER_PROFILE`.)

In `frontend/src/__tests__/governanceRouting.test.tsx`, following its render of `/governance` for a user without governance permissions:

```tsx
  it("a refused Governance page says why and offers a way back", async () => {
    renderApp("/governance/classifications", { user_id: "m", email: "m@x", platform_roles: [], permissions: [] });
    expect(await screen.findByRole("heading", { name: "You don't have access to this page." })).toBeInTheDocument();
    expect(screen.getByText("Governance needs a governance permission such as tag:read or policy:read.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Go to the home page" })).toHaveAttribute("href", "/");
  });
```

(Use the file's own render helper and `Me` builder names.)

In `ClassificationsIndex.test.tsx`:

```tsx
  it("drops Key, Description and BI objects below md, keeping Name, Data and Clearance", async () => {
    serve({ "/tags": ok([PII]) });
    openArea("/governance/classifications", STEWARD);
    const table = await screen.findByRole("table", undefined, T);
    for (const name of ["Key", "Description", "BI objects"]) {
      expect(within(table).getByRole("columnheader", { name: new RegExp(name) })).toHaveClass("hidden", "md:table-cell");
    }
    for (const name of ["Name", "Data", "Clearance"]) {
      expect(within(table).getByRole("columnheader", { name: new RegExp(name) })).not.toHaveClass("hidden");
    }
  });
```

In `data/__tests__/ObjectPage.test.tsx`:

```tsx
  it("stacks the columns table into labelled cards on a narrow screen", async () => {
    serveData(fixture());
    renderPage("/governance/data/ice/sales/orders");
    const columns = await within(panel()).findByRole("table", { name: "Columns" });
    expect(columns.className).toContain("max-sm:block");
    const email = within(columns).getByRole("link", { name: "email" }).closest("tr") as HTMLElement;
    expect([...email.querySelectorAll("td[data-label]")].map((td) => td.getAttribute("data-label"))).toEqual(["Type", "Classifications", "Rules"]);
  });
```

- [ ] **Step 2: Run to confirm they fail**

```bash
cd frontend && npx vitest run src/components/perm src/__tests__/governanceRouting.test.tsx src/routes/governance 2>&1 | tail -30
```

- [ ] **Step 3: Implement**

1. **`ExplainResult.tsx`**:
   - Replace the `honest-limits` `Alert`'s body with:
     - the visible `<p>` "Masks on Trino and row filters are not shown here, so *Allowed* can still return masked values or filtered rows.";
     - `<details><summary className="cursor-pointer text-xs font-medium">How this answer was worked out</summary>…</details>` holding the existing paragraphs unchanged.
   - Replace the no-profile paragraph with the short sentence, and move the long one into the same `details`.
   - **Keep every ⚠ comment** with the paragraph it explains.
2. **`MembersExplainView.tsx:208`**: "Nothing is enforced: no engine with a policy profile serves this catalog."
3. **`Forbidden.tsx`**: render `<h1>You don't have access to this page.</h1>`, `{reason && <p>{reason}</p>}` and `<Link to={back.to}>{back.label}</Link>` in the existing `Card`. All props are optional, so existing callers are unchanged (FE-TEST-3).
4. **`guards.tsx`**:
   - `RequireGovernanceAccess` passes `reason="Governance needs a governance permission such as tag:read or policy:read."`.
   - `RequirePermission` passes `reason={`This page needs the ${list} permission.`}`, where `list` is `perm` or `perm.join(" or ")`.
5. **`ClassificationsIndex.tsx`**: `className: "hidden md:table-cell"` on the Key, Description and BI objects columns. Keep `tabular-nums` on BI with `"hidden md:table-cell tabular-nums"`.
6. **`OverviewTab.tsx`**: `<Table aria-label="Columns" className={STACKED_TABLE}>`. Add `data-label="Type"`, `"Classifications"` and `"Rules"` on the three non-first cells. Import `STACKED_TABLE` from `@/routes/governance/tabParts`.

- [ ] **Step 4: Run, lint, types**

```bash
cd frontend && npx vitest run 2>&1 | tail -15 && npm run lint && npx tsc -b
```

- [ ] **Step 5: Commit**

```bash
git add frontend/src
git commit -m "feat(governance): explain leads with its verdict, a refused page offers a way back, classification tables fit 390 px

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

## Task 12: Full local gate and the live walkthrough on our own stack

**Files:**
- Create: `frontend/tests/e2e/classification-ux.spec.ts` (paint checks only: footer in view, contrast, 390 px)
- No other code. The walkthrough record goes to the scratchpad, not git.

- [ ] **Step 1: Backend gate** (one command at a time; each long one in the background)

```bash
./deploy/prune-target.sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test cargo test --workspace 2>&1 | tee "$SCRATCH"/classux-ws.log | tail -40
grep -c "skip: DATABASE_URL" "$SCRATCH"/classux-ws.log
grep -E "^test result: FAILED|panicked" "$SCRATCH"/classux-ws.log | head
cargo test -p egret-server committed_openapi_json_is_current
```

Expected:
- fmt and clippy are clean;
- every `test result: ok`;
- the skip grep prints `0`;
- the FAILED grep prints nothing.

Run the workspace suite **alone**: no other cargo or test process running (`pgrep -x cargo` empty first).

- [ ] **Step 2: Frontend gate** (from `frontend/`)

```bash
npm run gen:api && git status --short src/api/generated   # expect: no change
npm run lint
npx tsc -b
npm run test 2>&1 | tail -20
npm run build
npm run test:bundle && npm run test:bundle:governance-data && npm run test:bundle:governance-people \
  && npm run test:bundle:governance-overview && npm run test:bundle:governance-engines \
  && npm run test:bundle:governance-classification
```

Expected: every command exits 0 and `gen:api` leaves no diff.

- [ ] **Step 3: Bring up our own stack (never the one on 18080)**

Write `"$SCRATCH"/classux-demo.override.yml` (scratchpad, not git):

```yaml
networks:
  default:
    name: egret-classux_default
services:
  egret:
    environment:
      EGRET_ENGINES__DOCKER__NETWORK: egret-classux_default
      EGRET_ENGINES__DOCKER__PROJECT: egret-classux
  mailpit:
    ports: !reset []
```

Then:

```bash
export DEMO_PORT=18280 DEMO_URL=http://egret.localtest.me:18280 DEMO_APPS_URL=http://apps.localtest.me:18280
OVR="$SCRATCH"/classux-demo.override.yml
docker compose -p egret-classux --env-file deploy/demo/demo.env -f deploy/docker-compose.demo.yml -f "$OVR" up -d --build
./deploy/demo/wait.sh && ./deploy/demo/seed.sh
docker ps --format '{{.Names}}' | grep -c '^egret-demo-'   # the other session's stack: count must be unchanged
```

Expected: `seed done: http://egret.localtest.me:18280`. The other stack's containers are untouched.

- [ ] **Step 4: Paint checks** (`frontend/tests/e2e/classification-ux.spec.ts`, `BASE=http://egret.localtest.me:18280`, login with `loginLocalAdmin` from `tests/e2e/helpers.ts`)

```ts
import { expect, test } from "@playwright/test";
import { loginLocalAdmin } from "./helpers";

for (const viewport of [{ width: 1440, height: 900 }, { width: 390, height: 844 }]) {
  test(`classification pages fit and stay readable at ${viewport.width}px`, async ({ page }) => {
    await page.setViewportSize(viewport);
    await loginLocalAdmin(page);
    await page.goto("/governance/classifications");
    expect(await page.evaluate(() => document.scrollingElement!.scrollWidth <= window.innerWidth)).toBe(true);
    await page.getByRole("link").filter({ hasText: /./ }).locator("xpath=ancestor::td").first().getByRole("link").click();
    await page.getByRole("tab", { name: "Clearance" }).click();
    const note = page.getByRole("note").first();
    const [fg, bg] = await note.evaluate((el) => {
      const s = getComputedStyle(el);
      return [s.color, getComputedStyle(el).backgroundColor];
    });
    expect(fg).not.toBe(bg);
    // Contrast is measured with the same helper as focusIndicatorContrast.test.ts: ≥ 4.5:1 for body text.
    const ratio = await note.evaluate((el) => {
      const toRgb = (c: string) => c.match(/[\d.]+/g)!.slice(0, 3).map(Number);
      const lum = ([r, g, b]: number[]) => {
        const f = (v: number) => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; };
        return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b);
      };
      let bgEl: Element | null = el;
      let bgc = "rgba(0, 0, 0, 0)";
      while (bgEl && /rgba\(0, 0, 0, 0\)|transparent/.test(bgc)) { bgc = getComputedStyle(bgEl).backgroundColor; bgEl = bgEl.parentElement; }
      const a = lum(toRgb(getComputedStyle(el).color)), b = lum(toRgb(bgc));
      return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
    });
    expect(ratio).toBeGreaterThanOrEqual(4.5);
  });
}
```

Run, in light and in dark (`page.emulateMedia({ colorScheme: "dark" })` in a second loop):

```bash
cd frontend && BASE_URL=http://egret.localtest.me:18280 npx playwright test tests/e2e/classification-ux.spec.ts
```

(If `playwright.config.ts` reads another variable for the base URL, use that one: `grep -n baseURL frontend/playwright.config.ts`.) Also check the rule dialog footer in the same spec: open Add rule from a classification, press Preview with an empty name, and assert that `getBoundingClientRect().bottom <= innerHeight` for the footer group at both widths.

- [ ] **Step 5: The steward walkthrough, recorded**

With the Playwright MCP browser on `http://egret.localtest.me:18280`, as the seeded admin, do the job from the walkthrough and **count every click** (a click, a selection, or a submit; typing a field counts 0, its focus click counts 1):

> Mask `email` of `demo_warehouse.analytics.customers` for everyone except group `data-team`, with a new classification `classux-pii`.

The expected path (≤ 15):

1. Governance
2. Classifications
3. New classification
4. Create classification (name typed)
5. **Attach in Data** (Effect strip)
6. demo_warehouse
7. analytics
8. customers
9. `email` (opens the column panel)
10. Add classification
11. pick `classux-pii`
12. Add classification
13. the chip's link back to the classification
14. **Add rule** (Effect strip; prefilled Mask, Redact, Applies to = classux-pii)
15. exempt group `data-team` (pick) → Preview → Create

If the exempt pick and Preview/Create push the count past 15, record the real number and which steps cost the most. **Do not** change the counting to fit.

Also record:
- (a) the preview's effect sentence and whether "Not enforced yet" appears (it should: the demo catalogs have no engine), with a screenshot;
- (b) that no step needed a guess (each next control was named on the previous screen);
- (c) the detour of pass 2: attach `classux-pii` to the **table** `orders` and check that the Add classification dialog warns before submit (once the rule exists), with a screenshot;
- (d) remove the table attachment from the chip and from Where used, with no Control Plane visit;
- (e) 390 × 844 screenshots of the list, the tag page and the Data columns table.

Write the record to `"$SCRATCH"/classux-live/walkthrough.md` with screenshots beside it. **Not in git.**

- [ ] **Step 6: Clean up our stack only**

```bash
docker compose -p egret-classux --env-file deploy/demo/demo.env -f deploy/docker-compose.demo.yml -f "$OVR" down -v
docker ps -a --filter label=egret.project=egret-classux -q | xargs -r docker rm -f
```

- [ ] **Step 7: Commit the paint spec**

```bash
git add frontend/tests/e2e/classification-ux.spec.ts
git commit -m "test(classification): paint checks for the Classification pages at 1440 and 390 px, light and dark

Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"
```

---

## Self-review

### Spec coverage

| Spec decision | Task(s) |
|---|---|
| C1 glossary and vocabulary (N18, N13, F6, F7) | 5 (strings, `formatRuleKind`, breadcrumb, icon) |
| C2 Effect strip, empty states (F1, F2, N11) | 9 |
| C3 first use (m) | 9 |
| C4 chips, remove in place, picker (N2, N16, N17) | 4 (B5), 6 |
| C5 table-level conflict (N3) | 2 (B2), 6 (attach), 8 (rule dialog), 10 (rename) |
| C6 overlapping masks (N4) | 3 (B3 `alreadyMasked`), 8 |
| C7 preview in effect (N5) | 3 (B3), 8 |
| C8 rule dialog behaviour (N6, N11, N12, N22, N26) | 7 |
| C9 clearance scope, cleared people, contrast (F3, N7, N21) | 4 (B4), 9, 12 (measured) |
| C10 create/edit classification (N8, N9, N10) | 1 (B1), 10 |
| C11 delete flows (N13, N14, N15) | 1 (`tag_in_use`), 10 |
| C12 explain prose (N19) | 11 |
| C13 responsive, a11y, 403 (N20, N22, N23) | 7 (aria-disabled), 11, 12 |
| §6 "not in Package 1": resource-tags warehouse check | none, by decision (spec §6, §11.1) |
| Final gate + live walkthrough (≤ 15 clicks) | 12 |

### Type consistency

| Type | Defined in | Used in |
|---|---|---|
| `RuleEffect` / `ExistingMask` (Rust, camelCase) | Task 3 | generated TS (Task 3 Step 5) → `ruleEffect.ts` and `RuleDialog` (Task 8) |
| `table_tag_mask_conflict` fields | Task 2 `refusal_body()` | `TableTagMaskConflict` in `apiErrors.ts` (Task 5) → Tasks 6, 8, 10 |
| `ClearedPeople` / `usePermClearedPeople` | Task 4 | Task 9 |
| `TableUsage.warehouseId` | Task 4 | Task 6 |
| `RuleEntry` | Task 7 | Tasks 8 (tests), 9 (Effect strip, Clearance) and 10 (RulePage `from`, Task 5) |
| `useCanEditCatalogTags` | Task 6 | Task 8 |

### Ordering and green main

Each task leaves `main` green:

- Tasks 1–4 are backend-only plus a regenerated client, and the frontend still compiles because every generated field is additive.
- Task 5 renames strings and updates their tests in the same commit.
- Tasks 6–11 each consume only what earlier tasks produced.

### Placeholder scan

- No "TBD" or "similar to Task N".
- Where a test depends on a helper name in an existing file (`renderAt`, the per-profile fixture in `ExplainResult.test.tsx`, the roles/groups serve keys), the step says how to find or add it.
