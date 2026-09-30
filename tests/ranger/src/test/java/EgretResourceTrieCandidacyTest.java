import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

import java.nio.file.Files;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import org.apache.ranger.authorization.utils.JsonUtils;
import org.apache.ranger.plugin.model.RangerPolicy;
import org.apache.ranger.plugin.model.RangerServiceDef;
import org.apache.ranger.plugin.policyengine.RangerAccessResourceImpl;
import org.apache.ranger.plugin.policyengine.RangerPolicyEngineOptions;
import org.apache.ranger.plugin.policyengine.RangerResourceTrie;
import org.apache.ranger.plugin.policyevaluator.RangerDefaultPolicyEvaluator;
import org.apache.ranger.plugin.policyevaluator.RangerPolicyEvaluator;
import org.apache.ranger.plugin.policyresourcematcher.RangerPolicyResourceMatcher;
import org.apache.ranger.plugin.util.RangerResourceEvaluatorsRetriever;
import org.apache.ranger.plugin.util.ServiceDefUtil;
import org.junit.Test;

/**
 * SP4d-2b Task 3: which RESOURCE policies {@code getLikelyMatchPolicyEvaluators} returns,
 * measured from the real jar.
 *
 * <p><b>Why this exists, and why the answer is load-bearing now when it was not before.</b>
 * {@code egret-ranger-eval} evaluates every enabled policy of the requested type and lets
 * {@code RangerDefaultPolicyEvaluator.evaluate}'s own matcher decide what happens — which was
 * sound while every consequence of being in the list went through that matcher. Modelling offset
 * 553 of {@code evaluatePoliciesForOneAccessTypeNoAudit}
 * ({@code if (result.getIsAllowed() && !evaluator.hasDeny()) setIsAccessDetermined(true)}) broke
 * that: 553 lives in the ENGINE loop, after {@code evaluate} returns, and reads only
 * {@code hasDeny()}. A policy the trie never returns can therefore end the evaluation in this
 * crate and not in Ranger, and a policy it DOES return must be allowed to. So the crate needs
 * this table in both directions, and what it cannot establish it refuses.
 *
 * <p>{@code RangerPolicyEngineOptions.configureForPlugin} reads
 * {@code .policyengine.option.disable.trie.lookup.prefilter} with {@code iconst_0} as its
 * default, so the trie is ON in both plugins and this is the live path, not a configurable one.
 *
 * <p><b>Nothing here is hand-built, and that is the point (FIX ROUND 3).</b> The first version of
 * this file constructed its own {@code RangerServiceDef} and its own
 * {@link org.apache.ranger.plugin.policyresourcematcher.RangerResourceEvaluator} — including a
 * hand-written {@code isAncestorOf}, <em>the very gate row 6 measures</em>. Two drafts of it
 * measured the fixture instead of Ranger: one stubbed {@code isAncestorOf} to {@code false} and
 * made ANCESTOR look ABSENT (which would have licensed exactly the wrong fix), and one set
 * {@code isValidLeaf(false)} on {@code column}, where {@code trino.json} omits the field, which
 * invalidated every hierarchy and made every {@code getMatchType} answer {@code NONE}. Both were
 * caught only because a second assertion disagreed.
 *
 * <p>So this version hand-builds nothing that Ranger can build itself:
 *
 * <ul>
 *   <li>the service-def is {@code crates/egret-policy/src/service-defs/trino.json}, the file the
 *       plugin actually downloads, parsed by Ranger's own {@code JsonUtils} — all 13 resource
 *       defs, not a hand-written four;
 *   <li>the evaluators are real {@link RangerDefaultPolicyEvaluator}s built from real
 *       {@link RangerPolicy} objects, so {@code isAncestorOf}, {@code leafResourceDef} and the
 *       resource matchers are Ranger's;
 *   <li>the tries are built over {@code getResourceEvaluators()} and queried through
 *       {@link RangerResourceEvaluatorsRetriever}, which is what
 *       {@code RangerPolicyRepository.getLikelyMatchPolicyEvaluators(Map, request)} invokes.
 * </ul>
 *
 * <p><b>What the crate reads off this table:</b>
 *
 * <ol>
 *   <li><b>trie ⊇ matcher.</b> {@code SELF} is always present, so a {@code matched} view may end
 *       the evaluation at 553.
 *   <li><b>{@code ANCESTOR} is PRESENT</b> — {@code RangerResourceTrie}'s build loop adds a
 *       policy declaring no value for a level as an <em>inherited</em> evaluator when
 *       {@code isAncestorOf(resourceDef)}, and inherited evaluators are unioned into every
 *       lookup. That is the ordinary table-level grant against a column request, and the crate
 *       determines on it rather than refusing.
 *   <li><b>{@code NONE} with the request's own key set and literal values is ABSENT</b>, which is
 *       what lets ordinary requests answer at all.
 *   <li><b>{@code isExcludes} and mid-name wildcards are PRESENT while the matcher rejects
 *       them</b>, which is why the crate's rule is three-valued (present / absent / cannot tell)
 *       and not two.
 * </ol>
 */
