import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.Collections;
import java.util.HashMap;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import org.apache.ranger.authorization.hadoop.config.RangerPluginConfig;
import org.apache.ranger.authorization.utils.JsonUtils;
import org.apache.ranger.plugin.contextenricher.RangerTagForEval;
import org.apache.ranger.plugin.model.RangerPolicy;
import org.apache.ranger.plugin.model.RangerServiceDef;
import org.apache.ranger.plugin.model.RangerTag;
import org.apache.ranger.plugin.policyengine.RangerAccessRequestImpl;
import org.apache.ranger.plugin.policyengine.RangerAccessResourceImpl;
import org.apache.ranger.plugin.policyengine.RangerAccessResult;
import org.apache.ranger.plugin.policyengine.RangerPluginContext;
import org.apache.ranger.plugin.policyengine.RangerPolicyEngineImpl;
import org.apache.ranger.plugin.policyengine.RangerPolicyEngineOptions;
import org.apache.ranger.plugin.policyresourcematcher.RangerPolicyResourceMatcher;
import org.apache.ranger.plugin.util.RangerAccessRequestUtil;
import org.apache.ranger.plugin.util.ServicePolicies;
import org.junit.Test;

/**
 * Which service-def the {@code tagPolicies} block carries decides WHICH tag policies run.
 *
 * <p><b>The defect this measures (found live on Trino 483, plugin 2.5.0).</b> egret's
 * {@code download_policies} used to fetch the block's def by the TAG SERVICE's own {@code type}.
 * A tag service made through the admin API is a {@code trino} profile (the API accepts no other
 * type), so the block went out carrying {@code trino.json}. The plugin logged {@code
 * RangerDefaultPolicyResourceMatcher.init() failed: policyResources elements are not part of any
 * valid resourcedef hierarchy. (serviceDef=trino, policyResourceKeys=tag} once per tag policy and
 * then applied EVERY tag policy to EVERY tagged column: a cleared user read a column tagged only
 * {@code vgsecret} as NULL through a mask on {@code vgkey}, and an uncleared user was denied a
 * column tagged only {@code vgkey} by a deny on {@code vgsecret}.
 *
 * <p><b>The mechanism.</b> For a tag request {@code RangerDefaultPolicyEvaluator.evaluate} reads
 * the TAG's match type and never asks the policy's own matcher (see {@link
 * EgretTagTrieAgreementTest}), so the candidate filter is the only thing that ties a policy to a
 * tag. That filter is a trie per resource of the block's def; {@code trino.json} declares no
 * {@code tag} resource, so there is no trie to look the tag up in, the lookup does not narrow, and
 * every tag policy of the type is a candidate for every tag.
 *
 * <p>Both halves are asserted against the real jar: with {@code tag.json} a policy on one tag does
 * not touch a column carrying only the other; with {@code trino.json} it does. The second half is
 * the fail-open, kept as a measurement so a {@code ranger.version} bump that changes it is loud —
 * and so {@code egret_ranger_eval::eval::may_be_tag_candidate}, which returns {@code true} (no
 * narrowing) for a block whose def has no top-level {@code tag} resource, cites something
 * measured.
 */
public class EgretTagBlockServiceDefTest {

    private static final String DEFS = "../../crates/egret-policy/src/service-defs/";

    private static final long DENY_SECRET_ID = 9101L;
    private static final long MASK_KEY_ID = 9102L;

    @Test
    public void withTheTagDefAPolicyOnOneTagDoesNotReachAColumnCarryingOnlyTheOther() throws Exception {
        RangerPolicyEngineImpl engine = engine(def("tag.json"));

        // Deny on `vgsecret` must not reach `id`, which carries only `vgkey`.
        RangerAccessResult id = engine.evaluatePolicies(
                request("bob", "id", "vgkey"), RangerPolicy.POLICY_TYPE_ACCESS, null);
        assertFalse("no tag policy names vgkey for access", id.getIsAccessDetermined());
        assertEquals(-1L, id.getPolicyId());

        // ... and must reach `ssn`, which carries `vgsecret`.
        RangerAccessResult ssn = engine.evaluatePolicies(
                request("bob", "ssn", "vgsecret"), RangerPolicy.POLICY_TYPE_ACCESS, null);
        assertTrue(ssn.getIsAccessDetermined());
        assertFalse(ssn.getIsAllowed());
        assertEquals(DENY_SECRET_ID, ssn.getPolicyId());

        // The `vgkey` mask must not reach `ssn` ...
        RangerAccessResult ssnMask = engine.evaluatePolicies(
                request("alice", "ssn", "vgsecret"), RangerPolicy.POLICY_TYPE_DATAMASK, null);
        assertNull("the vgkey mask must not mask a vgsecret-only column", ssnMask.getMaskType());

        // ... and must reach `id`.
        RangerAccessResult idMask = engine.evaluatePolicies(
                request("alice", "id", "vgkey"), RangerPolicy.POLICY_TYPE_DATAMASK, null);
        assertEquals("MASK_NULL", idMask.getMaskType());
        assertEquals(MASK_KEY_ID, idMask.getPolicyId());
    }

