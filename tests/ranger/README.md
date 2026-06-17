# Ranger policy-engine enforcement gate

Proves Egret's ServicePolicies enforce in the real Apache Ranger policy engine
(the same RangerPolicyEngineImpl the Trino plugin runs).

Requires: JDK 11+, Maven, network (pulls org.apache.ranger:ranger-plugins-common:2.4.0 from Maven Central).

Run:

```bash
EGRET_TEST_RANGER_E2E=1 cargo test -p egret-ranger ranger_engine_gate
# or directly:
cd tests/ranger && mvn -q test
```

Optional — regenerate fixtures/service-policies.json from a running Egret:

```bash
curl -s -H "Authorization: Bearer devtoken" \
  "http://localhost:8080/service/plugins/policies/download/trino_prod" \
  > tests/ranger/fixtures/service-policies.json
```

Asserts: alice select allowed; bob denied; column email → MASK_HASH; row filter "country = 'US'".

(ranger-plugins-common version used: 2.4.0)

## API notes (ranger-plugins-common 2.4.0)

Two adjustments from the task scaffold were needed to compile/run against the
published 2.4.0 artifact (the local ranger trunk checkout differs):

- `JsonUtils.jsonToObject(Reader, Class)` does not exist in 2.4.0 — only
  `jsonToObject(String, Class)`. The test reads the fixture into a String.
- `new RangerPolicyEngineImpl(sp, null, null)` NPEs in `PolicyEngine.<init>`
  (null pluginContext). The test builds a minimal
  `RangerPluginContext(new RangerPluginConfig("trino","trino_prod","egret",null,null,new RangerPolicyEngineOptions()))`.
  Note `RangerPluginConfig` is in `org.apache.ranger.authorization.hadoop.config`.
- The row-filter request resource stops at `table` (no `column`), since the
  Trino `rowFilterDef` only defines catalog/schema/table.

Verified: `mvn test` → `Tests run: 4, Failures: 0, Errors: 0` (BUILD SUCCESS),
JDK 21 / Maven, 2026-06-17.
