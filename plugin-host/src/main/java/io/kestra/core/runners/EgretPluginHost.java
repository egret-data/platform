package io.kestra.core.runners;

import java.io.FileDescriptor;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.io.PrintStream;
import java.net.URI;
import java.net.URISyntaxException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.ArrayList;
import java.util.HexFormat;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.TreeMap;
import java.util.zip.ZipEntry;
import java.util.zip.ZipFile;

import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import com.fasterxml.jackson.core.type.TypeReference;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.SerializationFeature;

import io.kestra.core.docs.JsonSchemaGenerator;
import io.kestra.core.models.tasks.Output;
import io.kestra.core.models.tasks.RunnableTask;
import io.kestra.core.models.tasks.Task;
import io.kestra.core.models.triggers.AbstractTrigger;
import io.kestra.core.plugins.DefaultPluginRegistry;
import io.kestra.core.serializers.JacksonMapper;
import jakarta.validation.ConstraintViolationException;

/**
 * egret's Kestra plugin host (W5b spec §3). One JVM runs one task:
 *
 * <pre>
 * java -Xmx&lt;heap&gt; -cp '/opt/egret-plugin-host/lib/*:/opt/egret-plugin-host/plugins/*' \
 *      io.kestra.core.runners.EgretPluginHost run &lt;path-to-task.json&gt;
 * task.json = {"type": "&lt;FQCN without kestra.&gt;", "properties": {…rendered…}, "mask": ["…"],
 *              "storage": {"&lt;kestra:// uri&gt;": ".egret/storage/&lt;n&gt;"}, "baseUri": "kestra:///…/",
 *              "maxTaskBytes": n, "context": {"namespace", "flowId", "executionId", "taskId", "taskRunId"}}
 *             (the last four: W9a spec §5; a run with no internal storage has no baseUri)
 * stdout    = "&lt;LEVEL&gt; &lt;message&gt;" lines; one "::{"outputs": {…}}::" on success;
 *             one "::{"egretFailure": {"code": n, "message": "…"}}::" before a non-zero exit
 * exit      = 0 ok | 2 refused/invalid | 3 plugin threw | 4 needs what egret does not provide
 *             (namespace files W9b, KV W9c, Kestra's secret key or services)
 * SIGTERM   = the shutdown hook calls task.kill(), waits for run() up to EGRET_KILL_WAIT_MS, exits
 *
 * java … EgretPluginHost describe &lt;plugins-dir&gt;   -&gt; catalog.json on stdout
 * </pre>
 *
 * <p><b>Masking</b> (W5b fix round 1, I1): {@code run} replaces stdout and stderr with
 * {@link MaskingOutputStream}s before it reads {@code task.json}, and {@code mask} lists the secret
 * values egret resolved into the properties. Every line either stream carries -- the plugin's log, a
 * stack trace, the {@code egretFailure} marker -- has each of them replaced by {@code ******} before
 * it leaves the JVM; the failure message is masked before it is JSON-encoded. The outputs marker is
 * the one line left as it is ({@link #outputsLine}).
 */
public final class EgretPluginHost {
    static final String KESTRA_VERSION = "2.0.0-rc13";
    static final int EXIT_OK = 0;
    static final int EXIT_REFUSED = 2;
    static final int EXIT_PLUGIN_THREW = 3;
    static final int EXIT_NEEDS_W9 = 4;
    static final int MAX_FAILURE_BYTES = 4096;
    /** grace 15 s (workflow.plugin_host.kill_grace_s) − 2 s, when EGRET_KILL_WAIT_MS is unset. */
    static final long DEFAULT_KILL_WAIT_MS = 13_000;

    /** The JDBC plugins' pool switch, forced off by {@link #load}. */
    static final String CONNECTION_POOLING = "connectionPooling";

    private static final Logger HOST_LOG = LoggerFactory.getLogger("egret.host");
    private static final ObjectMapper MAPPER = JacksonMapper.ofJson();
    private static final TypeReference<Map<String, Object>> MAP = new TypeReference<>() {};

