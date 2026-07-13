# Access Control P3 — ABAC / Tag-based Policy: Design Spike

> Design spike (no code). Decides the shape of egret's attribute/tag-based access control and a phased delivery. Companion to `docs/en/plans/2026-07-13-access-control-roadmap.md`. Ties into the Data-Governance Policies (Ranger) epic.

## 1. Problem & motivation

Everything shipped so far (B1–P2b) is **identity-anchored**: access is a grant to a *principal* on a *specific resource* (role, workspace, object) — RBAC + ReBAC + time-boxing + request/approval. This does not scale to **data-classification governance**, where the rule is about the *data*, not the object:

> "Mask every **PII** column for anyone without the `pii_reader` clearance." · "Only **finance**-cleared users can open dashboards tagged **confidential**." · "Rows tagged **region=EU** are visible only to EU staff."

Expressing that with per-object grants is O(objects × principals) and drifts the moment a new PII column appears. **ABAC** inverts it: **tag once, govern everywhere** — a policy references *attributes/tags* on the resource (and optionally the principal), and every current/future resource carrying that tag is governed automatically.

egret has **no tag layer** today, and neither enforcement plane conditions on attributes. P3 introduces both.

## 2. Goals / non-goals

**Goals**
- A first-class **classification (tag) model**: define tags, assign them to BI objects and catalog tables/columns.
- **Data-plane** tag policies: tag-based **column masking / row filtering / access** enforced at query time (Trino/StarRocks via Ranger).
- **Control-plane / BI** tag policies: tag-conditioned allow/deny on BI objects via the Cedar PDP.
- Reuse, not replace: build on `access_grants` + Cedar + the existing Ranger integration + the Data-Governance Policies UI.

**Non-goals (this spike / early phases)**
- Automatic tag propagation via **lineage** (Atlas-style) — later phase.
- A full external metadata catalog (Apache Atlas) — egret keeps its own tag store (see Decision D1).
- Principal-attribute ABAC beyond a small clearance/group set (e.g. HR-attribute-driven) — later.
- ABAC on the notebook private-rw plane (governed reads only).

## 3. How the enterprise does it (survey)

| System | Tag model | Data-plane (mask/row) | Object/metadata ABAC | Notes |
|---|---|---|---|---|
| **Databricks Unity Catalog** | Governed **tags** on catalog/schema/table/column; **ABAC policies** (preview) reference tags | Column masks + row filters **by tag** (policy functions) | Tag-based grants emerging | Tags are first-class in the metastore; policies are functions bound by tag |
| **Snowflake** | **Object tags** (hierarchical, inherited) | **Tag-based masking policies** + row-access policies (attach a masking policy to a *tag*, applies to every tagged column) | — | The canonical "tag → masking policy" model; strong inheritance |
| **Immuta** | Tags/attributes are the *whole* product; sourced from catalogs/classifiers | Dynamic masking + row-level by attribute; purpose-based | Attribute-based subscription policies | Pure ABAC/policy engine layered over the warehouse |
| **Apache Ranger + Atlas** | **Atlas classifications** synced to Ranger as **tags** | **Tag-based policies** (mask/row/access) evaluated before resource policies | — | Exactly egret's data-plane substrate — the Trino Ranger plugin already polls a tags endpoint |
| **AWS (Verified Permissions / Cedar)** | Entity **attributes** | — | Cedar `when {{ resource.tags.contains(..) }}` conditions | egret's PDP is Cedar — attribute conditions are native once the schema + entity attributes exist |

**Takeaways for egret:** (1) the dominant, proven pattern is **"attach a policy to a tag"** (Snowflake/Ranger/UC) — do that on the data plane. (2) On the control plane, Cedar gives us attribute conditions **for free** once we add resource attributes + load the schema. (3) Everyone separates the **tag store** (metadata) from the **policy engine** — so should we.

## 4. egret current state (two planes, no tag layer)

