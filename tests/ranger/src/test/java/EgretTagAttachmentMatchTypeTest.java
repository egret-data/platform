import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotNull;

import java.lang.reflect.Method;
import java.nio.file.Files;
import java.nio.file.Paths;
import java.util.Arrays;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.Map;
import org.apache.ranger.authorization.utils.JsonUtils;
import org.apache.ranger.plugin.contextenricher.RangerTagEnricher;
import org.apache.ranger.plugin.model.RangerPolicy;
import org.apache.ranger.plugin.model.RangerServiceDef;
import org.apache.ranger.plugin.model.RangerServiceResource;
import org.apache.ranger.plugin.model.validation.RangerServiceDefHelper;
import org.apache.ranger.plugin.policyengine.RangerAccessResourceImpl;
import org.apache.ranger.plugin.policyresourcematcher.RangerPolicyResourceMatcher;
import org.junit.Test;

/**
 * SP4d-2b Task 7 fix round 1: the {@code MatchType} the enricher's matcher produces for ONE stored
 * {@code catalog_tag_assignments} row, measured against the real
 * {@code RangerServiceResourceMatcher}.
 *
 * <p><b>Why this table exists.</b> {@code egret_policy::resolved_tags_for_resource_via} claims to
 * reproduce {@code RangerTagEnricher.findMatchingTags} for the levels egret's storage can express.
 * That claim was checked against the LEVEL COUNT only ({@code
 * EgretTagServiceResourceValidationTest}) and against prose for everything else, and the review of
 * Task 7 measured it false twice: the matcher honours {@code wildCard} and {@code ignoreCase}, and
 * the query's comparison was a literal, case-sensitive {@code =}. A row stored
 * {@code ('sales','*','')} or {@code ('SALES','ORDERS','')} is ADMITTED by the enricher and was
 * returned by neither the query nor the gate that was supposed to catch what the query misses — a
 * set missing a tag the engine enforces, and an empty set is the licence to skip the tag phase.
 *
 * <p>So the two fixes need two different measurements, and both are rows below:
 *
 * <ul>
 *   <li><b>case is REPRODUCED</b> — the query now folds both sides ({@code lower(…) = lower($n)}),
 *       because declining instead would refuse a question egret can answer, and the page's own form
 *       produces mixed case from ordinary typing;
 *   <li><b>wildcards are UNSTORABLE</b> — {@code FilenameUtils.wildcardMatch} is a backtracking
 *       token matcher and SQL {@code LIKE} is not that algorithm. Until SP4d debt Task 3 the Rust
 *       gate refused the resolution while any stored value carried {@code *} or {@code ?}; migration
 *       0166's {@code catalog_tag_assignments_levels_are_present_and_literal} now forbids storing
 *       such a value at all, together with an empty schema or table. The rows below stay, because
 *       they are the MEASUREMENT of what the matcher would do and therefore the reason the
 *       constraint exists;
 * </ul>
 *
 * <p>Every row is the matcher's own answer. {@code findMatchingTags} (2.5.0 offsets 348-377) admits
 * {@code SELF}, {@code SELF_AND_ALL_DESCENDANTS} and {@code ANCESTOR} at the default
 * {@code ResourceMatchingScope.SELF}, so "admitted" below is exactly those three.
 *
 * <p><b>The tag service resource is built the way egret's writer builds it</b> —
 * {@code ServiceTags::from_assignments} writes {@code catalog} unconditionally and each of
 * {@code schema} / {@code table} / {@code column} only when non-empty — so a row of this table is a
 * payload egret can actually serve, not a hand-made one.
 */
public class EgretTagAttachmentMatchTypeTest {

    /** The request every row is measured against: {@code hive.sales.orders.email}. */
    private static final String[][] REQUEST = {
        {"catalog", "hive"}, {"schema", "sales"}, {"table", "orders"}, {"column", "email"}
    };

    /**
     * stored {@code (schema, table, column)} → the matcher's {@code MatchType}, plus whether
     * {@code resolved_tags_for_resource_via} returns it.
     *
     * <p>The second column is the whole point: every {@code ADMITTED|not-returned} row is a
     * fail-open unless something stops it, and the Rust side asserts which mechanism does — the fold
     * for case, migration 0166's {@code CHECK} for the empty-level and wildcard rows, and
     * {@code stored_coordinate_is_fold_divergent} for a fold-divergent value.
     */
    private static Map<String, String> expected() {
        Map<String, String> t = new LinkedHashMap<>();
        // The two levels egret's storage was designed for: reproduced by the query.
        t.put("sales|orders|email", "SELF");
        t.put("sales|orders|", "ANCESTOR");
        // Case: the matcher folds, and since fix round 1 so does the query.
        t.put("SALES|ORDERS|EMAIL", "SELF");
        t.put("SALES|ORDERS|", "ANCESTOR");
        t.put("Sales|Orders|Email", "SELF");
        // Wildcards: the matcher matches, the query cannot, migration 0166 refuses the write.
        t.put("sales|*|", "ANCESTOR");
        t.put("sales|ord*|", "ANCESTOR");
        t.put("*|*|", "ANCESTOR");
        t.put("sales|order?|", "ANCESTOR");
        t.put("sales|orders|ema*", "SELF");
        // The empty-level class, the only one the first gate covered — now also 0166's.
        t.put("sales||", "ANCESTOR");
        t.put("||", "ANCESTOR");
        // The control: a row that genuinely does not apply must be NONE, or every row above
        // is satisfied by a matcher that says yes to everything.
        t.put("other|orders|email", "NONE");
        t.put("sales|other|", "NONE");
        return t;
    }