    /**
     * Task classes egret ports natively ({@code spec::registry::TASK_TYPES}, canonical types and
     * aliases that are FQCNs). They never enter the catalog, so a native type never runs in the JVM.
     * Task 3's test checks this list against the Rust table.
     */
    static final Set<String> NATIVE_TYPES = Set.of(
        "io.kestra.plugin.scripts.shell.Commands",
        "io.kestra.plugin.scripts.shell.Script",
        "io.kestra.plugin.scripts.python.Commands",
        "io.kestra.plugin.scripts.python.Script",
        "io.kestra.plugin.core.log.Log",
        "io.kestra.plugin.core.debug.Return",
        "io.kestra.plugin.core.execution.Fail",
        "io.kestra.plugin.core.flow.Sleep"
    );

    /**
     * Task classes kept out of the catalog because they cannot run without something egret does not
     * provide, with the reason. <b>Empty since W9a</b>: W5b excluded {@code postgresql.Batch},
     * {@code mysql.Batch}, {@code postgresql.CopyIn} and {@code postgresql.CopyOut} for needing internal
     * storage; {@link EgretStorage} provides it, and W9a Task 1 ran each one's storage part on the
     * artifact ({@code oracle-exec/w9a-storage.json}, {@code plugin_storage.still_refused_values} is {}).
     */
    static final Map<String, String> NEEDS_STORAGE = Map.of();

    private EgretPluginHost() {
    }

    public static void main(String[] args) {
        if (args.length == 2 && args[0].equals("run")) {
            MaskingOutputStream stdout = new MaskingOutputStream(new FileOutputStream(FileDescriptor.out));
            MaskingOutputStream stderr = new MaskingOutputStream(new FileOutputStream(FileDescriptor.err));
            System.setOut(maskingPrintStream(stdout));
            System.setErr(maskingPrintStream(stderr));
            PrintStream outputs = maskingPrintStream(stdout.unmasked());
            int code = execute(Path.of(args[1]), System.out, outputs, killWaitMs());
            finish(stdout);
            finish(stderr);
            System.exit(code);
        } else if (args.length == 2 && args[0].equals("describe")) {
            try {
                Map<String, Object> catalog = describe(Path.of(args[1]));
                System.out.println(catalogMapper().writeValueAsString(catalog));
                System.out.flush();
                System.exit(EXIT_OK);
            } catch (Exception e) {
                System.err.println("describe failed: " + e);
                e.printStackTrace(System.err);
                System.exit(1);
            }
        } else {
            System.err.println("usage: EgretPluginHost run <task.json> | describe <plugins-dir>");
            System.exit(EXIT_REFUSED);
        }
    }

    /** A UTF-8 autoflushing {@code PrintStream} over {@code sink} (a {@link MaskingOutputStream}, or its unmasked side). */
    static PrintStream maskingPrintStream(OutputStream sink) {
        return new PrintStream(sink, true, StandardCharsets.UTF_8);
    }

    private static void finish(MaskingOutputStream stream) {
        try {
            stream.finish();
        } catch (IOException e) {
            // Nothing is left to report it on.
        }
    }

    private static long killWaitMs() {
        String v = System.getenv("EGRET_KILL_WAIT_MS");
        if (v == null || v.isBlank()) {
            return DEFAULT_KILL_WAIT_MS;
        }
        try {
            return Long.parseLong(v.trim());
        } catch (NumberFormatException e) {
            HOST_LOG.warn("EGRET_KILL_WAIT_MS={} is not a number; using {} ms", v, DEFAULT_KILL_WAIT_MS);
            return DEFAULT_KILL_WAIT_MS;
        }
    }

    // --- run ---------------------------------------------------------------------------

    /** Test seam: runs {@code task.json} with no shutdown hook and never calls {@code System.exit}. */
    static int execute(Path taskJson, PrintStream out) {
        return execute(taskJson, out, out, null);
    }

