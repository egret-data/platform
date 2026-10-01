package io.kestra.core.runners;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.io.ByteArrayInputStream;
import java.io.FileNotFoundException;
import java.io.IOException;
import java.io.InputStream;
import java.net.URI;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import java.util.Map;
import java.util.stream.Stream;

import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import com.fasterxml.jackson.databind.ObjectMapper;

import io.kestra.core.storages.FileAttributes;
import io.kestra.core.storages.StorageContext;
import io.kestra.core.storages.StorageObject;

/** W9a Task 9: the storage bridge (spec §9.1). */
class EgretStorageTest {
    private static final String EXEC = "0b2f4c3e-8a65-4d5b-9b0a-1c2d3e4f5a6b";
    private static final String RUN = "7e1d2c3b-4a59-4687-b6a5-948372615041";
    private static final String BASE = "kestra:///w9a/plugins/pg-storage/executions/" + EXEC + "/tasks/copy-out/" + RUN + "/";
    /** A file of ANOTHER task of the same execution, staged because a property named it. */
    private static final String STAGED = "kestra:///w9a/plugins/pg-storage/executions/" + EXEC
        + "/tasks/q-store/11111111-2222-4333-8444-555555555555/result.ion";
    private static final byte[] STAGED_BYTES = "{i:1}\n{i:2}\n".getBytes(StandardCharsets.UTF_8);

    @TempDir
    Path wd;

    EgretStorage storage;

    @BeforeEach
    void stage() throws IOException {
        Files.createDirectories(wd.resolve(".egret/storage"));
        Files.write(wd.resolve(".egret/storage/0"), STAGED_BYTES);
        storage = new EgretStorage(wd, Map.of(STAGED, ".egret/storage/0"), URI.create(BASE), 100);
    }

    private static StorageObject bytes(String s) {
        return new StorageObject(Map.of(), new ByteArrayInputStream(s.getBytes(StandardCharsets.UTF_8)));
    }

    private static byte[] read(InputStream in) throws IOException {
        try (in) {
            return in.readAllBytes();
        }
    }

    // --- reads -------------------------------------------------------------------------------

    @Test
    void get_of_a_staged_uri_returns_its_bytes() throws Exception {
        assertArrayEquals(STAGED_BYTES, read(storage.get(null, "w9a.plugins", URI.create(STAGED))));
        assertArrayEquals(STAGED_BYTES, read(storage.getWithMetadata(null, null, URI.create(STAGED)).inputStream()));
        assertTrue(storage.exists(null, null, URI.create(STAGED)));
        assertEquals(STAGED_BYTES.length, storage.getAttributes(null, null, URI.create(STAGED)).getSize());
    }

    @Test
    void get_of_anything_else_is_file_not_found_naming_the_uri() {
        // Same execution, same task, but not staged: a URI egret did not hand over is not readable.
        for (String other : List.of(
            STAGED.replace("result.ion", "other.ion"),
            "kestra:///w9a/plugins/other-flow/executions/" + EXEC + "/inputs/f/a.txt",
            BASE + "not-written-yet.ion")) {
            var e = assertThrows(FileNotFoundException.class, () -> storage.get(null, null, URI.create(other)));
            assertTrue(e.getMessage().contains(other), e.getMessage());
            assertFalse(storage.exists(null, null, URI.create(other)));
        }
        var e = assertThrows(IllegalArgumentException.class,
            () -> storage.get(null, null, URI.create("s3://bucket/w9a/x")));
        assertTrue(e.getMessage().contains("s3://bucket/w9a/x"), e.getMessage());
        assertThrows(IllegalArgumentException.class,
            () -> storage.get(null, null, URI.create(BASE + "../../q-store/x/result.ion")));
    }

    // --- writes ------------------------------------------------------------------------------

    @Test
    void put_under_base_uri_writes_egret_out_and_is_read_back() throws Exception {
        URI uri = storage.put(null, null, URI.create(BASE + "sub/copy-out"), bytes("i,t\n1,a\n"));
        assertEquals(URI.create(BASE + "sub/copy-out"), uri);
        assertEquals("i,t\n1,a\n", Files.readString(wd.resolve(".egret/out/sub/copy-out")));
        assertEquals("i,t\n1,a\n", new String(read(storage.get(null, null, uri)), StandardCharsets.UTF_8));
        // The scheme-less spelling StorageContext.Task builds names the same file.
        URI schemeless = URI.create("//" + URI.create(BASE).getPath() + "sub/copy-out");
        assertTrue(storage.exists(null, null, schemeless), schemeless.toString());
        try (Stream<Path> parts = Files.list(wd.resolve(".egret/out/sub"))) {
            assertEquals(List.of("copy-out"), parts.map(p -> p.getFileName().toString()).toList(),
                "no partial file is left beside it");
        }
    }