public class EgretResourceTrieCandidacyTest {

    /** `hive.sales.orders.id` — the column-level shape a Trino access check has. */
    private static final String[][] COLUMN_REQUEST = {
        {"catalog", "hive"}, {"schema", "sales"}, {"table", "orders"}, {"column", "id"}
    };

    /** `hive.sales.orders` — the table-level shape a row-filter check has. */
    private static final String[][] TABLE_REQUEST = {
        {"catalog", "hive"}, {"schema", "sales"}, {"table", "orders"}
    };

    /**
     * policy label → {@code present|matchType}, where {@code present} is membership of
     * {@link RangerResourceEvaluatorsRetriever}'s answer and {@code matchType} is what the
     * policy's own matcher says. Both columns in one map so they cannot drift apart.
     *
     * <p>The label is {@code name | catalog/schema/table/column}, with {@code -} for a level the
     * policy does not declare and {@code EXCL:} for {@code isExcludes}.
     */
    private static Map<String, String> expectedForColumnRequest() {
        Map<String, String> t = new LinkedHashMap<>();
        // 1. trie superset of matcher.
        t.put("self-exact | hive/sales/orders/id", "present|SELF");
        t.put("self-star-column | hive/sales/orders/*", "present|SELF");
        // 2. ANCESTOR — the ordinary table-level grant. PRESENT, and not SELF.
        t.put("ancestor-table | hive/sales/orders/-", "present|ANCESTOR");
        t.put("ancestor-schema | hive/sales/-/-", "present|ANCESTOR");
        t.put("ancestor-catalog | hive/-/-/-", "present|ANCESTOR");
        // 3. NONE with the request's own key set and literal values: provably absent.
        t.put("other-table | hive/sales/other/*", "absent|NONE");
        t.put("other-schema | hive/otherdb/othertable/*", "absent|NONE");
        t.put("other-catalog | otherc/sales/orders/*", "absent|NONE");
        // …and an ancestor of ANOTHER branch is absent too, so row 2 is about the hierarchy
        // and not about "declares fewer levels".
        t.put("ancestor-other-schema | hive/otherdb/-/-", "absent|NONE");
        // 4. Present while the matcher rejects: the shapes the crate can only refuse.
        t.put("endswith-table | hive/sales/*ders/*", "present|SELF");
        t.put("excludes-table | hive/sales/EXCL:orders/*", "present|NONE");
        // FIX ROUND 3, from a SURVIVING mutation: a policy that declares only a
        // PREFIX of the request's levels AND carries `isExcludes` on one of
        // them. Match type NONE (so the crate cannot call it a candidate), yet
        // present twice over — inherited at `schema` because of the excludes,
        // and inherited at `table`/`column` because of `isAncestorOf`. It is the
        // shape that makes "the policy does not declare this level" a fail-open
        // if read as a proof of absence.
        t.put("excludes-schema-partial | hive/EXCL:sales/-/-", "present|NONE");
        // FIX ROUND 4, the two conditions of `level_excluded_by_trie` that no
        // Rust test pinned — each is a shape where the trie is WIDER than the
        // matcher for a reason of its own, so reading the matcher's non-match as
        // absence would skip a policy Ranger holds.
        //
        // `isRecursive`: `RangerResourceTrie.insert` ends
        // `if (isWildcard || isRecursive) addWildcardEvaluator(...)` and the
        // caller passes `getIsRecursive()` with NO `recursiveSupported` check,
        // so `sale` becomes a PREFIX in the trie and matches `sales`, while
        // `RangerDefaultResourceMatcher` — which contains no reference to the
        // flag at all — still tests it exactly.
        t.put("recursive-schema | hive/REC:sale/-/-", "present|NONE");
        // A leading wildcard: `getNonWildcardPrefix` is the empty string, so the
        // value is parked at the trie ROOT and returned for every request value.
        t.put("leading-wildcard-table | hive/sales/*xyz/*", "present|NONE");
        return t;
    }

