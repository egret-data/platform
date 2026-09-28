import com.google.gson.Gson;
import com.google.gson.GsonBuilder;
import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.apache.ranger.authorization.hadoop.config.RangerPluginConfig;
import org.apache.ranger.authorization.utils.JsonUtils;
import org.apache.ranger.plugin.model.RangerPolicy;
import org.apache.ranger.plugin.policyengine.RangerAccessRequestImpl;
import org.apache.ranger.plugin.policyengine.RangerAccessResourceImpl;
import org.apache.ranger.plugin.policyengine.RangerAccessResult;
import org.apache.ranger.plugin.policyengine.RangerPluginContext;
import org.apache.ranger.plugin.policyengine.RangerPolicyEngineImpl;
import org.apache.ranger.plugin.policyengine.RangerPolicyEngineOptions;
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
 *     "servicePolicies": { … a whole ServicePolicies download … },
 *     "request": { "user": "…", "groups": […], "roles": […],
 *                  "accessType": "…", "resource": { "catalog": "…", … } } }, … ]
 * </pre>
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
            verdict.addProperty("error", t.getClass().getName() + ": " + t.getMessage());
        }
        return verdict;
    }

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
        return r;
    }

    private Set<String> stringSet(JsonObject o, String field) {
        Set<String> out = new LinkedHashSet<>();
        for (JsonElement e : o.getAsJsonArray(field)) {
            out.add(e.getAsString());
        }
        return out;
    }
}