    @Test
    void put_outside_base_uri_is_refused() throws Exception {
        for (String outside : List.of(
            STAGED.replace("result.ion", "forged.ion"),           // another task of this execution
            "kestra:///w9a/plugins/pg-storage/executions/" + EXEC + "/x.ion", // the execution's root
            BASE.substring(0, BASE.length() - 1),                  // the prefix itself, as a file
            BASE.replace("copy-out", "copy-outx") + "a")) {        // a sibling sharing the text prefix
            var e = assertThrows(IOException.class, () -> storage.put(null, null, URI.create(outside), bytes("x")),
                outside);
            assertTrue(e.getMessage().contains(BASE), e.getMessage());
        }
        try (Stream<Path> out = Files.walk(wd.resolve(".egret"))) {
            assertEquals(0, out.filter(p -> p.startsWith(wd.resolve(".egret/out")) && Files.isRegularFile(p)).count(),
                "nothing was written");
        }
    }

    @Test
    void put_past_max_task_bytes_is_refused_naming_the_cap() throws Exception {
        storage.put(null, null, URI.create(BASE + "a"), bytes("x".repeat(60)));
        var e = assertThrows(IOException.class,
            () -> storage.put(null, null, URI.create(BASE + "b"), bytes("y".repeat(41))));
        assertTrue(e.getMessage().contains("100"), e.getMessage());
        assertTrue(e.getMessage().contains("workflow.storage.max_task_bytes"), e.getMessage());
        assertFalse(Files.exists(wd.resolve(".egret/out/b")), "the refused file is not left");
        // Exactly at the cap is allowed; overwriting a file counts its new size, not both.
        storage.put(null, null, URI.create(BASE + "b"), bytes("y".repeat(40)));
        storage.put(null, null, URI.create(BASE + "a"), bytes("x".repeat(60)));
    }

    @Test
    void without_a_base_uri_every_write_fails_naming_storage() {
        var none = new EgretStorage(wd, Map.of(), null, 0);
        var e = assertThrows(IOException.class, () -> none.put(null, null, URI.create(BASE + "a"), bytes("x")));
        assertTrue(e.getMessage().contains("[storage]"), e.getMessage());
    }

    // --- delete, move, directories -----------------------------------------------------------

    @Test
    void delete_of_a_staged_file_is_refused() throws Exception {
        var e = assertThrows(UnsupportedOperationException.class,
            () -> storage.delete(null, null, URI.create(STAGED)));
        assertTrue(e.getMessage().contains(STAGED), e.getMessage());
        assertTrue(e.getMessage().contains("staged"), e.getMessage());
        assertTrue(Files.exists(wd.resolve(".egret/storage/0")));
        // Its own file is deleted, and its bytes no longer count.
        storage.put(null, null, URI.create(BASE + "a"), bytes("x".repeat(100)));
        assertTrue(storage.delete(null, null, URI.create(BASE + "a")));
        assertFalse(Files.exists(wd.resolve(".egret/out/a")));
        storage.put(null, null, URI.create(BASE + "b"), bytes("y".repeat(100)));
        // A prefix delete outside its own prefix: the execution's files (deleteExecutionFiles).
        assertThrows(UnsupportedOperationException.class, () -> storage.deleteByPrefix(null, null,
            URI.create("kestra:///w9a/plugins/pg-storage/executions/" + EXEC)));
        assertEquals(List.of(URI.create(BASE + "b")), storage.deleteByPrefix(null, null, URI.create(BASE)));
    }