- **Control-plane / BI plane — Cedar PDP** (`crates/egret-auth/src/pdp/`): per-role templates linked to `access_grants` at decision time (`grants_for`). Schema (`schema.cedarschema`) is **entities-only, not loaded** (`is_authorized` passes `None`). Entities (`build_entities`) carry **no attributes**. → ABAC here = add attributes + load schema + a new tag-policy layer.
- **Data plane — Ranger/Trino** (`crates/egret-policy/`): the Data-Governance Policies epic delivered resource-based **masking / row-filter / access** policies (Ranger-parity, Trino-first, through P4). The Trino Ranger plugin polls two endpoints: `download_policies` (real) and `download_tags` → **`ServiceTags::empty` — tag policies explicitly deferred**. → ABAC here = serve real tags from `download_tags` + author **tag-based** Ranger policies.
- **Catalog metadata**: table/column metadata stored (partly JSONB, being normalized). No classification field yet. → the tag-assignment target for columns.
- **`access_grants` + effect(allow/deny) + expires_at**: the grant substrate. Tag policies are a *different* policy type — they don't fit the per-role-grant shape; they need their own store + evaluation.

## 5. Proposed architecture

### 5.1 The tag (classification) layer — the foundation (plane-agnostic)

A single classification model both planes read from:

- **`tags`** (catalog): `{ id, key, name, description, color, created_by }` — e.g. `pii`, `phi`, `confidential`, `finance`, `region:eu`. Optionally key/value (`region=eu`) for row-filter use.
- **`tag_assignments`** (polymorphic): `{ tag_id, target_type, target_id, ... }` where `target_type ∈ { bi_object(kind+id), catalog_table, catalog_column, workspace }`. Column targets identify `(warehouse, schema, table, column)`.
- **Inheritance** (Snowflake-style): a table tag applies to its columns; a schema tag to its tables — resolved at read time or materialized. Start with **table→column** inheritance only.
- Owned by a new `egret-classification` module (or folded into `egret-policy` / catalog). Assignment gated by a new `tag:assign` permission (+ object-manage on the target).

This is **metadata only** — no enforcement — and is the P3.1 deliverable. It is independently useful (a data catalog / "what's sensitive where" view) even before policies consume it.

### 5.2 Data-plane ABAC (Ranger tag policies) — highest ROI

- **Serve tags**: implement `download_tags` to return egret `tag_assignments` for catalog tables/columns as Ranger **`ServiceTags`** (map egret tag → Ranger tag-def + tagged-resource entries; the wire shape already exists as `ServiceTags`). The Trino plugin consumes them.
- **Tag policies**: extend the Data-Governance Policies UI/model with a **tag-policy** type — mask/row-filter/access keyed on a **tag** instead of a concrete resource (Ranger evaluates tag policies before resource policies). "Mask columns tagged `pii` unless role `pii_reader`" becomes ONE policy covering every current + future PII column.
- Reuses the entire existing Ranger download/plugin path + the Policies editor (add a "by tag" resource mode). **This is the killer feature** and the recommended first enforcement phase.

### 5.3 Control-plane / BI ABAC (Cedar attribute conditions)

- **Load the Cedar schema** and give `Resource`/`Object` entities a **`tags: Set<String>`** attribute (populated in `build_entities` from `tag_assignments` for the object under decision — same shot as `grants_for`).
- **Tag policies as Cedar policies** (a new policy class alongside role templates): `forbid(principal, action, resource) when {{ resource.tags.contains("confidential") }} unless {{ principal in Egret::Role::"finance_cleared" }};` and permit-variants. Deny-wins already holds (forbid > permit), so a tag-forbid composes with the existing model.
- Effect: tag a dashboard `confidential` → only cleared principals can open it, with **zero per-object grants**. Complements Who-can-access (which would now show "blocked by tag policy: confidential").
- **Cost/risk**: this is the deepest change (schema loading was deliberately deferred in B1a; entity attributes + a second policy source + validation). Sequence it **after** the data-plane win.

### 5.4 Where policies live & how they compose

- Tag policies are a **new policy type**, not `access_grants` rows. Store: `classification_policies` (or reuse the Ranger `authz_services`/policy JSONB for the data plane, and a small table for Cedar tag policies).
- **Composition order** (must be explicit): existing RBAC/ReBAC grants decide *object visibility*; tag policies **further restrict** (deny-wins) — a tag policy can hide/mask but not grant beyond a role. Document this as the invariant so ABAC never *widens* access.

