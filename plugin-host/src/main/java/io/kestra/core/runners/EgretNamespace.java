package io.kestra.core.runners;

import java.io.FileNotFoundException;
import java.io.IOException;
import java.io.InputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.attribute.BasicFileAttributes;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.TreeMap;
import java.util.function.Predicate;

import org.apache.commons.lang3.tuple.Pair;

import io.kestra.core.models.namespaces.files.NamespaceFileMetadata;
import io.kestra.core.storages.FileAttributes;
import io.kestra.core.storages.Namespace;
import io.kestra.core.storages.NamespaceFile;

/**
 * egret's {@link Namespace} (W9b spec §10, §15): the namespace files egret staged for this task, of
 * ONE namespace, read-only. It holds no credential and opens no connection.
 *
 * <p><b>What it sees.</b> Before the JVM starts, egret stages every property value that is WHOLE an
 * {@code nsfile://} or {@code kestra:///…/_files/…} URI, read-checked in the execution's workspace,
 * at {@code .egret/storage/<n>}, and lists it in {@code task.json}'s {@code namespaceFiles}. Those
 * entries, for this namespace, are everything this class answers over. Nothing is listed or globbed
 * on the plugin's behalf: a plugin that computes a namespace path at run does not see it.
 *
 * <ul>
 *   <li><b>A revision read</b> ({@code getFileContent(path, n)}) answers from any staged entry of that
 *       path and revision -- a {@code kestra:///…/_files/…[.vN]} URI names one revision, and per the
 *       user's ruling R1 that revision is readable even when the path was soft-deleted.</li>
 *   <li><b>A latest read</b> ({@code getFileContent(path)}, {@code exists}, {@code getFileMetadata},
 *       {@code get}, {@code all}, {@code findAllFilesMatching}) answers only from entries egret
 *       resolved as the LATEST live revision: the {@code nsfile://} ones. A {@code kestra://} entry
 *       says nothing about which revision is latest, or whether the path is still live (R1: a latest
 *       read of a soft-deleted path is not-found), so it is not part of the latest view.</li>
 *   <li>An absent path is a {@link FileNotFoundException} naming it and saying why.</li>
 *   <li>{@code putFile}, {@code delete}, {@code move} and {@code createDirectory} throw
 *       {@link UnsupportedOperationException} with the reason: the pod holds no credential to egret's
 *       store, and egret's catalog holds no plugin that writes a namespace file. {@code children} answers
 *       over the latest view, as {@code all} does (fix round 1, M4).</li>
 *   <li>{@link #get} of a path not staged throws {@link FileNotFoundException}, where Kestra's returns a
 *       revision-1 {@code NamespaceFile} unchecked; the URI of one that is staged is readable through
 *       {@code storage().getFile} (fix round 1, I1).</li>
 * </ul>
 */
final class EgretNamespace implements Namespace {
    /** One entry of {@code task.json}'s {@code namespaceFiles}. */
    record Staged(String uri, String namespace, String path, int revision, long size, String staged) {
        /** egret resolved this entry as the path's latest live revision ({@code nsfile://}). */
        boolean latest() {
            return uri.startsWith(NAMESPACE_FILE_SCHEME + "://");
        }
    }

    static final String WHY_READ_ONLY = "egret's plugin host reads namespace files and writes none: the "
        + "pod holds no credential to egret's storage, and a plugin reads only the namespace files named "
        + "whole in its properties, which egret staged before the task started (W9b spec §10, §15)";

    private final String tenantId;
    private final String namespace;
    private final Path workingDir;
    private final List<Staged> entries;
    /** normalized path -> the latest-view entry. */
    private final Map<String, Staged> latest = new TreeMap<>();

    /**
     * @param workingDir {@code $WORKING_DIR}, where {@code .egret/storage/<n>} is
     * @param entries the staged entries of THIS namespace (the factory selects them)
     */
    EgretNamespace(String tenantId, String namespace, Path workingDir, List<Staged> entries) {
        this.tenantId = tenantId;
        this.namespace = namespace;
        this.workingDir = workingDir;
        this.entries = List.copyOf(entries);
        for (Staged s : this.entries) {
            if (s.latest()) {
                latest.merge(key(s.path()), s, (a, b) -> a.revision() >= b.revision() ? a : b);
            }
        }
    }

    /** A path as egret and Kestra both store it: forward slashes, one leading '/', no '..'. */
    private static String key(String path) {
        return NamespaceFile.toLogicalPath(NamespaceFile.normalize(Path.of(path)));
    }

    private static String key(Path path) {
        if (path == null) {
            throw new IllegalArgumentException("a namespace file path is required, got null");
        }
        return NamespaceFile.toLogicalPath(NamespaceFile.normalize(path));
    }

    private FileNotFoundException notFound(String path, Integer revision) {
        return new FileNotFoundException((revision == null ? "File" : "Revision " + revision + " of file")
            + " '" + path + "' is not a namespace file this task can read in namespace '" + namespace
            + "': egret's plugin host reads only the namespace files named whole in the task's properties "
            + "(an nsfile:// or kestra:///…/_files/… value), which egret staged before the task started "
            + "(W9b spec §10, §15)");
    }

    private static UnsupportedOperationException readOnly(String what) {
        return new UnsupportedOperationException(what + ": " + WHY_READ_ONLY);
    }

    private NamespaceFile file(Staged s) {
        return NamespaceFile.of(namespace, Path.of(key(s.path())), s.revision());
    }

