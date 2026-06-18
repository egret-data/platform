import org.apache.ranger.authorization.hadoop.config.RangerPluginConfig;
import org.apache.ranger.authorization.utils.JsonUtils;
import org.apache.ranger.plugin.model.RangerPolicy;
import org.apache.ranger.plugin.policyengine.*;
import org.apache.ranger.plugin.util.ServicePolicies;
import org.junit.Test;
import java.nio.file.Files;
import java.nio.file.Paths;
import java.util.*;
import static org.junit.Assert.*;

public class EgretRangerEnforcementTest {
    private RangerPolicyEngineImpl engine() throws Exception {
        // ranger-plugins-common 2.4.0 exposes JsonUtils.jsonToObject(String, Class)
        // (the Reader overload only exists on newer Ranger trunk), so read to a String.
        String json = new String(Files.readAllBytes(Paths.get("fixtures/service-policies.json")));
        ServicePolicies sp = JsonUtils.jsonToObject(json, ServicePolicies.class);
        // A null pluginContext makes PolicyEngine.<init> NPE in ranger 2.4.0, so build a
        // minimal one. RangerPluginConfig lives in org.apache.ranger.authorization.hadoop.config.
        RangerPluginConfig cfg = new RangerPluginConfig(
                "trino", "trino_prod", "egret", null, null, new RangerPolicyEngineOptions());
        RangerPluginContext ctx = new RangerPluginContext(cfg);
        return new RangerPolicyEngineImpl(sp, ctx, null);
    }
    private RangerAccessRequestImpl req(String user, String access) {
        Map<String,Object> m = new HashMap<>();
        m.put("catalog","hive"); m.put("schema","default"); m.put("table","t"); m.put("column","email");
        return request(user, access, m);
    }
    // Row filtering is a table-level decision; the Trino rowFilterDef has no `column`
    // resource, so the request resource must stop at the table for the filter to match.
    private RangerAccessRequestImpl tableReq(String user, String access) {
        Map<String,Object> m = new HashMap<>();
        m.put("catalog","hive"); m.put("schema","default"); m.put("table","t");
        return request(user, access, m);
    }
    private RangerAccessRequestImpl request(String user, String access, Map<String,Object> m) {
        RangerAccessRequestImpl r = new RangerAccessRequestImpl();
        r.setResource(new RangerAccessResourceImpl(m));
        r.setUser(user); r.setUserGroups(Set.of()); r.setAccessType(access);
        return r;
    }
    @Test public void aliceSelectAllowed() throws Exception {
        RangerAccessResult res = engine().evaluatePolicies(req("alice","select"), RangerPolicy.POLICY_TYPE_ACCESS, null);
        assertTrue("alice should be allowed select", res.getIsAllowed());
    }
    @Test public void bobSelectDenied() throws Exception {
        RangerAccessResult res = engine().evaluatePolicies(req("bob","select"), RangerPolicy.POLICY_TYPE_ACCESS, null);
        assertFalse("bob has no policy -> not allowed", res.getIsAllowed());
    }
    @Test public void emailMasked() throws Exception {
        RangerAccessResult res = engine().evaluatePolicies(req("alice","select"), RangerPolicy.POLICY_TYPE_DATAMASK, null);
        assertEquals("MASK_HASH", res.getMaskType());
    }
    @Test public void rowFilterApplied() throws Exception {
        RangerAccessResult res = engine().evaluatePolicies(tableReq("alice","select"), RangerPolicy.POLICY_TYPE_ROWFILTER, null);
        assertEquals("country = 'US'", res.getFilterExpr());
    }
}
