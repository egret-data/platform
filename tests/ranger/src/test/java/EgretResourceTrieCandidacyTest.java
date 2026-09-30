import static org.junit.Assert.assertEquals;

import java.util.ArrayList;
import java.util.Collections;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import org.apache.ranger.plugin.model.RangerPolicy;
import org.apache.ranger.plugin.model.RangerServiceDef;
import org.apache.ranger.plugin.model.validation.RangerServiceDefHelper;
import org.apache.ranger.plugin.policyengine.RangerAccessResourceImpl;
import org.apache.ranger.plugin.policyengine.RangerResourceTrie;
import org.apache.ranger.plugin.policyresourcematcher.RangerDefaultPolicyResourceMatcher;
import org.apache.ranger.plugin.policyresourcematcher.RangerPolicyResourceMatcher;
import org.apache.ranger.plugin.policyresourcematcher.RangerResourceEvaluator;
import org.apache.ranger.plugin.resourcematcher.RangerResourceMatcher;
import org.apache.ranger.plugin.util.RangerResourceEvaluatorsRetriever;
import org.junit.Test;

/**
 * SP4d-2b Task 3, fix round 2: which RESOURCE policies {@code
 * getLikelyMatchPolicyEvaluators} returns, measured from the real jar.
 *
 * <p><b>Why this exists, and why the answer is load-bearing now when it was not before.</b>
 * {@code egret-ranger-eval} evaluates every enabled policy of the requested type and lets
 * {@code RangerDefaultPolicyEvaluator.evaluate}'s own matcher decide what happens — which was
 * sound while every consequence of being in the list went through that matcher. Modelling offset
 * 553 of {@code evaluatePoliciesForOneAccessTypeNoAudit}
 * ({@code if (result.getIsAllowed() && !evaluator.hasDeny()) setIsAccessDetermined(true)}) broke
 * that: 553 lives in the ENGINE loop, after {@code evaluate} returns, and reads only
 * {@code hasDeny()}. A policy the trie never returns can therefore end the evaluation in this
 * crate and not in Ranger. So the crate now needs to know, per policy, whether Ranger's candidate
 * filter would have returned it — and what it cannot establish, it refuses.
 *
 * <p>{@code RangerPolicyEngineOptions} reads
 * {@code .policyengine.option.disable.trie.lookup.prefilter} with default {@code false}, so the
 * trie is ON in both plugins and this is the live path, not a configurable one.
 *
 * <p><b>The three claims the Rust side depends on</b>, one row each below:
 *
 * <ol>
 *   <li><b>trie ⊇ matcher.</b> A policy the matcher accepts at {@code SELF} is always returned,
 *       so the crate may let a {@code matched} view determine at 553.
 *   <li><b>NONE with literal values and the request's own key set ⟹ NOT returned.</b> This is the
 *       one that lets the crate answer at all: the ordinary "some other table's policy" is
 *       provably absent from Ranger's list, so it must not determine at 553.
 *   <li><b>ANCESTOR and DESCENDANT ARE returned.</b> Both are {@code MatchType != SELF}, i.e.
 *       {@code !matched} on the Rust side — so "exclude everything the matcher does not match"
 *       would be WRONG, in the direction that lets a later policy decide where Ranger had already
 *       stopped. This row is why the crate's rule is three-valued (returned / not returned /
 *       cannot tell) instead of two.
 * </ol>
 *
 * <p>The def is Trino's four-level {@code catalog/schema/table/column} hierarchy from
 * {@code crates/egret-policy/src/service-defs/trino.json}, including its
 * {@code ignoreCase: true} — the opposite of the {@code tag} def, which is why this measurement
 * cannot be inferred from {@code EgretTagTrieAgreementTest}'s.
 */
public class EgretResourceTrieCandidacyTest {

    /** `hive.sales.orders.id` — the column-level shape a Trino access check has. */
    private static final String[][] REQUEST = {
        {"catalog", "hive"}, {"schema", "sales"}, {"table", "orders"}, {"column", "id"}
    };

