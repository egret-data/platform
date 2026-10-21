import com.sun.net.httpserver.HttpServer;
import java.net.InetSocketAddress;
import java.nio.file.Files;
import java.util.*;
import java.util.concurrent.CopyOnWriteArrayList;
import org.apache.ranger.admin.client.RangerAdminRESTClient;
import org.apache.ranger.authorization.hadoop.config.RangerPluginConfig;
import org.apache.ranger.plugin.util.ServicePolicies;
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

  private static List<String> poll(String type, int status, Map<String, String> extra) throws Exception {
    List<String> seen = new CopyOnWriteArrayList<>();
    HttpServer s = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
    s.createContext("/", ex -> {
      seen.add(ex.getRequestMethod() + " " + ex.getRequestURI().getPath()
          + " auth=" + ex.getRequestHeaders().getFirst("Authorization"));
      byte[] b = status == 200 ? BODY.getBytes() : new byte[0];
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
      try { ServicePolicies sp = c.getServicePoliciesIfUpdated(-1, 0); seen.add("RESULT " + (sp == null ? "null" : "v" + sp.getPolicyVersion())); }
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
}