    /**
     * The same policies against a TABLE-level request, which is the only way to measure a real
     * {@code DESCENDANT} — a policy naming MORE levels than the request.
     *
     * <p>The crate does NOT use this row to determine: `DESCENDANT` is left in the "cannot tell"
     * bucket deliberately, because a descendant policy can also carry `isExcludes` or a wildcard
     * at the deeper level and this table measures one shape of it. It is here so the claim is
     * recorded rather than assumed, and so a future narrowing has its measurement already.
     */
    private static Map<String, String> expectedForTableRequest() {
        Map<String, String> t = new LinkedHashMap<>();
        t.put("self-exact | hive/sales/orders/id", "present|DESCENDANT");
        // A trailing `*` is SELF_AND_ALL_DESCENDANTS, not DESCENDANT — which the crate's
        // `matched` already accepts. Predicted DESCENDANT here and measured otherwise; the
        // measured value stands.
        t.put("self-star-column | hive/sales/orders/*", "present|SELF_AND_ALL_DESCENDANTS");
        t.put("ancestor-table | hive/sales/orders/-", "present|SELF");
        t.put("ancestor-schema | hive/sales/-/-", "present|ANCESTOR");
        t.put("ancestor-catalog | hive/-/-/-", "present|ANCESTOR");
        t.put("other-table | hive/sales/other/*", "absent|NONE");
        t.put("other-schema | hive/otherdb/othertable/*", "absent|NONE");
        t.put("other-catalog | otherc/sales/orders/*", "absent|NONE");
        t.put("ancestor-other-schema | hive/otherdb/-/-", "absent|NONE");
        t.put("endswith-table | hive/sales/*ders/*", "present|SELF_AND_ALL_DESCENDANTS");
        t.put("excludes-table | hive/sales/EXCL:orders/*", "present|NONE");
        t.put("excludes-schema-partial | hive/EXCL:sales/-/-", "present|NONE");
        t.put("recursive-schema | hive/REC:sale/-/-", "present|NONE");
        t.put("leading-wildcard-table | hive/sales/*xyz/*", "present|NONE");
        return t;
    }

    @Test
    public void candidacyAndMatchTypeForAColumnRequest() {
        assertEquals(
            "RangerResourceEvaluatorsRetriever's answer for hive.sales.orders.id changed, or "
                + "getMatchType's did. egret_ranger_eval::eval reads BOTH columns: `present` rows "
                + "whose matchType is SELF or ANCESTOR are the ones it may end the evaluation on "
                + "at offset 553, and `absent` rows are the ones trie_excludes is allowed to skip. "
                + "A row moving either way is a defect in the crate.",
            expectedForColumnRequest(),
            measure(COLUMN_REQUEST, expectedForColumnRequest().keySet()));
    }

    @Test
    public void candidacyAndMatchTypeForATableRequest() {
        assertEquals(
            "The table-level request's answers changed. This is where DESCENDANT is visible at "
                + "all, and where `ancestor-table` becomes SELF — the same policy, a different "
                + "request, opposite treatment in the crate.",
            expectedForTableRequest(),
            measure(TABLE_REQUEST, expectedForTableRequest().keySet()));
    }

    /**
     * **The sub path, pinned rather than argued.** `provably_a_candidate` is read on the
     * mask / row-filter walk too, and `RangerPolicyRepository.createResourceTrieMap` builds
     * all three trie maps from `serviceDef.getResources()` — the ACCESS defs — while the
     * match type comes from the mask def's own (normalized) hierarchy. So the two columns
     * are produced by different halves of the def and could in principle disagree there
     * when they agree here.
     *
     * <p>Same policies, same request, `policyType = DATAMASK`. The presence column must be
     * identical to the ACCESS one, because the tries are the same objects.
     */
    @Test
    public void candidacyAndMatchTypeForAColumnMaskRequest() {
        Map<String, String> measured =
            measure(
                COLUMN_REQUEST,
                expectedForColumnRequest().keySet(),
                RangerPolicy.POLICY_TYPE_DATAMASK);
        assertEquals(
            "The DATAMASK answers changed. `provably_a_candidate` is used on the "
                + "mask/row-filter walk as well, so ANCESTOR must be present and NONE absent "
                + "here exactly as on the ACCESS path.",
            expectedForColumnRequest(),
            measured);
    }

