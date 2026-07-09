# E0·P4 — OpenAPI emit + Frontend client codegen

> **Status:** Decomposition · **Date:** 2026-07-09 · **Epic:** E0·P4 (final Foundation piece of the [service-ready redesign](../architecture/2026-07-08-service-ready-architecture-design.md)) · Executed **subagent-driven** on branch `feat/e0-p4-openapi-codegen`.

## 1. Goal & scope (locked)
The design's seam recipe includes "OpenAPI/utoipa codegen for FE." Deliver it, **full coverage**:
- **Backend:** emit a single OpenAPI 3.1 document covering **all ~157 endpoints across 24 routers / ~12 crates**, generated from the Rust code (utoipa).
- **Frontend:** a **full generated client** (typed fetch + react-query hooks) replacing the 19 hand-written `frontend/src/api/*.ts`, adopted across the **~144 components** that import them.

Starting state: utoipa is **absent** (only named in comments); FE has 19 hand-written api modules + react-query hooks over `apiFetch`/`apiJson` (`src/api/client.ts`). axum **0.8**, FE = Vite + `tsc -b` + @tanstack/react-query.

## 2. Locked decisions
1. **OpenAPI generation → utoipa 5 (+ its chrono/uuid/`preserve_order` features), per-crate `#[derive(OpenApi)]` + merge.** Each router crate: `#[derive(ToSchema)]` on its request/response DTOs, `#[utoipa::path(...)]` on each handler, and one `pub fn openapi() -> utoipa::openapi::OpenApi` (a `#[derive(OpenApi)]` struct listing that crate's paths+components). `egret-server` merges all crate `openapi()`s into one doc. **Routers stay plain axum** (NOT converted to `utoipa-axum::OpenApiRouter`) — least invasive to the existing assembly. A shared `ErrorBody` schema (matching `ApiError`'s JSON) documents the error responses.
2. **Spec emission → a committed `openapi.json` produced by a Rust emitter** (`egret-bin` subcommand `openapi` OR a workspace test/xtask that serializes `egret_server::openapi_doc()` to `openapi.json` at repo root) + a live `GET /api-docs/openapi.json` route (+ swagger-ui at `/api-docs` for humans). FE codegen consumes the committed file (deterministic, no running server needed in CI).
3. **FE codegen → orval** (generates typed client + @tanstack/react-query hooks from `openapi.json`) into `frontend/src/api/generated/`; an `npm run gen:api` script; the 19 hand-written modules are replaced and the ~144 call sites rewired to the generated hooks. `apiFetch`/CSRF/credentials wiring preserved via orval's custom mutator (point it at the existing `client.ts`).

## 3. Parts (proven-slice-first, then scale)
| Part | Content | Risk |
|---|---|---|
| **P4a — backend infra + reference slice** | Workspace utoipa deps; `#[derive(ToSchema)]` on egret-query-api DTOs + egret-query registry/execution request/response DTOs; `#[utoipa::path]` on egret-query's 2 routers' handlers (databases + queries); `egret_query::openapi()`; the shared `ErrorBody` schema; `egret_server::openapi_doc()` merging (initially just query); `GET /api-docs/openapi.json` + swagger-ui; the `openapi.json` emitter + committed file. **Prove:** valid spec covers databases+queries end-to-end. Surfaces every utoipa/axum-0.8 gotcha (uuid/chrono/serde_json::Value schemas, the error-response convention) before scaling. | High (integration) |
| **P4b — backend full annotation** | The remaining ~10 crates, in batches (auth/me/oidc/idp · controlplane users/workspaces/roles/access · catalog admin+engine · policy admin+ranger · bi chart/dashboard/dataset/folder/sqllab · notebook · kernel). Each: ToSchema on DTOs + utoipa::path on handlers + `openapi()`, merged into `openapi_doc()`. Independent crates can parallelize. | Medium (mechanical, high volume) |
| **P4c — FE codegen infra** | Add orval + config consuming `openapi.json`; custom mutator over `client.ts` (apiFetch, credentials, CSRF, base URL); `npm run gen:api`; generate `src/api/generated/`; migrate ONE module (databases.ts) + its call sites as the proof. | Medium |
| **P4d — FE full migration** | Replace the remaining 18 api modules with generated hooks; rewire all ~144 call sites, in domain batches (charts/dashboards/datasets/folders/sqllab · notebooks/notebookSettings · controlPlane/access/secrets · policies · databases). Delete the hand-written modules. | High (breadth) |

## 4. Execution
Subagent-driven (implementer → spec review → code-quality review per part). Gate each part: `cargo fmt`+`clippy --workspace --all-targets` + affected tests; the emitted `openapi.json` must validate (openapi 3.1) and round-trip; FE `tsc -b`+`lint`+`test`+`build` green after each FE batch. **API route strings + wire shapes MUST stay byte-identical** (this is documentation + client codegen only — no behavior/contract change). Stacked on one branch → one squash PR (`--admin`). Full workspace test + FE gate + holistic review before merge.

## 5. Out of scope / notes
No behavior change to any endpoint (annotation only). No new auth. Swagger-UI is dev-convenience. If utoipa 5 + axum 0.8 surfaces an integration blocker (e.g. a DTO shape utoipa can't derive — `serde_json::Value` → `Object`, custom enums), the slice (P4a) catches it and we adapt the convention before P4b/P4d scale.