    /** {@code outputs} carries the success marker only; everything else goes to {@code out}. */
    private static int execute(Path taskJson, PrintStream out, PrintStream outputs, Long killWaitMs) {
        Loaded loaded;
        try {
            loaded = loadAll(taskJson);
        } catch (Refused e) {
            return fail(out, EXIT_REFUSED, e.getMessage(), null);
        }
        Task task = loaded.task();

        // The plugin's temp files go to $WORKING_DIR/.egret/tmp, which egret creates world-writable
        // (W9a Task 9, measured live: `docker cp` makes $WORKING_DIR itself root-owned and the image
        // runs as USER 65532, so Kestra's LocalWorkingDir could not create its directory there and
        // every STORE failed with NoSuchFileException on its temp file). Under $WORKING_DIR, scratch
        // stays bounded by the working directory's emptyDir sizeLimit on Kubernetes (fix round 1).
        String base = System.getenv("WORKING_DIR");
        Path workingDirBase = base == null || base.isBlank()
            ? Path.of(System.getProperty("java.io.tmpdir"))
            : Path.of(base);
        Path scratch = scratchUnder(workingDirBase);
        ShimRunContext runContext = new ShimRunContext(scratch, workingDirBase, task, loaded.storage());

        try {
            runContext.validate(task);
        } catch (ConstraintViolationException e) {
            return fail(out, EXIT_REFUSED, "invalid " + task.getType() + ": " + e.getMessage(), null);
        }

        KillSwitch killSwitch = null;
        if (killWaitMs != null) {
            RunnableTask<?> lifecycle = (RunnableTask<?>) task;
            KillSwitch ks = new KillSwitch(lifecycle::kill, killWaitMs);
            Runtime.getRuntime().addShutdownHook(new Thread(() -> ks.onTerm(System.out), "egret-shutdown"));
            killSwitch = ks;
        }

        Output output;
        try {
            output = ((RunnableTask<?>) task).run(runContext);
        } catch (Throwable t) {
            int code = exitCodeFor(t);
            return fail(out, code, failureMessage(t), t);
        } finally {
            if (killSwitch != null) {
                killSwitch.runReturned();
            }
            runContext.cleanup();
        }

        Map<String, Object> result = output == null ? Map.of() : JacksonMapper.toMap(output);
        try {
            outputs.println(outputsLine(result));
            outputs.flush();
        } catch (IOException e) {
            return fail(out, EXIT_PLUGIN_THREW, "the outputs could not be serialized: " + e.getMessage(), e);
        }
        return EXIT_OK;
    }

    /**
     * The success marker, <b>the one line the host does not mask</b> (W5b fix round 1, I1): outputs are
     * not masked, as in Kestra (spec §6, §12). Masking them was measured to corrupt real results: the
     * live test's username secret {@code w5b} turned the returned {@code 'w5b-t6'} into
     * {@code '******-t6'}. A value reaches this line only when the task returns it -- a query that
     * selects a secret -- and egret stores the same outputs on the task run.
     */
    static String outputsLine(Map<String, Object> outputs) throws IOException {
        return "::" + MAPPER.writeValueAsString(Map.of("outputs", outputs)) + "::";
    }

    /**
     * {@code <workingDir>/.egret/tmp} when it exists or can be made; else the JVM's temp directory (a
     * host run by hand, without egret's runner), with a WARN line.
     */
    static Path scratchUnder(Path workingDirBase) {
        Path scratch = workingDirBase.resolve(EgretStorage.TMP_DIR);
        try {
            Files.createDirectories(scratch);
            if (Files.isWritable(scratch)) {
                return scratch;
            }
        } catch (IOException e) {
            // fall through
        }
        Path tmp = Path.of(System.getProperty("java.io.tmpdir"));
        HOST_LOG.warn("{} is not writable; the plugin's temp files go to {}", scratch, tmp);
        return tmp;
    }

    static final class Refused extends Exception {
        Refused(String message) {
            super(message);
        }
    }

    /** A task and what {@code task.json} says about its storage. */
    record Loaded(Task task, ShimRunContext.StorageSpec storage) {
    }

    static Task load(Path taskJson) throws Refused {
        return loadAll(taskJson).task();
    }

