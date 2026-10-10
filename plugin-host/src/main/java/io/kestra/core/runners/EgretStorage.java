package io.kestra.core.runners;

import java.io.FileNotFoundException;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.URI;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.nio.file.attribute.BasicFileAttributes;
import java.time.Instant;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.TreeMap;
import java.util.TreeSet;

import io.kestra.core.storages.FileAttributes;
import io.kestra.core.storages.StorageInterface;
import io.kestra.core.storages.StorageObject;

/**
 * egret's storage bridge (W9a spec §9.1): the {@link StorageInterface} a plugin reaches through
 * Kestra's own {@code InternalStorage}. It holds no credential and opens no connection -- egret moves
 * every byte in and out of the pod (spec §5).
 *
 * <ul>
 *   <li><b>Reads</b> see two sets only: the files egret staged before the JVM started --
 *       {@code task.json}'s {@code storage} map, {@code kestra:// URI -> .egret/storage/<n>} -- and the
 *       files this task wrote. Anything else is a {@link FileNotFoundException} naming the URI.</li>
 *   <li><b>Writes</b> go only under {@code baseUri} (this task run's prefix,
 *       {@code kestra:///…/tasks/{task}/{taskRunId}/}) into {@code .egret/out/<relative path>}, which egret
 *       collects after the JVM exits and indexes under {@code baseUri + relative path}. Every byte counts
 *       against {@code maxTaskBytes} ({@code workflow.storage.max_task_bytes}); crossing it fails the
 *       write here, in the pod, naming the cap.</li>
 *   <li><b>Deleting, moving, creating directories</b> touch this task's own written files only; anything
 *       else throws {@link UnsupportedOperationException} with the reason.</li>
 *   <li>The {@code *Instance*} resources are Kestra's instance-wide store, which egret does not have.</li>
 * </ul>
 *
 * <p><b>URIs.</b> A URI is matched on its RAW path (fix round 1, I2): egret never percent-decodes a
 * URI, and a file name may hold a literal {@code %} ({@code inputs/f/a%20b.txt}). {@code kestra:///a/b}
 * and the scheme-less {@code ///a/b} that {@code StorageContext.Task#getContextStorageURI} builds
 * (measured on 2.0.0-rc13: {@code InternalStorage.putFile(stream, name)} passes {@code put} a URI with
 * no scheme) name the same file. A URI with another scheme, or with a segment that is -- or decodes
 * to -- {@code .} or {@code ..}, is refused. A file is written under its raw relative path and
 * {@code put} returns {@code kestra://} + the raw path, as Kestra's own {@code LocalStorage} does;
 * egret collects that file as {@code baseUri + raw relative path}, the same string.
 *
 * <p><b>A write in progress</b> is a {@code .part} file in {@code .egret/tmp}, moved atomically into
 * {@code .egret/out} when complete (fix round 1, M3): a killed JVM never leaves a partial file that
 * egret would collect and index.
 */
final class EgretStorage implements StorageInterface {
    static final String SCHEME = "kestra";
    static final String OUT_DIR = ".egret/out";
    /** Where writes in progress live; egret creates it world-writable beside {@link #OUT_DIR}. */
    static final String TMP_DIR = ".egret/tmp";

    private final Path outDir;
    private final Path tmpDir;
    /** decoded path -> the staged file. */
    private final Map<String, Path> staged = new TreeMap<>();
    /** decoded path -> size, for the files this task wrote. */
    private final Map<String, Long> written = new TreeMap<>();
    /** The decoded path of {@code baseUri}, ending with '/'; {@code null}: egret gave none. */
    private final String basePath;
    private final URI baseUri;
    private final long maxTaskBytes;
    private long writtenBytes;

    /**
     * @param workingDir the directory egret staged into and collects from ({@code $WORKING_DIR})
     * @param stagedFiles {@code kestra:// URI -> path relative to workingDir}
     * @param baseUri this task run's prefix; {@code null} when egret has no internal storage, which makes
     *     every write fail naming that
     * @param maxTaskBytes the most this task may write, all files together
     */
    EgretStorage(Path workingDir, Map<String, String> stagedFiles, URI baseUri, long maxTaskBytes) {
        this.outDir = workingDir.resolve(OUT_DIR).toAbsolutePath().normalize();
        this.tmpDir = workingDir.resolve(TMP_DIR).toAbsolutePath().normalize();
        stagedFiles.forEach((uri, rel) -> staged.put(keyOfStaged(uri), workingDir.resolve(rel)));
        this.baseUri = baseUri;
        if (baseUri == null) {
            this.basePath = null;
        } else {
            String p = baseUri.getRawPath();
            this.basePath = p.endsWith("/") ? p : p + "/";
        }
        this.maxTaskBytes = maxTaskBytes;
    }

