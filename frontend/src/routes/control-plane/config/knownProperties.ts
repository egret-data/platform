export type PropType = "string" | "number" | "boolean" | "enum" | "size" | "duration";
export interface KnownProperty { key: string; section: string; label: string; type: PropType; default?: string; help: string; docsUrl?: string; enumOptions?: string[] }
export const knownProperties: KnownProperty[] = [
  // queryMemory
  { key: "query.max-total-memory", section: "queryMemory", label: "Query max total memory", type: "size", help: "Cluster total memory (incl. revocable) a query may use.", docsUrl: "https://trino.io/docs/current/admin/properties-resource-management.html#query-max-total-memory" },
  { key: "query.max-cpu-time", section: "queryMemory", label: "Query max CPU time", type: "duration", help: "Max cluster CPU time a single query may use (runaway guard).", docsUrl: "https://trino.io/docs/current/admin/properties-query-management.html" },
  { key: "query.max-execution-time", section: "queryMemory", label: "Query max execution time", type: "duration", default: "100d", help: "Max active execution time before a query is killed.", docsUrl: "https://trino.io/docs/current/admin/properties-query-management.html#query-max-execution-time" },
  { key: "query.max-run-time", section: "queryMemory", label: "Query max run time", type: "duration", default: "100d", help: "Max total lifetime incl. queue time.", docsUrl: "https://trino.io/docs/current/admin/properties-query-management.html#query-max-run-time" },
  { key: "query.max-planning-time", section: "queryMemory", label: "Query max planning time", type: "duration", default: "10m", help: "Max planning time before failing a query.", docsUrl: "https://trino.io/docs/current/admin/properties-query-management.html" },
  { key: "query.client.timeout", section: "queryMemory", label: "Client timeout", type: "duration", default: "5m", help: "Abandon a query if the client stops contacting the server.", docsUrl: "https://trino.io/docs/current/admin/properties-query-management.html" },
  { key: "query.low-memory-killer.policy", section: "queryMemory", label: "Low-memory killer policy", type: "enum", default: "total-reservation-on-blocked-nodes", enumOptions: ["none","total-reservation","total-reservation-on-blocked-nodes"], help: "Which query to kill when the cluster runs low on memory." },
  { key: "query.max-history", section: "queryMemory", label: "Query history size", type: "number", default: "100", help: "Completed queries kept for the Web UI / API." },
  { key: "query.max-hash-partition-count", section: "queryMemory", label: "Max hash partition count", type: "number", default: "100", help: "Max partitions for distributed hash operations." },
  { key: "query.min-hash-partition-count", section: "queryMemory", label: "Min hash partition count", type: "number", default: "4", help: "Min partitions for distributed hash operations." },
  // spilling
  { key: "spill-enabled", section: "spilling", label: "Spill to disk", type: "boolean", default: "false", help: "Spill operator memory to disk when memory limits are hit.", docsUrl: "https://trino.io/docs/current/admin/properties-spilling.html" },
  { key: "spiller-spill-path", section: "spilling", label: "Spill path(s)", type: "string", help: "Directory/directories for spilled data (required when spill is enabled; needs a writable volume).", docsUrl: "https://trino.io/docs/current/admin/properties-spilling.html" },
  { key: "max-spill-per-node", section: "spilling", label: "Max spill per node", type: "size", default: "100GB", help: "Max spill space per node across all queries." },
  { key: "query-max-spill-per-node", section: "spilling", label: "Query max spill per node", type: "size", default: "100GB", help: "Max spill space per node for a single query." },
  // execution
  { key: "task.concurrency", section: "execution", label: "Task concurrency", type: "number", help: "Local concurrency for parallel operators (power of 2).", docsUrl: "https://trino.io/docs/current/admin/properties-task.html#task-concurrency" },
  { key: "task.max-worker-threads", section: "execution", label: "Max worker threads", type: "number", help: "Threads used to process splits (default CPU*2).", docsUrl: "https://trino.io/docs/current/admin/properties-task.html" },
  { key: "join-distribution-type", section: "execution", label: "Join distribution type", type: "enum", default: "AUTOMATIC", enumOptions: ["AUTOMATIC","PARTITIONED","BROADCAST"], help: "How joins are distributed across the cluster.", docsUrl: "https://trino.io/docs/current/admin/properties-general.html#join-distribution-type" },
  { key: "optimizer.join-reordering-strategy", section: "execution", label: "Join reordering strategy", type: "enum", default: "AUTOMATIC", enumOptions: ["AUTOMATIC","ELIMINATE_CROSS_JOINS","NONE"], help: "Cost-based join reordering.", docsUrl: "https://trino.io/docs/current/admin/properties-optimizer.html" },
  { key: "node-scheduler.include-coordinator", section: "execution", label: "Schedule work on coordinator", type: "boolean", default: "true", help: "Run splits on the coordinator too (keep off for large clusters).", docsUrl: "https://trino.io/docs/current/admin/properties-node-scheduler.html" },
  { key: "node-scheduler.max-splits-per-node", section: "execution", label: "Max splits per node", type: "number", default: "256", help: "Max splits scheduled on a node at once." },
  { key: "exchange.compression-codec", section: "execution", label: "Exchange compression", type: "enum", default: "NONE", enumOptions: ["NONE","LZ4","ZSTD"], help: "Compress inter-node exchange data." },
  // sqlWebUi
  { key: "sql.default-catalog", section: "sqlWebUi", label: "Default catalog", type: "string", help: "Default catalog for clients that don't set one." },
  { key: "sql.default-schema", section: "sqlWebUi", label: "Default schema", type: "string", help: "Default schema for clients that don't set one." },
  { key: "sql.forced-session-time-zone", section: "sqlWebUi", label: "Forced session time zone", type: "string", help: "Override the session time zone cluster-wide (e.g. UTC)." },
  { key: "web-ui.enabled", section: "sqlWebUi", label: "Web UI enabled", type: "boolean", default: "true", help: "Expose the Trino Web UI.", docsUrl: "https://trino.io/docs/current/admin/properties-web-interface.html" },
  { key: "web-ui.authentication.type", section: "sqlWebUi", label: "Web UI auth type", type: "enum", default: "FORM", enumOptions: ["FORM","FIXED","CERTIFICATE","KERBEROS","JWT","OAUTH2"], help: "Web UI authentication mechanism." },
  { key: "web-ui.shared-secret", section: "sqlWebUi", label: "Web UI shared secret", type: "string", help: "Stable secret for the Web-UI auth cookie (survives restarts)." },
  { key: "catalog.management", section: "sqlWebUi", label: "Catalog management", type: "enum", default: "static", enumOptions: ["static","dynamic"], help: "Allow CREATE/DROP CATALOG at runtime (dynamic)." },
  // reliability (folded into existing Reliability section)
  { key: "query-retry-attempts", section: "reliability", label: "Query retry attempts", type: "number", default: "4", help: "Max query retries under retry-policy=QUERY.", docsUrl: "https://trino.io/docs/current/admin/fault-tolerant-execution.html" },
  { key: "task-retry-attempts-per-task", section: "reliability", label: "Task retry attempts", type: "number", default: "4", help: "Max task retries under retry-policy=TASK." },
  // advanced (folded into existing Advanced section)
  { key: "log.format", section: "advanced", label: "Log format", type: "enum", default: "TEXT", enumOptions: ["TEXT","JSON"], help: "Server log format (JSON for centralized aggregation)." },
];
export const knownPropertyKeys = new Set(knownProperties.map(p => p.key));