    /**
     * policy label → is this policy in {@code RangerResourceEvaluatorsRetriever}'s answer for the
     * request above?
     *
     * <p>Each label is {@code name | catalog/schema/table/column}, with {@code -} for a level the
     * policy does not declare.
     */
    private static Map<String, Boolean> expectedCandidacy() {
        Map<String, Boolean> table = new LinkedHashMap<>();
        // 1. trie ⊇ matcher: the exact match, and the wildcard-column match.
        table.put("self-exact | hive/sales/orders/id", true);
        table.put("self-star-column | hive/sales/orders/*", true);
        // 2. NONE with literal values and the request's key set: provably absent.
        table.put("other-table | hive/sales/other/*", false);
        table.put("other-schema | hive/otherdb/othertable/*", false);
        table.put("other-catalog | otherc/sales/orders/*", false);
        // 3. ANCESTOR and DESCENDANT: present, though the matcher is not SELF.
        table.put("ancestor-schema | hive/sales/-/-", true);
        table.put("descendant-subcolumn | hive/sales/orders/id/sub", true);
        // The shapes the crate refuses rather than classifies, kept as a control that the
        // trie really is WIDER here too and not only on the tag def.
        table.put("endswith-table | hive/sales/*ders/*", true);
        table.put("excludes-table | hive/sales/EXCL:orders/*", true);
        return table;
    }

    @Test
    public void theResourceTrieReturnsExactlyWhatTheCrateAssumes() {
        RangerServiceDef def = trinoServiceDef();

        List<String> labels = new ArrayList<>(expectedCandidacy().keySet());
        List<Ev> evaluators = new ArrayList<>();
        for (int i = 0; i < labels.size(); i++) {
            evaluators.add(new Ev(i, labels.get(i), def));
        }

        // One trie per resource level, exactly as `RangerPolicyRepository.init` builds them.
        Map<String, RangerResourceTrie<Ev>> tries = new HashMap<>();
        for (RangerServiceDef.RangerResourceDef resourceDef : def.getResources()) {
            tries.put(
                resourceDef.getName(), new RangerResourceTrie<>(resourceDef, evaluators));
        }

        RangerAccessResourceImpl resource = new RangerAccessResourceImpl();
        resource.setServiceDef(def);
        for (String[] level : REQUEST) {
            resource.setValue(level[0], level[1]);
        }

        java.util.Collection<Ev> returned =
            RangerResourceEvaluatorsRetriever.getEvaluators(
                tries, resource.getAsMap(), Collections.emptyMap());

        Map<String, Boolean> measured = new LinkedHashMap<>();
        for (int i = 0; i < labels.size(); i++) {
            measured.put(labels.get(i), returned != null && returned.contains(evaluators.get(i)));
        }

        assertEquals(
            "RangerResourceEvaluatorsRetriever's answer for hive.sales.orders.id changed. "
                + "egret_ranger_eval::eval::trie_excludes reads this table: rows that are FALSE "
                + "are the ones it is allowed to skip at offset 553, and any row flipping to TRUE "
                + "is a fail-open in the crate.",
            expectedCandidacy(),
            measured);
    }