    // --- keys ----------------------------------------------------------------------------

    /**
     * egret's own URIs are written as they are, never encoded or decoded: the key is everything after
     * the scheme's {@code kestra://}, the same string {@link URI#getRawPath()} gives a request.
     */
    private static String keyOfStaged(String uri) {
        String prefix = SCHEME + "://";
        return uri.startsWith(prefix) ? uri.substring(prefix.length()) : uri;
    }

    /** The decoded path a request names, or an {@link IllegalArgumentException} naming the URI. */
    private static String keyOf(URI uri) {
        if (uri == null) {
            throw new IllegalArgumentException("a storage URI is required, got null");
        }
        if (uri.getScheme() != null && !uri.getScheme().equals(SCHEME)) {
            throw new IllegalArgumentException(uri + " is not a kestra:// URI");
        }
        String path = uri.getRawPath();
        if (path == null || path.isEmpty() || !path.startsWith("/")) {
            throw new IllegalArgumentException(uri + " has no absolute path");
        }
        for (String segment : path.split("/")) {
            String decoded = segment.replaceAll("(?i)%2e", ".");
            if (decoded.equals("..") || decoded.equals(".")) {
                throw new IllegalArgumentException(uri + " holds a '" + segment + "' segment");
            }
        }
        return path;
    }

    /** A raw key back to a URI: it came from {@link URI#getRawPath()} or egret, so it is already legal. */
    private static URI toUri(String path) {
        return URI.create(SCHEME + "://" + path);
    }

    private Path outFile(String key) {
        Path file = outDir.resolve(key.substring(basePath.length())).normalize();
        if (!file.startsWith(outDir) || file.equals(outDir)) {
            throw new IllegalArgumentException(toUri(key) + " is not a file under " + baseUri);
        }
        return file;
    }

    private boolean underBase(String key) {
        return basePath != null && key.startsWith(basePath) && key.length() > basePath.length();
    }

    /** The prefix a listing or a prefix delete names, as a directory key ending with '/'. */
    private static String dirKey(URI prefix) {
        String key = keyOf(prefix);
        return key.endsWith("/") ? key : key + "/";
    }

    /** Every key egret can read: staged and written. */
    private TreeSet<String> known() {
        TreeSet<String> all = new TreeSet<>(staged.keySet());
        all.addAll(written.keySet());
        return all;
    }

    private Path fileOf(String key) {
        if (written.containsKey(key)) {
            return outFile(key);
        }
        return staged.get(key);
    }

    private FileNotFoundException notFound(URI uri) {
        return new FileNotFoundException(uri + " is not a file this task can read: egret's plugin host "
            + "reads the kestra:// URIs written whole in the task's properties, which egret staged "
            + "before the task started, and the files this task wrote");
    }

    private UnsupportedOperationException notOwn(String what, URI uri) {
        return new UnsupportedOperationException(what + " " + uri + ": egret's plugin host changes only "
            + "the files this task wrote" + (baseUri == null ? "" : " under " + baseUri)
            + (staged.containsKey(safeKey(uri)) ? "; " + uri + " is an input egret staged for it" : ""));
    }

    private static String safeKey(URI uri) {
        try {
            return keyOf(uri);
        } catch (IllegalArgumentException e) {
            return "";
        }
    }

    private static UnsupportedOperationException noInstance(String what) {
        return new UnsupportedOperationException(what + ": Kestra's instance resources are not part of "
            + "egret's internal storage");
    }

    // --- reads ---------------------------------------------------------------------------

    @Override
    public InputStream get(String tenantId, String namespace, URI uri) throws IOException {
        Path file = fileOf(keyOf(uri));
        if (file == null) {
            throw notFound(uri);
        }
        try {
            return Files.newInputStream(file);
        } catch (java.nio.file.NoSuchFileException e) {
            throw notFound(uri);
        }
    }

    @Override
    public StorageObject getWithMetadata(String tenantId, String namespace, URI uri) throws IOException {
        return new StorageObject(Map.of(), get(tenantId, namespace, uri));
    }

    @Override
    public boolean exists(String tenantId, String namespace, URI uri) {
        Path file = fileOf(safeKey(uri));
        return file != null && Files.isRegularFile(file);
    }