    /**
     * The premise the whole table rests on, asserted rather than assumed: the def really did come
     * off disk with the shape the crate's own copy has.
     */
    @Test
    public void theServiceDefIsTheOneThePluginDownloads() {
        RangerServiceDef def = trinoServiceDef();
        assertEquals("trino", def.getName());
        assertTrue(
            "trino.json is expected to declare more than the four data levels; a hand-written "
                + "four-level def is what the first version of this file used",
            def.getResources().size() > 4);

        Map<String, String> leafness = new LinkedHashMap<>();
        for (String name : Arrays.asList("catalog", "schema", "table", "column")) {
            RangerServiceDef.RangerResourceDef r = resourceDef(def, name);
            leafness.put(name, r.getLevel() + "/" + r.getIsValidLeaf() + "/" + r.getMandatory());
        }
        Map<String, String> expected = new LinkedHashMap<>();
        expected.put("catalog", "10/true/true");
        expected.put("schema", "20/true/true");
        expected.put("table", "30/true/true");
        // `column` OMITS isValidLeaf. Setting it to FALSE instead invalidates the only
        // four-level hierarchy and turns every getMatchType into NONE — the second draft's bug.
        expected.put("column", "40/null/true");
        assertEquals(expected, leafness);
    }

    // ── the measurement ──────────────────────────────────────────────────

    private static Map<String, String> measure(String[][] request, Iterable<String> labels) {
        return measure(request, labels, RangerPolicy.POLICY_TYPE_ACCESS);
    }

    private static Map<String, String> measure(
        String[][] request, Iterable<String> labels, int policyType) {
        RangerServiceDef def = trinoServiceDef();

        List<RangerPolicyEvaluator> evaluators = new ArrayList<>();
        List<String> ordered = new ArrayList<>();
        for (String label : labels) {
            ordered.add(label);
            evaluators.add(evaluatorFor(label, def, policyType));
        }

        // One trie per resource def, over every resource evaluator — exactly what
        // `RangerPolicyRepository.init` builds, and over the same `getResourceEvaluators()`
        // list, so `isAncestorOf` and `leafResourceDef` are Ranger's own.
        List<RangerPolicyEvaluator.RangerPolicyResourceEvaluator> resourceEvaluators =
            new ArrayList<>();
        for (RangerPolicyEvaluator e : evaluators) {
            resourceEvaluators.addAll(e.getResourceEvaluators());
        }
        Map<String, RangerResourceTrie<RangerPolicyEvaluator.RangerPolicyResourceEvaluator>> tries =
            new HashMap<>();
        for (RangerServiceDef.RangerResourceDef rd : def.getResources()) {
            tries.put(rd.getName(), new RangerResourceTrie<>(rd, resourceEvaluators));
        }

        RangerAccessResourceImpl resource = new RangerAccessResourceImpl();
        resource.setServiceDef(def);
        for (String[] level : request) {
            resource.setValue(level[0], level[1]);
        }

        java.util.Collection<RangerPolicyEvaluator.RangerPolicyResourceEvaluator> returned =
            RangerResourceEvaluatorsRetriever.getEvaluators(
                tries, resource.getAsMap(), Collections.emptyMap());

        Map<String, String> measured = new LinkedHashMap<>();
        for (int i = 0; i < ordered.size(); i++) {
            RangerPolicyEvaluator evaluator = evaluators.get(i);
            boolean present = false;
            RangerPolicyResourceMatcher.MatchType best =
                RangerPolicyResourceMatcher.MatchType.NONE;
            for (RangerPolicyEvaluator.RangerPolicyResourceEvaluator re :
                evaluator.getResourceEvaluators()) {
                if (returned != null && returned.contains(re)) {
                    present = true;
                }
                RangerPolicyResourceMatcher.MatchType m =
                    re.getPolicyResourceMatcher()
                        .getMatchType(resource, Collections.emptyMap(), null);
                if (m != RangerPolicyResourceMatcher.MatchType.NONE) {
                    best = m;
                }
            }
            measured.put(ordered.get(i), (present ? "present" : "absent") + "|" + best);
        }
        return measured;
    }