    /**
     * The companion half: what {@code getMatchType} says about the same policies. Asserted in the
     * same file so the two columns cannot drift apart, and so the ANCESTOR/DESCENDANT rows are
     * visibly {@code != SELF} rather than being asserted as prose.
     */
    @Test
    public void theMatcherIsStrictlyNarrowerThanTheTrieOnThisRequest() {
        RangerServiceDef def = trinoServiceDef();

        RangerAccessResourceImpl resource = new RangerAccessResourceImpl();
        resource.setServiceDef(def);
        for (String[] level : REQUEST) {
            resource.setValue(level[0], level[1]);
        }

        Map<String, String> measured = new LinkedHashMap<>();
        int i = 0;
        for (String label : expectedCandidacy().keySet()) {
            Ev ev = new Ev(i++, label, def);
            measured.put(
                label,
                String.valueOf(
                    ev.getPolicyResourceMatcher()
                        .getMatchType(resource, Collections.emptyMap(), null)));
        }

        Map<String, String> expected = new LinkedHashMap<>();
        expected.put("self-exact | hive/sales/orders/id", "SELF");
        expected.put("self-star-column | hive/sales/orders/*", "SELF");
        expected.put("other-table | hive/sales/other/*", "NONE");
        expected.put("other-schema | hive/otherdb/othertable/*", "NONE");
        expected.put("other-catalog | otherc/sales/orders/*", "NONE");
        expected.put("ancestor-schema | hive/sales/-/-", "ANCESTOR");
        expected.put("descendant-subcolumn | hive/sales/orders/id/sub", "NONE");
        expected.put("endswith-table | hive/sales/*ders/*", "SELF");
        expected.put("excludes-table | hive/sales/EXCL:orders/*", "NONE");

        assertEquals(
            "getMatchType's answers changed. Read together with the candidacy table above: every "
                + "row where the matcher says SELF must be a TRUE there (trie superset of "
                + "matcher), and the rows where it says ANCESTOR/NONE-but-TRUE are why the crate "
                + "cannot simply exclude what the matcher rejects.",
            expected,
            measured);
    }

    // ── fixtures ─────────────────────────────────────────────────────────

    /**
     * Trino's four data levels, verbatim from
     * {@code crates/egret-policy/src/service-defs/trino.json}: {@code wildCard: true},
     * {@code ignoreCase: true}, {@code excludesSupported: true}, every level {@code mandatory}
     * and the first three {@code isValidLeaf}.
     */
    private static RangerServiceDef trinoServiceDef() {
        RangerServiceDef def = new RangerServiceDef();
        def.setName("trino");

        String[] names = {"catalog", "schema", "table", "column"};
        String[] parents = {"", "catalog", "schema", "table"};
        List<RangerServiceDef.RangerResourceDef> resources = new ArrayList<>();
        for (int i = 0; i < names.length; i++) {
            RangerServiceDef.RangerResourceDef r = new RangerServiceDef.RangerResourceDef();
            r.setItemId((long) (i + 1));
            r.setName(names[i]);
            r.setType("string");
            r.setLevel((i + 1) * 10);
            r.setParent(parents[i]);
            r.setMandatory(true);
            // trino.json sets `isValidLeaf` on the first three levels and OMITS it on
            // `column`. Setting it to FALSE there instead of leaving it null makes the only
            // four-level hierarchy invalid, `init()` fails, and every getMatchType comes back
            // NONE — which is what the first draft did.
            if (i < 3) {
                r.setIsValidLeaf(true);
            }
            r.setExcludesSupported(true);
            r.setMatcher("org.apache.ranger.plugin.resourcematcher.RangerDefaultResourceMatcher");
            Map<String, String> options = new HashMap<>();
            options.put("wildCard", "true");
            options.put("ignoreCase", "true");
            r.setMatcherOptions(options);
            resources.add(r);
        }
        def.setResources(resources);

        RangerServiceDef.RangerAccessTypeDef select = new RangerServiceDef.RangerAccessTypeDef();
        select.setItemId(1L);
        select.setName("select");
        def.setAccessTypes(new ArrayList<>(Collections.singletonList(select)));

        return def;
    }

    /**
     * The minimum {@link RangerResourceEvaluator} the trie and the retriever need. The label
     * carries the policy's resource map so the fixture and the assertion table are the same text.
     */
    private static final class Ev implements RangerResourceEvaluator {
        private final long id;
        private final RangerServiceDef def;
        private final RangerDefaultPolicyResourceMatcher matcher;
        private final Map<String, RangerPolicy.RangerPolicyResource> resources;

        Ev(long id, String label, RangerServiceDef def) {
            this.id = id;
            this.def = def;
            this.resources = parse(label);

            // Order copied from EgretTagTrieAgreementTest: def and helper BEFORE the
            // resources. Setting the resources first leaves `init()` with no hierarchy to
            // validate against and every getMatchType comes back NONE — which the first
            // draft of this file did, and the second assertion below caught.
            RangerDefaultPolicyResourceMatcher m = new RangerDefaultPolicyResourceMatcher();
            m.setServiceDef(def);
            m.setServiceDefHelper(new RangerServiceDefHelper(def, false));
            m.setPolicyResources(this.resources, RangerPolicy.POLICY_TYPE_ACCESS);
            m.init();
            this.matcher = m;
        }

