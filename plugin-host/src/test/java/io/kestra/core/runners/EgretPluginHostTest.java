package io.kestra.core.runners;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.PrintStream;
import java.net.ServerSocket;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;

import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import com.fasterxml.jackson.databind.ObjectMapper;

class EgretPluginHostTest {
    private static final ObjectMapper MAPPER = new ObjectMapper();

    @TempDir
    Path tmp;

    record Result(int exit, String stdout) {
        /** The message of the one `egretFailure` marker, or fails the test if there is none. */
        @SuppressWarnings("unchecked")
        String failureMessage() throws IOException {
            Map<String, Object> marker = marker("egretFailure");
            assertEquals(exit, ((Number) marker.get("code")).intValue(), "marker code = exit code");
            return (String) marker.get("message");
        }

        @SuppressWarnings("unchecked")
        Map<String, Object> marker(String key) throws IOException {
            List<String> lines = stdout.lines().filter(l -> l.startsWith("::{") && l.endsWith("}::")).toList();
            assertEquals(1, lines.size(), "exactly one marker line in: " + stdout);
            String body = lines.getFirst();
            Map<String, Object> m = MAPPER.readValue(body.substring(2, body.length() - 2), Map.class);
            assertTrue(m.containsKey(key), "marker has " + key + ": " + body);
            return (Map<String, Object>) m.get(key);
        }
    }

    private Result run(String taskJson) throws IOException {
        Path file = tmp.resolve("task.json");
        Files.writeString(file, taskJson);
        ByteArrayOutputStream bytes = new ByteArrayOutputStream();
        int exit;
        try (PrintStream out = new PrintStream(bytes, true, StandardCharsets.UTF_8)) {
            exit = EgretPluginHost.execute(file, out);
        }
        return new Result(exit, bytes.toString(StandardCharsets.UTF_8));
    }

    private static int closedPort() throws IOException {
        try (ServerSocket s = new ServerSocket(0)) {
            return s.getLocalPort();
        }
    }

    // --- run: exit codes ---------------------------------------------------------------

    @Test
    void a_type_absent_from_the_catalog_exits_2() throws Exception {
        var r = run("{\"type\":\"io.kestra.plugin.core.log.Log\",\"properties\":{\"message\":\"x\"}}");
        assertEquals(2, r.exit());
        assertTrue(r.failureMessage().contains("io.kestra.plugin.core.log.Log"), r.stdout());
    }

    @Test
    void a_constraint_violation_exits_2() throws Exception {
        var r = run("{\"type\":\"io.kestra.plugin.jdbc.postgresql.Query\",\"properties\":"
            + "{\"url\":\"jdbc:postgresql://127.0.0.1:" + closedPort() + "/x\"}}");
        assertEquals(2, r.exit(), r.stdout());
        assertTrue(r.failureMessage().contains("sql"), r.stdout());
    }

    @Test
    void a_plugin_exception_exits_3_with_class_and_message() throws Exception {
        var r = run("{\"type\":\"io.kestra.plugin.jdbc.postgresql.Query\",\"properties\":"
            + "{\"url\":\"jdbc:postgresql://127.0.0.1:" + closedPort() + "/x\",\"sql\":\"SELECT 1\"}}");
        assertEquals(3, r.exit(), r.stdout());
        String msg = r.failureMessage();
        assertTrue(msg.contains("org.postgresql.util.PSQLException"), msg);
        assertTrue(msg.contains("Connection to 127.0.0.1"), msg);
    }

    @Test
    void egret_unsupported_anywhere_in_the_cause_chain_is_exit_4() {
        var wrapped = new RuntimeException("plugin wrapped it", new EgretUnsupported("storage()"));
        assertEquals(4, EgretPluginHost.exitCodeFor(wrapped));
        assertEquals(3, EgretPluginHost.exitCodeFor(new IllegalStateException("x")));
    }

    // --- the shim -----------------------------------------------------------------------

    /**
     * W9a Task 9 flips W5b's {@code storage_on_the_shim_is_exit_4_naming_w9}: storage() is Kestra's
     * InternalStorage over EgretStorage. The replacement proving it runs is
     * {@code EgretStorageTest.a_plugin_reads_and_writes_through_kestras_internal_storage}; with no
     * storage context in task.json it says so rather than naming W9.
     */
    @Test
    void storage_on_the_shim_is_internal_storage_and_needs_the_context() {
        var shim = new ShimRunContext(tmp);
        var e = assertThrows(IllegalStateException.class, shim::storage);
        assertTrue(e.getMessage().contains("context"), e.getMessage());
        assertFalse(e.getMessage().contains("W9"), e.getMessage());
    }

