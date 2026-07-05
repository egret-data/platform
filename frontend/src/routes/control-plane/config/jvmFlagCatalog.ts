export interface JvmFlag {
  /** The flag text to insert (a template; value-taking flags end with `=`). */
  flag: string;
  group: string;
  help: string;
  docsUrl?: string;
}

const JVM_CONFIG_DOCS =
  "https://trino.io/docs/current/installation/deployment.html#jvm-config";

export const jvmFlagCatalog: JvmFlag[] = [
  {
    flag: "-XX:+UseG1GC",
    group: "GC",
    help: "Use the G1 garbage collector (the default and recommended collector for Trino).",
    docsUrl: JVM_CONFIG_DOCS,
  },
  {
    flag: "-XX:G1HeapRegionSize=",
    group: "GC",
    help: "Set the size of a G1 heap region; larger regions reduce fragmentation for large heaps.",
    docsUrl: JVM_CONFIG_DOCS,
  },
  {
    flag: "-XX:+ExplicitGCInvokesConcurrent",
    group: "GC",
    help: "Run explicit System.gc() calls concurrently instead of triggering a full stop-the-world GC.",
    docsUrl: JVM_CONFIG_DOCS,
  },
  {
    flag: "-XX:+UseStringDeduplication",
    group: "GC",
    help: "Let G1 deduplicate identical String backing arrays to reduce heap usage.",
    docsUrl: JVM_CONFIG_DOCS,
  },
  {
    flag: "-verbose:gc",
    group: "GC",
    help: "Print basic garbage collection events to the JVM output.",
    docsUrl: JVM_CONFIG_DOCS,
  },
  {
    flag: "-Xlog:gc*",
    group: "GC",
    help: "Enable detailed unified GC logging (all GC-related log tags at info level).",
    docsUrl: JVM_CONFIG_DOCS,
  },
  {
    flag: "-XX:+HeapDumpOnOutOfMemoryError",
    group: "Diagnostics",
    help: "Write a heap dump to disk automatically when the JVM throws an OutOfMemoryError.",
    docsUrl: JVM_CONFIG_DOCS,
  },
  {
    flag: "-XX:+ExitOnOutOfMemoryError",
    group: "Diagnostics",
    help: "Terminate the JVM immediately on OutOfMemoryError instead of continuing in a degraded state.",
    docsUrl: JVM_CONFIG_DOCS,
  },
  {
    flag: "-XX:-OmitStackTraceInFastThrow",
    group: "Diagnostics",
    help: "Keep full stack traces on common hot exceptions instead of the JIT's abbreviated fast-throw form.",
    docsUrl: JVM_CONFIG_DOCS,
  },
  {
    flag: "-XX:MaxRAMPercentage=",
    group: "Memory",
    help: "Cap the maximum JVM heap as a percentage of the container's available memory.",
    docsUrl: JVM_CONFIG_DOCS,
  },
  {
    flag: "-XX:InitialRAMPercentage=",
    group: "Memory",
    help: "Set the initial JVM heap size as a percentage of the container's available memory.",
    docsUrl: JVM_CONFIG_DOCS,
  },
  {
    flag: "-XX:ReservedCodeCacheSize=",
    group: "Memory",
    help: "Set the maximum size of the JIT compiler's reserved code cache.",
    docsUrl: JVM_CONFIG_DOCS,
  },
  {
    flag: "-Djdk.attach.allowAttachSelf=true",
    group: "Runtime",
    help: "Allow the JVM to attach diagnostic tools (e.g. async-profiler, jcmd) to itself at runtime.",
    docsUrl: JVM_CONFIG_DOCS,
  },
  {
    flag: "-Dfile.encoding=UTF-8",
    group: "Runtime",
    help: "Force UTF-8 as the default charset for file and string encoding.",
    docsUrl: JVM_CONFIG_DOCS,
  },
  {
    flag: "-XX:PerMethodRecompilationCutoff=",
    group: "Runtime",
    help: "Set the number of times a single method may be JIT-recompiled before compilation is capped.",
    docsUrl: JVM_CONFIG_DOCS,
  },
  {
    flag: "-XX:PerBytecodeRecompilationCutoff=",
    group: "Runtime",
    help: "Set the number of times a single bytecode location may trigger recompilation before it is capped.",
    docsUrl: JVM_CONFIG_DOCS,
  },
];
