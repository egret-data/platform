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

import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.function.Executable;
import org.junit.jupiter.api.io.TempDir;

import io.kestra.core.storages.Namespace;
import io.kestra.core.storages.NamespaceFile;

/** W9b Task 10: the plugin bridge's namespace (spec §10, §15). */
class EgretNamespaceTest {
    private static final String NS = "w9b.t10";
    private static final String OTHER = "other.ns";
    private static final String EXEC = "0b2f4c3e-8a65-4d5b-9b0a-1c2d3e4f5a6b";
    private static final String RUN = "7e1d2c3b-4a59-4687-b6a5-948372615041";
    private static final String BASE = "kestra:///w9b/t10/flow/executions/" + EXEC + "/tasks/copy-in/" + RUN + "/";
    /** `fileURI('data.csv')`: one revision, named by a kestra:// URI. */
    private static final String DATA_URI = "kestra:///w9b/t10/_files/data/data.csv";

    private static final byte[] Q = "SELECT 1".getBytes(StandardCharsets.UTF_8);
    private static final byte[] DATA = "i,t\n1,a\n".getBytes(StandardCharsets.UTF_8);
    private static final byte[] OTHER_BYTES = "other".getBytes(StandardCharsets.UTF_8);

    private static final List<EgretNamespace.Staged> STAGED = List.of(
        new EgretNamespace.Staged("nsfile:///q.sql", NS, "/q.sql", 2, Q.length, ".egret/storage/0"),
        new EgretNamespace.Staged(DATA_URI, NS, "/data/data.csv", 1, DATA.length, ".egret/storage/1"),
        new EgretNamespace.Staged("nsfile://other.ns/o.txt", OTHER, "/o.txt", 1, OTHER_BYTES.length, ".egret/storage/2"));

    @TempDir
    Path wd;

    @BeforeEach
    void stage() throws IOException {
        Files.createDirectories(wd.resolve(".egret/storage"));
        Files.write(wd.resolve(".egret/storage/0"), Q);
        Files.write(wd.resolve(".egret/storage/1"), DATA);
        Files.write(wd.resolve(".egret/storage/2"), OTHER_BYTES);
    }

    private EgretNamespace own() {
        return new EgretNamespace(null, NS, wd, STAGED.stream().filter(s -> s.namespace().equals(NS)).toList());
    }

    private ShimRunContext shim() {
        return new ShimRunContext(wd, new ShimRunContext.StorageSpec(Map.of(), URI.create(BASE), 100,
            Map.of("namespace", NS, "flowId", "flow", "executionId", EXEC, "taskId", "copy-in", "taskRunId", RUN),
            STAGED));
    }

    private static byte[] read(InputStream in) throws IOException {
        try (in) {
            return in.readAllBytes();
        }
    }

    // --- reads -------------------------------------------------------------------------------

    @Test
    void get_file_content_of_a_staged_path_returns_its_bytes() throws Exception {
        var ns = own();
        assertArrayEquals(Q, read(ns.getFileContent(Path.of("q.sql"))));
        assertArrayEquals(Q, read(ns.getFileContent(Path.of("/q.sql"))));
        assertArrayEquals(Q, read(ns.getFileContent(Path.of("/q.sql"), 2)), "its own revision");
        // A kestra:// URI names one revision, readable by that revision (R1).
        assertArrayEquals(DATA, read(ns.getFileContent(Path.of("data/data.csv"), 1)));
        assertEquals(Q.length, ns.getFileMetadata(Path.of("q.sql")).getSize());
        assertEquals("q.sql", ns.getFileMetadata(Path.of("q.sql")).getFileName());
        NamespaceFile f = ns.get(Path.of("q.sql"));
        assertEquals(2, f.revision());
        assertEquals("kestra:///w9b/t10/_files/q.sql.v2", f.uri().toString());
    }

    @Test
    void any_other_path_is_file_not_found_naming_it_and_why() {
        var ns = own();
        for (Executable read : List.<Executable>of(
            () -> ns.getFileContent(Path.of("nope.sql")),
            () -> ns.getFileContent(Path.of("q.sql"), 1),
            () -> ns.getFileMetadata(Path.of("nope.sql")),
            () -> ns.get(Path.of("nope.sql")),
            // A kestra:// entry says nothing about the latest revision: a latest read does not see it (R1).
            () -> ns.getFileContent(Path.of("data/data.csv")))) {
            var e = assertThrows(FileNotFoundException.class, read);
            assertTrue(e.getMessage().contains("named whole in the task's properties"), e.getMessage());
            assertTrue(e.getMessage().contains("'" + NS + "'"), e.getMessage());
        }
        var e = assertThrows(FileNotFoundException.class, () -> ns.getFileContent(Path.of("nope.sql")));
        assertTrue(e.getMessage().contains("'/nope.sql'"), e.getMessage());
        var r = assertThrows(FileNotFoundException.class, () -> ns.getFileContent(Path.of("q.sql"), 1));
        assertTrue(r.getMessage().startsWith("Revision 1 of file '/q.sql'"), r.getMessage());
    }

