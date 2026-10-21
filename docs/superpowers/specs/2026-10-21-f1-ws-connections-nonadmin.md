# F1: Workspace connections for non-admins (KI-159)

Package F1 of the Known-issues fix programme. One row: KI-159 (S1).

## Problem

`GET /api/ws/{ws}/databases` required `workspace:manage_members`, so every member
who is not a workspace admin got 403. The SQL Lab database picker stayed empty
("Select a database") and said nothing. Queries sent through the API with a
`database_id` ran, because `POST /queries` checks `query:run` and only looks
for the id among the workspace's connections.

## Decision (D9, settled)

- `query:run` **or** `dataset:read` in the workspace lists the connections
  granted to that workspace. The handler checks `workspace:manage_members`
  first: an admin gets the unchanged `DatabaseDetailDto` rows.
- Everyone else who passes gets `DatabaseListDto` rows: `id`, `name`, `engine`,
  `delegated`. No host, port, options (user, catalog, scheme, TLS), secret
  reference, status or engine id. `delegated` stays because it is a boolean
  that tells the user whether queries run as them, and the schema already
  carried it.
- A principal with none of the three permissions, a non-member and a member of
  another workspace still get 403.
- Create, update, delete, detail, test (saved and unsaved) stay admin-only.
- The response is declared as `DatabaseListing`, an untagged `oneOf` of the two
  arrays, so the OpenAPI document and the generated client describe both.

## Frontend

- `@/api/databases` types the list as `DatabaseConn | DatabaseListDto` and
  exports `isFullConnection` (a row with `host`).
- SQL Lab Explorer (`DataPanel`): an empty list shows *No connection is granted
  to this workspace. Ask a workspace admin.*; a 403 or other failure shows an
  error instead of a silent empty picker.
- Workspace settings → Engines (`DatabasesPage`) is a management page: a
  redacted listing shows the existing workspace-admin notice. Limitation: an
  EMPTY listing cannot tell admin from non-admin, so a non-admin in a workspace
  with no connection sees the empty state (its Add button then gets a 403).
- Data API pickers read `governed` only from full rows; the governance hint
  answers "not governed" for a redacted row, as it does for one not loaded.

## Tests

- Backend (`registry_workspace_routes_api.rs`): the listing as ws_viewer,
  ws_editor direct and through a group, a custom role with only `query:run`,
  one with only `dataset:read` (200, exact key set, no host/credential text);
  a custom role with only `chart:read`, a non-member and another workspace's
  admin (403); the admin keeps the full shape; ws_viewer and ws_editor still get
  403 on every write, detail and test route.
- Frontend: `DataPanel.connections.test.tsx` (fetch stubbed), `DatabasesPage`
  redacted-listing case.
