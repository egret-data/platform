package io.kestra.core.runners;

import java.io.IOException;
import java.net.URI;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.stream.Collectors;

import org.hibernate.validator.messageinterpolation.ParameterMessageInterpolator;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import io.kestra.core.models.Plugin;
import io.kestra.core.models.executions.AbstractMetricEntry;
import io.kestra.core.models.property.PropertyValueExtractor;
import io.kestra.core.models.tasks.Task;
import io.kestra.core.storages.Storage;
import io.kestra.core.storages.kv.KVStore;
import jakarta.validation.ConstraintViolation;
import jakarta.validation.ConstraintViolationException;
import jakarta.validation.Validation;
import jakarta.validation.Validator;

/**
 * The {@link RunContext} one Kestra plugin task runs against, in egret's plugin host, with no
 * Micronaut context (programme spec §3, W5b spec §3).
 *
 * <p>It extends {@link DefaultRunContext}, not {@link RunContext}: {@code RunContextProperty}'s
 * constructor casts to {@code DefaultRunContext} to read the task (spike trap 2). It lives in
 * Kestra's own package because that constructor and {@code setTask} are package-private (trap 3).
 *
 * <ul>
 *   <li>Every {@code render*} returns its input: egret rendered the properties before dispatch.</li>
 *   <li>{@link #logger()} writes {@code LEVEL message} lines to stdout (logback.xml).</li>
 *   <li>{@link #metric} logs one DEBUG line.</li>
 *   <li>{@link #workingDir()} is Kestra's own {@link LocalWorkingDir} under the given directory.</li>
 *   <li>Storage, KV, namespace files, secrets, encryption, ACLs and the application context throw
 *       {@link EgretUnsupported}.</li>
 * </ul>
 */
public class ShimRunContext extends DefaultRunContext {
    static final Logger TASK_LOG = LoggerFactory.getLogger("egret.task");

    private static final class ValidatorHolder {
        // Kestra's own validator, without its Micronaut factory: CustomValidatorFactoryProvider builds
        // Hibernate Validator with ParameterMessageInterpolator, no XML configuration and
        // PropertyValueExtractor (without it, a constraint on Property<T>'s T is a declaration error).
        static final Validator VALIDATOR = Validation.byDefaultProvider()
            .configure()
            .messageInterpolator(new ParameterMessageInterpolator())
            .ignoreXmlConfiguration()
            .addValueExtractor(new PropertyValueExtractor())
            .buildValidatorFactory()
            .getValidator();
    }

    private final WorkingDir workingDir;
    private final List<AbstractMetricEntry<?>> metrics = new ArrayList<>();

    public ShimRunContext(Path workingDirBase) {
        this.workingDir = new LocalWorkingDir(workingDirBase);
        setVariables(Collections.emptyMap());
        setPluginConfiguration(Collections.emptyMap());
    }

    ShimRunContext(Path workingDirBase, Task task) {
        this(workingDirBase);
        setTask(task);
    }

    // --- rendering: identity ------------------------------------------------------------

    @Override
    public String render(String inline) {
        return inline;
    }

    @Override
    public Object renderTyped(String inline) {
        return inline;
    }

    @Override
    public String render(String inline, Map<String, Object> variables) {
        return inline;
    }

    @Override
    public List<String> render(List<String> inline) {
        return inline;
    }

    @Override
    public List<String> render(List<String> inline, Map<String, Object> variables) {
        return inline;
    }

    @Override
    public Set<String> render(Set<String> inline) {
        return inline;
    }

    @Override
    public Set<String> render(Set<String> inline, Map<String, Object> variables) {
        return inline;
    }

    @Override
    public Map<String, Object> render(Map<String, Object> inline) {
        return inline;
    }

    @Override
    public Map<String, Object> render(Map<String, Object> inline, Map<String, Object> variables) {
        return inline;
    }

    @Override
    public Map<String, String> renderMap(Map<String, String> inline) {
        return inline;
    }