    /**
     * The URI Kestra gives this staged revision ({@code NamespaceFile.of(ns, path, revision).uri()}):
     * what {@link #get} returns and what {@code ShimRunContext.StorageSpec#readableByUri} keys it under
     * (fix round 1, I1).
     */
    static String canonicalUri(Staged s) {
        return NamespaceFile.of(s.namespace(), Path.of(key(s.path())), s.revision()).uri().toString();
    }

    private Path fileOf(Staged s) {
        return workingDir.resolve(s.staged());
    }

    private List<NamespaceFile> latestFiles() {
        return latest.values().stream().map(this::file).toList();
    }

    // --- identity --------------------------------------------------------------------------

    @Override
    public String namespace() {
        return namespace;
    }

    @Override
    public String tenantId() {
        return tenantId;
    }

    // --- reads -----------------------------------------------------------------------------

    @Override
    public List<NamespaceFile> all() {
        return latestFiles();
    }

    /**
     * The latest view's files whose path contains {@code containing}. No directory is ever staged, so
     * {@code includeDirectories} adds none.
     */
    @Override
    public List<NamespaceFile> all(String containing, boolean includeDirectories) {
        return latest.values().stream()
            .filter(s -> containing == null || key(s.path()).contains(containing))
            .map(this::file)
            .toList();
    }

    @Override
    public NamespaceFile get(Path path) throws IOException {
        String k = key(path);
        Staged s = latest.get(k);
        if (s == null) {
            throw notFound(k, null);
        }
        return file(s);
    }

    @Override
    public List<NamespaceFile> findAllFilesMatching(Predicate<Path> predicate) {
        return latestFiles().stream().filter(f -> predicate.test(f.filePath())).toList();
    }

    @Override
    public InputStream getFileContent(Path path, Integer revision) throws IOException {
        String k = key(path);
        Optional<Staged> found = revision == null
            ? Optional.ofNullable(latest.get(k))
            : entries.stream()
                .filter(s -> key(s.path()).equals(k) && s.revision() == revision)
                .max(Comparator.comparing(Staged::latest));
        if (found.isEmpty()) {
            throw notFound(k, revision);
        }
        try {
            return Files.newInputStream(fileOf(found.get()));
        } catch (java.nio.file.NoSuchFileException e) {
            throw notFound(k, revision);
        }
    }

    @Override
    public FileAttributes getFileMetadata(Path path) throws IOException {
        String k = key(path);
        Staged s = latest.get(k);
        if (s == null) {
            throw notFound(k, null);
        }
        BasicFileAttributes a = Files.readAttributes(fileOf(s), BasicFileAttributes.class);
        String name = k.substring(k.lastIndexOf('/') + 1);
        return new Attributes(name, a.lastModifiedTime().toMillis(), a.creationTime().toMillis(), a.size());
    }

    @Override
    public boolean exists(Path path) {
        return latest.containsKey(key(path));
    }

    // --- writes: none ----------------------------------------------------------------------

    @Override
    public List<NamespaceFile> putFile(Path path, InputStream content, Conflicts onAlreadyExist) {
        throw readOnly("cannot write the namespace file '" + path + "' of namespace '" + namespace + "'");
    }

    @Override
    public NamespaceFile createDirectory(Path path) {
        throw readOnly("cannot create the directory '" + path + "' in namespace '" + namespace + "'");
    }

    @Override
    public List<NamespaceFile> delete(Path path) {
        throw readOnly("cannot delete '" + path + "' of namespace '" + namespace + "'");
    }

    @Override
    public List<Pair<NamespaceFile, NamespaceFile>> move(Path source, Path target) {
        throw readOnly("cannot move '" + source + "' to '" + target + "' in namespace '" + namespace + "'");
    }

    /**
     * Fix round 1, M4: over the same latest view as {@link #all()}, as Kestra's
     * {@code NamespaceFileMetadataStateStore.findChildren} filters: the files whose parent path is
     * {@code parentPath + "/"} (or, {@code recursive}, starts with it). No directory is staged, so no
     * directory entry is returned, where Kestra also returns directory rows.
     */
    @Override
    public List<NamespaceFileMetadata> children(String parentPath, boolean recursive) throws IOException {
        String dir = parentPath == null ? "/" : key(parentPath);
        String prefix = dir.endsWith("/") ? dir : dir + "/";
        List<NamespaceFileMetadata> out = new ArrayList<>();
        for (Staged s : latest.values()) {
            String path = key(s.path());
            String parent = NamespaceFileMetadata.parentPath(path);
            if (parent != null && (recursive ? parent.startsWith(prefix) : parent.equals(prefix))) {
                BasicFileAttributes a = Files.readAttributes(fileOf(s), BasicFileAttributes.class);
                out.add(NamespaceFileMetadata.builder()
                    .tenantId(tenantId)
                    .namespace(namespace)
                    .path(path)
                    .revision(s.revision())
                    .size(s.size())
                    .created(a.creationTime().toInstant())
                    .updated(a.lastModifiedTime().toInstant())
                    .build());
            }
        }
        return out;
    }

    private record Attributes(String fileName, long lastModifiedTime, long creationTime, long size)
        implements FileAttributes {
        @Override
        public String getFileName() {
            return fileName;
        }

        @Override
        public long getLastModifiedTime() {
            return lastModifiedTime;
        }

        @Override
        public long getCreationTime() {
            return creationTime;
        }

        @Override
        public FileAttributes.FileType getType() {
            return FileAttributes.FileType.File;
        }

        @Override
        public long getSize() {
            return size;
        }

        @Override
        public Map<String, String> getMetadata() {
            return Map.of();
        }
    }

    /** For the tests: the entries this namespace answers over. */
    List<Staged> entries() {
        return new ArrayList<>(entries);
    }
}
