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
  "coordinator.heap.minSize": {
    help: "Fixed JVM initial heap (-Xms) for the coordinator. Usually set equal to the max size.",
    docsUrl:
      "https://trino.io/docs/current/installation/deployment.html#jvm-config",
    unit: "size",
  },
  "coordinator.heap.maxPercent": {
    help: "JVM MaxRAMPercentage — cap the max heap at this percent of the pod memory limit.",
    docsUrl:
      "https://trino.io/docs/current/installation/deployment.html#jvm-config",
    unit: "count",
  },
  "coordinator.heap.initialPercent": {
    help: "JVM InitialRAMPercentage — size the initial heap at this percent of the pod memory limit.",
    docsUrl:
      "https://trino.io/docs/current/installation/deployment.html#jvm-config",
    unit: "count",
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
  "worker.heap.minSize": {
    help: "Fixed JVM initial heap (-Xms) for each worker. Usually set equal to the max size.",
    docsUrl:
      "https://trino.io/docs/current/installation/deployment.html#jvm-config",
    unit: "size",
  },
  "worker.heap.maxPercent": {
    help: "JVM MaxRAMPercentage — cap the max heap at this percent of the pod memory limit.",
    docsUrl:
      "https://trino.io/docs/current/installation/deployment.html#jvm-config",
    unit: "count",
  },
  "worker.heap.initialPercent": {
    help: "JVM InitialRAMPercentage — size the initial heap at this percent of the pod memory limit.",
    docsUrl:
      "https://trino.io/docs/current/installation/deployment.html#jvm-config",
    unit: "count",
  },
  "worker.gracefulShutdown": {
    help: "Drain the worker before termination so in-flight queries finish instead of failing on scale-down.",
    docsUrl: "https://trino.io/docs/current/admin/graceful-shutdown.html",
    unit: "duration",
  },
  "worker.gracePeriod": {
    help: "Seconds to wait for in-flight queries to drain before the worker pod is terminated.",
    docsUrl: "https://trino.io/docs/current/admin/graceful-shutdown.html",
    unit: "count",
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

  // --- Probes (shared across liveness/readiness/startup) ---
  "probe.initialDelaySeconds": {
    help: "Seconds after the container starts before the probe is first run.",
    docsUrl:
      "https://kubernetes.io/docs/concepts/workloads/pods/pod-lifecycle/#configuring-probes",
    unit: "count",
  },
  "probe.periodSeconds": {
    help: "How often (in seconds) the probe is run.",
    docsUrl:
      "https://kubernetes.io/docs/concepts/workloads/pods/pod-lifecycle/#configuring-probes",
    unit: "count",
  },
  "probe.timeoutSeconds": {
    help: "Seconds after which the probe times out and counts as a failure.",
    docsUrl:
      "https://kubernetes.io/docs/concepts/workloads/pods/pod-lifecycle/#configuring-probes",
    unit: "count",
  },
  "probe.failureThreshold": {
    help: "Consecutive failures before the container is considered unhealthy and restarted/unready.",
    docsUrl:
      "https://kubernetes.io/docs/concepts/workloads/pods/pod-lifecycle/#configuring-probes",
    unit: "count",
  },
  "probe.successThreshold": {
    help: "Consecutive successes required to mark the probe healthy after a failure.",
    docsUrl:
      "https://kubernetes.io/docs/concepts/workloads/pods/pod-lifecycle/#configuring-probes",
    unit: "count",
  },

  // --- General ---
  "logLevel": {
    help: "Root logger level applied to the Trino server (INFO, DEBUG, WARN, ERROR).",
    docsUrl:
      "https://trino.io/docs/current/installation/deployment.html#log-levels",
  },

  // --- Reliability ---
  "faultTolerantEnabled": {
    help: "Turn on fault-tolerant execution so failed tasks/queries retry, spooling exchange data to S3 storage.",
    docsUrl:
      "https://trino.io/docs/current/admin/fault-tolerant-execution.html",
  },
  "exchangeStorage": {
    help: "The S3-compatible storage connection used to spool exchange data between stages.",
    docsUrl:
      "https://trino.io/docs/current/admin/fault-tolerant-execution.html#exchange-manager",
  },
  "exchangePrefix": {
    help: "Key prefix (directory) within the exchange storage bucket where spooled data is written.",
    docsUrl:
      "https://trino.io/docs/current/admin/fault-tolerant-execution.html#exchange-manager",
  },

  // --- Security ---
  "authenticationType": {
    help: "How clients authenticate to the coordinator (e.g. PASSWORD, OAUTH2, JWT, CERTIFICATE, KERBEROS).",
    docsUrl:
      "https://trino.io/docs/current/security/authentication-types.html",
  },
  "passwordUsers": {
    help: "Username + egret-secret password pairs authenticated via Trino's file-based password authenticator.",
    docsUrl:
      "https://trino.io/docs/current/security/password-file.html",
  },
  "jwtEnabled": {
    help: "Also accept egret-issued JWTs (e.g. from notebooks) as client credentials via JWT authentication.",
    docsUrl: "https://trino.io/docs/current/security/jwt.html",
  },
  "jwksUri": {
    help: "URL of the JWKS endpoint whose public keys verify incoming JWT signatures.",
    docsUrl: "https://trino.io/docs/current/security/jwt.html#jwks",
  },
  "requiredAudience": {
    help: "If set, only JWTs whose audience (aud) claim matches this value are accepted.",
    docsUrl: "https://trino.io/docs/current/security/jwt.html",
  },
  "ingressEnabled": {
    help: "Create a Kubernetes Ingress for the coordinator so clients reach it outside the cluster.",
    docsUrl: "https://kubernetes.io/docs/concepts/services-networking/ingress/",
  },
  "ingress.host": {
    help: "Hostname routed to the coordinator by the Ingress (e.g. trino.example.com).",
    docsUrl: "https://kubernetes.io/docs/concepts/services-networking/ingress/",
  },
  "ingress.className": {
    help: "IngressClass that selects which ingress controller serves this Ingress (e.g. nginx).",
    docsUrl:
      "https://kubernetes.io/docs/concepts/services-networking/ingress/#ingress-class",
  },
  "ingress.tlsSecret": {
    help: "Kubernetes TLS secret used to terminate HTTPS at the ingress for the host.",
    docsUrl:
      "https://kubernetes.io/docs/concepts/services-networking/ingress/#tls",
  },
};