    @Override
    public Map<String, String> renderMap(Map<String, String> inline, Map<String, Object> variables) {
        return inline;
    }

    // --- what the shim provides ---------------------------------------------------------

    @Override
    public <T> void validate(T bean) {
        Set<ConstraintViolation<T>> violations = ValidatorHolder.VALIDATOR.validate(bean);
        if (!violations.isEmpty()) {
            throw new ConstraintViolationException(describe(violations), violations);
        }
    }

    static String describe(Set<? extends ConstraintViolation<?>> violations) {
        return violations.stream()
            .map(v -> v.getPropertyPath() + ": " + v.getMessage())
            .sorted()
            .collect(Collectors.joining("; "));
    }

    @Override
    public Logger logger() {
        return TASK_LOG;
    }

    @Override
    public List<AbstractMetricEntry<?>> metrics() {
        return metrics;
    }

    @Override
    public <T> RunContext metric(AbstractMetricEntry<T> metricEntry) {
        metrics.add(metricEntry);
        TASK_LOG.debug("metric {} {} = {}", metricEntry.getType(), metricEntry.getName(), metricEntry.getValue());
        return this;
    }

    @Override
    public WorkingDir workingDir() {
        return workingDir;
    }

    @Override
    public void cleanup() {
        try {
            workingDir.cleanup();
        } catch (IOException e) {
            TASK_LOG.warn("Unable to clean up the working directory: {}", e.getMessage());
        }
    }

    /**
     * A no-op: assets are Kestra lineage metadata (an Enterprise feature). {@code AbstractJdbcBaseQuery}
     * emits one for every {@code CREATE TABLE}, so throwing here would fail a plain DDL statement.
     */
    @Override
    public AssetEmitter assets() {
        return new AssetEmitter() {
            private final List<AssetEmit> emitted = new ArrayList<>();

            @Override
            public void emit(AssetEmit assetEmit) {
                emitted.add(assetEmit);
                TASK_LOG.debug("asset not recorded (egret keeps no Kestra assets): {}", assetEmit);
            }

            @Override
            public List<AssetEmit> emitted() {
                return emitted;
            }
        };
    }

    // --- what egret does not provide ----------------------------------------------------

    @Override
    public Storage storage() {
        throw new EgretUnsupported("storage() needs internal storage (W9)");
    }

    @Override
    public URI logFileURI() {
        throw new EgretUnsupported("logFileURI() needs internal storage (W9)");
    }

    @Override
    public KVStore namespaceKv(String namespace) {
        throw new EgretUnsupported("namespaceKv() needs the KV store (W9)");
    }

    @Override
    public InputAndOutput inputAndOutput() {
        throw new EgretUnsupported("inputAndOutput() needs internal storage (W9)");
    }

    @Override
    public LocalPath localPath() {
        throw new EgretUnsupported("localPath() reads files outside the working directory, which egret's host does not allow");
    }

    @Override
    public String decrypt(String encrypted) {
        throw new EgretUnsupported("decrypt() needs Kestra's secret key, which egret's host does not have");
    }

    @Override
    public String encrypt(String plaintext) {
        throw new EgretUnsupported("encrypt() needs Kestra's secret key, which egret's host does not have");
    }

    @Override
    public AclChecker acl() {
        throw new EgretUnsupported("acl() needs Kestra's namespace ACLs, which egret does not have");
    }

    @Override
    public SDK sdk() {
        throw new EgretUnsupported("sdk() needs a Kestra server, which egret's host does not have");
    }

    @Override
    public RunContext cloneForPlugin(Plugin plugin) {
        throw new EgretUnsupported("cloneForPlugin() (a nested plugin such as a task runner) is not supported by egret's host");
    }

    @Override
    public Services services() {
        throw new EgretUnsupported("services() needs Kestra's application context, which egret's host does not have");
    }

    @Override
    public <T> java.util.Optional<T> getTaskProperty(String name, Class<T> clazz) {
        throw new EgretUnsupported("getTaskProperty() needs Kestra's application context, which egret's host does not have");
    }
}