    @Override
    public FileAttributes getAttributes(String tenantId, String namespace, URI uri) throws IOException {
        String key = keyOf(uri);
        Path file = fileOf(key);
        if (file != null && Files.isRegularFile(file)) {
            return attributes(name(key), file, FileAttributes.FileType.File);
        }
        String dir = key.endsWith("/") ? key : key + "/";
        if (known().stream().anyMatch(k -> k.startsWith(dir))) {
            return directory(name(key));
        }
        throw notFound(uri);
    }

    @Override
    public List<FileAttributes> list(String tenantId, String namespace, URI prefix) throws IOException {
        String dir = dirKey(prefix);
        Map<String, FileAttributes> children = new LinkedHashMap<>();
        for (String key : known()) {
            if (!key.startsWith(dir)) {
                continue;
            }
            String rest = key.substring(dir.length());
            int slash = rest.indexOf('/');
            if (slash < 0) {
                children.put(rest, attributes(rest, fileOf(key), FileAttributes.FileType.File));
            } else {
                String child = rest.substring(0, slash);
                children.putIfAbsent(child, directory(child));
            }
        }
        if (children.isEmpty()) {
            throw notFound(prefix);
        }
        return new ArrayList<>(children.values());
    }

    @Override
    public List<URI> allByPrefix(String tenantId, String namespace, URI prefix, boolean includeDirectories) throws IOException {
        String dir = dirKey(prefix);
        TreeSet<String> out = new TreeSet<>();
        for (String key : known()) {
            if (!key.startsWith(dir)) {
                continue;
            }
            out.add(key);
            if (includeDirectories) {
                String rest = key.substring(dir.length());
                int at = rest.indexOf('/');
                while (at >= 0) {
                    out.add(dir + rest.substring(0, at + 1));
                    at = rest.indexOf('/', at + 1);
                }
            }
        }
        return out.stream().map(EgretStorage::toUri).toList();
    }

    // --- writes ----------------------------------------------------------------------------

    @Override
    public URI put(String tenantId, String namespace, URI uri, StorageObject storageObject) throws IOException {
        if (basePath == null) {
            throw new IOException("cannot write " + uri + ": egret has no internal storage configured (the "
                + "[storage] section of its configuration), so this task cannot store files");
        }
        String key = keyOf(uri);
        if (!underBase(key)) {
            throw new IOException("cannot write " + uri + ": egret's plugin host writes only under this task "
                + "run's storage prefix " + baseUri);
        }
        Path file;
        try {
            file = outFile(key);
        } catch (IllegalArgumentException e) {
            throw new IOException("cannot write " + uri + ": egret's plugin host writes only under this task "
                + "run's storage prefix " + baseUri + ", into " + OUT_DIR, e);
        }
        Files.createDirectories(file.getParent());
        Files.createDirectories(tmpDir);
        long before = written.getOrDefault(key, 0L);
        long budget = maxTaskBytes - (writtenBytes - before);
        long n = 0;
        Path partial = Files.createTempFile(tmpDir, "put-", ".part");
        try (InputStream in = storageObject.inputStream(); OutputStream out = Files.newOutputStream(partial)) {
            byte[] buf = new byte[64 * 1024];
            int r;
            while ((r = in.read(buf)) != -1) {
                n += r;
                if (n > budget) {
                    throw new IOException("cannot write " + uri + ": this task's files would come to more "
                        + "than the " + maxTaskBytes + " bytes one task run may store "
                        + "(workflow.storage.max_task_bytes)");
                }
                out.write(buf, 0, r);
            }
        } catch (IOException | RuntimeException e) {
            Files.deleteIfExists(partial);
            throw e;
        }
        Files.move(partial, file, StandardCopyOption.REPLACE_EXISTING, StandardCopyOption.ATOMIC_MOVE);
        written.put(key, n);
        writtenBytes = writtenBytes - before + n;
        return toUri(key);
    }

    @Override
    public boolean delete(String tenantId, String namespace, URI uri) throws IOException {
        String key = keyOf(uri);
        if (!written.containsKey(key)) {
            throw notOwn("cannot delete", uri);
        }
        Files.deleteIfExists(outFile(key));
        writtenBytes -= written.remove(key);
        return true;
    }