## 6. Phased plan (recommended sequencing)

| Phase | Scope | Plane | Size | Value |
|---|---|---|---|---|
| **P3.1 — Tag model + UI** | `tags` + `tag_assignments` + table→column inheritance; assign tags to BI objects & catalog tables/columns; a "Classifications" admin page + a tag chip on object/column views | metadata | Epic (BE table+API+FE) | Foundation; also a standalone "sensitive-data map" |
| **P3.2 — Data-plane tag policies** | Real `download_tags`; tag-based mask/row/access in the Policies UI ("mask `pii` unless `pii_reader`") | data (Ranger/Trino) | Epic | **Highest** — column masking by classification, the #1 governance ask; reuses the Ranger stack |
| **P3.3 — Control-plane BI ABAC** | Load Cedar schema; `resource.tags` attribute; tag-conditioned forbid/permit on BI objects; surface in Who-can-access | control (Cedar) | Epic (PDP change) | High; governs BI objects by tag without grants |
| **P3.4 — Propagation & attributes (later)** | Lineage-based tag propagation; principal-attribute conditions (department/clearance from OIDC claims); tag-based access requests (P2b reuse); StarRocks tag parity | both | Large | Maturity |

Each phase is its own dated plan + PR series (as B1–P2b were). **Recommended order: P3.1 → P3.2 → P3.3.** Rationale: P3.1 unblocks both; P3.2 delivers the marquee capability on an existing, proven substrate (lower risk than touching the PDP); P3.3 takes on the Cedar schema change last.

## 7. Key decisions (need your call)

- **D1 — Own tag store vs Apache Atlas.** Recommend **own store** (`tags`/`tag_assignments`) — Atlas is heavy, and egret already owns its catalog + Ranger integration; serve Atlas-shaped tags to Ranger from our store. (Atlas integration = optional future import.)
- **D2 — First enforcement plane.** Recommend **data plane (P3.2) first** (column masking by tag) over BI/Cedar — bigger ask, lower risk, reuses Ranger. Alternative: BI-object ABAC first if the near-term need is dashboard-level confidentiality over data masking.
- **D3 — Tag semantics.** Start **key-only** tags (`pii`, `confidential`) for masking/access; add **key=value** (`region=eu`) when row-filtering needs it (P3.2/P3.4). Hierarchical/inherited tags: **table→column only** at first.
- **D4 — Cedar schema loading.** P3.3 flips the PDP to **load the schema** + attach `resource.tags`. Confirm appetite for that PDP change, or keep BI ABAC as a lightweight pre-filter outside Cedar (less clean, avoids the schema flip).
- **D5 — Composition invariant.** Tag policies **only restrict** (deny/mask), never widen. Lock this in so ABAC can't become a privilege-escalation path.

## 8. Open questions / risks

- **Performance:** per-decision tag lookups. Mitigation: the tag set for the object under decision is one extra query alongside `grants_for` (control plane); the data plane is Ranger-side (already polled/cached).
- **StarRocks parity:** the Ranger tag path is Trino-first; StarRocks tag enforcement may lag (as resource policies did).
- **Column identity across engines** for column tags (naming, case, nested fields).
- **UX of tag governance:** who can create tags vs assign them (a `tag:manage` vs `tag:assign` split); preventing tag sprawl.
- **Migration:** existing per-object deny grants that emulate classification could be migrated to tags later (not required).

## 9. Recommendation

Proceed **P3.1 → P3.2 → P3.3**, own tag store (D1), data-plane-first (D2), key-only tags to start (D3). P3.1 is a bounded foundation we can plan+build next in the established subagent-driven cadence; P3.2 is the highest-value follow-on on the existing Ranger substrate; P3.3 (the Cedar schema flip) comes last with its own risk review.

**Next step options:** (a) write the **P3.1 (tag model + UI) implementation plan** and build it; (b) adjust the decisions above (e.g. BI-ABAC-first) before planning; (c) go deeper on one plane (a focused P3.2 Ranger-tag design or P3.3 Cedar-schema design) before committing.
