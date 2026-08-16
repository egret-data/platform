# Data API P5 — design decisions already approved

**Status: not a spec.** These are the decisions the user approved during the P4.5 session,
captured so the P5 brainstorm resumes at section 3 rather than re-litigating sections 1 and 2.
Write the real spec at `docs/en/specs/YYYY-MM-DD-data-api-p5-design.md` (+ vi mirror).

## Approved — section 1: navigation and pages

**Audience: internal only.** Try-it and the generated OpenAPI serve the endpoint's *author*,
not external API consumers. Consumers get a URL and a downloadable OpenAPI file. One design
language, no developer portal. (The `ui-ux-pro-max` skill classifies a developer portal as a
different product type — "Trust & Authority + Minimalism", dark code theme — which is *not*
egret's "Data-Dense Dashboard". Building both would put two design languages in one product.)

**Placement: the `Data Engineering` sidebar group, not Explore.** This was the user's
correction to an earlier proposal, and the codebase already agrees —
`frontend/src/components/layout/Sidebar.tsx` already contains:

```tsx
{ heading: "Data Engineering", items: [
    { label: "Data API", icon: Plug },      // no `to:` — an inert placeholder
    { label: "Pipelines", icon: Waypoints },
]},
```

P5 gives that item `to: "data-api"` and `feature: "data_api"` (the feature key exists at
`crates/egret-workspace/src/features.rs:39`). Explore is where you *consume* and analyse;
Data API is where you *produce* an interface, next to Pipelines.

**Route `/explore/:ws/data-api`, with a vertical section-nav on the left** — the Workspace
Settings idiom, not a tab strip, because there are three areas of different natures and more
will come:

```
Data API
  Endpoints     list, canonical Explore scaffold
  Clients       service accounts + OAuth2 clients
  Activity      statistics from dataapi.access_log
```

Keeping Clients *inside* Data API rather than in Workspace Settings → Connections is
deliberate: you ship an API from one place.

**Endpoints list** follows the canonical Explore list-page scaffold — `p-4`, `h1 text-lg`,
`"+ Endpoint"`, a hand-rolled `<table>` like Charts/Datasets (**not** `FilterableTable`, which
cannot model folder grouping), `SummaryStrip` counts that double as filter chips, and
`RowActionsMenu` per row. Columns: name · method + path · state (draft/live/disabled) ·
version · engine · last edited.

**Endpoint detail is its own route** (`/data-api/:id`), never a dialog — the lesson from
Alerts #485 — with its own section nav: `Definition` · `Response` · `Access` · `Try it` ·
`Versions`. Being a route obliges it to arm `useUnsavedChangesWarning`.

**Clients** is a flat list, so `FilterableTable` + `SummaryStrip` is right here (as
`DatabasesPage` does). Two levels: service account → its clients. The secret appears **once**,
in a dialog after creation, with a copy button and a sentence saying it cannot be shown again.

**Deliberately not in scope:** endpoints get no bookmark/trash/share (they are operational
contracts, not saved content); no consumer-facing portal.

## Approved — section 2: Try-it authenticates by server-side proxy

The serving path takes only service-account bearer tokens — it does not accept a session
cookie — so the browser cannot simply call it.

**New route:** `POST /api/ws/{ws}/data-api/endpoints/{id}/try`, gated on
`data_api_endpoint:manage` (not `:read` — this acts on someone else's behalf). Body:
`{ service_account_id, params }`.

**It must invoke the real serving router in-process through its tower `Service`** — the same
router, layers and handler a real consumer traverses — after minting a real short-lived
(60 s) token. Not a re-implemented "equivalent" path, and not an HTTP loopback (which would
need the server's own URL and TLS). The P4 tests already drive the router this way, so the
mechanism is proven. *Reason this is non-negotiable:* P4.5 repeatedly showed a simulated path
staying green while the real one failed. A Try-it that says OK while consumers get 403 is
worse than no Try-it.

**It must not bypass any gate.** No grant → the console shows a real `403 not_granted`, which
is the most common first failure; that screen should name the fix and link to the `Access`
tab. Rate limit, cache, Ranger and the access log all apply for real.

Three consequences to document rather than let someone discover:
- A trial **consumes the service account's rate-limit budget**.
- A trial **writes to `access_log`**, polluting Activity statistics. Proposed: a nullable
  `actor_user_id` column — non-null means "run from Try-it by this person" — which both filters
  it out of statistics and serves as the audit trail. Needs a small migration.
- On a `cache.scope: shared` endpoint a trial **can populate the cache real consumers read**.
  Same data, so not a leak, but it belongs in the docs.

**Audit:** this is an impersonation action; write an `audit.audit_log` row (who, endpoint,
service account, result). Precedent: user impersonation, #332.

**The token never reaches the browser.** The console shows the full request with
`Authorization: Bearer ••••`, plus a copyable `curl` block using a `$TOKEN` placeholder.

## Not yet designed — section 3

Still to brainstorm: per-endpoint generated OpenAPI (and whether a workspace-level aggregate is
the more useful artifact); the Activity tab's statistics from `access_log` (calls, error rate,
p50/p95, cache-hit rate) and their time-range control; and where the §5.3 warning goes — that
**publishing does not grant data access**, since each service account still needs both an
invoke grant and Ranger permission.

## Constraints P5 inherits, discovered during P4/P4.5

- **The member picker must not offer service accounts without labelling them.** `MemberRepo::add`
  has no `users.provider` filter, and after migration 0124 a `ws_admin` membership carries
  `data_api_endpoint:invoke`, which the Cedar workspace→object cascade applies to **every**
  endpoint in the workspace. Never offer `data_api_invoker` as a membership role.
- **`egret-bi`'s `kind_perms` arm for `data_api_endpoints`** is P5's, so endpoints appear in the
  Access Control console like dashboards and datasets rather than existing only in a grant table.
- **Disabling the `data_api` feature does not stop serving.** Decided in P2; live endpoints keep
  running and only the management surface hides. An operator will reach for that toggle first,
  so say it where they will see it.
- **`ParamLocation::Body` is refused at publish and `serve_post` has no body extractor.** If P5
  surfaces a `location` control at all, `body` must not be offered.
