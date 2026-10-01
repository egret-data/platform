import static org.junit.Assert.assertEquals;

import java.util.ArrayList;
import java.util.Arrays;
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
 * SP4d-2b Task 2: the OTHER half of {@code egret_ranger_eval::resource::trie_reproduces_matcher}.
 *
 * <p>For a TAG request, {@code RangerDefaultPolicyEvaluator.evaluate} reads the tag's own match
 * type and never asks the policy's matcher (2.5.0 offsets 188-204; the matcher call at 247 is on
 * the other branch). So {@code RangerResourceEvaluatorsRetriever} — the trie — is the ONLY thing
 * deciding which tag policies run, and anywhere the trie and the matcher disagree, the matcher is
 * the wrong instrument. {@code egret-ranger-eval} refuses those shapes rather than answering them.
 *
 * <p><b>Why this file exists.</b> The Rust side asserts a hard-coded table against the function
 * that implements that same table — a correlated oracle that cannot fail. This test measures the
 * table from the REAL jar instead: it builds a real {@link RangerResourceTrie} over the {@code tag}
 * service-def, a real {@link RangerDefaultPolicyResourceMatcher} per policy value, and compares
 * what each returns. The two halves must agree, and a {@code ranger.version} bump that changes
 * {@code getNonWildcardPrefix} or the retriever breaks THIS one.
 *
 * <p>The classification is deliberately coarse — AGREES everywhere versus DIFFERS somewhere — the
 * same granularity {@code trie_reproduces_matcher} returns, because that is the granularity the
 * refusal acts on.
 */
public class EgretTagTrieAgreementTest {

    /** Ranger hard-codes this in {@code RangerTagResource.KEY_TAG}; it is not read from the def. */
    private static final String KEY_TAG = "tag";

    /**
     * The request values every policy value is measured against. Chosen to separate the four
     * matcher shapes: {@code p}/{@code pi}/{@code pix} walk the literal prefix of {@code p*x} and
     * {@code pi?}, {@code piix}/{@code pii_x} are the extra reach {@code isRecursive} buys, and
     * {@code PII} is the case-sensitivity control ({@code tag.json} sets {@code ignoreCase: false}).
     */
    private static final String[] REQUESTS = {
        "pii", "PII", "p", "pix", "pi", "xii", "ii", "", "piix", "pii_x"
    };

    /**
     * policy value → does the trie return exactly what {@code getMatchType} accepts, for every
     * request above?
     *
     * <p>{@code EXCLUDES:} and {@code RECURSIVE:} prefixes set the corresponding flag on the
     * {@code RangerPolicyResource} rather than being part of the value.
     *
     * <p>{@code RECURSIVE:p*} is the control for the deliberate OVER-refusal on the Rust side:
     * a value that is ALREADY a wildcard is already parked as a wildcard evaluator, so
     * {@code isRecursive} adds nothing and the two agree — yet the crate refuses it anyway,
     * because it refuses the flag rather than reproducing the trie. See
     * {@code TagResourceProblem::Recursive}.
     */
    private static Map<String, Boolean> expectedAgreement() {
        Map<String, Boolean> table = new LinkedHashMap<>();
        table.put("pii", true);
        table.put("PII", true);
        table.put("*", true);
        table.put("**", true);
        table.put("p*", true);
        table.put("p**", true);
        // Parked at the ROOT: the prefix before a leading `*` is the empty string.
        table.put("*i", false);
        table.put("*ii", false);
        // Parked at the literal prefix before the first wildcard character.
        table.put("p*x", false);
        table.put("pi?", false);
        table.put("pi*i", false);
        table.put("p?i", false);
        // An excludes resource lands in the trie's inherited evaluators, which the retriever
        // unions into EVERY lookup — including the tag the policy excludes.
        table.put("EXCLUDES:pii", false);
        // `insert`: `if (isWildcard || isRecursive) addWildcardEvaluator(...)`, so a recursive
        // LITERAL is parked as a wildcard at its own node and reaches every tag with that prefix.
        table.put("RECURSIVE:pii", false);
        table.put("RECURSIVE:p*", true);
        return table;
    }