    @Test
    public void withTheTrinoDefEveryTagPolicyReachesEveryTaggedColumn() throws Exception {
        RangerPolicyEngineImpl engine = engine(def("trino.json"));

        RangerAccessResult id = engine.evaluatePolicies(
                request("bob", "id", "vgkey"), RangerPolicy.POLICY_TYPE_ACCESS, null);
        assertTrue("MEASURED FAIL-OPEN: the vgsecret deny reaches a vgkey-only column",
                id.getIsAccessDetermined());
        assertFalse(id.getIsAllowed());
        assertEquals(DENY_SECRET_ID, id.getPolicyId());

        RangerAccessResult ssnMask = engine.evaluatePolicies(
                request("alice", "ssn", "vgsecret"), RangerPolicy.POLICY_TYPE_DATAMASK, null);
        assertEquals("MEASURED: the vgkey mask masks a vgsecret-only column",
                "MASK_NULL", ssnMask.getMaskType());
        assertEquals(MASK_KEY_ID, ssnMask.getPolicyId());
    }

    // ── fixtures ─────────────────────────────────────────────────────────

    /** An embedded egret def, with {@code contextEnrichers} stripped as the differential does. */
    private static RangerServiceDef def(String file) throws Exception {
        String json = new String(Files.readAllBytes(Paths.get(DEFS + file)), StandardCharsets.UTF_8);
        JsonObject o = JsonParser.parseString(json).getAsJsonObject();
        o.remove("contextEnrichers");
        return JsonUtils.jsonToObject(o.toString(), RangerServiceDef.class);
    }

    private static RangerPolicyEngineImpl engine(RangerServiceDef tagBlockDef) throws Exception {
        ServicePolicies sp = new ServicePolicies();
        sp.setServiceName("vg_trino");
        sp.setServiceId(1L);
        sp.setPolicyVersion(1L);
        sp.setServiceDef(def("trino.json"));
        sp.setPolicies(new ArrayList<>());

        ServicePolicies.TagPolicies tp = new ServicePolicies.TagPolicies();
        tp.setServiceName("vg_trino_tag");
        tp.setServiceId(2L);
        tp.setPolicyVersion(1L);
        tp.setServiceDef(tagBlockDef);
        List<RangerPolicy> policies = new ArrayList<>();
        policies.add(denySecret());
        policies.add(maskKey());
        tp.setPolicies(policies);
        sp.setTagPolicies(tp);

        RangerPluginConfig cfg = new RangerPluginConfig(
                "trino", "vg_trino", "egret", null, null, new RangerPolicyEngineOptions());
        return new RangerPolicyEngineImpl(sp, new RangerPluginContext(cfg), null);
    }

    private static RangerPolicy tagPolicy(long id, String name, int type, String tag) {
        RangerPolicy p = new RangerPolicy();
        p.setId(id);
        p.setName(name);
        p.setService("vg_trino_tag");
        p.setServiceType("tag");
        p.setPolicyType(type);
        p.setIsEnabled(true);
        Map<String, RangerPolicy.RangerPolicyResource> res = new HashMap<>();
        res.put("tag", new RangerPolicy.RangerPolicyResource(tag, false, false));
        p.setResources(res);
        return p;
    }

    private static RangerPolicy denySecret() {
        RangerPolicy p = tagPolicy(DENY_SECRET_ID, "deny-vgsecret", RangerPolicy.POLICY_TYPE_ACCESS, "vgsecret");
        RangerPolicy.RangerPolicyItem item = new RangerPolicy.RangerPolicyItem();
        item.setUsers(Collections.singletonList("bob"));
        item.setAccesses(Collections.singletonList(
                new RangerPolicy.RangerPolicyItemAccess("select", true)));
        p.setDenyPolicyItems(Collections.singletonList(item));
        return p;
    }

    private static RangerPolicy maskKey() {
        RangerPolicy p = tagPolicy(MASK_KEY_ID, "mask-vgkey", RangerPolicy.POLICY_TYPE_DATAMASK, "vgkey");
        RangerPolicy.RangerDataMaskPolicyItem item = new RangerPolicy.RangerDataMaskPolicyItem();
        item.setGroups(Collections.singletonList("public"));
        item.setAccesses(Collections.singletonList(
                new RangerPolicy.RangerPolicyItemAccess("select", true)));
        item.setDataMaskInfo(new RangerPolicy.RangerPolicyItemDataMaskInfo("MASK_NULL", null, null));
        p.setDataMaskPolicyItems(Collections.singletonList(item));
        return p;
    }

    /** A fresh request per evaluation — see {@code EgretCaseRunner.request}. */
    private static RangerAccessRequestImpl request(String user, String column, String tag) {
        Map<String, Object> m = new HashMap<>();
        m.put("catalog", "iceberg");
        m.put("schema", "pii");
        m.put("table", "people");
        m.put("column", column);
        RangerAccessRequestImpl r = new RangerAccessRequestImpl();
        r.setResource(new RangerAccessResourceImpl(m));
        r.setUser(user);
        r.setUserGroups(Collections.emptySet());
        r.setAccessType("select");
        Set<RangerTagForEval> tags = new LinkedHashSet<>();
        tags.add(new RangerTagForEval(
                new RangerTag(tag, Collections.emptyMap()),
                RangerPolicyResourceMatcher.MatchType.SELF));
        RangerAccessRequestUtil.setRequestTagsInContext(r.getContext(), tags);
        return r;
    }
}