    static Loaded loadAll(Path taskJson) throws Refused {
        Map<String, Object> doc;
        try {
            doc = MAPPER.readValue(Files.readAllBytes(taskJson), MAP);
        } catch (IOException e) {
            throw new Refused("cannot read " + taskJson + ": " + e.getMessage());
        }
        // First, before anything below can log a property: the values to mask.
        if (doc.get("mask") instanceof List<?> mask) {
            Masker.set(mask.stream().filter(String.class::isInstance).map(String.class::cast).toList());
        }
        if (!(doc.get("type") instanceof String type) || type.isBlank()) {
            throw new Refused("task.json has no \"type\"");
        }
        Object props = doc.getOrDefault("properties", Map.of());
        if (!(props instanceof Map<?, ?> properties)) {
            throw new Refused("task.json \"properties\" is not an object");
        }

        Map<?, ?> catalog = catalogClasses();
        if (!(catalog.get(type) instanceof Map<?, ?> cls)) {
            throw new Refused("type " + type + " is not in the plugin host's catalog");
        }

        Map<String, Object> spec = new LinkedHashMap<>();
        properties.forEach((k, v) -> spec.put(String.valueOf(k), v));
        spec.put("id", "egret");
        spec.put("type", type);
        // W5b Task 6 fix 1: every JDBC task runs WITHOUT a connection pool, whatever the author
        // wrote. The plugin defaults to a Hikari pool that registers its own JVM shutdown hook
        // (`kestra-jdbc-pool-shutdown`), which on SIGTERM aborts the connection -- closing the
        // socket and sending no cancel -- concurrently with our KillSwitch's kill(). When it wins,
        // pgjdbc's cancelQuery() sees a closed connection and never sends the cancel, and the query
        // outlives its timed-out task. A one-shot JVM gains nothing from a pool, so an author's
        // `connectionPooling: true` is overridden rather than refused: it changes performance only,
        // never what the task does.
        if (cls.get("properties") instanceof Map<?, ?> schema && schema.containsKey(CONNECTION_POOLING)) {
            if (spec.containsKey(CONNECTION_POOLING)) {
                HOST_LOG.debug("{}: {} = {} overridden to false", type, CONNECTION_POOLING, spec.get(CONNECTION_POOLING));
            }
            spec.put(CONNECTION_POOLING, false);
        }

        Task task;
        try {
            // PluginDeserializer falls back to DefaultPluginRegistry.getOrCreate() with no
            // KestraContext (spike trap 1): no Micronaut.
            task = MAPPER.convertValue(spec, Task.class);
        } catch (IllegalArgumentException e) {
            throw new Refused("invalid " + type + ": " + e.getMessage());
        }
        if (task == null || !task.getClass().getName().equals(type)) {
            throw new Refused("type " + type + " deserialized to " + (task == null ? "null" : task.getClass().getName()));
        }
        if (!(task instanceof RunnableTask<?>)) {
            throw new Refused("type " + type + " is not a runnable task");
        }
        return new Loaded(task, storageSpec(doc));
    }

    /** {@code task.json}'s storage keys (W9a spec §5); each one absent means "none". */
    static ShimRunContext.StorageSpec storageSpec(Map<String, Object> doc) throws Refused {
        Map<String, String> staged = new LinkedHashMap<>();
        if (doc.get("storage") instanceof Map<?, ?> m) {
            for (Map.Entry<?, ?> e : m.entrySet()) {
                if (!(e.getKey() instanceof String uri) || !(e.getValue() instanceof String rel)) {
                    throw new Refused("task.json \"storage\" maps a URI to a path, both strings");
                }
                staged.put(uri, rel);
            }
        } else if (doc.get("storage") != null) {
            throw new Refused("task.json \"storage\" is not an object");
        }
        URI baseUri = null;
        if (doc.get("baseUri") instanceof String b) {
            try {
                baseUri = new URI(b);
            } catch (URISyntaxException e) {
                throw new Refused("task.json \"baseUri\" is not a URI: " + e.getMessage());
            }
        }
        long maxTaskBytes = doc.get("maxTaskBytes") instanceof Number n ? n.longValue() : 0;
        Map<String, String> context = null;
        if (doc.get("context") instanceof Map<?, ?> c) {
            context = new LinkedHashMap<>();
            for (String k : List.of("namespace", "flowId", "executionId", "taskId", "taskRunId")) {
                if (!(c.get(k) instanceof String v)) {
                    throw new Refused("task.json \"context\" has no string \"" + k + "\"");
                }
                context.put(k, v);
            }
        }
        return new ShimRunContext.StorageSpec(staged, baseUri, maxTaskBytes, context);
    }

    /** The catalog's {@code classes} map: FQCN -> {properties, outputs}. */
    private static Map<?, ?> catalogClasses() throws Refused {
        try (InputStream in = EgretPluginHost.class.getClassLoader().getResourceAsStream("catalog.json")) {
            if (in == null) {
                throw new Refused("the plugin host has no catalog.json on its classpath");
            }
            Map<String, Object> catalog = MAPPER.readValue(in, MAP);
            if (!(catalog.get("classes") instanceof Map<?, ?> m)) {
                throw new Refused("the plugin host's catalog.json has no \"classes\"");
            }
            return m;
        } catch (IOException e) {
            throw new Refused("the plugin host's catalog.json cannot be read: " + e.getMessage());
        }
    }