    @Test
    public void trieAndMatcherAgreeExactlyWhereTheCrateSaysTheyDo() {
        RangerServiceDef def = tagServiceDef();

        List<Spec> specs = new ArrayList<>();
        for (String key : expectedAgreement().keySet()) {
            specs.add(Spec.parse(key));
        }

        // No try/catch around this, deliberately. An earlier draft skipped the test on
        // NoClassDefFoundError, on a belief that turned out to be false: Maven DOES resolve
        // `ranger-authz-api` for 2.8.0 (2.5.0 does not need it), and `mvn -o test` runs this
        // on both versions with `Skipped: 0`. The skip was also self-defeating — a future
        // classpath break would have turned the measurement into a SILENT skip with a green
        // `mvn test`, which is the unfalsifiable green this file was committed to remove, for
        // precisely the event (a version bump) it exists to catch. It fails loudly instead.
        List<Ev> evaluators = new ArrayList<>();
        for (int i = 0; i < specs.size(); i++) {
            evaluators.add(new Ev(i, specs.get(i), def));
        }

        RangerResourceTrie<Ev> trie = new RangerResourceTrie<>(def.getResources().get(0), evaluators);
        Map<String, RangerResourceTrie<Ev>> tries = new HashMap<>();
        tries.put(KEY_TAG, trie);

        Map<String, Boolean> measured = new LinkedHashMap<>();
        for (int i = 0; i < specs.size(); i++) {
            Ev ev = evaluators.get(i);
            boolean agrees = true;
            for (String request : REQUESTS) {
                RangerAccessResourceImpl resource = new RangerAccessResourceImpl();
                resource.setServiceDef(def);
                resource.setValue(KEY_TAG, request);

                java.util.Collection<Ev> fromTrie =
                    RangerResourceEvaluatorsRetriever.getEvaluators(
                        tries, resource.getAsMap(), Collections.emptyMap());
                boolean trieSaysCandidate = fromTrie != null && fromTrie.contains(ev);

                RangerPolicyResourceMatcher.MatchType matchType =
                    ev.getPolicyResourceMatcher()
                        .getMatchType(resource, Collections.emptyMap(), null);
                boolean matcherSaysCandidate =
                    matchType != RangerPolicyResourceMatcher.MatchType.NONE;

                if (trieSaysCandidate != matcherSaysCandidate) {
                    agrees = false;
                    break;
                }
            }
            measured.put(specs.get(i).key, agrees);
        }

        assertEquals(
            "the trie/matcher agreement measured from ranger-plugins-common changed. "
                + "Re-check egret_ranger_eval::resource::trie_reproduces_matcher and its "
                + "`trie_agreement_matches_the_measured_table` unit test — the two are halves "
                + "of one claim and must not drift apart.",
            expectedAgreement(),
            measured);
    }

    // ── fixtures ─────────────────────────────────────────────────────────

    /** One row of the table: a policy value plus the two flags that can be set on it. */
    private static final class Spec {
        final String key;
        final String value;
        final boolean excludes;
        final boolean recursive;

        private Spec(String key, String value, boolean excludes, boolean recursive) {
            this.key = key;
            this.value = value;
            this.excludes = excludes;
            this.recursive = recursive;
        }

        static Spec parse(String key) {
            if (key.startsWith("EXCLUDES:")) {
                return new Spec(key, key.substring("EXCLUDES:".length()), true, false);
            }
            if (key.startsWith("RECURSIVE:")) {
                return new Spec(key, key.substring("RECURSIVE:".length()), false, true);
            }
            return new Spec(key, key, false, false);
        }
    }

    /**
     * The `tag` service-def, built to match `crates/egret-policy/src/service-defs/tag.json` and the
     * def embedded in `crates/egret-ranger-eval/tests/fixtures/tag-policies-live.json`: one `tag`
     * resource, `isValidLeaf: true`, `excludesSupported: false`, and — the one that bites —
     * `matcherOptions.ignoreCase: false`, where trino/starrocks resource matching is
     * case-INSENSITIVE.
     */
    private static RangerServiceDef tagServiceDef() {
        RangerServiceDef def = new RangerServiceDef();
        def.setName("tag");

        RangerServiceDef.RangerResourceDef tag = new RangerServiceDef.RangerResourceDef();
        tag.setItemId(1L);
        tag.setName(KEY_TAG);
        tag.setType("string");
        tag.setLevel(1);
        tag.setMandatory(true);
        tag.setIsValidLeaf(true);
        tag.setExcludesSupported(false);
        tag.setMatcher("org.apache.ranger.plugin.resourcematcher.RangerDefaultResourceMatcher");
        Map<String, String> options = new HashMap<>();
        options.put("wildCard", "true");
        options.put("ignoreCase", "false");
        tag.setMatcherOptions(options);
        def.setResources(new ArrayList<>(Collections.singletonList(tag)));

        RangerServiceDef.RangerAccessTypeDef select = new RangerServiceDef.RangerAccessTypeDef();
        select.setItemId(1L);
        select.setName("select");
        def.setAccessTypes(new ArrayList<>(Collections.singletonList(select)));

        return def;
    }

    /**
     * The minimum {@link RangerResourceEvaluator} the trie and the retriever need: an id, the
     * policy resource map, and a real initialised {@link RangerDefaultPolicyResourceMatcher}.
     */
    private static final class Ev implements RangerResourceEvaluator {
        private final long id;
        private final Map<String, RangerPolicy.RangerPolicyResource> resources;
        private final RangerPolicyResourceMatcher matcher;

        Ev(long id, Spec spec, RangerServiceDef def) {
            this.id = id;

            RangerPolicy.RangerPolicyResource resource = new RangerPolicy.RangerPolicyResource();
            resource.setValues(new ArrayList<>(Arrays.asList(spec.value)));
            resource.setIsExcludes(spec.excludes);
            resource.setIsRecursive(spec.recursive);
            this.resources = new HashMap<>();
            this.resources.put(KEY_TAG, resource);

            RangerDefaultPolicyResourceMatcher m = new RangerDefaultPolicyResourceMatcher();
            m.setServiceDef(def);
            m.setServiceDefHelper(new RangerServiceDefHelper(def, false));
            m.setPolicyResources(this.resources, RangerPolicy.POLICY_TYPE_ACCESS);
            m.init();
            this.matcher = m;
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
            return matcher.getResourceMatcher(resourceName);
        }

        @Override
        public boolean isAncestorOf(RangerServiceDef.RangerResourceDef resourceDef) {
            return false;
        }

        /** 2.8.0 added this to the interface; harmless on 2.5.0, which has no such method. */
        public boolean isLeaf(String resourceName) {
            return true;
        }
    }
}
