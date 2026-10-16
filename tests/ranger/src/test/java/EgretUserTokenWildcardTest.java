import static org.junit.Assert.assertEquals;

import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Paths;
import java.util.Collections;
import java.util.HashMap;
import java.util.Map;
import org.apache.ranger.authorization.hadoop.config.RangerPluginConfig;
import org.apache.ranger.authorization.utils.JsonUtils;
import org.apache.ranger.plugin.model.RangerPolicy;
import org.apache.ranger.plugin.model.RangerServiceDef;
import org.apache.ranger.plugin.policyengine.RangerAccessRequestImpl;
import org.apache.ranger.plugin.policyengine.RangerAccessResourceImpl;
import org.apache.ranger.plugin.policyengine.RangerAccessResult;
import org.apache.ranger.plugin.policyengine.RangerPluginContext;
import org.apache.ranger.plugin.policyengine.RangerPolicyEngineImpl;
import org.apache.ranger.plugin.policyengine.RangerPolicyEngineOptions;
import org.apache.ranger.plugin.util.ServicePolicies;
import org.junit.Test;

/**
 * DG Task 7: egret seeds {@code trinouser = {USER}} ({@code impersonate}, users {@code {USER}}) on
 * every delegating Trino profile, so a user may become only themself. {@code trinouser} is declared
 * {@code wildCard: true, ignoreCase: true} in {@code trino.json}. The question this measures in
 * the REAL policy engine: after {@code {USER}} is expanded to the requesting user, is the result
 * matched as a literal or as a wildcard pattern? If it were a pattern, a user named {@code *},
 * {@code a*} or {@code ?lice} could become other users.
 *
 * <p>The policy is the exact body {@code egret_engine_mgmt::delegator_policy::self_impersonate_policy}
 * produces (pinned field by field by that module's tests). Each row prints its outcome, so a run
 * records what the jar does even where an assertion fails.
 */
public class EgretUserTokenWildcardTest {

    private static final String DEFS = "../../crates/egret-policy/src/service-defs/";
    private static final long SELF_ID = 9201L;

    @Test
    public void theExpandedUserIsMatchedAsALiteralNotAsAPattern() throws Exception {
        RangerPolicyEngineImpl engine = engine();
        Object[][] rows = {
            // requester, target, expected allowed
            {"alice", "alice", true},
            {"alice", "bob", false},
            {"*", "bob", false},
            {"*", "alice", false},
            {"a*", "alice2", false},
            {"a*", "alice", false},
            {"?lice", "alice", false},
            {"al?ce", "alice", false},
            {"alice", "alice2", false},
            {"alice2", "alice", false},
            // ignoreCase: measured and pinned, not a wildcard question.
            {"alice", "ALICE", true},
        };
        StringBuilder report = new StringBuilder();
        boolean ok = true;
        for (Object[] row : rows) {
            String user = (String) row[0];
            String target = (String) row[1];
            boolean expected = (Boolean) row[2];
            RangerAccessResult r = engine.evaluatePolicies(
                    request(user, target), RangerPolicy.POLICY_TYPE_ACCESS, null);
            boolean allowed = r.getIsAccessDetermined() && r.getIsAllowed();
            report.append(String.format("  %-7s -> %-7s allowed=%-5s policyId=%d%n",
                    user, target, allowed, r.getPolicyId()));
            if (allowed != expected) {
                ok = false;
            }
        }
        System.out.println("EgretUserTokenWildcardTest (ranger " + version() + "):\n" + report);
        assertEquals("rows:\n" + report, true, ok);
    }

    /**
     * The policy cannot reach a request on another hierarchy, whatever {@code {USER}} expands to:
     * the counterpart of {@code egret_ranger_eval::eval::resource_tokens_cannot_match}, which stops
     * reporting the marker exactly there.
     */
    @Test
    public void theSelfPolicyDoesNotReachATableRequest() throws Exception {
        RangerPolicyEngineImpl engine = engine();
        for (String user : new String[] {"alice", "*"}) {
            Map<String, Object> m = new HashMap<>();
            m.put("catalog", "hive");
            m.put("schema", "sales");
            m.put("table", "orders");
            m.put("column", "id");
            RangerAccessRequestImpl r = new RangerAccessRequestImpl();
            r.setResource(new RangerAccessResourceImpl(m));
            r.setUser(user);
            r.setUserGroups(Collections.emptySet());
            r.setAccessType("select");
            RangerAccessResult res = engine.evaluatePolicies(r, RangerPolicy.POLICY_TYPE_ACCESS, null);
            System.out.println("table request as " + user + ": determined=" + res.getIsAccessDetermined()
                    + " allowed=" + res.getIsAllowed() + " policyId=" + res.getPolicyId());
            // No policy decided it (policy id -1) and it is not allowed: the engine's own
            // "nothing matched", which egret-ranger-eval reports as Denied{NoPolicyMatched}.
            assertEquals(user, false, res.getIsAllowed());
            assertEquals(user, -1L, res.getPolicyId());
        }
    }

    private static String version() {
        Package p = RangerPolicyEngineImpl.class.getPackage();
        return p == null ? "?" : String.valueOf(p.getImplementationVersion());
    }

    private static RangerServiceDef def() throws Exception {
        String json = new String(Files.readAllBytes(Paths.get(DEFS + "trino.json")), StandardCharsets.UTF_8);
        JsonObject o = JsonParser.parseString(json).getAsJsonObject();
        o.remove("contextEnrichers");
        return JsonUtils.jsonToObject(o.toString(), RangerServiceDef.class);
    }

    private static RangerPolicyEngineImpl engine() throws Exception {
        RangerPolicy p = new RangerPolicy();
        p.setId(SELF_ID);
        p.setName("egret-user-impersonate-self");
        p.setService("vg_trino");
        p.setServiceType("trino");
        p.setPolicyType(RangerPolicy.POLICY_TYPE_ACCESS);
        p.setIsEnabled(true);
        Map<String, RangerPolicy.RangerPolicyResource> res = new HashMap<>();
        res.put("trinouser", new RangerPolicy.RangerPolicyResource("{USER}", false, false));
        p.setResources(res);
        RangerPolicy.RangerPolicyItem item = new RangerPolicy.RangerPolicyItem();
        item.setUsers(Collections.singletonList("{USER}"));
        item.setAccesses(Collections.singletonList(
                new RangerPolicy.RangerPolicyItemAccess("impersonate", true)));
        p.setPolicyItems(Collections.singletonList(item));

        ServicePolicies sp = new ServicePolicies();
        sp.setServiceName("vg_trino");
        sp.setServiceId(1L);
        sp.setPolicyVersion(1L);
        sp.setServiceDef(def());
        sp.setPolicies(Collections.singletonList(p));
        RangerPluginConfig cfg = new RangerPluginConfig(
                "trino", "vg_trino", "egret", null, null, new RangerPolicyEngineOptions());
        return new RangerPolicyEngineImpl(sp, new RangerPluginContext(cfg), null);
    }

    private static RangerAccessRequestImpl request(String user, String target) {
        Map<String, Object> m = new HashMap<>();
        m.put("trinouser", target);
        RangerAccessRequestImpl r = new RangerAccessRequestImpl();
        r.setResource(new RangerAccessResourceImpl(m));
        r.setUser(user);
        r.setUserGroups(Collections.emptySet());
        r.setAccessType("impersonate");
        return r;
    }
}