    /** 4 when anything in the cause chain is {@link EgretUnsupported}, 2 for a constraint violation, else 3. */
    static int exitCodeFor(Throwable t) {
        for (Throwable c = t; c != null; c = c.getCause() == c ? null : c.getCause()) {
            if (c instanceof EgretUnsupported) {
                return EXIT_NEEDS_W9;
            }
        }
        if (t instanceof ConstraintViolationException) {
            return EXIT_REFUSED;
        }
        return EXIT_PLUGIN_THREW;
    }

    private static String failureMessage(Throwable t) {
        for (Throwable c = t; c != null; c = c.getCause() == c ? null : c.getCause()) {
            if (c instanceof EgretUnsupported) {
                return c.getMessage();
            }
        }
        StringBuilder sb = new StringBuilder(t.getClass().getName()).append(": ").append(t.getMessage());
        int depth = 0;
        for (Throwable c = t.getCause(); c != null && c != c.getCause() && depth < 3; c = c.getCause(), depth++) {
            sb.append(" (caused by ").append(c.getClass().getName()).append(": ").append(c.getMessage()).append(')');
        }
        return sb.toString();
    }

    static int fail(PrintStream out, int code, String rawMessage, Throwable t) {
        // Masked before it is logged, truncated or JSON-encoded: an exception's text can echo a
        // connection string, and a value JSON escapes would not match on the encoded line.
        String message = Masker.mask(rawMessage);
        if (t != null) {
            HOST_LOG.error(message, t);
        } else {
            HOST_LOG.error(message);
        }
        Map<String, Object> failure = new LinkedHashMap<>();
        failure.put("code", code);
        failure.put("message", truncate(message));
        try {
            out.println("::" + MAPPER.writeValueAsString(Map.of("egretFailure", failure)) + "::");
        } catch (IOException e) {
            out.println("::{\"egretFailure\":{\"code\":" + code + ",\"message\":\"(unserializable)\"}}::");
        }
        out.flush();
        return code;
    }

    static String truncate(String message) {
        byte[] bytes = message.getBytes(StandardCharsets.UTF_8);
        if (bytes.length <= MAX_FAILURE_BYTES) {
            return message;
        }
        // Cut on a code-point boundary so the result stays valid UTF-8.
        int end = message.length();
        while (message.substring(0, end).getBytes(StandardCharsets.UTF_8).length > MAX_FAILURE_BYTES - 3) {
            end = message.offsetByCodePoints(end, -1);
        }
        return message.substring(0, end) + "…";
    }

    // --- describe ----------------------------------------------------------------------

    /**
     * The catalog egret embeds: every runnable task class in the locked jars, with its properties'
     * JSON schemas (Kestra's own {@link JsonSchemaGenerator}) and its output names. The jars must
     * also be on this JVM's classpath; {@code plugins-dir/plugins.lock} is the lockfile
     * {@code fetch-plugins.sh} checked them against.
     */
    static Map<String, Object> describe(Path pluginsDir) throws IOException {
        byte[] lock = Files.readAllBytes(pluginsDir.resolve("plugins.lock"));
        JsonSchemaGenerator generator = new JsonSchemaGenerator(DefaultPluginRegistry.getOrCreate());

        Map<String, Object> classes = new TreeMap<>();
        Map<String, String> excluded = new TreeMap<>();
        for (String line : new String(lock, StandardCharsets.UTF_8).split("\n")) {
            line = line.strip();
            if (line.isEmpty() || line.startsWith("#")) {
                continue;
            }
            String[] coord = line.split("\\s+")[0].split(":");
            String jarName = coord[1] + "-" + coord[2] + ".jar";
            for (String fqcn : servicesOf(pluginsDir.resolve(jarName))) {
                Class<?> cls;
                try {
                    cls = Class.forName(fqcn, false, EgretPluginHost.class.getClassLoader());
                } catch (ClassNotFoundException e) {
                    throw new IOException(fqcn + " (from " + jarName + ") is not on the classpath", e);
                }
                String source = cls.getProtectionDomain().getCodeSource().getLocation().getPath();
                if (!source.endsWith("/" + jarName)) {
                    throw new IOException(fqcn + " loaded from " + source + ", not from " + jarName);
                }

                String reason = exclusion(cls);
                if (reason != null) {
                    excluded.put(fqcn, reason);
                    continue;
                }
                classes.put(fqcn, describeClass(generator, cls));
            }
        }
        excluded.forEach((fqcn, why) -> System.err.println("describe: excluded " + fqcn + ": " + why));

        Map<String, Object> catalog = new LinkedHashMap<>();
        catalog.put("lockSha256", sha256Hex(lock));
        catalog.put("kestra", KESTRA_VERSION);
        catalog.put("classes", classes);
        return catalog;
    }

