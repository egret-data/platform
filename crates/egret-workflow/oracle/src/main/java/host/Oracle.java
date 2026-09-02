package host;

import java.io.IOException;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Collections;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.UUID;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.node.ArrayNode;
import com.fasterxml.jackson.databind.node.ObjectNode;

import io.kestra.core.runners.pebble.Extension;
import io.kestra.core.runners.pebble.JsonWriter;
import io.pebbletemplates.pebble.PebbleEngine;
import io.pebbletemplates.pebble.extension.Filter;
import io.pebbletemplates.pebble.extension.Function;
import io.pebbletemplates.pebble.extension.Test;
import io.pebbletemplates.pebble.template.PebbleTemplate;

/**
 * Standalone Java harness for the W2 expression-language oracle. Not a Kestra
 * plugin -- built and run outside Micronaut, against Kestra's own
 * io.kestra.core.runners.pebble.Extension (public no-arg constructor).
 *
 * <p>Two modes, see crates/egret-workflow/oracle/README.md for how these are used:
 * <ul>
 *   <li>{@code dump}: prints the filter/function/test names the built engine
 *       actually carries, with counts -- Task 1 Step 2.</li>
 *   <li>{@code generate &lt;corpus.json&gt; &lt;golden.json&gt;}: renders each corpus
 *       entry through the real engine and writes the golden file -- Task 1 Step 4.</li>
 * </ul>
 */
public final class Oracle {
    /**
     * VariableRenderer.RAW_PATTERN, copied verbatim from
     * core/src/main/java/io/kestra/core/runners/VariableRenderer.java:23.
     *
     * <p>{@code raw} is NOT a Pebble tag -- Pebble 4.1.2 throws
     * {@code ParserException: Unexpected tag name "raw"} -- and Kestra registers no
     * custom tags (Extension.getTokenParsers() returns null). Kestra supports it one
     * layer ABOVE Pebble, in VariableRenderer, by substituting raw regions out of the
     * string before compiling and putting them back after evaluating. This harness
     * originally omitted that (documented at the time as "no corpus entry needs it");
     * the corpus now has raw entries, so the protection is reproduced here.
     *
     * <p>Note there is no DOTALL flag, so {@code .} does not match a newline: a raw
     * region spanning lines is NOT protected, and Pebble then fails on the unknown
     * tag. That is real Kestra behaviour, pinned by the {@code raw-multiline} entry.
     */
    private static final Pattern RAW_PATTERN =
        Pattern.compile("(\\{%-*\\s*raw\\s*-*%}(.*?)\\{%-*\\s*endraw\\s*-*%})");

    private Oracle() {
    }

    public static void main(String[] args) throws Exception {
        if (args.length == 0) {
            System.err.println("usage: dump | generate <corpus.json> <golden.json>");
            System.exit(2);
            return;
        }

        switch (args[0]) {
            case "dump" -> dump();
            case "generate" -> {
                if (args.length != 3) {
                    System.err.println("usage: generate <corpus.json> <golden.json>");
                    System.exit(2);
                    return;
                }
                generate(Path.of(args[1]), Path.of(args[2]));
            }
            default -> {
                System.err.println("unknown mode: " + args[0]);
                System.exit(2);
            }
        }
    }

    /**
     * Builds Kestra's Extension standalone. Its @Inject fields (secretFunction,
     * kvFunction, readFileFunction, fileURIFunction, fileSizeFunction,
     * isFileEmptyFunction, fileExistsFunction, httpFunction) are left null by this
     * no-arg construction -- Micronaut never runs to populate them. getFunctions()
     * still `put`s their NAME keys with a null Function value (see the source: none
     * of those eight puts is guarded by a null check, unlike renderFunction /
     * renderOnceFunction / errorLogsFunction / subflowFunction, which ARE guarded
     * and so are silently absent from a standalone build). Calling one of the
     * null-valued eight fails with a NullPointerException, not a graceful
     * "service unavailable" error -- see oracle/README.md.
     */
    private static Extension buildExtension() {
        return new Extension();
    }

    /**
     * newPebbleEngineBuilder()'s settings, from PebbleEngineFactory (the real,
     * Micronaut-driven builder Kestra uses in production), reproduced here by hand
     * since that factory itself needs an ApplicationContext this harness doesn't
     * have.
     *
     * <p>`strictVariables(true)` is a DELIBERATE DEVIATION from the task brief's
     * literal `.strictVariables(false)`. Empirically (see oracle/README.md):
     * with `false`, {@code ???} never throws AttributeNotFoundException for a
     * genuinely-absent root variable (lenient mode resolves it to null before the
     * operator's catch block ever runs), so it can never fall back -- which
     * defeats the corpus's own `undef-coalesce-absent` entry, the one case that
     * is supposed to prove {@code ???} treats "absent" differently from
     * "explicitly null" (unlike {@code ??}, which treats both the same). Only
     * `strictVariables(true)` -- production's actual setting -- reproduces that
     * distinction, so the oracle would otherwise be silently wrong about the one
     * operator pair the brief calls out as most likely to be gotten wrong.
     *
     * <p>`allowOverrideCoreOperators(true)` is NOT optional: without it Kestra's
     * custom `>`, `>=`, `<`, `<=`, `??`, `???`, `isIn` operators registered by
     * Extension.getBinaryOperators() would collide with Pebble's built-ins.
     */
    private static PebbleEngine buildEngine(Extension extension) {
        return new PebbleEngine.Builder()
            .extension(extension)
            .strictVariables(true)
            .cacheActive(false)
            .newLineTrimming(false)
            .autoEscaping(false)
            .allowOverrideCoreOperators(true)
            .build();
    }

