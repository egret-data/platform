import com.google.gson.Gson;
import com.google.gson.GsonBuilder;
import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.apache.ranger.authorization.hadoop.config.RangerPluginConfig;
import org.apache.ranger.authorization.utils.JsonUtils;
import org.apache.ranger.plugin.contextenricher.RangerTagForEval;
import org.apache.ranger.plugin.model.RangerPolicy;
import org.apache.ranger.plugin.model.RangerTag;
import org.apache.ranger.plugin.policyengine.RangerAccessRequest;
import org.apache.ranger.plugin.policyengine.RangerAccessRequestImpl;
import org.apache.ranger.plugin.policyengine.RangerAccessResourceImpl;
import org.apache.ranger.plugin.policyengine.RangerAccessResult;
import org.apache.ranger.plugin.policyengine.RangerPluginContext;
import org.apache.ranger.plugin.policyengine.RangerPolicyEngineImpl;
import org.apache.ranger.plugin.policyengine.RangerPolicyEngineOptions;
import org.apache.ranger.plugin.policyresourcematcher.RangerPolicyResourceMatcher;
import org.apache.ranger.plugin.util.RangerAccessRequestUtil;
import org.apache.ranger.plugin.util.ServicePolicies;
import org.junit.Assume;
import org.junit.Test;

import java.io.Writer;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.LinkedHashMap;
import java.util.LinkedHashSet;
import java.util.Map;
import java.util.Set;

/**
 * The differential gate's Java half (SP4d-1 Task 7).
 *
 * <p>Reads a generated case file, runs every case through the REAL
 * {@link RangerPolicyEngineImpl}, and writes one verdict per case. It asserts
 * nothing about what Ranger answers — the comparison lives on the Rust side
 * (`crates/egret-ranger-eval/tests/differential.rs`), which is the half that
 * holds the other implementation.
 *
 * <p>Deliberately separate from {@code EgretRangerEnforcementTest}: those four
 * hand-written cases are a regression anchor with known-good expected values,
 * and a generated corpus asserting only AGREEMENT is a weaker claim (two
 * implementations can agree and both be wrong). Entangling the two would let a
 * change to the generator silently move the anchor.
 *
 * <p>Skipped — not failed — when {@code -Degret.cases} is absent, so a plain
 * {@code mvn test} still runs the anchors alone.
 *
 * <p>Protocol, cases.json (input):
 * <pre>
 * [ { "id": "…",
 *     "serviceType": "trino" | "starrocks",
 *     "policyType": 0 | 1 | 2,
 *     "servicePolicies": { … a whole ServicePolicies download, tagPolicies included … },
 *     "request": { "user": "…", "groups": […], "roles": […],
 *                  "accessType": "…", "resource": { "catalog": "…", … },
 *                  "tags": [ { "type": "pii", "matchType": "SELF",
 *                              "attributes": { … } }, … ] } }, … ]
 * </pre>
 *
 * <p>{@code request.tags} is OPTIONAL and is the enricher's OUTPUT, handed in
 * directly — see {@link #tagsFrom}.
 *
 * <p>verdicts.json (output): one object per case, in the same order —
 * {@code { id, error, isAllowed, isAccessDetermined, policyId, maskType,
 * maskedValue, maskCondition, filterExpr }}. A case that threw carries
 * {@code error} and nothing else; the Rust half fails on any such entry rather
 * than skipping it.
 */
public class EgretCaseRunner {
    @Test
    public void runGeneratedCases() throws Exception {
        String casesPath = property("egret.cases");
        Assume.assumeTrue("egret.cases not set — nothing to run", casesPath != null);

        String verdictsPath = property("egret.verdicts");
        if (verdictsPath == null) {
            throw new IllegalStateException("egret.cases given without egret.verdicts");
        }

        String casesJson = new String(Files.readAllBytes(Paths.get(casesPath)), StandardCharsets.UTF_8);
        JsonArray cases = JsonParser.parseString(casesJson).getAsJsonArray();

        JsonArray verdicts = new JsonArray();
        for (JsonElement element : cases) {
            verdicts.add(runOne(element.getAsJsonObject()));
        }

        Gson gson = new GsonBuilder().serializeNulls().create();
        Path out = Paths.get(verdictsPath);
        if (out.getParent() != null) {
            Files.createDirectories(out.getParent());
        }
        try (Writer w = Files.newBufferedWriter(out, StandardCharsets.UTF_8)) {
            gson.toJson(verdicts, w);
        }
        System.out.println("egret: wrote " + verdicts.size() + " verdicts to " + out);
    }

    /**
     * A system property Maven did not substitute comes through as the literal
     * {@code ${egret.cases}} when the POM declares it with a default; treat
     * that, an empty string and an absent property alike.
     */
    private static String property(String name) {
        String v = System.getProperty(name);
        if (v == null || v.isEmpty() || v.startsWith("${")) {
            return null;
        }
        return v;
    }

