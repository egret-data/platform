/**
 * Per-field help metadata for the engine Configuration UI.
 *
 * Keys are stable strings referenced by `<FieldLabel fieldKey="..." />` and
 * `<QuantityInput fieldKey="..." />`. `help` is a one-line explanation, `docsUrl`
 * links to the canonical Trino docs, and `unit` hints the expected value kind
 * ("size" = k8s/JVM byte quantity, "cpu" = k8s CPU quantity, "count" = integer,
 * "duration" = Go/Trino duration string).
 */
export interface FieldHelp {
  help: string;
  docsUrl?: string;
  unit?: string;
}

export const fieldHelp: Record<string, FieldHelp> = {
  // --- Coordinator ---
  "coordinator.replicas": {
    help: "Number of coordinator pods. Trino supports a single active coordinator; more than one requires fault-tolerant / HA setup.",
    docsUrl:
      "https://trino.io/docs/current/overview/concepts.html#coordinator",
    unit: "count",
  },
  "coordinator.heap.maxSize": {
    help: "Fixed JVM max heap (-Xmx) for the coordinator. Leave blank to derive a percentage of the memory limit.",
    docsUrl:
      "https://trino.io/docs/current/installation/deployment.html#jvm-config",
    unit: "size",
  },
  "coordinator.resources.cpu": {
    help: "Kubernetes CPU request/limit for each coordinator pod.",
    docsUrl:
      "https://kubernetes.io/docs/concepts/configuration/manage-resources-containers/",
    unit: "cpu",
  },
  "coordinator.resources.memory": {
    help: "Kubernetes memory request/limit for each coordinator pod. The JVM heap must fit inside this with headroom.",
    docsUrl:
      "https://trino.io/docs/current/admin/properties-resource-management.html",
    unit: "size",
  },

  // --- Worker ---
  "worker.replicas": {
    help: "Number of worker pods. More workers add parallelism and cluster memory. Autoscaling can override this.",
    docsUrl: "https://trino.io/docs/current/overview/concepts.html#worker",
    unit: "count",
  },
  "worker.heap.maxSize": {
    help: "Fixed JVM max heap (-Xmx) for each worker. Leave blank to derive a percentage of the memory limit.",
    docsUrl:
      "https://trino.io/docs/current/installation/deployment.html#jvm-config",
    unit: "size",
  },
  "worker.resources.cpu": {
    help: "Kubernetes CPU request/limit for each worker pod.",
    docsUrl:
      "https://kubernetes.io/docs/concepts/configuration/manage-resources-containers/",
    unit: "cpu",
  },
  "worker.resources.memory": {
    help: "Kubernetes memory request/limit for each worker pod. Query memory settings must fit inside this.",
    docsUrl:
      "https://trino.io/docs/current/admin/properties-resource-management.html",
    unit: "size",
  },
  "worker.gracefulShutdown": {
    help: "Drain the worker before termination so in-flight queries finish instead of failing on scale-down.",
    docsUrl: "https://trino.io/docs/current/admin/graceful-shutdown.html",
    unit: "duration",
  },

  // --- Workload / resource management ---
  "queryMaxMemory": {
    help: "Cluster-wide user memory a single query may use summed across all workers.",
    docsUrl:
      "https://trino.io/docs/current/admin/properties-resource-management.html#query-max-memory",
    unit: "size",
  },
  "queryMaxMemoryPerNode": {
    help: "Maximum user memory a single query may use on any one worker.",
    docsUrl:
      "https://trino.io/docs/current/admin/properties-resource-management.html#query-max-memory-per-node",
    unit: "size",
  },
  "queryMaxTotalMemory": {
    help: "Maximum total (user + system) memory a query may use across the cluster.",
    docsUrl:
      "https://trino.io/docs/current/admin/properties-resource-management.html#query-max-total-memory",
    unit: "size",
  },
  "memoryHeapHeadroomPerNode": {
    help: "Heap reserved for allocations not tracked by the memory manager. Increase if workers OOM under load.",
    docsUrl:
      "https://trino.io/docs/current/admin/properties-resource-management.html#memory-heap-headroom-per-node",
    unit: "size",
  },
  "spillEnabled": {
    help: "Spill intermediate query state to disk when memory limits are hit, trading speed for the ability to finish large queries.",
    docsUrl:
      "https://trino.io/docs/current/admin/properties-spilling.html#spill-enabled",
  },

  // --- Reliability ---
  "retryPolicy": {
    help: "Fault-tolerant execution mode: NONE, QUERY (retry whole query) or TASK (retry individual tasks; needs an exchange manager).",
    docsUrl:
      "https://trino.io/docs/current/admin/fault-tolerant-execution.html#retry-policy",
  },
  "queryMaxExecutionTime": {
    help: "Hard cap on active query execution time before the query is failed.",
    docsUrl:
      "https://trino.io/docs/current/admin/properties-query-management.html#query-max-execution-time",
    unit: "duration",
  },
  "queryMaxRunTime": {
    help: "Maximum wall-clock time a query may run including queued time before it is failed.",
    docsUrl:
      "https://trino.io/docs/current/admin/properties-query-management.html#query-max-run-time",
    unit: "duration",
  },

  // --- Autoscaling ---
  "autoscaling.enabled": {
    help: "Scale worker pods automatically between the min and max replica bounds based on the configured metrics.",
    docsUrl:
      "https://kubernetes.io/docs/tasks/run-application/horizontal-pod-autoscale/",
  },
  "autoscaling.minReplicas": {
    help: "Lower bound the autoscaler will never scale worker pods below.",
    docsUrl:
      "https://kubernetes.io/docs/tasks/run-application/horizontal-pod-autoscale/",
    unit: "count",
  },
  "autoscaling.maxReplicas": {
    help: "Upper bound the autoscaler will never scale worker pods above. Caps cluster cost and capacity.",
    docsUrl:
      "https://kubernetes.io/docs/tasks/run-application/horizontal-pod-autoscale/",
    unit: "count",
  },

  // --- Pod / scheduling ---
  "terminationGraceSeconds": {
    help: "Time Kubernetes waits after SIGTERM before force-killing a pod. Give workers enough to drain on shutdown.",
    docsUrl:
      "https://kubernetes.io/docs/concepts/workloads/pods/pod-lifecycle/#pod-termination",
    unit: "count",
  },

  // --- Security ---
  "authenticationType": {
    help: "How clients authenticate to the coordinator (e.g. PASSWORD, OAUTH2, JWT, CERTIFICATE, KERBEROS).",
    docsUrl:
      "https://trino.io/docs/current/security/authentication-types.html",
  },
};
