import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;

import java.lang.reflect.Method;
import java.nio.file.Files;
import java.nio.file.Paths;
import java.util.Arrays;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import org.apache.ranger.authorization.utils.JsonUtils;
import org.apache.ranger.plugin.contextenricher.RangerTagEnricher;
import org.apache.ranger.plugin.model.RangerPolicy;
import org.apache.ranger.plugin.model.RangerServiceDef;
import org.apache.ranger.plugin.model.RangerServiceResource;
import org.apache.ranger.plugin.model.validation.RangerServiceDefHelper;
import org.junit.Test;

/**
 * SP4d-2b Task 4: which tag service resources {@code RangerTagEnricher} KEEPS, measured by
 * running the real {@code createRangerServiceResourceMatcher} rather than by reading its
 * bytecode.
 *
 * <p><b>Why this needs a probe at all.</b> {@code processServiceTags} branches on exactly one
 * thing — whether {@code createRangerServiceResourceMatcher} returned null — and on that branch
 * it does two destructive things: {@code listIterator.remove()} and
 * {@code serviceTags.getResourceToTagIds().remove(resource.getId())}. So "which key sets return
 * null" IS the drop rule, and {@code egret-ranger-eval}'s {@code service_resource_validates} is a
 * hand-written copy of it. A copy checked only against the prose that produced it is a
 * correlated oracle; this table is measured on the jar and the Rust side asserts the same rows.
 *
 * <p><b>The rule the rows are here to pin, and the one a reader is most likely to get wrong.</b>
 * {@code hierarchyHasAllResources(hierarchy, keySet)} is a SUBSET test — every name in the key
 * set appears somewhere in the hierarchy — and NOT a covering test. So {@code {catalog}} alone
 * validates against Trino's {@code catalog/schema/table/column} chain and is KEPT, even though a
 * one-level tag attachment is not something egret can write. That is why the crate's
 * "fewer than three levels" answer is a REFUSAL by provenance and not a reproduction of a drop:
 * conflating the two would have made egret answer where Ranger drops, and drop where Ranger
 * answers.
 *
 * <p><b>Nothing is hand-built that Ranger can build itself.</b> The service-def is
 * {@code crates/egret-policy/src/service-defs/trino.json}, parsed by Ranger's own
 * {@code JsonUtils}, and the helper, the hierarchies and the matcher are Ranger's. The only
 * reflection is on the method's ARITY: 2.5.0 takes three arguments and 2.8.0 added a fourth
 * ({@code RangerPluginContext}), and one source file has to compile against both.
 */
public class EgretTagServiceResourceValidationTest {

    /**
     * {@code resourceElements} key set → {@code kept} or {@code dropped}.
     *
     * <p>Rows 1-3 are what egret's own {@code ServiceTags::from_assignments} writes (column
     * level, table level) plus the two shorter prefixes it cannot write. Rows 4-6 are the
     * shapes that actually drop: a name from another engine's vocabulary, a name from no
     * vocabulary, and a key set that straddles two hierarchies.
     */
    private static Map<String, String> expected() {
        Map<String, String> t = new LinkedHashMap<>();
        // Egret writes these two.
        t.put("catalog,schema,table,column", "kept");
        t.put("catalog,schema,table", "kept");
        // Egret cannot write these two, and Ranger keeps them anyway — the subset test.
        t.put("catalog,schema", "kept");
        t.put("catalog", "kept");
        // The empty key set validates vacuously: hierarchyHasAllResources iterates the key set.
        t.put("", "kept");
        // StarRocks' middle level against Trino's def: in no hierarchy of any policy type.
        t.put("catalog,database,table,column", "dropped");
        t.put("database", "dropped");
        // A name in no vocabulary at all.
        t.put("bogus", "dropped");
        t.put("catalog,schema,table,column,bogus", "dropped");
        // Two levels that exist but never in ONE hierarchy: `column` belongs to the
        // catalog/schema/table/column chain and `queryid` is its own isolated resource.
        t.put("catalog,queryid", "dropped");
        return t;
    }

    @Test
    public void createRangerServiceResourceMatcherDropsExactlyTheInvalidKeySets() throws Exception {
        RangerServiceDef def = trinoServiceDef();
        RangerServiceDefHelper helper = new RangerServiceDefHelper(def);
        Method create = createMethod();

        Map<String, String> measured = new LinkedHashMap<>();
        for (String keys : expected().keySet()) {
            // A FRESH ResourceHierarchies per row: it is a cache keyed on the key set, so
            // sharing one across rows would measure the cache and not the rule.
            Object hierarchies = RangerTagEnricher.ResourceHierarchies.class
                    .getDeclaredConstructor()
                    .newInstance();
            RangerServiceResource resource = serviceResource(keys);
            Object matcher =
                    create.getParameterCount() == 3
                            ? create.invoke(null, resource, helper, hierarchies)
                            : create.invoke(null, resource, helper, hierarchies, null);
            measured.put(keys, matcher == null ? "dropped" : "kept");
        }

        assertEquals(
                "the key sets Ranger keeps and drops must be exactly the table the Rust "
                        + "`service_resource_validates` reproduces",
                expected(),
                measured);
    }

