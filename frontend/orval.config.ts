import { defineConfig } from "orval";

/**
 * Frontend OpenAPI codegen (E0·P4c).
 *
 * Input  : the committed spec at the repo root (`../openapi.json`), emitted by
 *          the backend via `cargo run -p egret-bin -- openapi`.
 * Output : `src/api/generated/`, split one file per OpenAPI tag (`tags-split`)
 *          so each backend domain (databases, queries, charts, …) is its own
 *          module; shared TS types land under `generated/model/`.
 * Client : react-query — `useQuery` for GETs, `useMutation` for the rest
 *          (orval's verb-based default; we intentionally do NOT override it).
 * Mutator: every request funnels through `src/api/mutator.ts#customInstance`,
 *          which delegates to `client.ts#apiFetch` (auth cookies, CSRF header,
 *          ApiError/UnauthorizedError). See that file for the `/api` reconcile.
 *
 * operationName: the backend reuses terse per-tag operationIds (`list`,
 * `create`, `update`, `remove`, `detail`, `status`, …) that collide across
 * tags. OpenAPI requires operationIds to be globally unique; orval dedupes by
 * generated symbol name, so without this every duplicate would clobber the
 * others (e.g. only one `list` would survive, silently dropping the rest). We
 * prefix each name with its tag → `databasesList`, `controlPlaneStorageList`,
 * … which is both collision-free and far more readable than bare `useList`.
 */
function toCamel(input: string): string {
  return input
    .replace(/[^a-zA-Z0-9]+(.)?/g, (_m, c: string | undefined) =>
      c ? c.toUpperCase() : "",
    )
    .replace(/^(.)/, (m) => m.toLowerCase());
}

export default defineConfig({
  egret: {
    input: {
      target: "../openapi.json",
    },
    output: {
      mode: "tags-split",
      target: "src/api/generated",
      schemas: "src/api/generated/model",
      client: "react-query",
      // Committed as a build artifact; prettier isn't wired in CI, so skip it.
      prettier: false,
      override: {
        mutator: {
          path: "src/api/mutator.ts",
          name: "customInstance",
        },
        // Tag-prefix every operation so duplicate operationIds stay unique.
        operationName: (operation, _route, _verb) => {
          const tag = operation.tags?.[0] ?? "api";
          const id = operation.operationId ?? "op";
          return toCamel(`${tag}-${id}`);
        },
      },
    },
  },
});
