/**
 * Full Trino operator-facing property catalog, extracted from the Trino admin
 * docs (`docs/src/main/sphinx/admin/properties-*.md` + `fault-tolerant-execution.md`).
 *
 * This is a generated-by-hand data file: each entry is a well-documented
 * `config.properties` / `node.properties` / `log.properties` knob an operator
 * may set on a cluster. `help` is a one-line paraphrase of the doc; `docsUrl`
 * deep-links to the property's anchor on trino.io. Truly obscure internal knobs
 * are intentionally omitted; nothing here is invented — every key appears in the
 * docs. Folds in the 31 hand-curated entries formerly in `knownProperties.ts`.
 */

export type PropType =
  | "string"
  | "number"
  | "boolean"
  | "enum"
  | "size"
  | "duration";

export interface TrinoProp {
  key: string;
  group: string;
  file: "config" | "node" | "log";
  type: PropType;
  default?: string;
  help: string;
  docsUrl?: string;
  enumOptions?: string[];
}

const ADMIN = "https://trino.io/docs/current/admin/";

/** Sphinx/MyST heading slug: lowercase, non-alphanumerics collapse to hyphens. */
function anchor(key: string): string {
  return key
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

/** A section entry before `group`/`file`/`docsUrl` are applied. */
type Entry = Omit<TrinoProp, "group" | "file" | "docsUrl"> & {
  file?: "config" | "node" | "log";
  /** Override the derived docs anchor (e.g. when the heading text differs). */
  docsUrl?: string;
};

function section(
  page: string,
  group: string,
  file: "config" | "node" | "log",
  entries: Entry[],
): TrinoProp[] {
  return entries.map((e) => ({
    key: e.key,
    group,
    file: e.file ?? file,
    type: e.type,
    ...(e.default !== undefined ? { default: e.default } : {}),
    help: e.help,
    docsUrl: e.docsUrl ?? `${ADMIN}${page}.html#${anchor(e.key)}`,
    ...(e.enumOptions ? { enumOptions: e.enumOptions } : {}),
  }));
}

export const trinoCatalog: TrinoProp[] = [
  // ── General ──────────────────────────────────────────────────────────────
  ...section("properties-general", "General", "config", [
    {
      key: "join-distribution-type",
      type: "enum",
      default: "AUTOMATIC",
      enumOptions: ["AUTOMATIC", "PARTITIONED", "BROADCAST"],
      help: "How joins are distributed across the cluster (cost-based when AUTOMATIC).",
    },
    {
      key: "redistribute-writes",
      type: "boolean",
      default: "true",
      help: "Hash-redistribute data before writing to avoid skew (at some overhead).",
    },
  ]),

  // ── Memory / resource management ─────────────────────────────────────────
  ...section("properties-resource-management", "Memory", "config", [
    {
      key: "query.max-cpu-time",
      type: "duration",
      default: "1_000_000_000d",
      help: "Max cluster CPU time a single query may use (runaway guard).",
    },
    {
      key: "query.max-memory-per-node",
      type: "size",
      help: "Max user memory a query may use on a single worker.",
    },
    {
      key: "query.max-memory",
      type: "size",
      default: "20GB",
      help: "Max user memory a query may use across the whole cluster.",
    },
    {
      key: "query.max-total-memory",
      type: "size",
      help: "Cluster total memory (incl. revocable) a query may use.",
    },
    {
      key: "memory.heap-headroom-per-node",
      type: "size",
      help: "JVM heap set aside as headroom for allocations Trino does not track.",
    },
    {
      key: "exchange.deduplication-buffer-size",
      type: "size",
      default: "32MB",
      help: "Coordinator in-memory buffer for spooled data under fault-tolerant execution.",
    },
  ]),

  // ── Query management ─────────────────────────────────────────────────────
  ...section("properties-query-management", "Query management", "config", [
    {
      key: "query.client.timeout",
      type: "duration",
      default: "5m",
      help: "Abandon a query if the client stops contacting the server.",
    },
    {
      key: "query.execution-policy",
      type: "enum",
      default: "phased",
      enumOptions: ["phased", "all-at-once"],
      help: "How query stages are scheduled (phased avoids inter-stage blockages).",
    },
    {
      key: "query.determine-partition-count-for-write-enabled",
      type: "boolean",
      default: "false",
      help: "Derive write-query partition count from the data read and processed.",
    },
    {
      key: "query.max-hash-partition-count",
      type: "number",
      default: "100",
      help: "Max partitions for distributed hash operations (joins, aggregations).",
    },
    {
      key: "query.min-hash-partition-count",
      type: "number",
      default: "4",
      help: "Min partitions for distributed hash operations.",
    },
    {
      key: "query.min-hash-partition-count-for-write",
      type: "number",
      default: "50",
      help: "Min partitions for distributed operations in write queries.",
    },
    {
      key: "query.max-writer-task-count",
      type: "number",
      default: "100",
      help: "Max tasks writing data during INSERT / CTAS / EXECUTE.",
    },
    {
      key: "query.low-memory-killer.policy",
      type: "enum",
      default: "total-reservation-on-blocked-nodes",
      enumOptions: [
        "none",
        "total-reservation",
        "total-reservation-on-blocked-nodes",
      ],
      help: "Which query to kill when the cluster runs low on memory.",
    },
    {
      key: "task.low-memory-killer.policy",
      type: "enum",
      default: "total-reservation-on-blocked-nodes",
      enumOptions: [
        "none",
        "total-reservation-on-blocked-nodes",
        "least-waste",
      ],
      help: "Which task to kill on low memory when task retries are enabled.",
    },
    {
      key: "query.max-execution-time",
      type: "duration",
      default: "100d",
      help: "Max active execution time before a query is killed (excludes queue/plan).",
    },
    {
      key: "query.max-length",
      type: "number",
      default: "1000000",
      help: "Max number of characters allowed in the SQL query text.",
    },
    {
      key: "query.max-planning-time",
      type: "duration",
      default: "10m",
      help: "Max planning time before failing a query.",
    },
    {
      key: "query.max-run-time",
      type: "duration",
      default: "100d",
      help: "Max total lifetime of a query incl. analysis, planning and queue time.",
    },
    {
      key: "query.max-scan-physical-bytes",
      type: "size",
      help: "Max bytes a query may scan before it is terminated.",
    },
    {
      key: "query.max-write-physical-size",
      type: "size",
      help: "Max physical size a query may write before it is terminated.",
    },
    {
      key: "query.max-stage-count",
      type: "number",
      default: "150",
      help: "Max stages per query before it fails with QUERY_HAS_TOO_MANY_STAGES.",
    },
    {
      key: "query.max-history",
      type: "number",
      default: "100",
      help: "Completed queries kept for the Web UI / API.",
    },
    {
      key: "query.min-expire-age",
      type: "duration",
      default: "15m",
      help: "Minimum age before a query is removed from the history buffer.",
    },
    {
      key: "query.remote-task.enable-adaptive-request-size",
      type: "boolean",
      default: "true",
      help: "Dynamically split large server requests to avoid remote-task OOM.",
    },
    {
      key: "query.remote-task.guaranteed-splits-per-task",
      type: "number",
      default: "3",
      help: "Minimum splits assigned to each remote task.",
    },
    {
      key: "query.remote-task.max-error-duration",
      type: "duration",
      default: "1m",
      help: "Treat a remote task as failed after this long without coordinator contact.",
    },
    {
      key: "query.remote-task.max-request-size",
      type: "size",
      default: "8MB",
      help: "Max size of a single request made by a remote task.",
    },
    {
      key: "query.remote-task.request-size-headroom",
      type: "size",
      default: "2MB",
      help: "Headroom allocated beyond the remote-task request data size.",
    },
    {
      key: "query.info-url-template",
      type: "string",
      help: "Redirect clients to an alternate query-info URL (with ${QUERY_ID}).",
    },
    {
      key: "retry-policy",
      type: "enum",
      default: "NONE",
      enumOptions: ["NONE", "TASK", "QUERY"],
      help: "Fault-tolerant execution retry policy (per-task, per-query, or off).",
    },
  ]),

  // ── Spilling ─────────────────────────────────────────────────────────────
  ...section("properties-spilling", "Spilling", "config", [
    {
      key: "spill-enabled",
      type: "boolean",
      default: "false",
      help: "Spill operator memory to disk when memory limits are hit.",
    },
    {
      key: "spiller-spill-path",
      type: "string",
      help: "Directory (or comma-separated list) for spilled data; required when spill is on.",
    },
    {
      key: "spiller-max-used-space-threshold",
      type: "number",
      default: "0.9",
      help: "Disk-usage ratio above which a spill path becomes ineligible.",
    },
    {
      key: "spiller-threads",
      type: "number",
      help: "Number of spiller threads (default = spill dirs × 2, min 4).",
    },
    {
      key: "max-spill-per-node",
      type: "size",
      default: "100GB",
      help: "Max spill space per node across all queries.",
    },
    {
      key: "query-max-spill-per-node",
      type: "size",
      default: "100GB",
      help: "Max spill space per node for a single query.",
    },
    {
      key: "aggregation-operator-unspill-memory-limit",
      type: "size",
      default: "4MB",
      help: "Memory limit for unspilling a single aggregation operator instance.",
    },
    {
      key: "spill-compression-codec",
      type: "enum",
      default: "NONE",
      enumOptions: ["NONE", "LZ4", "ZSTD"],
      help: "Compression codec used when spilling pages to disk.",
    },
    {
      key: "spill-encryption-enabled",
      type: "boolean",
      default: "false",
      help: "Encrypt spilled data with a per-file randomly generated key.",
    },
  ]),

  // ── Exchange ─────────────────────────────────────────────────────────────
  ...section("properties-exchange", "Exchange", "config", [
    {
      key: "exchange.client-threads",
      type: "number",
      default: "25",
      help: "Threads used by exchange clients to fetch data from other nodes.",
    },
    {
      key: "exchange.concurrent-request-multiplier",
      type: "number",
      default: "3",
      help: "Multiplier for concurrent exchange requests relative to buffer memory.",
    },
    {
      key: "exchange.compression-codec",
      type: "enum",
      default: "NONE",
      enumOptions: ["NONE", "LZ4", "ZSTD"],
      help: "Compress inter-node exchange data.",
    },
    {
      key: "exchange.data-integrity-verification",
      type: "enum",
      default: "ABORT",
      enumOptions: ["NONE", "ABORT", "RETRY"],
      help: "Behavior on detecting an exchange data-integrity issue.",
    },
    {
      key: "exchange.max-buffer-size",
      type: "size",
      default: "32MB",
      help: "Exchange-client buffer holding data fetched from other nodes.",
    },
    {
      key: "exchange.max-response-size",
      type: "size",
      default: "16MB",
      help: "Maximum size of a response returned from an exchange request.",
    },
    {
      key: "sink.max-buffer-size",
      type: "size",
      default: "32MB",
      help: "Output buffer for task data waiting to be pulled by upstream tasks.",
    },
    {
      key: "sink.max-broadcast-buffer-size",
      type: "size",
      default: "200MB",
      help: "Broadcast output buffer for replicated-join build-side data.",
    },
  ]),

  // ── Task ─────────────────────────────────────────────────────────────────
  ...section("properties-task", "Task", "config", [
    {
      key: "task.concurrency",
      type: "number",
      help: "Local concurrency for parallel operators (power of two).",
    },
    {
      key: "task.http-response-threads",
      type: "number",
      default: "100",
      help: "Max threads for handling HTTP responses.",
    },
    {
      key: "task.http-timeout-threads",
      type: "number",
      default: "3",
      help: "Threads used to handle timeouts when generating HTTP responses.",
    },
    {
      key: "task.info-update-interval",
      type: "duration",
      default: "3s",
      help: "Staleness of task info used in scheduling.",
    },
    {
      key: "task.max-drivers-per-task",
      type: "number",
      default: "2147483647",
      help: "Max drivers a task runs concurrently.",
    },
    {
      key: "task.max-partial-aggregation-memory",
      type: "size",
      default: "16MB",
      help: "Max size of partial aggregation results for distributed aggregations.",
    },
    {
      key: "task.max-worker-threads",
      type: "number",
      help: "Threads used to process splits (default = node CPUs × 2).",
    },
    {
      key: "task.min-drivers",
      type: "number",
      help: "Target running leaf splits per worker (default = max-worker-threads × 2).",
    },
    {
      key: "task.min-drivers-per-task",
      type: "number",
      default: "3",
      help: "Min drivers guaranteed to run concurrently for a single task.",
    },
    {
      key: "task.min-writer-count",
      type: "number",
      default: "1",
      help: "Concurrent writer threads per worker per query without writer scaling.",
    },
    {
      key: "task.max-writer-count",
      type: "number",
      help: "Concurrent writer threads per worker per query with writer scaling (power of two).",
    },
    {
      key: "task.interrupt-stuck-split-tasks-enabled",
      type: "boolean",
      default: "true",
      help: "Detect and fail tasks with splits stuck in the regex library.",
    },
    {
      key: "task.interrupt-stuck-split-tasks-warning-threshold",
      type: "duration",
      default: "10m",
      help: "Emit call stacks / JMX metrics for splits running longer than this.",
    },
    {
      key: "task.interrupt-stuck-split-tasks-timeout",
      type: "duration",
      default: "10m",
      help: "How long to wait for a blocked split thread before failing the task.",
    },
    {
      key: "task.interrupt-stuck-split-tasks-detection-interval",
      type: "duration",
      default: "2m",
      help: "Interval at which Trino checks for stuck split-processing threads.",
    },
  ]),

  // ── Optimizer ────────────────────────────────────────────────────────────
  ...section("properties-optimizer", "Optimizer", "config", [
    {
      key: "optimizer.dictionary-aggregation",
      type: "boolean",
      default: "false",
      help: "Enable optimization for aggregations on dictionaries.",
    },
    {
      key: "optimizer.optimize-metadata-queries",
      type: "boolean",
      default: "false",
      help: "Answer some aggregations from partition metadata (can be unsafe on empty partitions).",
    },
    {
      key: "optimizer.distinct-aggregations-strategy",
      type: "enum",
      default: "AUTOMATIC",
      enumOptions: [
        "AUTOMATIC",
        "MARK_DISTINCT",
        "SINGLE_STEP",
        "PRE_AGGREGATE",
        "SPLIT_TO_SUBQUERIES",
      ],
      help: "Strategy for computing multiple distinct aggregations.",
    },
    {
      key: "optimizer.push-aggregation-through-outer-join",
      type: "boolean",
      default: "true",
      help: "Push an aggregation below an outer join when grouping covers the outer side.",
    },
    {
      key: "optimizer.push-table-write-through-union",
      type: "boolean",
      default: "true",
      help: "Parallelize writes for UNION ALL write queries.",
    },
    {
      key: "optimizer.push-filter-into-values-max-row-count",
      type: "number",
      default: "100",
      help: "VALUES row count below which a filter is pushed into the VALUES node.",
    },
    {
      key: "optimizer.join-reordering-strategy",
      type: "enum",
      default: "AUTOMATIC",
      enumOptions: ["AUTOMATIC", "ELIMINATE_CROSS_JOINS", "NONE"],
      help: "Cost-based join reordering strategy.",
    },
    {
      key: "optimizer.max-reordered-joins",
      type: "number",
      default: "8",
      help: "Max joins reordered at once under cost-based reordering.",
    },
    {
      key: "optimizer.optimize-duplicate-insensitive-joins",
      type: "boolean",
      default: "true",
      help: "Skip duplicated join output rows when the result is duplicate-insensitive.",
    },
    {
      key: "optimizer.use-exact-partitioning",
      type: "boolean",
      default: "false",
      help: "Re-partition unless upstream partitioning exactly matches the downstream stage.",
    },
    {
      key: "optimizer.use-table-scan-node-partitioning",
      type: "boolean",
      default: "true",
      help: "Use connector-provided table partitioning (e.g. Hive buckets) when scanning.",
    },
    {
      key: "optimizer.table-scan-node-partitioning-min-bucket-to-task-ratio",
      type: "number",
      default: "0.5",
      help: "Min bucket-to-task ratio required to use table-scan node partitioning.",
    },
    {
      key: "optimizer.colocated-joins-enabled",
      type: "boolean",
      default: "true",
      help: "Use co-located joins when both sides share table partitioning on the keys.",
    },
    {
      key: "optimizer.filter-conjunction-independence-factor",
      type: "number",
      default: "0.75",
      help: "Independence assumption strength for conjunction selectivity estimates.",
    },
    {
      key: "optimizer.join-multi-clause-independence-factor",
      type: "number",
      default: "0.25",
      help: "Independence assumption strength for multi-clause join output estimates.",
    },
    {
      key: "optimizer.non-estimatable-predicate-approximation.enabled",
      type: "boolean",
      default: "true",
      help: "Approximate output row count for filters whose cost cannot be estimated.",
    },
    {
      key: "optimizer.join-partitioned-build-min-row-count",
      type: "number",
      default: "1000000",
      help: "Min build-side rows required to use partitioned join lookup.",
    },
    {
      key: "optimizer.min-input-size-per-task",
      type: "size",
      default: "5GB",
      help: "Min input size per task used to size hash partitions.",
    },
    {
      key: "optimizer.min-input-rows-per-task",
      type: "number",
      default: "10000000",
      help: "Min input rows per task used to size hash partitions.",
    },
    {
      key: "optimizer.use-cost-based-partitioning",
      type: "boolean",
      default: "true",
      help: "Use the cost optimizer to decide whether to repartition a partitioned stage.",
    },
  ]),

  // ── Node scheduler ───────────────────────────────────────────────────────
  ...section("properties-node-scheduler", "Node scheduler", "config", [
    {
      key: "node-scheduler.include-coordinator",
      type: "boolean",
      default: "true",
      help: "Run splits on the coordinator too (keep off for large clusters).",
    },
    {
      key: "node-scheduler.max-splits-per-node",
      type: "number",
      default: "256",
      help: "Target total running splits per worker node.",
    },
    {
      key: "node-scheduler.min-pending-splits-per-task",
      type: "number",
      default: "16",
      help: "Min pending splits guaranteed scheduled per task (anti-starvation).",
    },
    {
      key: "node-scheduler.max-adjusted-pending-splits-per-task",
      type: "number",
      default: "2000",
      help: "Upper bound on the dynamically adjusted pending-split queue per task.",
    },
    {
      key: "node-scheduler.max-unacknowledged-splits-per-task",
      type: "number",
      default: "2000",
      help: "Max splits queued on the coordinator but not yet confirmed by the worker.",
    },
    {
      key: "node-scheduler.min-candidates",
      type: "number",
      default: "10",
      help: "Min candidate nodes evaluated when choosing a target for a split.",
    },
    {
      key: "node-scheduler.policy",
      type: "enum",
      default: "uniform",
      enumOptions: ["uniform", "topology"],
      help: "Split scheduling policy (uniform vs topology-aware).",
    },
    {
      key: "node-scheduler.network-topology.segments",
      type: "string",
      default: "machine",
      help: "Comma-separated names of each segment of a network location.",
    },
    {
      key: "node-scheduler.network-topology.type",
      type: "enum",
      default: "flat",
      enumOptions: ["flat", "file", "subnet"],
      help: "Network topology source (requires topology scheduler policy).",
    },
    {
      key: "node-scheduler.network-topology.file",
      type: "string",
      help: "Path to the network-topology file (topology type = file).",
    },
    {
      key: "node-scheduler.network-topology.refresh-period",
      type: "duration",
      default: "5m",
      help: "How often the network-topology file is reloaded.",
    },
    {
      key: "node-scheduler.network-topology.subnet.ip-address-protocol",
      type: "enum",
      default: "IPv4",
      enumOptions: ["IPv4", "IPv6"],
      help: "IP protocol used for subnet-based topology.",
    },
    {
      key: "node-scheduler.network-topology.subnet.cidr-prefix-lengths",
      type: "string",
      help: "Comma-separated increasing CIDR prefix lengths for subnet masks.",
    },
  ]),

  // ── Web UI ───────────────────────────────────────────────────────────────
  ...section("properties-web-interface", "Web UI", "config", [
    {
      key: "web-ui.authentication.type",
      type: "enum",
      default: "FORM",
      enumOptions: ["FORM", "FIXED", "CERTIFICATE", "KERBEROS", "JWT", "OAUTH2"],
      help: "Web UI authentication mechanism.",
    },
    {
      key: "web-ui.enabled",
      type: "boolean",
      default: "true",
      help: "Expose the Trino Web UI.",
    },
    {
      key: "web-ui.shared-secret",
      type: "string",
      help: "Stable secret for the Web-UI auth cookie (survives restarts).",
    },
    {
      key: "web-ui.session-timeout",
      type: "duration",
      default: "1d",
      help: "How long a Web UI login stays valid before auto log-out.",
    },
    {
      key: "web-ui.user",
      type: "string",
      help: "Username used for the FIXED Web UI authentication type.",
    },
  ]),

  // ── Logging ──────────────────────────────────────────────────────────────
  ...section("properties-logging", "Logging", "config", [
    {
      key: "log.annotation-file",
      type: "string",
      help: "Properties file of annotations to include with each JSON/TCP log message.",
    },
    {
      key: "log.format",
      type: "enum",
      default: "TEXT",
      enumOptions: ["TEXT", "JSON"],
      help: "Server log format (JSON for centralized aggregation).",
    },
    {
      key: "log.console-format",
      type: "enum",
      default: "TEXT",
      enumOptions: ["TEXT", "JSON"],
      help: "Format for log records written to the console.",
    },
    {
      key: "log.path",
      type: "string",
      help: "Path to the server log file (or tcp://host:port for TCP output).",
    },
    {
      key: "log.max-size",
      type: "size",
      default: "100MB",
      help: "Max size of the general application log file.",
    },
    {
      key: "log.max-total-size",
      type: "size",
      default: "1GB",
      help: "Max combined size of all general application log files.",
    },
    {
      key: "log.compression",
      type: "enum",
      default: "GZIP",
      enumOptions: ["GZIP", "NONE"],
      help: "Compression format for rotated log files.",
    },
    {
      key: "http-server.log.enabled",
      type: "boolean",
      default: "true",
      help: "Enable HTTP-server request logging.",
    },
    {
      key: "http-server.log.compression.enabled",
      type: "boolean",
      default: "true",
      help: "Compress the HTTP-server request log files.",
    },
    {
      key: "http-server.log.path",
      type: "string",
      default: "var/log/http-request.log",
      help: "Path to the HTTP-server request log file.",
    },
    {
      key: "http-server.log.max-history",
      type: "number",
      default: "15",
      help: "Max number of HTTP-server log files kept before rotation.",
    },
    {
      key: "http-server.log.max-size",
      type: "size",
      help: "Max size of the HTTP-server log file (unlimited by default).",
    },
  ]),

  // ── HTTP server ──────────────────────────────────────────────────────────
  ...section("properties-http-server", "HTTP server", "config", [
    {
      key: "http-server.process-forwarded",
      type: "boolean",
      default: "false",
      help: "Treat forwarded HTTPS-over-HTTP requests as secure (behind a TLS load balancer).",
    },
    {
      key: "http-server.http.port",
      type: "number",
      default: "8080",
      help: "HTTP port for the server.",
    },
    {
      key: "http-server.https.enabled",
      type: "boolean",
      default: "false",
      help: "Enable TLS on the HTTP server.",
    },
    {
      key: "http-server.https.port",
      type: "number",
      default: "8443",
      help: "HTTPS port for the server.",
    },
    {
      key: "http-server.https.keystore.path",
      type: "string",
      help: "Location of the PEM or Java keystore file for TLS.",
    },
    {
      key: "http-server.https.keystore.key",
      type: "string",
      help: "Password for the PEM or Java keystore.",
    },
    {
      key: "http-server.https.truststore.path",
      type: "string",
      help: "Location of an optional PEM/Java truststore for extra CAs.",
    },
    {
      key: "http-server.https.truststore.key",
      type: "string",
      help: "Password for the optional PEM/Java truststore.",
    },
    {
      key: "http-server.https.keymanager.password",
      type: "string",
      help: "Password for a specific key within the keystore.",
    },
    {
      key: "http-server.https.secure-random-algorithm",
      type: "string",
      help: "Algorithm used to generate secure random values for internal comms.",
    },
    {
      key: "http-server.https.ssl-session-timeout",
      type: "duration",
      default: "4h",
      help: "Validity duration of a TLS client session.",
    },
    {
      key: "http-server.https.ssl-session-cache-size",
      type: "number",
      default: "10000",
      help: "Max number of SSL session cache entries.",
    },
    {
      key: "http-server.https.ssl-context.refresh-time",
      type: "duration",
      default: "1m",
      help: "Interval between reloading default certificates.",
    },
    {
      key: "http-server.authentication.type",
      type: "string",
      help: "Ordered list of enabled authentication types.",
    },
    {
      key: "http-server.authentication.allow-insecure-over-http",
      type: "boolean",
      default: "true",
      help: "Allow HTTP when authentication is active (insecure; auto-off with auth).",
    },
    {
      key: "internal-communication.shared-secret",
      type: "string",
      help: "Shared secret coordinators and workers use to authenticate internally.",
    },
    {
      key: "internal-communication.http2.enabled",
      type: "boolean",
      default: "true",
      help: "Use HTTP/2 for internal communication.",
    },
    {
      key: "internal-communication.https.required",
      type: "boolean",
      default: "false",
      help: "Require SSL/TLS for all internal communication.",
    },
  ]),

  // ── Writer scaling ───────────────────────────────────────────────────────
  ...section("properties-writer-scaling", "Writer scaling", "config", [
    {
      key: "scale-writers",
      type: "boolean",
      default: "true",
      help: "Dynamically scale out the number of writer tasks.",
    },
    {
      key: "task.scale-writers.enabled",
      type: "boolean",
      default: "true",
      help: "Scale the number of concurrent writers within a task.",
    },
    {
      key: "task.scale-writers.max-writer-memory-percentage",
      type: "number",
      default: "70",
      help: "Max % of per-node memory writers may use before scaling pauses.",
    },
    {
      key: "writer-scaling-min-data-processed",
      type: "size",
      default: "120MB",
      help: "Data a writer must process before another writer can be added.",
    },
  ]),

  // ── Write partitioning ───────────────────────────────────────────────────
  ...section("properties-write-partitioning", "Write partitioning", "config", [
    {
      key: "use-preferred-write-partitioning",
      type: "boolean",
      default: "true",
      help: "Write each partition with a separate writer (fewer files per partition).",
    },
  ]),

  // ── SQL environment ──────────────────────────────────────────────────────
  ...section("properties-sql-environment", "SQL environment", "config", [
    {
      key: "sql.forced-session-time-zone",
      type: "string",
      help: "Override the session time zone cluster-wide (e.g. UTC).",
    },
    {
      key: "sql.default-catalog",
      type: "string",
      help: "Default catalog for clients that don't set one.",
    },
    {
      key: "sql.default-schema",
      type: "string",
      help: "Default schema for clients that don't set one.",
    },
    {
      key: "sql.default-function-catalog",
      type: "string",
      help: "Default catalog used for UDF storage.",
    },
    {
      key: "sql.default-function-schema",
      type: "string",
      help: "Default schema used for UDF storage.",
    },
    {
      key: "sql.path",
      type: "string",
      help: "Default SQL path of catalog.schema entries for function resolution.",
    },
  ]),

  // ── Catalog management ───────────────────────────────────────────────────
  ...section("properties-catalog", "Catalog", "config", [
    {
      key: "catalog.management",
      type: "enum",
      default: "static",
      enumOptions: ["static", "dynamic"],
      help: "Allow CREATE/DROP CATALOG at runtime (dynamic).",
    },
    {
      key: "catalog.prune.update-interval",
      type: "duration",
      default: "5s",
      help: "Interval for pruning dropped catalogs (dynamic management).",
    },
    {
      key: "catalog.store",
      type: "enum",
      default: "file",
      enumOptions: ["file", "memory"],
      help: "Where dynamically created catalogs are stored (file or memory).",
    },
    {
      key: "catalog.config-dir",
      type: "string",
      default: "etc/catalog/",
      help: "Directory containing catalog property files.",
    },
    {
      key: "catalog.disabled-catalogs",
      type: "string",
      help: "Comma-separated list of catalogs to ignore on startup.",
    },
    {
      key: "catalog.read-only",
      type: "boolean",
      default: "false",
      help: "Prevent CREATE/DROP CATALOG from adding or removing catalog files.",
    },
  ]),

  // ── HTTP client ──────────────────────────────────────────────────────────
  ...section("properties-http-client", "HTTP client", "config", [
    {
      key: "http-client.connect-timeout",
      type: "duration",
      default: "5s",
      help: "Timeout for establishing a connection to an external service.",
    },
    {
      key: "max-content-length",
      type: "size",
      default: "16MB",
      help: "Max content size for each HTTP request and response.",
    },
    {
      key: "http-client.request-timeout",
      type: "duration",
      default: "5m",
      help: "Timeout for the overall HTTP request.",
    },
    {
      key: "http-client.https.excluded-cipher",
      type: "string",
      help: "Comma-separated regexes of cipher algorithm names to exclude.",
    },
    {
      key: "http-client.https.included-cipher",
      type: "string",
      help: "Comma-separated regexes of cipher algorithm names to use.",
    },
    {
      key: "http-client.https.hostname-verification",
      type: "boolean",
      default: "true",
      help: "Verify the server hostname against the certificate SAN.",
    },
    {
      key: "http-client.key-store-password",
      type: "string",
      help: "Password for the HTTP-client keystore.",
    },
    {
      key: "http-client.key-store-path",
      type: "string",
      help: "File path to the HTTP-client keystore.",
    },
    {
      key: "http-client.secure-random-algorithm",
      type: "string",
      help: "Secure random algorithm for the connection (OS-dependent default).",
    },
    {
      key: "http-client.trust-store-password",
      type: "string",
      help: "Password for the HTTP-client truststore.",
    },
    {
      key: "http-client.trust-store-path",
      type: "string",
      help: "File path to the HTTP-client truststore.",
    },
    {
      key: "http-client.http-proxy",
      type: "string",
      help: "Host and port of an HTTP proxy (host:port).",
    },
    {
      key: "http-client.http-proxy.user",
      type: "string",
      help: "Username for basic auth with the HTTP proxy.",
    },
    {
      key: "http-client.http-proxy.password",
      type: "string",
      help: "Password for basic auth with the HTTP proxy.",
    },
    {
      key: "http-client.http-proxy.secure",
      type: "boolean",
      default: "false",
      help: "Enable HTTPS for the proxy.",
    },
    {
      key: "http-client.socks-proxy",
      type: "string",
      help: "Host and port of a SOCKS proxy.",
    },
    {
      key: "http-client.log.compression.enabled",
      type: "boolean",
      default: "true",
      help: "Compress HTTP-client log files (.gz).",
    },
    {
      key: "http-client.log.enabled",
      type: "boolean",
      default: "false",
      help: "Enable logging of HTTP-client requests.",
    },
    {
      key: "http-client.log.flush-interval",
      type: "duration",
      default: "10s",
      help: "How often HTTP-client log data is flushed to disk.",
    },
    {
      key: "http-client.log.max-history",
      type: "number",
      default: "15",
      help: "Retention (days) for HTTP-client log files.",
    },
    {
      key: "http-client.log.max-size",
      type: "size",
      default: "1GB",
      help: "Max total size of all HTTP-client log files on disk.",
    },
    {
      key: "http-client.log.path",
      type: "string",
      default: "var/log/",
      help: "Directory for HTTP-client log files.",
    },
    {
      key: "http-client.log.queue-size",
      type: "number",
      default: "10000",
      help: "Size of the HTTP-client logging queue.",
    },
  ]),

  // ── Regexp functions ─────────────────────────────────────────────────────
  ...section("properties-regexp-function", "Regexp functions", "config", [
    {
      key: "regex-library",
      type: "enum",
      default: "JONI",
      enumOptions: ["JONI", "RE2J"],
      help: "Library used for regular-expression functions.",
    },
    {
      key: "re2j.dfa-states-limit",
      type: "number",
      default: "2147483647",
      help: "Max DFA states RE2J builds before falling back to NFA.",
    },
    {
      key: "re2j.dfa-retries",
      type: "number",
      default: "5",
      help: "Times RE2J retries the DFA algorithm before switching to NFA.",
    },
  ]),

  // ── Fault-tolerant execution ─────────────────────────────────────────────
  ...section("fault-tolerant-execution", "Fault-tolerant", "config", [
    {
      key: "retry-policy.allowed",
      type: "string",
      help: "List of retry policies that may be configured on this cluster.",
    },
    {
      key: "fault-tolerant-execution.exchange-encryption-enabled",
      type: "boolean",
      default: "true",
      help: "Encrypt spooled fault-tolerant exchange data.",
    },
    {
      key: "query-retry-attempts",
      type: "number",
      default: "4",
      help: "Max query retries under retry-policy=QUERY.",
    },
    {
      key: "task-retry-attempts-per-task",
      type: "number",
      default: "4",
      help: "Max task retries under retry-policy=TASK.",
    },
    {
      key: "retry-initial-delay",
      type: "duration",
      default: "10s",
      help: "Minimum wait before a failed query or task is retried.",
    },
    {
      key: "retry-max-delay",
      type: "duration",
      default: "1m",
      help: "Maximum wait before a failed query or task is retried.",
    },
    {
      key: "retry-delay-scale-factor",
      type: "number",
      default: "2.0",
      help: "Factor by which retry delay grows on each failure.",
    },
    {
      key: "fault-tolerant-execution-standard-split-size",
      type: "size",
      default: "64MB",
      help: "Standard split data size processed by source-reading tasks.",
    },
    {
      key: "fault-tolerant-execution-max-task-split-count",
      type: "number",
      default: "2048",
      help: "Max splits processed by a single task (not weight-adjusted).",
    },
    {
      key: "fault-tolerant-execution-arbitrary-distribution-compute-task-target-size-growth-period",
      type: "number",
      default: "64",
      help: "Tasks per arbitrary-distribution compute stage before target size grows.",
    },
    {
      key: "fault-tolerant-execution-arbitrary-distribution-compute-task-target-size-growth-factor",
      type: "number",
      default: "1.26",
      help: "Growth factor for arbitrary-distribution compute-task target size.",
    },
    {
      key: "fault-tolerant-execution-arbitrary-distribution-compute-task-target-size-min",
      type: "size",
      default: "512MB",
      help: "Initial/min target input size for arbitrary-distribution compute tasks.",
    },
    {
      key: "fault-tolerant-execution-arbitrary-distribution-compute-task-target-size-max",
      type: "size",
      default: "50GB",
      help: "Max target input size for arbitrary-distribution compute tasks.",
    },
    {
      key: "fault-tolerant-execution-arbitrary-distribution-write-task-target-size-growth-period",
      type: "number",
      default: "64",
      help: "Tasks per arbitrary-distribution writer stage before target size grows.",
    },
    {
      key: "fault-tolerant-execution-arbitrary-distribution-write-task-target-size-growth-factor",
      type: "number",
      default: "1.26",
      help: "Growth factor for arbitrary-distribution writer-task target size.",
    },
    {
      key: "fault-tolerant-execution-arbitrary-distribution-write-task-target-size-min",
      type: "size",
      default: "4GB",
      help: "Initial/min target input size for arbitrary-distribution writer tasks.",
    },
    {
      key: "fault-tolerant-execution-arbitrary-distribution-write-task-target-size-max",
      type: "size",
      default: "50GB",
      help: "Max target input size for arbitrary-distribution writer tasks.",
    },
    {
      key: "fault-tolerant-execution-hash-distribution-compute-task-target-size",
      type: "size",
      default: "512MB",
      help: "Target input size for hash-distribution compute tasks.",
    },
    {
      key: "fault-tolerant-execution-hash-distribution-write-task-target-size",
      type: "size",
      default: "4GB",
      help: "Target input size for hash-distribution writer tasks.",
    },
    {
      key: "fault-tolerant-execution-hash-distribution-write-task-target-max-count",
      type: "number",
      default: "2000",
      help: "Soft upper bound on writer tasks in a hash-distribution stage.",
    },
    {
      key: "fault-tolerant-execution-task-memory",
      type: "size",
      default: "5GB",
      help: "Initial task memory estimate for node bin-packing (TASK policy).",
    },
    {
      key: "fault-tolerant-execution-task-descriptor-storage-max-memory",
      type: "size",
      help: "Max memory for storing task descriptors of fault-tolerant queries.",
    },
    {
      key: "fault-tolerant-execution-max-partition-count",
      type: "number",
      default: "50",
      help: "Max partitions for distributed joins/aggregations (TASK policy).",
    },
    {
      key: "fault-tolerant-execution-min-partition-count",
      type: "number",
      default: "4",
      help: "Min partitions for distributed joins/aggregations (TASK policy).",
    },
    {
      key: "fault-tolerant-execution-min-partition-count-for-write",
      type: "number",
      default: "50",
      help: "Min partitions for distributed operations in write queries (TASK policy).",
    },
    {
      key: "max-tasks-waiting-for-node-per-query",
      type: "number",
      default: "50",
      help: "Tasks allowed to wait for node allocation per query before pausing.",
    },
  ]),

  // ── Node identity (node.properties) ──────────────────────────────────────
  ...section("../installation/deployment", "Node", "node", [
    {
      key: "node.environment",
      type: "string",
      help: "Name of the environment; all nodes in a cluster must share it.",
      docsUrl:
        "https://trino.io/docs/current/installation/deployment.html#node-properties",
    },
    {
      key: "node.id",
      type: "string",
      help: "Unique identifier for this node (stable across restarts).",
      docsUrl:
        "https://trino.io/docs/current/installation/deployment.html#node-properties",
    },
    {
      key: "node.data-dir",
      type: "string",
      help: "Filesystem path Trino uses for data and logs.",
      docsUrl:
        "https://trino.io/docs/current/installation/deployment.html#node-properties",
    },
    {
      key: "node.internal-address",
      type: "string",
      help: "Address other nodes use to reach this node for internal communication.",
      docsUrl:
        "https://trino.io/docs/current/installation/deployment.html#node-properties",
    },
    {
      key: "node.internal-address-source",
      type: "enum",
      enumOptions: ["IP", "FQDN"],
      help: "How the node's internal address is derived (IP or FQDN).",
      docsUrl:
        "https://trino.io/docs/current/installation/deployment.html#node-properties",
    },
  ]),

  // ── Per-logger levels (log.properties) ───────────────────────────────────
  // `log.properties` maps a Java package/logger name to a level. Logger names
  // are open-ended (any package works via the "add custom key" path); these are
  // the common Trino roots operators reach for. The value is a level enum.
  ...section("../installation/deployment", "Logging levels", "log", [
    {
      key: "io.trino",
      type: "enum",
      enumOptions: ["DEBUG", "INFO", "WARN", "ERROR"],
      help: "Root Trino logger level (affects most server logging).",
      docsUrl:
        "https://trino.io/docs/current/installation/deployment.html#log-levels",
    },
    {
      key: "io.trino.execution",
      type: "enum",
      enumOptions: ["DEBUG", "INFO", "WARN", "ERROR"],
      help: "Query scheduling and execution engine logging.",
      docsUrl:
        "https://trino.io/docs/current/installation/deployment.html#log-levels",
    },
    {
      key: "io.trino.server",
      type: "enum",
      enumOptions: ["DEBUG", "INFO", "WARN", "ERROR"],
      help: "HTTP server / coordinator startup and request logging.",
      docsUrl:
        "https://trino.io/docs/current/installation/deployment.html#log-levels",
    },
    {
      key: "io.trino.security",
      type: "enum",
      enumOptions: ["DEBUG", "INFO", "WARN", "ERROR"],
      help: "Authentication, authorization and access-control logging.",
      docsUrl:
        "https://trino.io/docs/current/installation/deployment.html#log-levels",
    },
    {
      key: "io.trino.event",
      type: "enum",
      enumOptions: ["DEBUG", "INFO", "WARN", "ERROR"],
      help: "Query-completion event / event-listener logging.",
      docsUrl:
        "https://trino.io/docs/current/installation/deployment.html#log-levels",
    },
    {
      key: "io.trino.plugin.hive",
      type: "enum",
      enumOptions: ["DEBUG", "INFO", "WARN", "ERROR"],
      help: "Hive connector logging.",
      docsUrl:
        "https://trino.io/docs/current/installation/deployment.html#log-levels",
    },
    {
      key: "io.trino.plugin.iceberg",
      type: "enum",
      enumOptions: ["DEBUG", "INFO", "WARN", "ERROR"],
      help: "Iceberg connector logging.",
      docsUrl:
        "https://trino.io/docs/current/installation/deployment.html#log-levels",
    },
    {
      key: "io.trino.plugin.jdbc",
      type: "enum",
      enumOptions: ["DEBUG", "INFO", "WARN", "ERROR"],
      help: "Shared JDBC connector logging (PostgreSQL, MySQL, …).",
      docsUrl:
        "https://trino.io/docs/current/installation/deployment.html#log-levels",
    },
  ]),
];

export const trinoCatalogByKey: Record<string, TrinoProp> = Object.fromEntries(
  trinoCatalog.map((p) => [p.key, p]),
);