    @Test
    void move_and_create_directory_stay_inside_the_tasks_own_files() throws Exception {
        storage.put(null, null, URI.create(BASE + "a"), bytes("x"));
        assertEquals(URI.create(BASE + "d/b"), storage.move(null, null, URI.create(BASE + "a"), URI.create(BASE + "d/b")));
        assertEquals("x", Files.readString(wd.resolve(".egret/out/d/b")));
        assertThrows(UnsupportedOperationException.class,
            () -> storage.move(null, null, URI.create(STAGED), URI.create(BASE + "c")));
        assertThrows(UnsupportedOperationException.class,
            () -> storage.move(null, null, URI.create(BASE + "d/b"), URI.create(STAGED + ".moved")));
        assertThrows(UnsupportedOperationException.class,
            () -> storage.createDirectory(null, null, URI.create("kestra:///w9a/elsewhere")));
        storage.createDirectory(null, null, URI.create(BASE + "e"));
        assertTrue(Files.isDirectory(wd.resolve(".egret/out/e")));
    }

    @Test
    void instance_resources_are_unsupported() {
        URI u = URI.create("kestra:///x");
        assertThrows(UnsupportedOperationException.class, () -> storage.getInstanceResource(null, u));
        assertThrows(UnsupportedOperationException.class, () -> storage.listInstanceResource(null, u));
        assertThrows(UnsupportedOperationException.class, () -> storage.getInstanceAttributes(null, u));
        assertThrows(UnsupportedOperationException.class, () -> storage.putInstanceResource(null, u, bytes("x")));
        assertThrows(UnsupportedOperationException.class, () -> storage.deleteInstanceResource(null, u));
        assertThrows(UnsupportedOperationException.class, () -> storage.createInstanceDirectory(null, u));
    }

    @Test
    void list_and_all_by_prefix_see_staged_and_written_files() throws Exception {
        storage.put(null, null, URI.create(BASE + "a.ion"), bytes("1"));
        storage.put(null, null, URI.create(BASE + "d/b.ion"), bytes("22"));
        List<FileAttributes> listed = storage.list(null, null, URI.create(BASE));
        assertEquals(List.of("a.ion:File:1", "d:Directory:0"),
            listed.stream().map(f -> f.getFileName() + ":" + f.getType() + ":" + f.getSize()).toList());
        assertEquals(List.of(URI.create(BASE + "a.ion"), URI.create(BASE + "d/"), URI.create(BASE + "d/b.ion")),
            storage.allByPrefix(null, null, URI.create(BASE), true));
        String execution = "kestra:///w9a/plugins/pg-storage/executions/" + EXEC + "/";
        // Sorted by path: this task's `copy-out` before the staged `q-store`.
        assertEquals(List.of(URI.create(BASE + "a.ion"), URI.create(BASE + "d/b.ion"), URI.create(STAGED)),
            storage.allByPrefix(null, null, URI.create(execution), false));
    }

    // --- through Kestra's InternalStorage ----------------------------------------------------

    private ShimRunContext shim() {
        return new ShimRunContext(wd, new ShimRunContext.StorageSpec(Map.of(STAGED, ".egret/storage/0"), URI.create(BASE), 100,
            Map.of("namespace", "w9a.plugins", "flowId", "pg-storage", "executionId", EXEC,
                "taskId", "copy_out", "taskRunId", RUN)));
    }

    @Test
    void a_plugin_reads_and_writes_through_kestras_internal_storage() throws Exception {
        var storage = shim().storage();
        assertArrayEquals(STAGED_BYTES, read(storage.getFile(URI.create(STAGED))));
        // CopyOut's call: putFile(stream, "copy-out").
        URI out = storage.putFile(new ByteArrayInputStream("i\n1\n".getBytes(StandardCharsets.UTF_8)), "copy-out");
        assertEquals(BASE + "copy-out", out.toString());
        assertEquals("i\n1\n", Files.readString(wd.resolve(".egret/out/copy-out")));
        // AbstractJdbcQuery's call: putFile(File) -- the name is the temp file's, and Kestra deletes it.
        Path tmp = Files.writeString(wd.resolve("123.ion"), "{i:1}");
        assertEquals(BASE + "123.ion", storage.putFile(tmp.toFile()).toString());
        assertFalse(Files.exists(tmp));
        assertEquals(URI.create("///w9a/plugins/pg-storage/executions/" + EXEC + "/tasks/copy-out/" + RUN),
            storage.getContextBaseURI(), "Kestra's own context URI, unchanged");
    }