    // ── fixtures, none of them hand-rolling Ranger behaviour ─────────────

    /**
     * `crates/egret-policy/src/service-defs/trino.json` — the def egret's
     * `/service/plugins/policies/download` embeds and the plugin evaluates against — parsed by
     * Ranger's own {@code JsonUtils}, as `EgretRangerEnforcementTest` parses its fixture.
     */
    private static RangerServiceDef trinoServiceDef() {
        try {
            String json =
                new String(
                    Files.readAllBytes(
                        Paths.get("../../crates/egret-policy/src/service-defs/trino.json")));
            RangerServiceDef def = JsonUtils.jsonToObject(json, RangerServiceDef.class);
            if (def == null) {
                throw new IllegalStateException("trino.json did not parse into a RangerServiceDef");
            }
            // `RangerPolicyRepository` normalizes before it builds anything, and the
            // mask/row-filter rows below need it: `trino.json`'s `dataMaskDef.resources`
            // are name-only stubs with null `level` and null `parent`, which
            // `ServiceDefUtil.normalize` fills from the access defs. Without it a
            // DATAMASK policy has no hierarchy and every match type is NONE.
            return ServiceDefUtil.normalize(def);
        } catch (java.io.IOException e) {
            throw new RuntimeException("cannot read trino.json — is the CWD tests/ranger?", e);
        }
    }

    private static RangerServiceDef.RangerResourceDef resourceDef(
        RangerServiceDef def, String name) {
        for (RangerServiceDef.RangerResourceDef r : def.getResources()) {
            if (name.equals(r.getName())) {
                return r;
            }
        }
        throw new IllegalStateException("trino.json declares no resource named " + name);
    }

    /** A real {@link RangerDefaultPolicyEvaluator} over a real {@link RangerPolicy}. */
    private static RangerPolicyEvaluator evaluatorFor(
        String label, RangerServiceDef def, int policyType) {
        RangerPolicy policy = new RangerPolicy();
        policy.setId((long) Math.abs(label.hashCode()));
        policy.setName(label);
        policy.setService("trino");
        policy.setServiceType("trino");
        policy.setPolicyType(policyType);
        policy.setIsEnabled(true);
        policy.setResources(parse(label));

        RangerPolicy.RangerPolicyItem item = new RangerPolicy.RangerPolicyItem();
        item.setUsers(new ArrayList<>(Collections.singletonList("alice")));
        RangerPolicy.RangerPolicyItemAccess access = new RangerPolicy.RangerPolicyItemAccess();
        access.setType("select");
        access.setIsAllowed(true);
        item.setAccesses(new ArrayList<>(Collections.singletonList(access)));
        if (policyType == RangerPolicy.POLICY_TYPE_DATAMASK) {
            RangerPolicy.RangerDataMaskPolicyItem mask = new RangerPolicy.RangerDataMaskPolicyItem();
            mask.setUsers(item.getUsers());
            mask.setAccesses(item.getAccesses());
            mask.setDataMaskInfo(new RangerPolicy.RangerPolicyItemDataMaskInfo("MASK", null, null));
            policy.setDataMaskPolicyItems(new ArrayList<>(Collections.singletonList(mask)));
        } else {
            policy.setPolicyItems(new ArrayList<>(Collections.singletonList(item)));
        }

        RangerDefaultPolicyEvaluator evaluator = new RangerDefaultPolicyEvaluator();
        evaluator.init(policy, def, new RangerPolicyEngineOptions());
        return evaluator;
    }

    /** `name | a/b/c/d`, `-` for an undeclared level, `EXCL:` for `isExcludes`. */
    private static Map<String, RangerPolicy.RangerPolicyResource> parse(String label) {
        String[] names = {"catalog", "schema", "table", "column"};
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
            boolean recursive = v.startsWith("REC:");
            if (recursive) {
                v = v.substring("REC:".length());
            }
            map.put(
                names[i],
                new RangerPolicy.RangerPolicyResource(
                    new ArrayList<>(Collections.singletonList(v)), excludes, recursive));
        }
        return map;
    }
}