    @Test
    public void theMatcherHonoursWildcardAndCaseForEveryRowEgretCanStore() throws Exception {
        RangerServiceDef def = trinoServiceDef();
        // `wildCard` and `ignoreCase` are the premise of half the rows, so they are asserted off
        // the parsed def rather than assumed: a def that had them off would make this table
        // measure nothing and still pass most of its rows.
        for (String level : Arrays.asList("catalog", "schema", "table", "column")) {
            RangerServiceDef.RangerResourceDef r =
                    def.getResources().stream()
                            .filter(x -> x.getName().equals(level))
                            .findFirst()
                            .orElse(null);
            assertNotNull("trino.json must carry `" + level + "`", r);
            assertEquals(
                    "`" + level + "` must declare wildCard: true, or the wildcard rows measure nothing",
                    Boolean.TRUE,
                    r.getMatcherOptions().get("wildCard") == null
                            ? null
                            : Boolean.valueOf(r.getMatcherOptions().get("wildCard")));
            assertEquals(
                    "`" + level + "` must declare ignoreCase: true, or the case rows measure nothing",
                    Boolean.TRUE,
                    r.getMatcherOptions().get("ignoreCase") == null
                            ? null
                            : Boolean.valueOf(r.getMatcherOptions().get("ignoreCase")));
        }

        RangerAccessResourceImpl request = new RangerAccessResourceImpl();
        request.setServiceDef(def);
        for (String[] level : REQUEST) {
            request.setValue(level[0], level[1]);
        }

        Method create = createMethod();
        Map<String, String> measured = new LinkedHashMap<>();
        for (String row : expected().keySet()) {
            // A FRESH helper and hierarchies per row, each under its own def name: both cache,
            // and sharing them measures the cache instead of the rule. (The same trap
            // `EgretTagServiceResourceValidationTest` documents.)
            RangerServiceDef perRow = trinoServiceDef();
            perRow.setName("trino-t7-" + row.replace('|', '_').replace('*', 'w').replace('?', 'q'));
            RangerServiceDefHelper helper = new RangerServiceDefHelper(perRow);
            Object matcher = invoke(create, serviceResource(row), helper);
            if (matcher == null) {
                measured.put(row, "DROPPED");
                continue;
            }
            Method getPolicyResourceMatcher = matcher.getClass().getMethod("getPolicyResourceMatcher");
            RangerPolicyResourceMatcher prm =
                    (RangerPolicyResourceMatcher) getPolicyResourceMatcher.invoke(matcher);
            measured.put(
                    row, prm.getMatchType(request, Collections.emptyMap(), null).toString());
        }

        assertEquals(
                "the MatchType per stored row must be exactly this table — it is what "
                        + "`resolved_tags_for_resource_via`'s fold, migration 0166's CHECK and "
                        + "`stored_coordinate_is_fold_divergent` are each measured against",
                expected(),
                measured);
    }

    /**
     * {@code ServiceTags::from_assignments}' own rule: {@code catalog} always, and each deeper
     * level only when the stored value is non-empty.
     */
    private static RangerServiceResource serviceResource(String pipeSeparated) {
        String[] parts = pipeSeparated.split("\\|", -1);
        String schema = parts.length > 0 ? parts[0] : "";
        String table = parts.length > 1 ? parts[1] : "";
        String column = parts.length > 2 ? parts[2] : "";
        Map<String, RangerPolicy.RangerPolicyResource> elements = new LinkedHashMap<>();
        elements.put("catalog", new RangerPolicy.RangerPolicyResource("hive"));
        if (!schema.isEmpty()) {
            elements.put("schema", new RangerPolicy.RangerPolicyResource(schema));
        }
        if (!table.isEmpty()) {
            elements.put("table", new RangerPolicy.RangerPolicyResource(table));
        }
        if (!column.isEmpty()) {
            elements.put("column", new RangerPolicy.RangerPolicyResource(column));
        }
        RangerServiceResource resource = new RangerServiceResource("trino", elements);
        resource.setId(1L);
        return resource;
    }

    private static Object invoke(
            Method create, RangerServiceResource resource, RangerServiceDefHelper helper)
            throws Exception {
        Object hierarchies =
                RangerTagEnricher.ResourceHierarchies.class.getDeclaredConstructor().newInstance();
        return create.getParameterCount() == 3
                ? create.invoke(null, resource, helper, hierarchies)
                : create.invoke(null, resource, helper, hierarchies, null);
    }

    private static Method createMethod() {
        for (Method m : RangerTagEnricher.class.getMethods()) {
            if (m.getName().equals("createRangerServiceResourceMatcher")) {
                return m;
            }
        }
        throw new IllegalStateException("createRangerServiceResourceMatcher not found on this jar");
    }

    private static RangerServiceDef trinoServiceDef() throws Exception {
        String json =
                new String(
                        Files.readAllBytes(
                                Paths.get("../../crates/egret-policy/src/service-defs/trino.json")),
                        "UTF-8");
        RangerServiceDef def = JsonUtils.jsonToObject(json, RangerServiceDef.class);
        assertNotNull("trino.json must parse through Ranger's own JsonUtils", def);
        return def;
    }
}