    /**
     * Live, W9a Task 9: on Docker $WORKING_DIR is root-owned and the host runs as USER 65532, so the
     * plugin's scratch (Kestra's LocalWorkingDir) is a separate, writable base; staged and written
     * files stay under $WORKING_DIR.
     */
    @Test
    void the_plugins_scratch_and_egrets_files_have_separate_bases(@TempDir Path scratch) throws Exception {
        var shim = new ShimRunContext(scratch, wd, new ShimRunContext.StorageSpec(Map.of(STAGED, ".egret/storage/0"),
            URI.create(BASE), 100, Map.of("namespace", "w9a.plugins", "flowId", "pg-storage", "executionId", EXEC,
                "taskId", "copy_out", "taskRunId", RUN)));
        Path tmp = shim.workingDir().createTempFile(".ion");
        assertTrue(tmp.startsWith(scratch), tmp.toString());
        Files.writeString(tmp, "{i:1}");
        URI uri = shim.storage().putFile(tmp.toFile());
        assertEquals("{i:1}", Files.readString(wd.resolve(".egret/out").resolve(Path.of(uri.getPath()).getFileName())));
        assertArrayEquals(STAGED_BYTES, read(shim.storage().getFile(URI.create(STAGED))));
    }

    @Test
    void namespace_files_are_w9b_and_exit_4() {
        var storage = shim().storage();
        var e = assertThrows(EgretUnsupported.class, storage::namespace);
        assertTrue(e.getMessage().contains("W9b"), e.getMessage());
        assertEquals(4, EgretPluginHost.exitCodeFor(e));
    }

    @Test
    void task_json_carries_the_storage_spec() throws Exception {
        Path file = wd.resolve("task.json");
        Files.writeString(file, "{\"type\": \"io.kestra.plugin.jdbc.postgresql.CopyIn\", \"properties\": "
            + "{\"url\": \"jdbc:postgresql://127.0.0.1:1/x\", \"table\": \"t\", \"from\": \"" + STAGED + "\"},"
            + "\"storage\": {\"" + STAGED + "\": \".egret/storage/0\"}, \"baseUri\": \"" + BASE + "\","
            + "\"maxTaskBytes\": 5368709120, \"context\": {\"namespace\": \"w9a.plugins\", \"flowId\": "
            + "\"pg-storage\", \"executionId\": \"" + EXEC + "\", \"taskId\": \"copy_out\", \"taskRunId\": \"" + RUN + "\"}}");
        var loaded = EgretPluginHost.loadAll(file);
        assertEquals("io.kestra.plugin.jdbc.postgresql.CopyIn", loaded.task().getClass().getName());
        assertEquals(Map.of(STAGED, ".egret/storage/0"), loaded.storage().staged());
        assertEquals(URI.create(BASE), loaded.storage().baseUri());
        assertEquals(5_368_709_120L, loaded.storage().maxTaskBytes());
        assertEquals("copy_out", loaded.storage().context().get("taskId"));
    }

    // --- the base URI: Kestra's StorageContext and egret agree ---------------------------------

    /**
     * {@code base-uri-cases.json} is written by the Rust test
     * {@code kestra::tests::the_base_uri_fixture_is_what_egret_computes} from {@code uri::task_prefix}.
     * Kestra's {@code StorageContext.forTask(...).getContextStorageURI()} for the same five values must
     * name the same prefix: {@code InternalStorage.putFile(stream, name)} resolves against it and
     * {@link EgretStorage#put} accepts only what lies under egret's {@code baseUri}.
     */
    @Test
    @SuppressWarnings("unchecked")
    void storage_context_for_task_is_the_base_uri_egret_computes() throws Exception {
        List<Map<String, String>> cases;
        try (InputStream in = getClass().getClassLoader().getResourceAsStream("base-uri-cases.json")) {
            assertTrue(in != null, "base-uri-cases.json is on the test classpath");
            cases = new ObjectMapper().readValue(in, List.class);
        }
        assertTrue(cases.size() >= 4, "the fixture holds the cases: " + cases.size());
        for (Map<String, String> c : cases) {
            URI kestra = StorageContext.forTask(null, c.get("namespace"), c.get("flowId"), c.get("executionId"),
                c.get("taskId"), c.get("taskRunId"), null).getContextStorageURI();
            assertEquals(c.get("baseUri"), "kestra:" + kestra + "/", c.toString());
        }
    }
}
