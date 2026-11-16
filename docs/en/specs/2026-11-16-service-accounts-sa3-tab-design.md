# Service accounts SA3 — Service accounts tab (frontend) — design

> Third package of the "service accounts as principals" programme. Frontend-only.
> Builds on SA1 (`/service-accounts` routes) + SA2 (`/service-accounts/{id}/credentials`).
> Extends the programme design `2026-11-15-service-accounts-as-principals-design.md`.
> Every FE brief/review binds `docs/en/frontend-rules.md`.

## Background (current FE)

- **People & groups** tab (`frontend/src/routes/governance/people/`, sidebar "People & groups" → `people`, perm `policy:read`) lists people and, via a `?serviceAccounts=1` toggle, surfaces SAs (SA1 backend) — list only, no management.
- **Data API > Clients** (`frontend/src/routes/explore/dataapi/`, sidebar "Clients" → `data-api/clients`, wsPerm `service_account:read`): workspace-scoped self-service to create a Data-API SA + client. Stays as self-service.
- The generated client already exposes the SA1 `controlPlaneAccessListServiceAccounts`/`...CreateServiceAccount`/etc.; the SA2 `/credentials` fns land when #869 merges (SA3 plan/impl starts after that).

## Goal

A dedicated **"Service accounts"** tab (a sidebar entry peer to "People & groups") that manages service accounts as first-class principals: list/create/edit, platform vs workspace scope, group/role membership, and credential management across both audiences (Data-API + Catalog). Admin/unified surface; the Data-API Clients self-service UI **coexists** (deprecation noted later).

## Decisions (brainstorming 2026-11-16)

- **Dedicated sidebar entry** "Service accounts", peer to People & groups (per the direction memo "own tab, peer of users"). The People `?serviceAccounts=1` toggle may stay as a read-only cross-reference.
- **Coexist** with Data API > Clients (the new tab is the admin/unified surface; Clients stays for workspace self-service). A later package may deprecate Clients.
- **Full scope in one package:** list/create/edit + scope + membership + credentials.

## Navigation & routes

- Sidebar entry "Service accounts" (icon e.g. `BotMessageSquare`/`KeyRound`-family; perm `user:read` — the SA list route's permission). Place adjacent to "People & groups" in the governance/access-control nav group.
- Routes:
  - `/service-accounts` — list, with URL-driven scope filter (`?scope=platform|workspace`) and workspace filter, per FE-PAGE-2 (shareable URL state).
  - `/service-accounts/{id}` — detail (identity, membership, credentials), with the active section in the URL.

## List page

- Table: name, scope (platform/workspace badge), workspace (for workspace SAs), status (active/disabled), created. Scope filter control. Create button.
- **Create dialog:** scope (platform | workspace), name, description, workspace picker (shown only for workspace scope). On success, route to the new SA's detail.
- Row actions: disable/enable (kill switch), delete (confirm dialog; surfaces server guards — e.g. last-admin/content-ownership 409 → clear message).
- Empty/loading/error states per the rulebook.

## Detail page (sections)

1. **Identity** — name + description (editable), scope + workspace (read-only), `svc-` username, status. "This is a machine identity: it never signs in and can never hold platform_admin."
2. **Membership** — the groups and roles the SA holds; add (group picker / role picker) + remove. Surface the backend guards as inline errors, not crashes: adding to an **OIDC-synced group** is refused ("membership owned by the identity provider"); granting **platform_admin** is refused; adding to a group that holds platform_admin is refused.
3. **Credentials** — list across BOTH audiences with audience badge (data-api/catalog) + status (active/revoked/expired) + last-used; for catalog rows show warehouse + role.
   - **Create:** audience selector. Catalog form carries warehouse + catalog_role + consumer_type (engine/external) pickers; Data-API form is minimal. A **platform-scoped SA** cannot create a Data-API credential — disable that option with an explanation (Data-API credentials are workspace-bound).
   - **Rotate / revoke / set-expiry** per credential. Rotate and create return a **one-time secret**: show it once in a copy-able reveal with a "won't be shown again" warning; never store/echo it afterwards.
   - A visible note: **catalog credentials read the data lake (Iceberg) directly and are outside the DG masking/row-filter/clearance model** — the SA disable/revoke/expire controls govern them, not engine masks.
4. **Lifecycle** — disable/enable (the kill switch, same effect as disabling a user) and delete, with confirmation.

## UX constraints (from the rulebook + this domain)

- URL carries shareable state (filters, selected SA, active section) — FE-PAGE-2.
- One-time secret: reveal-once + copy + explicit warning; the secret is never re-fetchable.
- Permission-aware controls: actions the caller lacks permission for are hidden/disabled, but the server remains the authority (a 403 surfaces as a clear message, never a crash).
- Every refusal path (guards, scope mismatch, 409s) renders an inline, human message.
- Works at phone width (390px), matching the People tab's responsive tabs.
- No `any`; generated client types reused; queries invalidated on mutation (mirror the People/engines query patterns).

## Decomposition (plan tasks, each its own review)

- **T1** — API wrappers in `frontend/src/api/` for the SA routes (list/get/create/disable/enable/delete + group/role membership) and the credential routes (list/create/rotate/revoke/expiry) + query-key factories, over the generated client. Mirror the existing `policies.ts`/`controlPlane.ts` wrapper style. Pure unit-testable.
- **T2** — Sidebar entry + routes + the **list page** (list, scope filter, create dialog, disable/enable, delete). Tests: list renders, create dialog validates scope/workspace, row actions invalidate.
- **T3** — Detail page **Identity + Membership**: editable name/description, scope display, group/role add/remove with the guard errors surfaced. Tests: membership add/remove, OIDC-group + platform_admin refusals render.
- **T4** — Detail page **Credentials panel**: list both audiences, per-audience create (catalog pickers), rotate/revoke/expiry, one-time-secret reveal, the catalog/DG note, platform-SA-no-dataapi rule. Tests: list, create per audience, rotate shows secret once, revoke/expiry.
- **T5** — Full FE gate (`npm run lint`, `npx tsc -b`, `npm run test`, `npm run build`, the governance/access bundle test) + screenshots for the docs PR (the SA tab, create, detail, credentials).

## Global constraints

- Branch `feat/svc-accounts-sa3` off **main that already has SA2 (#869)** — the generated client must carry the credential routes. FE-only; no `CARGO_TARGET_DIR`. No prettier in this tree. Regenerate the client only with `npm run gen:api` (SA3 adds NO endpoint, so no regen expected).
- Chat VI; code/commits/docs EN. Co-author `Claude Opus 4.8`. Conventional Commits; `Closes #N` only in PR body. Stage by explicit path. No secrets.
- Each FE task runs the rulebook's gate subset; T5 runs the full gate. Docs (the SA tab narrative + screenshots) land in `egret-data/docs` as a companion PR.
- Binds `docs/en/frontend-rules.md` (its MUST rules).

## Deferred (carried)

- SA4: the comprehensive `platform_admin` invariant + the catalog-raw-read vs DG-mask semantics docs + live-check.
- Deprecating the Data API > Clients self-service UI in favour of the unified tab — a later decision.
- A platform-admin-only "all workspaces" SA roll-up, if wanted, beyond the scope filter.