    private static String exclusion(Class<?> cls) {
        if (AbstractTrigger.class.isAssignableFrom(cls)) {
            return "a trigger (plugin triggers are out of scope, spec §11)";
        }
        if (!Task.class.isAssignableFrom(cls) || !RunnableTask.class.isAssignableFrom(cls)) {
            return "not a runnable task";
        }
        if (java.lang.reflect.Modifier.isAbstract(cls.getModifiers())) {
            return "abstract";
        }
        if (NATIVE_TYPES.contains(cls.getName())) {
            return "egret ports it natively (spec::registry::TASK_TYPES)";
        }
        return NEEDS_STORAGE.get(cls.getName());
    }

    @SuppressWarnings("unchecked")
    private static Map<String, Object> describeClass(JsonSchemaGenerator generator, Class<?> cls) {
        Map<String, Object> schema = generator.properties(null, (Class<Object>) cls);
        Map<String, Object> props = (Map<String, Object>) schema.getOrDefault("properties", Map.of());
        List<String> required = (List<String>) schema.getOrDefault("required", List.of());

        Map<String, Object> properties = new TreeMap<>();
        props.forEach((name, propSchema) -> {
            Map<String, Object> entry = new LinkedHashMap<>();
            entry.put("schema", stripDocs(propSchema));
            entry.put("required", required.contains(name));
            properties.put(name, entry);
        });

        Map<String, Object> outputSchema = generator.outputs(null, (Class<Object>) cls);
        List<String> outputs = new ArrayList<>(
            ((Map<String, Object>) outputSchema.getOrDefault("properties", Map.of())).keySet());
        outputs.sort(null);

        Map<String, Object> entry = new LinkedHashMap<>();
        entry.put("properties", properties);
        entry.put("outputs", outputs);
        return entry;
    }

    private static final Set<String> DOC_KEYS = Set.of("title", "description", "markdownDescription", "$examples");

    /** Drops documentation text from a schema; keeps every key that constrains a value. */
    @SuppressWarnings("unchecked")
    static Object stripDocs(Object node) {
        if (node instanceof Map<?, ?> map) {
            Map<String, Object> out = new TreeMap<>();
            ((Map<String, Object>) map).forEach((k, v) -> {
                if (DOC_KEYS.contains(k)) {
                    return;
                }
                if (k.equals("properties") && v instanceof Map<?, ?> props) {
                    Map<String, Object> kept = new TreeMap<>();
                    ((Map<String, Object>) props).forEach((name, s) -> kept.put(name, stripDocs(s)));
                    out.put(k, kept);
                } else {
                    out.put(k, stripDocs(v));
                }
            });
            return out;
        }
        if (node instanceof List<?> list) {
            return list.stream().map(EgretPluginHost::stripDocs).toList();
        }
        return node;
    }

    private static List<String> servicesOf(Path jar) throws IOException {
        try (ZipFile zip = new ZipFile(jar.toFile())) {
            ZipEntry entry = zip.getEntry("META-INF/services/io.kestra.core.models.Plugin");
            if (entry == null) {
                throw new IOException(jar + " declares no io.kestra.core.models.Plugin service");
            }
            try (InputStream in = zip.getInputStream(entry)) {
                return new String(in.readAllBytes(), StandardCharsets.UTF_8).lines()
                    .map(String::strip)
                    .filter(l -> !l.isEmpty() && !l.startsWith("#"))
                    .toList();
            }
        }
    }

    static String sha256Hex(byte[] bytes) {
        try {
            return HexFormat.of().formatHex(MessageDigest.getInstance("SHA-256").digest(bytes));
        } catch (NoSuchAlgorithmException e) {
            throw new IllegalStateException(e);
        }
    }

    static ObjectMapper catalogMapper() {
        return JacksonMapper.ofJson().copy()
            .enable(SerializationFeature.INDENT_OUTPUT)
            .enable(SerializationFeature.ORDER_MAP_ENTRIES_BY_KEYS);
    }
}