    private JsonObject runOne(JsonObject testCase) {
        JsonObject verdict = new JsonObject();
        String id = testCase.get("id").getAsString();
        verdict.addProperty("id", id);
        try {
            ServicePolicies sp = JsonUtils.jsonToObject(
                    testCase.get("servicePolicies").toString(), ServicePolicies.class);
            if (sp == null) {
                throw new IllegalStateException("servicePolicies did not deserialise");
            }

            // A null pluginContext NPEs in PolicyEngine.<init> at 2.5.0 and
            // 2.8.0 alike — see tests/ranger/README.md. serviceType must be
            // the service-def's own name, since the plugin config keys the
            // engine's behaviour off it.
            RangerPluginConfig cfg = new RangerPluginConfig(
                    testCase.get("serviceType").getAsString(),
                    sp.getServiceName(),
                    "egret",
                    null,
                    null,
                    new RangerPolicyEngineOptions());
            RangerPolicyEngineImpl engine = new RangerPolicyEngineImpl(sp, new RangerPluginContext(cfg), null);

            int policyType = testCase.get("policyType").getAsInt();
            RangerAccessResult res = engine.evaluatePolicies(request(testCase.getAsJsonObject("request")), policyType, null);

            verdict.add("error", null);
            verdict.addProperty("isAllowed", res.getIsAllowed());
            verdict.addProperty("isAccessDetermined", res.getIsAccessDetermined());
            verdict.addProperty("policyId", res.getPolicyId());
            verdict.addProperty("maskType", res.getMaskType());
            verdict.addProperty("maskedValue", res.getMaskedValue());
            verdict.addProperty("maskCondition", res.getMaskCondition());
            verdict.addProperty("filterExpr", res.getFilterExpr());
        } catch (Throwable t) {
            // WITH the top frames. "the Ranger engine threw NullPointerException"
            // names a class and a message and says nothing about WHICH of the
            // engine's several hundred methods produced it, which turned an
            // afternoon into a jar-wide disassembly once. Four frames is enough
            // to name the method and its caller, and short enough to stay
            // readable inside a failure message that also carries the policy
            // JSON.
            StringBuilder where = new StringBuilder();
            StackTraceElement[] frames = t.getStackTrace();
            for (int i = 0; i < frames.length && i < 4; i++) {
                where.append("\n      at ").append(frames[i]);
            }
            verdict.addProperty("error", t.getClass().getName() + ": " + t.getMessage() + where);
        }
        return verdict;
    }

    /**
     * A FRESH request per evaluation, deliberately — do not hoist this out of
     * the loop and reuse it.
     *
     * {@code RangerAccessRequestImpl.preProcess} early-returns when the
     * context carries {@code ISREQUESTPREPROCESSED}, and that flag is cleared
     * by {@code setResource} (which calls
     * {@code RangerAccessRequestUtil.setIsRequestPreprocessed(context, FALSE)})
     * but NOT by {@code setUser}, whose whole body is a {@code putfield}.
     * Verified with {@code javap -c} on the 2.5.0 jar.
     *
     * So an "optimisation" that built one request and only changed the user
     * between cases would keep a stale preprocessed context — including the
     * {@code token:USER} the expression resolver reads — and every case after
     * the first would be evaluated for the WRONG user. That fails OPEN, and
     * silently: the corpus would still be green wherever the stale user
     * happened to be allowed.
     */
    private RangerAccessRequestImpl request(JsonObject req) {
        Map<String, Object> resource = new LinkedHashMap<>();
        for (Map.Entry<String, JsonElement> e : req.getAsJsonObject("resource").entrySet()) {
            resource.put(e.getKey(), e.getValue().getAsString());
        }
        RangerAccessRequestImpl r = new RangerAccessRequestImpl();
        r.setResource(new RangerAccessResourceImpl(resource));
        r.setUser(req.get("user").getAsString());
        r.setUserGroups(stringSet(req, "groups"));
        r.setUserRoles(stringSet(req, "roles"));
        // setAccessType rewrites null/empty to ANY_ACCESS; the generator never
        // emits either, so what is set here is what is evaluated.
        r.setAccessType(req.get("accessType").getAsString());
        // SP4d-4 Task 1: the Trino plugin's filter* methods ask with
        // SELF_OR_DESCENDANTS (RangerSystemAccessControl.java:972/:990 at tag 470).
        // Absent = Ranger's default (SELF), so every existing case serialises and
        // evaluates exactly as before. Measured with javap -c at 2.5.0 and 2.8.0:
        // besides the field write, the setter only resets ISREQUESTPREPROCESSED
        // to FALSE (as setResource does) and never touches the TAGS key, so it
        // stays BEFORE the tags block below and the seeded tag set survives.
        if (req.has("resourceMatchingScope")) {
            r.setResourceMatchingScope(RangerAccessRequest.ResourceMatchingScope.valueOf(
                    req.get("resourceMatchingScope").getAsString()));
        }
        if (req.has("tags")) {
            // AFTER setResource (and setResourceMatchingScope), the setters on
            // this class that touch the context (both clear ISREQUESTPREPROCESSED). Nothing
            // between here and evaluatePolicies writes the TAGS key — see
            // tagsFrom.
            RangerAccessRequestUtil.setRequestTagsInContext(
                    r.getContext(), tagsFrom(req.getAsJsonArray("tags")));
        }
        return r;
    }