        /** `name | a/b/c/d`, `-` for an undeclared level, `EXCL:` for `isExcludes`. */
        private static Map<String, RangerPolicy.RangerPolicyResource> parse(String label) {
            String[] names = {"catalog", "schema", "table", "column", "subcolumn"};
            String[] values = label.substring(label.indexOf('|') + 1).trim().split("/");
            Map<String, RangerPolicy.RangerPolicyResource> map = new LinkedHashMap<>();
            for (int i = 0; i < values.length && i < names.length; i++) {
                String v = values[i];
                if ("-".equals(v)) {
                    continue;
                }
                boolean excludes = v.startsWith("EXCL:");
                if (excludes) {
                    v = v.substring("EXCL:".length());
                }
                map.put(
                    names[i],
                    new RangerPolicy.RangerPolicyResource(
                        new ArrayList<>(Collections.singletonList(v)), excludes, false));
            }
            return map;
        }

        @Override
        public long getId() {
            return id;
        }

        @Override
        public RangerPolicyResourceMatcher getPolicyResourceMatcher() {
            return matcher;
        }

        @Override
        public Map<String, RangerPolicy.RangerPolicyResource> getPolicyResource() {
            return resources;
        }

        @Override
        public RangerResourceMatcher getResourceMatcher(String resourceName) {
            return matcher == null ? null : matcher.getResourceMatcher(resourceName);
        }

        /**
         * {@code ServiceDefUtil.isAncestorOf(serviceDef, leafResourceDef, resourceDef)},
         * reproduced: walk up from {@code resourceDef} through {@code getParent()} and answer
         * whether this policy's LEAF level is a strict ancestor of it.
         *
         * <p>This is not decoration. {@code RangerResourceTrie}'s build loop (2.5.0 offsets
         * 376-430) does: if the policy declares no value for this level, {@code
         * addInheritedEvaluator(evaluator)} — but ONLY {@code if (evaluator.isAncestorOf(
         * resourceDef))}. Inherited evaluators are unioned into every lookup, so this method IS
         * the mechanism by which a schema-level policy is a candidate for a column request. The
         * first draft returned {@code false} here and measured the stub instead of Ranger: the
         * ancestor row came back {@code false} and would have licensed the crate to skip
         * ancestor policies at offset 553, which is the fail-open this whole round is about.
         */
        @Override
        public boolean isAncestorOf(RangerServiceDef.RangerResourceDef resourceDef) {
            String leaf = leafLevelName();
            if (leaf == null || resourceDef == null) {
                return false;
            }
            for (RangerServiceDef.RangerResourceDef walk = resourceDef;
                    walk != null;
                    walk = resourceDefByName(walk.getParent())) {
                if (leaf.equalsIgnoreCase(walk.getParent())) {
                    return true;
                }
            }
            return false;
        }

        /** The deepest level this policy declares, by the def's own {@code level} ordering. */
        private String leafLevelName() {
            String leaf = null;
            int deepest = Integer.MIN_VALUE;
            for (RangerServiceDef.RangerResourceDef r : def.getResources()) {
                if (resources.containsKey(r.getName()) && r.getLevel() > deepest) {
                    deepest = r.getLevel();
                    leaf = r.getName();
                }
            }
            return leaf;
        }

        private RangerServiceDef.RangerResourceDef resourceDefByName(String name) {
            if (name == null || name.isEmpty()) {
                return null;
            }
            for (RangerServiceDef.RangerResourceDef r : def.getResources()) {
                if (name.equalsIgnoreCase(r.getName())) {
                    return r;
                }
            }
            return null;
        }

        /** 2.8.0 added this to the interface; harmless on 2.5.0, which has no such method. */
        public boolean isLeaf(String resourceName) {
            return false;
        }
    }
}