    @Test
    void every_unprovided_collaborator_throws_egret_unsupported() {
        var shim = new ShimRunContext(tmp);
        var kv = assertThrows(EgretUnsupported.class, () -> shim.namespaceKv("ns"));
        assertTrue(kv.getMessage().contains("W9c"), kv.getMessage());
        assertThrows(EgretUnsupported.class, shim::logFileURI);
        assertThrows(EgretUnsupported.class, () -> shim.decrypt("x"));
        assertThrows(EgretUnsupported.class, () -> shim.encrypt("x"));
        assertThrows(EgretUnsupported.class, shim::inputAndOutput);
        assertThrows(EgretUnsupported.class, shim::acl);
    }

    @Test
    void render_is_identity() throws Exception {
        assertEquals("{{ not rendered }}", new ShimRunContext(tmp).render("{{ not rendered }}"));
        assertEquals("{{ x }}", new ShimRunContext(tmp).render("{{ x }}", Map.of("x", "y")));
    }

    @Test
    void the_working_dir_is_under_the_given_path() {
        Path p = new ShimRunContext(tmp).workingDir().path();
        assertTrue(p.startsWith(tmp), p.toString());
    }

    // --- the shutdown hook --------------------------------------------------------------

    @Test
    void a_kill_that_blocks_does_not_hold_the_hook_beyond_the_wait() throws Exception {
        // StarRocks through Connector/J: kill() blocks until the query ends (Task 1).
        var killCalled = new CountDownLatch(1);
        var neverReturns = new CountDownLatch(1);
        var killSwitch = new KillSwitch(() -> {
            killCalled.countDown();
            try {
                neverReturns.await();
            } catch (InterruptedException ignored) {
                Thread.currentThread().interrupt();
            }
        }, 300);
        ByteArrayOutputStream bytes = new ByteArrayOutputStream();
        long start = System.nanoTime();
        killSwitch.onTerm(new PrintStream(bytes, true, StandardCharsets.UTF_8));
        long ms = TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - start);