    @Override
    public List<URI> deleteByPrefix(String tenantId, String namespace, URI prefix) throws IOException {
        String dir = dirKey(prefix);
        if (basePath == null || !dir.startsWith(basePath)) {
            throw notOwn("cannot delete by the prefix", prefix);
        }
        List<URI> deleted = new ArrayList<>();
        for (String key : new ArrayList<>(written.keySet())) {
            if (key.startsWith(dir)) {
                delete(tenantId, namespace, toUri(key));
                deleted.add(toUri(key));
            }
        }
        return deleted;
    }

    @Override
    public URI move(String tenantId, String namespace, URI from, URI to) throws IOException {
        String src = keyOf(from);
        String dst = keyOf(to);
        if (!written.containsKey(src)) {
            throw notOwn("cannot move", from);
        }
        if (!underBase(dst)) {
            throw notOwn("cannot move " + from + " to", to);
        }
        Path target = outFile(dst);
        Files.createDirectories(target.getParent());
        Files.move(outFile(src), target, StandardCopyOption.REPLACE_EXISTING);
        long size = written.remove(src);
        Long replaced = written.put(dst, size);
        if (replaced != null) {
            writtenBytes -= replaced;
        }
        return toUri(dst);
    }

    @Override
    public URI createDirectory(String tenantId, String namespace, URI uri) throws IOException {
        String key = keyOf(uri);
        String dir = key.endsWith("/") ? key : key + "/";
        if (basePath == null || !dir.startsWith(basePath)) {
            throw notOwn("cannot create the directory", uri);
        }
        if (dir.length() > basePath.length()) {
            Files.createDirectories(outFile(dir.substring(0, dir.length() - 1)));
        }
        return toUri(key);
    }

    @Override
    public List<URI> purgeByLastModified(String tenantId, String namespace, URI prefix, Instant startDate,
                                         Instant endDate, boolean dryRun) throws IOException {
        String dir = dirKey(prefix);
        if (basePath == null || !dir.startsWith(basePath)) {
            throw notOwn("cannot purge by the prefix", prefix);
        }
        List<URI> matched = new ArrayList<>();
        for (String key : new ArrayList<>(written.keySet())) {
            if (!key.startsWith(dir)) {
                continue;
            }
            long modified = Files.getLastModifiedTime(outFile(key)).toMillis();
            if ((startDate == null || modified >= startDate.toEpochMilli())
                && (endDate == null || modified <= endDate.toEpochMilli())) {
                matched.add(toUri(key));
            }
        }
        if (!dryRun) {
            for (URI uri : matched) {
                delete(tenantId, namespace, uri);
            }
        }
        return matched;
    }

    // --- instance resources --------------------------------------------------------------

    @Override
    public InputStream getInstanceResource(String namespace, URI uri) {
        throw noInstance("getInstanceResource " + uri);
    }

    @Override
    public List<FileAttributes> listInstanceResource(String namespace, URI uri) {
        throw noInstance("listInstanceResource " + uri);
    }

    @Override
    public boolean existsInstanceResource(String namespace, URI uri) {
        throw noInstance("existsInstanceResource " + uri);
    }

    @Override
    public FileAttributes getInstanceAttributes(String namespace, URI uri) {
        throw noInstance("getInstanceAttributes " + uri);
    }

    @Override
    public URI putInstanceResource(String namespace, URI uri, StorageObject storageObject) {
        throw noInstance("putInstanceResource " + uri);
    }

    @Override
    public boolean deleteInstanceResource(String namespace, URI uri) {
        throw noInstance("deleteInstanceResource " + uri);
    }

    @Override
    public URI createInstanceDirectory(String namespace, URI uri) {
        throw noInstance("createInstanceDirectory " + uri);
    }

    // --- attributes ----------------------------------------------------------------------

    private static String name(String key) {
        String k = key.endsWith("/") ? key.substring(0, key.length() - 1) : key;
        return k.substring(k.lastIndexOf('/') + 1);
    }

    private static FileAttributes attributes(String name, Path file, FileAttributes.FileType type) throws IOException {
        BasicFileAttributes a = Files.readAttributes(file, BasicFileAttributes.class);
        return new Attributes(name, a.lastModifiedTime().toMillis(), a.creationTime().toMillis(), type, a.size());
    }

    private static FileAttributes directory(String name) {
        return new Attributes(name, 0, 0, FileAttributes.FileType.Directory, 0);
    }

    private record Attributes(String fileName, long lastModifiedTime, long creationTime,
                              FileAttributes.FileType type, long size) implements FileAttributes {
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
            return type;
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
}