    /**
     * The request's resolved tag set, built as {@code RangerTagEnricher.enrich}
     * would have left it.
     *
     * <p><b>The enricher is bypassed on BOTH sides, deliberately and
     * symmetrically.</b> {@code egret-ranger-eval} takes the resolved tag set as
     * an INPUT ({@code AccessRequest::resolved_tags}) rather than deriving it —
     * whether the set is the one an enricher would have produced is documented
     * there as the CALLER's obligation. So the differential question is what
     * {@code evaluateTagPolicies} does with a given set, and the honest way to
     * ask it is to hand the same set to both implementations. Running a real
     * {@link org.apache.ranger.plugin.contextenricher.RangerTagEnricher} here
     * would need a tag store and a poll against a Ranger admin, and would test
     * the enricher rather than the phase.
     *
     * <p>Two consequences are load-bearing and are honoured by the generator
     * (`crates/egret-ranger-eval/tests/differential.rs`):
     *
     * <ul>
     *   <li>Neither service-def in a generated case declares
     *       {@code contextEnrichers} (the Rust side strips the key), so
     *       {@code RangerDefaultRequestProcessor.enrich} iterates an EMPTY
     *       enricher list and cannot overwrite what is put here. Measured with
     *       {@code javap -c} on 2.5.0 and 2.8.0: the only writer of
     *       {@code RangerAccessRequestUtil.KEY_CONTEXT_TAGS} on the
     *       {@code evaluatePolicies → preProcess} path is
     *       {@code RangerContextEnricher.enrich}; {@code preProcess} itself
     *       writes only CURRENT_USER, CURRENT_USER_ROLES, OWNER, ZONE_NAMES and
     *       ISREQUESTPREPROCESSED.</li>
     *   <li>The enricher's own DROP of a service resource whose key set no
     *       hierarchy admits ({@code processServiceTags}) therefore does not run
     *       here, so the generator never emits an attachment that would be
     *       dropped — it would be a divergence manufactured by the harness
     *       rather than one found in Ranger.</li>
     * </ul>
     *
     * <p>A {@link java.util.LinkedHashSet} rather than a {@code HashSet}: the
     * order the generator listed the tags in is the order the Rust side's
     * {@code Vec} carries, so the one thing neither implementation's comparator
     * can break a tie on is at least the SAME on both sides.
     * {@code RangerTagForEval.equals} — type, attributes, matchType, options,
     * validityPeriods — still does the de-duplication, exactly as the Rust
     * {@code ResolvedTag::identity} does.
     */
    private Set<RangerTagForEval> tagsFrom(JsonArray tags) {
        Set<RangerTagForEval> out = new LinkedHashSet<>();
        for (JsonElement element : tags) {
            JsonObject t = element.getAsJsonObject();
            Map<String, String> attributes = new LinkedHashMap<>();
            if (t.has("attributes") && !t.get("attributes").isJsonNull()) {
                for (Map.Entry<String, JsonElement> e : t.getAsJsonObject("attributes").entrySet()) {
                    attributes.put(e.getKey(), e.getValue().getAsString());
                }
            }
            // The 2-arg constructor, which is `RangerTag(type, attributes)`.
            // `owner`, `options` and `validityPeriods` stay null — and
            // RangerTagForEval's constructor substitutes emptyMap()/emptyList()
            // for the two it reads, which is the shape the generator emits
            // (it never produces a tag carrying options or validity periods,
            // because egret-ranger-eval REFUSES those rather than modelling
            // them).
            RangerTag tag = new RangerTag(t.get("type").getAsString(), attributes);
            // valueOf, not a lookup table: an unknown match type must THROW and
            // surface as this case's `error`, not silently become NONE.
            RangerPolicyResourceMatcher.MatchType matchType =
                    RangerPolicyResourceMatcher.MatchType.valueOf(t.get("matchType").getAsString());
            out.add(new RangerTagForEval(tag, matchType));
        }
        return out;
    }

    private Set<String> stringSet(JsonObject o, String field) {
        Set<String> out = new LinkedHashSet<>();
        for (JsonElement e : o.getAsJsonArray(field)) {
            out.add(e.getAsString());
        }
        return out;
    }
}
