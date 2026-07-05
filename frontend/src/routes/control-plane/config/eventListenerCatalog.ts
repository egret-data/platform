/**
 * Trino event-listener property catalog, extracted from the Trino docs
 * (`docs/src/main/sphinx/develop/event-listener.md` +
 * `docs/src/main/sphinx/admin/event-listeners-{http,mysql,openlineage}.md`).
 *
 * Same shape as `trinoCatalog.ts`'s `TrinoProp` — `file` is set to the dummy
 * value `"config"` for every entry since `KeyedPropertyList` ignores it; these
 * keys live in `event-listener.properties`, not `config.properties`. Nothing
 * here is invented — every key appears in the docs.
 */

import type { TrinoProp } from "./trinoCatalog";

const DEVELOP = "https://trino.io/docs/current/develop/event-listener.html";
const HTTP = "https://trino.io/docs/current/admin/event-listeners-http.html";
const MYSQL = "https://trino.io/docs/current/admin/event-listeners-mysql.html";
const OPENLINEAGE =
  "https://trino.io/docs/current/admin/event-listeners-openlineage.html";

export const eventListenerCatalog: TrinoProp[] = [
  // ── General ──────────────────────────────────────────────────────────────
  {
    key: "event-listener.name",
    group: "General",
    file: "config",
    type: "enum",
    enumOptions: ["http", "mysql", "openlineage"],
    help: "Which event listener plugin to load.",
    docsUrl: DEVELOP,
  },
  // ── HTTP ─────────────────────────────────────────────────────────────────
  {
    key: "http-event-listener.connect-ingest-uri",
    group: "HTTP",
    file: "config",
    type: "string",
    help: "URI that receives POSTed event JSON.",
    docsUrl: HTTP,
  },
  {
    key: "http-event-listener.connect-http-headers",
    group: "HTTP",
    file: "config",
    type: "string",
    help: "Comma-separated list of extra HTTP headers to send with each request.",
    docsUrl: HTTP,
  },
  {
    key: "http-event-listener.log-created",
    group: "HTTP",
    file: "config",
    type: "boolean",
    default: "false",
    help: "Send an event when a query is created.",
    docsUrl: HTTP,
  },
  {
    key: "http-event-listener.log-completed",
    group: "HTTP",
    file: "config",
    type: "boolean",
    default: "true",
    help: "Send an event when a query completes.",
    docsUrl: HTTP,
  },
  {
    key: "http-event-listener.log-split",
    group: "HTTP",
    file: "config",
    type: "boolean",
    default: "false",
    help: "Send an event for each split completion.",
    docsUrl: HTTP,
  },
  {
    key: "http-event-listener.connect-retry-count",
    group: "HTTP",
    file: "config",
    type: "number",
    default: "0",
    help: "Number of retries on failed delivery before giving up.",
    docsUrl: HTTP,
  },
  {
    key: "http-event-listener.connect-retry-delay",
    group: "HTTP",
    file: "config",
    type: "duration",
    default: "1s",
    help: "Initial delay before the first retry attempt.",
    docsUrl: HTTP,
  },
  {
    key: "http-event-listener.connect-backoff-base",
    group: "HTTP",
    file: "config",
    type: "number",
    default: "2",
    help: "Exponential backoff base used to grow the retry delay.",
    docsUrl: HTTP,
  },
  {
    key: "http-event-listener.connect-max-delay",
    group: "HTTP",
    file: "config",
    type: "duration",
    default: "1m",
    help: "Maximum delay between retry attempts.",
    docsUrl: HTTP,
  },
  // ── MySQL ────────────────────────────────────────────────────────────────
  {
    key: "mysql-event-listener.db.url",
    group: "MySQL",
    file: "config",
    type: "string",
    help: "JDBC connection URL for the MySQL database that stores events.",
    docsUrl: MYSQL,
  },
  // ── OpenLineage ──────────────────────────────────────────────────────────
  {
    key: "openlineage-event-listener.transport.type",
    group: "OpenLineage",
    file: "config",
    type: "string",
    help: "Transport used to emit OpenLineage events (e.g. http, kafka, console).",
    docsUrl: OPENLINEAGE,
  },
  {
    key: "openlineage-event-listener.transport.url",
    group: "OpenLineage",
    file: "config",
    type: "string",
    help: "Endpoint URL the transport delivers OpenLineage events to.",
    docsUrl: OPENLINEAGE,
  },
  {
    key: "openlineage-event-listener.trino.uri",
    group: "OpenLineage",
    file: "config",
    type: "string",
    help: "URI of this Trino cluster, recorded on emitted lineage events.",
    docsUrl: OPENLINEAGE,
  },
];

export const eventListenerCatalogByKey: Record<string, TrinoProp> =
  Object.fromEntries(eventListenerCatalog.map((p) => [p.key, p]));