    private static void dump() {
        Extension extension = buildExtension();

        Map<String, Filter> filters = extension.getFilters();
        Map<String, Function> functions = extension.getFunctions();
        Map<String, Test> tests = extension.getTests();

        List<String> filterNames = new ArrayList<>(filters.keySet());
        List<String> functionNames = new ArrayList<>(functions.keySet());
        List<String> testNames = new ArrayList<>(tests.keySet());
        Collections.sort(filterNames);
        Collections.sort(functionNames);
        Collections.sort(testNames);

        List<String> nullFunctions = new ArrayList<>();
        for (String name : functionNames) {
            if (functions.get(name) == null) {
                nullFunctions.add(name);
            }
        }
        Collections.sort(nullFunctions);

        System.out.println("filters=" + filterNames.size());
        for (String n : filterNames) {
            System.out.println("  filter: " + n);
        }
        System.out.println("functions=" + functionNames.size());
        for (String n : functionNames) {
            System.out.println("  function: " + n + (functions.get(n) == null ? "  [NULL Function object -- see README]" : ""));
        }
        System.out.println("tests=" + testNames.size());
        for (String n : testNames) {
            System.out.println("  test: " + n);
        }
        System.out.println("null-valued functions=" + nullFunctions.size() + " " + nullFunctions);
    }

    private static void generate(Path corpusPath, Path goldenPath) throws IOException {
        ObjectMapper mapper = new ObjectMapper();
        JsonNode corpus = mapper.readTree(corpusPath.toFile());
        if (!corpus.isArray()) {
            throw new IllegalArgumentException("corpus.json must be a JSON array, got: " + corpus.getNodeType());
        }

        Extension extension = buildExtension();
        PebbleEngine engine = buildEngine(extension);

        ArrayNode golden = mapper.createArrayNode();
        int errors = 0;
        for (JsonNode entry : corpus) {
            String id = entry.get("id").asText();
            String expr = entry.get("expr").asText();
            JsonNode context = entry.has("context") ? entry.get("context") : mapper.createObjectNode();

            ObjectNode out = mapper.createObjectNode();
            out.put("id", id);
            out.put("expr", expr);
            out.set("context", context.deepCopy());

            Map<String, Object> variables = toMap(mapper, context);

            try {
                String rendered = render(engine, expr, variables);
                out.put("result", rendered);
            } catch (Throwable t) {
                errors++;
                ObjectNode error = mapper.createObjectNode();
                error.put("error", t.getClass().getName() + ": " + t.getMessage());
                out.set("result", error);
            }

            golden.add(out);
        }

        mapper.writerWithDefaultPrettyPrinter().writeValue(goldenPath.toFile(), golden);
        System.out.println("wrote " + golden.size() + " golden entries (" + errors + " errors) to " + goldenPath);
    }

    /**
     * Mirrors VariableRenderer.render(String, Map)'s default path:
     *
     * <ol>
     *   <li>the {@code inlineStr.indexOf('{') == -1} short-circuit
     *       (VariableRenderer.java:76-79) -- a source with no brace at all is not a
     *       template and is returned unchanged;</li>
     *   <li>recursive re-render (stop once output stops changing, matching
     *       MAX_RENDERING_AMOUNT=100) through a JsonWriter (stringify=true), which is
     *       why a filter/function returning a Map/List/array renders as its JSON string
     *       form rather than a raw toString() -- see the jq finding in
     *       oracle/README.md;</li>
     *   <li>{@code RAW_PATTERN.matcher(x).replaceAll("$2")} (VariableRenderer.java:85-87),
     *       which strips the raw markers and keeps the body.</li>
     * </ol>
     *
     * <p>Each pass is renderOnce (VariableRenderer.java:93-140): protect raw regions
     * with UUID placeholders, evaluate, put them back WITH their markers so the next
     * pass protects them again.
     */
    private static String render(PebbleEngine engine, String expr, Map<String, Object> variables) throws IOException {
        if (expr.indexOf('{') == -1) {
            return expr;
        }

        Object inline = expr;
        for (int i = 0; i < 100; i++) {
            Object result = renderOnce(engine, (String) inline, variables);

            if (result == null) {
                return "";
            }
            if (Objects.equals(result, inline)) {
                return RAW_PATTERN.matcher((String) result).replaceAll("$2");
            }
            inline = result;
        }
        throw new IllegalStateException("Too many rendering attempts (>100) for: " + expr);
    }

    /** VariableRenderer.renderOnce, stringify=true. */
    private static Object renderOnce(PebbleEngine engine, String inline, Map<String, Object> variables) throws IOException {
        Matcher rawMatcher = RAW_PATTERN.matcher(inline);
        Map<String, String> replacers = new HashMap<>();
        String protectedSource = rawMatcher.replaceAll(matchResult -> {
            String uuid = UUID.randomUUID().toString();
            replacers.put(uuid, matchResult.group(1));
            return uuid;
        });

        PebbleTemplate compiled = engine.getLiteralTemplate(protectedSource);
        JsonWriter writer = new JsonWriter();
        compiled.evaluate(writer, variables);
        Object result = writer.output();

        if (result instanceof String stringValue) {
            for (Map.Entry<String, String> entry : replacers.entrySet()) {
                stringValue = stringValue.replace(entry.getKey(), entry.getValue());
            }
            return stringValue;
        }

        return result;
    }

    @SuppressWarnings("unchecked")
    private static Map<String, Object> toMap(ObjectMapper mapper, JsonNode context) {
        return mapper.convertValue(context, Map.class);
    }
}