    /**
     * The falsifier for the row that matters most. If {@code hierarchyHasAllResources} were the
     * covering test a reader expects, {@code {catalog}} would be DROPPED and egret's
     * "fewer than three levels" refusal would be reproducing a Ranger behaviour instead of
     * adding one. It is not, and this says so on its own so the claim does not rest on a
     * whole-map comparison a future edit could quietly weaken.
     */
    @Test
    public void aSingleLevelKeySetIsKeptBecauseTheHierarchyTestIsASubsetTest() throws Exception {
        RangerServiceDefHelper helper = new RangerServiceDefHelper(trinoServiceDef());
        Method create = createMethod();
        Object hierarchies = RangerTagEnricher.ResourceHierarchies.class
                .getDeclaredConstructor()
                .newInstance();
        RangerServiceResource resource = serviceResource("catalog");
        Object matcher =
                create.getParameterCount() == 3
                        ? create.invoke(null, resource, helper, hierarchies)
                        : create.invoke(null, resource, helper, hierarchies, null);
        assertNotNull(
                "{catalog} is a SUBSET of catalog/schema/table/column, so Ranger keeps it",
                matcher);
    }

    /**
     * The oracle for the THREE-policy-type loop — fix round 1, ruling 2.
     *
     * <p>{@code createRangerServiceResourceMatcher} loops
     * {@code RangerPolicy.POLICY_TYPES} and keeps a resource that validates under ANY of them.
     * On every service-def egret ships that loop is <b>unobservable</b>: `trino.json`'s and
     * `starrocks.json`'s {@code dataMaskDef}/{@code rowFilterDef} resource names are SUBSETS of
     * their access names, so any key set valid for masking is valid for access and a one-type
     * loop answers identically. "Right because Ranger does it" is exactly the unfalsifiable
     * assertion this package keeps catching, so the loop needs a case where the types disagree.
     *
     * <p><b>No real def discriminates, so this def is CONSTRUCTED</b>, and it is constructed by
     * adding ONE resource to the real {@code trino.json}: a {@code dataMaskDef} entry named
     * {@code maskonly} with no counterpart in {@code resources}. Everything else is the shipped
     * file, and the helper, the hierarchies and the matcher are still Ranger's. The key set
     * {@code {maskonly}} then validates under policy type 1 and under no other, so a resource
     * carrying it survives if and only if the loop really consults the masking hierarchies.
     */
    @Test
    public void aKeySetValidUnderMaskingOnlySurvivesBecauseTheLoopVisitsAllThreeTypes()
            throws Exception {
        RangerServiceDef def = trinoServiceDef();
        RangerServiceDef.RangerResourceDef maskOnly = new RangerServiceDef.RangerResourceDef();
        maskOnly.setName("maskonly");
        maskOnly.setType("string");
        maskOnly.setLevel(10);
        maskOnly.setMandatory(Boolean.TRUE);
        maskOnly.setIsValidLeaf(Boolean.TRUE);
        maskOnly.setMatcher("org.apache.ranger.plugin.resourcematcher.RangerDefaultResourceMatcher");
        def.getDataMaskDef().getResources().add(maskOnly);
        // ⚠ RENAMED, and this is not cosmetic. `RangerServiceDefHelper`'s constructor caches
        // its `Delegate` in a static map keyed on `serviceDef.getName()` + `getUpdateTime()`
        // (offsets 37-43, 83, 170). Leaving the name as `trino` makes this helper a CACHE HIT on
        // whatever the other test methods in this class built first, so the added resource is
        // silently absent and the assertion below passes or fails by JUnit's method order. It
        // failed exactly that way on the first run of this test.
        def.setName("trino-maskonly-synthetic");

        RangerServiceDefHelper helper = new RangerServiceDefHelper(def);
        Method create = createMethod();

        assertNotNull(
                "{maskonly} validates under policy type 1 only, so it survives exactly when the "
                        + "loop over RangerPolicy.POLICY_TYPES reaches the masking hierarchies",
                invoke(create, serviceResource("maskonly"), helper));

        // The falsifier, on the SAME helper: a name in no hierarchy of any type still drops, so
        // the row above is about `maskonly` being mask-only and not about the def being permissive.
        assertNull(
                "a name in no hierarchy of any policy type must still drop",
                invoke(create, serviceResource("bogus"), helper));
    }

    private static Object invoke(Method create, RangerServiceResource resource, RangerServiceDefHelper helper)
            throws Exception {
        // A FRESH ResourceHierarchies per call: it caches by key set.
        Object hierarchies = RangerTagEnricher.ResourceHierarchies.class
                .getDeclaredConstructor()
                .newInstance();
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

    private static RangerServiceResource serviceResource(String commaSeparatedKeys) {
        Map<String, RangerPolicy.RangerPolicyResource> elements = new LinkedHashMap<>();
        if (!commaSeparatedKeys.isEmpty()) {
            for (String key : commaSeparatedKeys.split(",")) {
                elements.put(key, new RangerPolicy.RangerPolicyResource("v"));
            }
        }
        RangerServiceResource resource = new RangerServiceResource("trino", elements);
        resource.setId(1L);
        return resource;
    }

    private static RangerServiceDef trinoServiceDef() throws Exception {
        String json =
                new String(
                        Files.readAllBytes(
                                Paths.get("../../crates/egret-policy/src/service-defs/trino.json")),
                        "UTF-8");
        RangerServiceDef def = JsonUtils.jsonToObject(json, RangerServiceDef.class);
        assertNotNull("trino.json must parse through Ranger's own JsonUtils", def);
        List<String> names =
                Arrays.asList(
                        "catalog", "schema", "table", "column", "queryid", "sessionproperty");
        for (String name : names) {
            assertNotNull(
                    "the parsed def must carry `" + name + "`, or this table measures a stub",
                    def.getResources().stream()
                            .filter(r -> r.getName().equals(name))
                            .findFirst()
                            .orElse(null));
        }
        return def;
    }
}