    @Test
    void exists_answers_over_the_staged_latest_files() {
        var ns = own();
        assertTrue(ns.exists(Path.of("q.sql")));
        assertTrue(ns.exists(Path.of("/q.sql")));
        assertFalse(ns.exists(Path.of("nope.sql")));
        assertFalse(ns.exists(Path.of("o.txt")), "another namespace's file");
    }

    @Test
    void find_all_files_matching_is_over_the_staged_set() throws Exception {
        var ns = own();
        assertEquals(List.of("/q.sql"),
            ns.findAllFilesMatching(List.of("**.sql"), List.of()).stream().map(f -> f.filePath().toString()).toList());
        assertEquals(List.of(), ns.findAllFilesMatching(List.of("**.csv"), List.of()));
        assertEquals(List.of("q.sql"), ns.all().stream().map(NamespaceFile::path).toList());
        assertEquals(List.of(), ns.all("nope", false));
        assertTrue(ns.isDirectoryEmpty("/data"), "nothing listed on the plugin's behalf");
    }

    /** Fix round 1, M4: {@code children} answers over the same latest view as {@code all()}. */
    @Test
    void children_answer_over_the_same_staged_set_as_all() throws Exception {
        var staged = List.of(
            new EgretNamespace.Staged("nsfile:///q.sql", NS, "/q.sql", 2, Q.length, ".egret/storage/0"),
            new EgretNamespace.Staged("nsfile:///data/data.csv", NS, "/data/data.csv", 1, DATA.length, ".egret/storage/1"),
            new EgretNamespace.Staged("nsfile:///data/deep/o.txt", NS, "/data/deep/o.txt", 1, OTHER_BYTES.length,
                ".egret/storage/2"));
        var ns = new EgretNamespace(null, NS, wd, staged);
        assertEquals(List.of("/q.sql"), ns.children("/", false).stream().map(m -> m.getPath()).toList());
        assertEquals(List.of("/data/data.csv", "/data/deep/o.txt", "/q.sql"),
            ns.children("/", true).stream().map(m -> m.getPath()).sorted().toList());
        assertEquals(List.of("/data/data.csv"), ns.children("data", false).stream().map(m -> m.getPath()).toList());
        assertEquals(List.of("/data/data.csv", "/data/deep/o.txt"),
            ns.children("/data/", true).stream().map(m -> m.getPath()).sorted().toList());
        var q = ns.children("/", false).get(0);
        assertEquals(2, q.getRevision());
        assertEquals(Q.length, q.getSize());
        assertEquals(NS, q.getNamespace());
        // own(): the kestra:// entry is not in the latest view, so neither all() nor children() list it.
        assertEquals(List.of(), own().children("/data", true));
    }

    // --- writes ------------------------------------------------------------------------------

    @Test
    void writes_are_unsupported_with_the_reason() {
        var ns = own();
        for (Executable write : List.<Executable>of(
            () -> ns.putFile(Path.of("q.sql"), new ByteArrayInputStream(Q)),
            () -> ns.putFile(Path.of("new.sql"), new ByteArrayInputStream(Q), Namespace.Conflicts.ERROR),
            () -> ns.putFile(NamespaceFile.of(NS, Path.of("q.sql")), new ByteArrayInputStream(Q)),
            () -> ns.delete(Path.of("q.sql")),
            () -> ns.delete(NamespaceFile.of(NS, Path.of("q.sql"))),
            () -> ns.move(Path.of("q.sql"), Path.of("r.sql")),
            () -> ns.createDirectory(Path.of("dir")))) {
            var e = assertThrows(UnsupportedOperationException.class, write);
            assertTrue(e.getMessage().contains(EgretNamespace.WHY_READ_ONLY), e.getMessage());
        }
        assertArrayEquals(Q, assertDoesNotThrowRead(ns), "a refused write changed nothing");
    }

    private static byte[] assertDoesNotThrowRead(EgretNamespace ns) {
        try {
            return read(ns.getFileContent(Path.of("q.sql")));
        } catch (IOException e) {
            throw new AssertionError(e);
        }
    }

    // --- through Kestra's InternalStorage ------------------------------------------------------

    @Test
    void storage_namespace_is_the_staged_entries_of_that_namespace_only() throws Exception {
        var storage = shim().storage();
        Namespace own = storage.namespace(NS);
        assertArrayEquals(Q, read(own.getFileContent(Path.of("q.sql"))));
        assertFalse(own.exists(Path.of("o.txt")), "another namespace's entry is not this one's");
        assertArrayEquals(Q, read(storage.namespace().getFileContent(Path.of("q.sql"))), "the context's namespace");

        Namespace other = storage.namespace(OTHER);
        assertEquals(OTHER, other.namespace());
        assertArrayEquals(OTHER_BYTES, read(other.getFileContent(Path.of("o.txt"))));
        assertFalse(other.exists(Path.of("q.sql")), "the flow's own entry is not another namespace's");
        assertEquals(List.of("o.txt"), other.all().stream().map(NamespaceFile::path).toList());
    }