        assertTrue(killCalled.await(1, TimeUnit.SECONDS), "kill() was called");
        assertTrue(ms >= 300 && ms < 2000, "the hook waited the bound, not the kill: " + ms + " ms");
        assertTrue(bytes.toString(StandardCharsets.UTF_8).contains("WARN killed: timeout"), bytes.toString());
    }

    @Test
    void the_hook_returns_as_soon_as_run_returns() throws Exception {
        var killed = new AtomicBoolean();
        var killSwitch = new KillSwitch(() -> killed.set(true), 10_000);
        new Thread(() -> {
            try {
                Thread.sleep(100);
            } catch (InterruptedException ignored) {
            }
            killSwitch.runReturned();
        }).start();
        long start = System.nanoTime();
        killSwitch.onTerm(new PrintStream(new ByteArrayOutputStream(), true, StandardCharsets.UTF_8));
        long ms = TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - start);
        assertTrue(killed.get(), "kill() was called");
        assertTrue(ms < 5_000, "returned when run returned: " + ms + " ms");
    }

    /**
     * W5b Task 6 fix 1: the hook must not return while the kill it started is still running. On
     * SIGTERM the JDBC pool's own shutdown hook can close the connection first, making run() throw and
     * open the latch while kill() (the Postgres cancel) is still in flight; the JVM then halts and the
     * cancel is never sent.
     */
    @Test
    void the_hook_waits_for_a_kill_in_flight_after_run_returned() throws Exception {
        var cancelSent = new AtomicBoolean();
        var killSwitch = new KillSwitch(() -> {
            try {
                Thread.sleep(300);
            } catch (InterruptedException ignored) {
                Thread.currentThread().interrupt();
            }
            cancelSent.set(true);
        }, 10_000);
        new Thread(() -> {
            try {
                Thread.sleep(10);
            } catch (InterruptedException ignored) {
            }
            killSwitch.runReturned();
        }).start();
        killSwitch.onTerm(new PrintStream(new ByteArrayOutputStream(), true, StandardCharsets.UTF_8));
        assertTrue(cancelSent.get(), "onTerm returned before the kill in flight finished");
    }

    /** Fix 1 (iii): a kill() that found nothing to cancel yet is tried again until run() returns. */
    @Test
    void kill_is_retried_until_run_returns() throws Exception {
        var calls = new java.util.concurrent.atomic.AtomicInteger();
        var killSwitch = new KillSwitch(calls::incrementAndGet, 10_000);
        new Thread(() -> {
            try {
                Thread.sleep(1_000);
            } catch (InterruptedException ignored) {
            }
            killSwitch.runReturned();
        }).start();
        killSwitch.onTerm(new PrintStream(new ByteArrayOutputStream(), true, StandardCharsets.UTF_8));
        int n = calls.get();
        assertTrue(n >= 3, "kill() re-invoked while run() had not returned: " + n + " calls");
        Thread.sleep(500);
        assertEquals(n, calls.get(), "no kill() after run() returned");
    }

    /** Fix 1 (i): a one-shot JVM runs every JDBC task without a pool, whatever the author wrote. */
    @Test
    void load_turns_connection_pooling_off() throws Exception {
        for (String pooling : new String[] {"", ", \"connectionPooling\": true"}) {
            Path file = tmp.resolve("pool.json");
            Files.writeString(file, "{\"type\": \"io.kestra.plugin.jdbc.postgresql.Query\", \"properties\": "
                + "{\"url\": \"jdbc:postgresql://127.0.0.1:1/x\", \"sql\": \"SELECT 1\"" + pooling + "}}");
            var task = EgretPluginHost.load(file);
            Object value = io.kestra.core.serializers.JacksonMapper.toMap(task).get("connectionPooling");
            assertEquals("false", String.valueOf(value), "connectionPooling with '" + pooling + "'");
        }
    }

    @Test
    void after_run_returned_the_hook_does_not_kill() {
        var killed = new AtomicBoolean();
        var killSwitch = new KillSwitch(() -> killed.set(true), 10_000);
        killSwitch.runReturned();
        killSwitch.onTerm(new PrintStream(new ByteArrayOutputStream(), true, StandardCharsets.UTF_8));
        assertFalse(killed.get());
    }

    // --- masking (W5b fix round 1, I1) -----------------------------------------------------

    /** A host name that cannot resolve, so the driver's exception carries it. */
    private static final String SECRET = "s3cr3t-pw-zq9.invalid";

    /**
     * What the container's stdout would hold: the host's own masking streams over a buffer, installed
     * as {@code System.out} and {@code System.err} exactly as {@code main} installs them.
     */
    private String capturedStdout(ThrowingRunnable body) throws Exception {
        ByteArrayOutputStream raw = new ByteArrayOutputStream();
        MaskingOutputStream masking = new MaskingOutputStream(raw);
        PrintStream stream = EgretPluginHost.maskingPrintStream(masking);
        PrintStream prevOut = System.out;
        PrintStream prevErr = System.err;
        System.setOut(stream);
        System.setErr(stream);
        try {
            body.run();
        } finally {
            System.out.flush();
            System.setOut(prevOut);
            System.setErr(prevErr);
            masking.finish();
            Masker.set(List.of());
        }
        return raw.toString(StandardCharsets.UTF_8);
    }

    interface ThrowingRunnable {
        void run() throws Exception;
    }

    @Test
    void a_secret_never_reaches_stdout_from_a_plugin_log_an_exception_or_a_marker() throws Exception {
        Path file = tmp.resolve("task.json");
        Files.writeString(file, "{\"type\":\"io.kestra.plugin.jdbc.postgresql.Query\",\"properties\":"
            + "{\"url\":\"jdbc:postgresql://" + SECRET + ":5432/x\",\"sql\":\"SELECT 1\"},"
            + "\"mask\":[\"" + SECRET + "\"]}");
        int[] exit = new int[1];
        String stdout = capturedStdout(() -> {
            exit[0] = EgretPluginHost.execute(file, System.out);
            // The plugin's own logger, a logged exception, and a line written in two pieces.
            ShimRunContext.TASK_LOG.info("connecting to {}", SECRET);
            ShimRunContext.TASK_LOG.error("it failed", new IllegalStateException("cannot reach " + SECRET));
            System.out.print("split " + SECRET.substring(0, 7));
            System.out.println(SECRET.substring(7) + " end");
        });

        assertEquals(3, exit[0], stdout);
        assertFalse(stdout.contains(SECRET), stdout);
        // And none of its pieces long enough to identify it survives around the mask.
        assertFalse(stdout.contains("zq9.invalid"), stdout);
        assertTrue(stdout.contains("INFO connecting to ******"), stdout);
        assertTrue(stdout.contains("cannot reach ******"), stdout);
        assertTrue(stdout.contains("split ****** end"), stdout);
        // The driver's exception named the host: its text reached the marker, masked.
        String marker = stdout.lines().filter(l -> l.startsWith("::{\"egretFailure\"")).findFirst().orElseThrow();
        assertTrue(marker.contains("******"), marker);
    }

    @Test
    void a_value_json_escapes_is_masked_in_the_failure_marker() throws Exception {
        String value = "pa\"ss\\word";
        String stdout = capturedStdout(() -> {
            Masker.set(List.of(value));
            EgretPluginHost.fail(System.out, 3, "login as u/" + value + " failed", null);
        });
        assertFalse(stdout.contains("pa\\\"ss"), stdout);
        assertFalse(stdout.contains("pa\"ss"), stdout);
        assertTrue(stdout.contains("login as u/****** failed"), stdout);
    }

    /**
     * The outputs marker is the one line left as it is (outputs are not masked, spec §6): written
     * through the masking stream's unmasked side, in order with the masked lines around it.
     */
    @Test
    void the_outputs_marker_is_the_one_line_the_host_does_not_mask() throws Exception {
        ByteArrayOutputStream raw = new ByteArrayOutputStream();
        MaskingOutputStream masking = new MaskingOutputStream(raw);
        PrintStream masked = EgretPluginHost.maskingPrintStream(masking);
        PrintStream outputs = EgretPluginHost.maskingPrintStream(masking.unmasked());
        try {
            Masker.set(List.of("w5b"));
            masked.print("before w5b");
            outputs.println(EgretPluginHost.outputsLine(Map.of("tag", "w5b-t6")));
            masked.println("after w5b");
            masking.finish();
        } finally {
            Masker.set(List.of());
        }
        assertEquals(
            "before ******::{\"outputs\":{\"tag\":\"w5b-t6\"}}::\nafter ******\n",
            raw.toString(StandardCharsets.UTF_8));
    }

    @Test
    void masking_takes_the_longest_value_and_never_rematches_the_mask() {
        try {
            Masker.set(List.of("abc", "abcdef", "", "***"));
            assertEquals("x ****** y ****** z", Masker.mask("x abcdef y abc z"));
            assertEquals("******", Masker.mask("***"));
            assertEquals("nothing", Masker.mask("nothing"));
        } finally {
            Masker.set(List.of());
        }
        assertEquals("abc", Masker.mask("abc"), "no values, no masking");
    }

    // --- describe ------------------------------------------------------------------------

    @Test
    @SuppressWarnings("unchecked")
    void describe_lists_the_runnable_task_classes_and_nothing_else() throws Exception {
        Path plugins = Path.of(System.getProperty("egret.plugins.dir"));
        Map<String, Object> catalog = EgretPluginHost.describe(plugins);

        assertEquals("2.0.0-rc13", catalog.get("kestra"));
        assertEquals(64, ((String) catalog.get("lockSha256")).length());
        var classes = (Map<String, Map<String, Object>>) catalog.get("classes");
        assertEquals(
            // W9a Task 9: the four W5b excluded as needing storage are back.
            List.of(
                "io.kestra.plugin.jdbc.mysql.Batch",
                "io.kestra.plugin.jdbc.mysql.Queries",
                "io.kestra.plugin.jdbc.mysql.Query",
                "io.kestra.plugin.jdbc.postgresql.Batch",
                "io.kestra.plugin.jdbc.postgresql.CopyIn",
                "io.kestra.plugin.jdbc.postgresql.CopyOut",
                "io.kestra.plugin.jdbc.postgresql.Queries",
                "io.kestra.plugin.jdbc.postgresql.Query",
                "io.kestra.plugin.jdbc.trino.Query"
            ),
            List.copyOf(classes.keySet())
        );
        var query = classes.get("io.kestra.plugin.jdbc.postgresql.Query");
        var props = (Map<String, Map<String, Object>>) query.get("properties");
        assertEquals(true, props.get("sql").get("required"));
        assertEquals(false, props.get("fetchType").get("required"));
        assertEquals(List.of("row", "rows", "size", "uri"), query.get("outputs"));
    }
}
