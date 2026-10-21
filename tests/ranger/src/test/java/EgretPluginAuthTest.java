import com.sun.net.httpserver.HttpServer;
import java.net.InetSocketAddress;
import java.nio.file.Files;
import java.util.*;
import java.util.concurrent.CopyOnWriteArrayList;
import org.apache.ranger.admin.client.RangerAdminRESTClient;
import org.apache.ranger.authorization.hadoop.config.RangerPluginConfig;
import org.apache.ranger.plugin.util.RangerRoles;
import org.apache.ranger.plugin.util.RangerUserStore;
import org.apache.ranger.plugin.util.ServicePolicies;
import org.apache.ranger.plugin.util.ServiceTags;
import org.junit.Test;
import static org.junit.Assert.*;

/**
 * What a stock Ranger plugin SENDS to its admin. Measured, not assumed: egret's
 * auth design (docs/en/specs/2026-10-20-ranger-surface-auth-design.md) rests on it.
 * Run per version: mvn -q test -Dranger.version=2.8.0 -Dtest=EgretPluginAuthTest
 */
public class EgretPluginAuthTest {
  private static final String BODY =
      "{\"serviceName\":\"svc\",\"serviceId\":1,\"policyVersion\":5,\"policies\":[]}";

  /** One admin-client call; returns a short description of what it returned. */
  interface Call { String run(RangerAdminRESTClient c) throws Exception; }

  private static final Call POLICIES = c -> {
    ServicePolicies sp = c.getServicePoliciesIfUpdated(-1, 0);
    return sp == null ? "null" : "v" + sp.getPolicyVersion();
  };

  private static List<String> poll(String type, int status, Map<String, String> extra) throws Exception {
    return poll(type, status, extra, BODY, POLICIES);
  }

  private static List<String> poll(String type, int status, Map<String, String> extra, String body, Call call)
      throws Exception {
    List<String> seen = new CopyOnWriteArrayList<>();
    HttpServer s = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
    s.createContext("/", ex -> {
      seen.add(ex.getRequestMethod() + " " + ex.getRequestURI().getPath()
          + " auth=" + ex.getRequestHeaders().getFirst("Authorization"));
      byte[] b = status == 200 ? body.getBytes() : new byte[0];
      ex.getResponseHeaders().add("Content-Type", "application/json");
      ex.sendResponseHeaders(status, status == 200 ? b.length : -1);
      if (status == 200) ex.getResponseBody().write(b);
      ex.close();
    });
    s.start();
    try {
      String prefix = "ranger.plugin." + type;
      RangerPluginConfig cfg = new RangerPluginConfig(type, "svc", type, "cl", type, null);
      cfg.set(prefix + ".policy.rest.url", "http://127.0.0.1:" + s.getAddress().getPort() + "/ranger");
      cfg.set(prefix + ".service.name", "svc");
      cfg.set(prefix + ".policy.cache.dir", Files.createTempDirectory("rcache").toString());
      extra.forEach((k, v) -> cfg.set(prefix + "." + k, v));
      RangerAdminRESTClient c = new RangerAdminRESTClient();
      c.init("svc", "cl", prefix, cfg);
      try { seen.add("RESULT " + call.run(c)); }
      catch (Exception e) { seen.add("RESULT exception " + e.getClass().getSimpleName()); }
      return seen;
    } finally { s.stop(0); }
  }

  private static final Map<String, String> CREDS = Map.of(
      "policy.rest.client.username", "engine-1", "policy.rest.client.password", "s3cret");

  @Test public void withCredentialsThePluginSendsBasicOnTheSecurePath() throws Exception {
    for (String type : List.of("trino", "starrocks")) {
      List<String> seen = poll(type, 200, CREDS);
      assertTrue(type + ": " + seen, seen.contains(
          "GET /ranger/service/plugins/secure/policies/download/svc auth=Basic ZW5naW5lLTE6czNjcmV0"));
      assertTrue(type + ": " + seen, seen.contains("RESULT v5"));
    }
  }

  @Test public void withoutCredentialsThePluginSendsNoHeaderOnTheLegacyPath() throws Exception {
    List<String> seen = poll("trino", 200, Map.of());
    assertTrue(seen.toString(), seen.contains("GET /ranger/service/plugins/policies/download/svc auth=null"));
  }

  @Test public void a401IsSurvivedWithoutAnException() throws Exception {
    // Q2 (cold cache): the result of a 401 on a first poll. Record it, whatever it is.
    List<String> seen = poll("trino", 401, CREDS);
    System.out.println("Q2-cold " + seen);
    assertTrue(seen.toString(), seen.stream().anyMatch(x -> x.startsWith("RESULT")));
  }

  // Q4: the tag, roles and userstore downloads. Q3 ran a plugin whose service-def has no tag
  // enricher, so it never asked for tags; a plugin whose service has a tag service does
  // (RangerAdminTagRetriever -> getServiceTagsIfUpdated). Called directly on the client here.
  private static final Call TAGS = c -> {
    ServiceTags t = c.getServiceTagsIfUpdated(-1, 0);
    return t == null ? "null" : "v" + t.getTagVersion();
  };
  private static final Call ROLES = c -> {
    RangerRoles r = c.getRolesIfUpdated(-1, 0);
    return r == null ? "null" : "v" + r.getRoleVersion();
  };
  private static final Call USERSTORE = c -> {
    RangerUserStore u = c.getUserStoreIfUpdated(-1, 0);
    return u == null ? "null" : "v" + u.getUserStoreVersion();
  };
  private static final String TAGS_BODY = "{\"serviceName\":\"svc\",\"tagVersion\":5}";
  private static final String ROLES_BODY = "{\"serviceName\":\"svc\",\"roleVersion\":5}";
  private static final String USERSTORE_BODY = "{\"userStoreVersion\":5}";

  private static List<String> q4(String label, Map<String, String> extra, String body, Call call) throws Exception {
    List<String> seen = poll("trino", 200, extra, body, call);
    System.out.println("Q4 " + label + (extra.isEmpty() ? " no-creds " : " creds ") + seen);
    return seen;
  }

  @Test public void withCredentialsTagsAreFetchedFromTheSecureTagsPath() throws Exception {
    List<String> seen = q4("tags", CREDS, TAGS_BODY, TAGS);
    assertTrue(seen.toString(), seen.contains(
        "GET /ranger/service/tags/secure/download/svc auth=Basic ZW5naW5lLTE6czNjcmV0"));
    assertTrue(seen.toString(), seen.contains("RESULT v5"));
  }

  @Test public void withoutCredentialsTagsAreFetchedFromThePlainTagsPath() throws Exception {
    List<String> seen = q4("tags", Map.of(), TAGS_BODY, TAGS);
    assertTrue(seen.toString(), seen.contains("GET /ranger/service/tags/download/svc auth=null"));
  }

  @Test public void rolesAndUserstorePathsAreRecorded() throws Exception {
    for (Map<String, String> extra : List.of(CREDS, Map.<String, String>of())) {
      List<String> roles = q4("roles", extra, ROLES_BODY, ROLES);
      List<String> users = q4("userstore", extra, USERSTORE_BODY, USERSTORE);
      assertTrue(roles.toString(), roles.stream().anyMatch(x -> x.startsWith("RESULT")));
      assertTrue(users.toString(), users.stream().anyMatch(x -> x.startsWith("RESULT")));
    }
  }
}