    @Test
    void storage_namespace_of_a_namespace_nothing_named_is_empty_not_exit_4() throws Exception {
        Namespace none = shim().storage().namespace("unnamed.ns");
        assertEquals("unnamed.ns", none.namespace());
        assertEquals(List.of(), none.all());
        assertFalse(none.exists(Path.of("q.sql")));
        var e = assertThrows(FileNotFoundException.class, () -> none.getFileContent(Path.of("q.sql")));
        assertTrue(e.getMessage().contains("'unnamed.ns'"), e.getMessage());
    }

    @Test
    void egret_storage_get_of_a_staged_files_uri_returns_its_bytes() throws Exception {
        var storage = shim().storage();
        assertArrayEquals(DATA, read(storage.getFile(URI.create(DATA_URI))), "CopyIn's `from`");
        assertTrue(storage.isFileExist(URI.create(DATA_URI)));
        // An nsfile:// value is not a storage URI; Kestra's own guard refuses it, as in Kestra.
        assertThrows(IllegalArgumentException.class, () -> storage.getFile(URI.create("nsfile:///q.sql")));
        // A namespace file that was not staged is not readable by URI either.
        assertThrows(FileNotFoundException.class,
            () -> storage.getFile(URI.create("kestra:///w9b/t10/_files/nope.csv")));
    }

    /**
     * Fix round 1, I1: Kestra's own read of a URI value ({@code URIFetcher.fetch}, which {@code Data.from}
     * and {@code FilesService} use) works against egret's run context, for an {@code nsfile://} value --
     * {@code storage().getFile(namespace(ns).get(path).uri())} -- and for a {@code kestra://} one.
     */
    @Test
    void kestras_uri_fetcher_reads_a_staged_nsfile_and_a_staged_kestra_uri() throws Exception {
        var shim = shim();
        assertArrayEquals(Q, read(io.kestra.core.models.property.URIFetcher.of("nsfile:///q.sql").fetch(shim)));
        assertArrayEquals(Q, read(io.kestra.core.models.property.URIFetcher.of("nsfile://w9b.t10/q.sql").fetch(shim)));
        assertArrayEquals(OTHER_BYTES,
            read(io.kestra.core.models.property.URIFetcher.of("nsfile://other.ns/o.txt").fetch(shim)));
        assertArrayEquals(DATA, read(io.kestra.core.models.property.URIFetcher.of(DATA_URI).fetch(shim)));
        // The canonical URI names the staged revision only.
        assertArrayEquals(Q, read(shim.storage().getFile(URI.create("kestra:///w9b/t10/_files/q.sql.v2"))));
        assertThrows(FileNotFoundException.class,
            () -> shim.storage().getFile(URI.create("kestra:///w9b/t10/_files/q.sql")));
        // A namespace nothing staged: not found, naming the path.
        var e = assertThrows(FileNotFoundException.class,
            () -> io.kestra.core.models.property.URIFetcher.of("nsfile:///nope.sql").fetch(shim));
        assertTrue(e.getMessage().contains("'/nope.sql'"), e.getMessage());
    }

    @Test
    void task_json_carries_the_namespace_files() throws Exception {
        Path file = wd.resolve("task.json");
        Files.writeString(file, "{\"type\": \"io.kestra.plugin.jdbc.postgresql.CopyIn\", \"properties\": "
            + "{\"url\": \"jdbc:postgresql://127.0.0.1:1/x\", \"table\": \"t\", \"from\": \"" + DATA_URI + "\"},"
            + "\"storage\": {}, \"namespaceFiles\": [{\"uri\": \"" + DATA_URI + "\", \"namespace\": \"" + NS
            + "\", \"path\": \"/data/data.csv\", \"revision\": 1, \"size\": " + DATA.length
            + ", \"staged\": \".egret/storage/1\"}]}");
        var loaded = EgretPluginHost.loadAll(file);
        assertEquals(Map.of(), loaded.storage().staged(), "W9a's storage map is unchanged");
        assertEquals(List.of(STAGED.get(1)), loaded.storage().namespaceFiles());

        Files.writeString(file, "{\"type\": \"io.kestra.plugin.jdbc.postgresql.CopyIn\", \"properties\": "
            + "{\"url\": \"jdbc:postgresql://127.0.0.1:1/x\", \"table\": \"t\", \"from\": \"x\"},"
            + "\"namespaceFiles\": [{\"uri\": \"" + DATA_URI + "\", \"namespace\": \"" + NS + "\"}]}");
        var e = assertThrows(EgretPluginHost.Refused.class, () -> EgretPluginHost.loadAll(file));
        assertTrue(e.getMessage().contains("namespaceFiles"), e.getMessage());
    }
}
