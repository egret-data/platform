# StarRocks delegation and engine lifecycle (package D): implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** an impersonate-mode StarRocks engine under Ranger runs a person's query through egret,
its CN survives the root password, and deleting it leaves no Job pods and no FE metadata behind.

**Architecture.**
- **D1b:** a new IR object `InfraObject::AnyUserExcept`, emitted for StarRocks impersonate profiles
  and rendered as `egret-delegator-impersonate` on resource `user` with `isExcludes: true`.
- **D1a:** a new engine-mgmt module `starrocks_users.rs` reconciles one engine's StarRocks users
  against the workspace members and the platform admins, recorded in `engine_mgmt.starrocks_users`. It is woken by
  `pg_notify` triggers (migration 0219), a 10-minute sweep, every StarRocks apply, and the bootstrap.
- **Native access control** (an engine bound to no profile): the same sync grants the delegator
  `IMPERSONATE` per managed user and revokes `ON ALL USERS`, which the bootstrap stops granting.
- **D2 and D3:** provisioner changes in `crates/egret-provisioner/src/starrocks.rs`, and a StarRocks
  create refuses (409) a name whose old FE metadata claim still exists.

**Tech stack:** Rust (sqlx 0.9, tokio, axum), PostgreSQL triggers, the kube-starrocks operator,
StarRocks 4.1.7 (egret's fork), Apache Ranger 2.9.0 plugin.

**Spec:** `docs/en/specs/2026-11-11-starrocks-delegation-and-lifecycle-design.md` (and its `docs/vi`
mirror). **Root causes:** `.superpowers/sdd/package-d-root-causes.md`.

**Branch:** `fix/starrocks-delegation-and-lifecycle` in the worktree
`/Users/1pro-l4r8/Documents/github/data-portal/egret-srd`, based on package A's
`feat/starrocks-query-log` (PR #849, not merged). Commit; do not push. Docs go to `egret-data/docs`,
branch `docs/starrocks-delegation-and-lifecycle`, after the platform PR merges.

**Task reports** go to `.superpowers/sdd/2026-11-11-starrocks-delegation-and-lifecycle/task-<n>-report.md`
(git-ignored).

## Decided on 2026-11-11

The user ruled on the spec's four open questions (spec, "Decided on 2026-11-11"):

1. **Platform admins are synced** on every impersonate engine assigned to at least one workspace,
   with trigger coverage for platform-role grants and revokes (`access_grants`) and the role table
   (`roles`). Tasks 7, 8, 13.
2. **Native access control is fixed here**: the sync grants `IMPERSONATE ON USER` per user it
   manages and revokes `ON ALL USERS`; reserved users are never granted; the bootstrap stops granting
   `ON ALL USERS`; existing engines converge on their next apply or sweep. Tasks 1 (M6, M7), 10, 12,
   13.
3. **Users egret did not create stay untouched**, logged, with the handover documented. Tasks 8, 10,
   14.
4. **`POST /engines` (StarRocks) answers 409 `engine_metadata_left_over`** when
   `fe-meta-<name>-fe-0` exists, with the `kubectl` cleanup in the message; the upgrade note stays.
   Tasks 6, 13, 14.

## Global Constraints

- **Build:** your own target directory, `CARGO_TARGET_DIR=$PWD/target-srd`, run from the worktree
  root. Never the shared `target/`.
- **DB tests:** run with
  `env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test`.
  A DB-gated test that skipped is a failure of the report, not a pass; give its duration.
- **Never run two test suites at once** (one `cargo test` at a time, across all agents).
- **Docker and stacks:** never prune Docker; never touch containers or kind objects you did not
  start, except the ones a controller task names; never run `just up`, `just down`,
  `just reset-testdata`.
- **StarRocks images:** only egret's fork images (`ghcr.io/egret-data/egret-starrocks-fe`,
  `-cn`, `-allin1`), never the official ones.
- **Frontend client:** regenerated only with `npm run gen:api` in `frontend/`. Task 6 changes one
  response description of `POST /engines`; regenerate `openapi.json`
  (`CARGO_TARGET_DIR=$PWD/target-srd cargo run -q -p egret-bin -- openapi > openapi.json`), then run
  `npm run gen:api` and commit whatever it changes.
- **SQL quoting:** every user name and password reaches StarRocks SQL only through
  `egret_query_api::quote_string(Engine::StarRocks, …)` (it doubles `'` and escapes `\`). An
  identifier in backticks goes through `egret_catalog_registry::escape_ident`.
- **sqlx 0.9:** `sqlx::query*` take `&'static str`: SQL is a literal or `concat!`, never `format!`.
- **FE errors:** reach a log or a response only through `FeError::scrubbed(&fe, &secrets)`, with
  every password the statement carried in `secrets`.
- **Tests:** every behaviour change has a test that fails without it and a mutant of the change
  that the test kills, recorded in the task report (the mutant, the command, the failing test).
- **Golden gates:** any change to infra rows re-blesses and verifies the golden corpus:
  1. bless: `EGRET_GOLDEN_BLESS=1 cargo test -p egret-perm-compile --features harness --test golden_compile`
     (with the DB env above);
  2. verify semantically with Task 3's script: the only change is the new row;
  3. pass unblessed: the same command without `EGRET_GOLDEN_BLESS`;
  4. pass on OPA: `EGRET_GOLDEN_MECHANISM=opa cargo test -p egret-perm-compile --features harness --test golden_compile --no-fail-fast`.
- **Migration number:** 0219. Before committing it, check that no file `migrations/0219_*` exists on
  `main` or on any open PR (`gh pr list --state open --json files`); renumber above both if taken.
- **CI parity before finishing** (Task 11 runs it, and any later code change re-runs it):
  `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets -- -D warnings`; the policy
  suites `egret-perm-compile --features harness`, `egret-perm-eval`, `egret-policy` and
  `egret-ranger-eval`, including the `EGRET_TEST_RANGER_E2E=1` gates CI runs.
- **Commits:** Conventional Commits, each ending with
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. No `Closes #N` in commits.

---

### Task 1: Measure M1 to M3, M6 and M7 before code depends on them (controller)

The spec's "Not measured yet" M1 to M3, M6 and M7. Tasks 2, 8 and 10 build on them; if a result differs from what
they assume, stop and tell the user before Task 2.

**Files:** none in git. Write
`.superpowers/sdd/2026-11-11-starrocks-delegation-and-lifecycle/task-1-measurements.md` with each
command and its verbatim output.

- [ ] **Step 1: Start the fork e2e with the D1b row added, kept running.**

  Copy the fork's e2e to the scratchpad (do not commit to the fork):

  ```bash
  SCRATCH=/private/tmp/claude-501/-Users-1pro-l4r8-Documents-github-data-portal/739fff40-b4cd-44a8-8f4d-649cbeb36373/scratchpad/pkgd-e2e
  rm -rf "$SCRATCH" && mkdir -p "$SCRATCH"
  cp -R /Users/1pro-l4r8/Documents/github/data-portal/egret-starrocks/tests/e2e "$SCRATCH/e2e"
  ```

  In `$SCRATCH/e2e/policies.py`, after the `policy("impersonate", …)` line, add (keep `svc`'s own
  row: the fork's checks use it):

  ```python
  policy("egret-delegator-impersonate", 0,
         {"user": {"values": ["egret_delegator", "egret_query_log", "root"],
                   "isExcludes": True, "isRecursive": False}},
         policyItems=[item(["egret_delegator"], accesses=[{"type": "impersonate", "isAllowed": True}])])
  ```

  Run it and leave it up:

  ```bash
  KEEP=1 bash "$SCRATCH/e2e/run.sh" ghcr.io/egret-data/egret-starrocks-allin1:4.1.7-egret.4 2>&1 | tee "$SCRATCH/run.log"
  ```

  Expected: the fork's own checks pass, and the last line names the kept containers (`egs-e2e-*`)
  and the fake Ranger pid.

- [ ] **Step 2: M1, the exclusion on the real plugin.** As root on port 19130 (no password in the
  allin1 image):

  ```bash
  M="mysql -h127.0.0.1 -P19130 -uroot"
  $M -e "CREATE USER 'egret_delegator'@'%' IDENTIFIED BY 'pkgd-delegator'; \
         CREATE USER IF NOT EXISTS 'egret_query_log'@'%' IDENTIFIED BY 'pkgd-loader'; \
         CREATE USER 'pkgd_bob'@'%' IDENTIFIED BY 'pkgd-bob';"
  D="mysql -h127.0.0.1 -P19130 -uegret_delegator -ppkgd-delegator"
  for target in pkgd_bob root ROOT egret_query_log egret_delegator; do
    echo "== EXECUTE AS $target"; $D -e "EXECUTE AS \`$target\` WITH NO REVERT; SELECT current_user();" 2>&1
  done
  ```

  Expected: `pkgd_bob` answers `'pkgd_bob'@'%'`; `root`, `egret_query_log` and `egret_delegator`
  fail with `ERROR 5204 … IMPERSONATE privilege(s) on USER …`; `ROOT` fails either with that error
  or with `cannot find user` (record which: it says whether StarRocks resolves user names
  case-insensitively). If `root` is **allowed**, the plugin ignores `isExcludes` here: stop and tell
  the user (the spec's fallback is a separate deny row).

- [ ] **Step 3: M2, `SHOW USERS`.**

  ```bash
  $M --column-names -e "SHOW USERS"
  ```

  Record the header and two values verbatim. Task 8's `listed_users` assumes one column whose
  values read `'pkgd_bob'@'%'`; if the shape differs, write the measured shape into Task 8's report
  and adjust `listed_users` and its test to it there.

- [ ] **Step 4: M3, the two error texts.**

  ```bash
  $M -e "CREATE USER 'pkgd_bob'@'%' IDENTIFIED BY 'x'" 2>&1
  $M -e "DROP USER 'pkgd_absent'@'%'" 2>&1
  ```

  Record both. Task 8's `FakeStarRocksDdl` answers with these exact texts.

- [ ] **Step 5: M6 and M7, native access control.** The fork e2e runs Ranger, so use a plain
  allin1 container (native access control by default), your own:

  ```bash
  docker run -d --name pkgd-native -p 19230:9030 ghcr.io/egret-data/egret-starrocks-allin1:4.1.7-egret.4
  N="mysql -h127.0.0.1 -P19230 -uroot"
  until $N -e 'SELECT 1' >/dev/null 2>&1; do sleep 5; done
  $N -e "CREATE USER 'egret_delegator'@'%' IDENTIFIED BY 'pkgd-d'; \
         CREATE USER 'pkgd_a'@'%' IDENTIFIED BY 'a'; CREATE USER 'pkgd_b'@'%' IDENTIFIED BY 'b';"
  $N -e "GRANT IMPERSONATE ON ALL USERS TO USER 'egret_delegator'@'%'; \
         GRANT IMPERSONATE ON USER 'pkgd_a'@'%' TO USER 'egret_delegator'@'%';"
  echo "== M6"; $N --column-names -e "SHOW GRANTS FOR 'egret_delegator'@'%'"
  $N -e "REVOKE IMPERSONATE ON ALL USERS FROM USER 'egret_delegator'@'%'" 2>&1
  echo "== M7a: is pkgd_a still granted?"; $N --column-names -e "SHOW GRANTS FOR 'egret_delegator'@'%'"
  echo "== M7b: revoke of nothing"; $N -e "REVOKE IMPERSONATE ON ALL USERS FROM USER 'egret_delegator'@'%'" 2>&1
  echo "== M7c: repeated grant"; $N -e "GRANT IMPERSONATE ON USER 'pkgd_a'@'%' TO USER 'egret_delegator'@'%'" 2>&1
  $N -e "GRANT IMPERSONATE ON USER 'pkgd_a'@'%' TO USER 'egret_delegator'@'%'" 2>&1
  D="mysql -h127.0.0.1 -P19230 -uegret_delegator -ppkgd-d"
  for t in pkgd_a pkgd_b root; do
    echo "== EXECUTE AS $t"; $D -e "EXECUTE AS \`$t\` WITH NO REVERT; SELECT current_user();" 2>&1
  done
  docker rm -f pkgd-native
  ```

  Record every answer verbatim. Expected: `pkgd_a` allowed; `pkgd_b` and `root` refused (with
  M7a's answer deciding whether `pkgd_a` needed the grant again). Task 10's parser
  (`delegator_impersonation`) and its fake answers follow M6's `Grants` text and M7's answers; if the
  `Grants` column does not contain `IMPERSONATE ON ALL USERS` / `IMPERSONATE ON USER '<u>'@'%'`
  verbatim, write the measured form into Task 10's report and adjust the parser there.

- [ ] **Step 6: Tear down.** `docker rm -f egs-e2e-sr egs-e2e-rest egs-e2e-s3`,
  `docker network rm egs-e2e-net`, kill the fake Ranger pid from step 1. Remove `$SCRATCH`'s
  `.venv` only if you created it.

---

### Task 2: The reserved StarRocks users and the D1b row in the IR, its body and the evaluator

**Files:**
- Modify: `crates/egret-core/src/engine_principals.rs` (constants, predicate, tests)
- Modify: `crates/egret-core/src/engine_delegation.rs:76-82` (`delegator_impersonates`) and its
  test `delegator_needs_trino_impersonate_and_auth`
- Modify: `crates/egret-perm-ir/src/statement.rs:84-99` (`InfraObject`)
- Modify: `crates/egret-perm-ir/src/lower/infra.rs` (`InfraInputs` doc, `lower_infra`, tests)
- Modify: `crates/egret-policy-api/src/infra_bodies.rs` (new body, test)
- Modify: `crates/egret-perm-compile/src/ranger/infra.rs:51-54` (`render`), tests
- Modify: `crates/egret-perm-eval/src/decide.rs:350-358` (impersonation arm)
- Test: `crates/egret-perm-eval/tests/decide.rs` (new test)
- Modify: `crates/egret-engine-mgmt/src/engines.rs:9276-9290` (the StarRocks loop of the
  delegation-parity test)

**Interfaces:**
- Consumes: nothing new.
- Produces:

  ```rust
  // egret_core::engine_principals
  pub const STARROCKS_ROOT_USER: &str = "root";
  pub const STARROCKS_RESERVED_USERS: [&str; 3];   // ["egret_delegator", "egret_query_log", "root"]
  pub fn is_starrocks_reserved_user(name: &str) -> bool;
  // egret_perm_ir::statement
  pub enum InfraObject { /* … */ AnyUserExcept(Vec<String>) }
  // egret_policy_api::infra_bodies
  pub fn starrocks_delegator_impersonate_policy(service: &str, excluded: &[String]) -> serde_json::Value;
  ```

- [ ] **Step 1: Write the failing tests.**

  `crates/egret-core/src/engine_principals.rs`, in `mod tests`:

  ```rust
  /// Package D: one list for the Ranger exclusion (D1b) and the user sync
  /// (D1a). Sorted, so a rendered body is byte-stable.
  #[test]
  fn the_starrocks_reserved_users_are_sorted_and_name_root_and_egrets_own() {
      assert_eq!(
          STARROCKS_RESERVED_USERS,
          ["egret_delegator", "egret_query_log", "root"]
      );
      assert!(is_starrocks_reserved_user("root"));
      assert!(is_starrocks_reserved_user("ROOT"));
      assert!(is_starrocks_reserved_user("egret_query_log"));
      assert!(!is_starrocks_reserved_user("root-2"));
      assert!(!is_starrocks_reserved_user("alice"));
  }
  ```

  `crates/egret-core/src/engine_delegation.rs`, replace the body of
  `delegator_needs_trino_impersonate_and_auth` with:

  ```rust
  let auth = json!({"trinoConfig": {"auth": {"passwordUsers": []}}});
  assert!(delegator_impersonates("trino", &auth));
  assert!(!delegator_impersonates("trino", &json!({})));
  assert!(!delegator_impersonates("starrocks", &auth));
  let jwt = json!({"trinoConfig": {"delegationMode": "jwt", "auth": {"jwt": {}}}});
  assert!(!delegator_impersonates("trino", &jwt));
  // Package D: a StarRocks engine in impersonate mode creates the delegator
  // in its bootstrap, so it impersonates with it.
  assert!(delegator_impersonates(
      "starrocks",
      &json!({"starrocksConfig": {"delegation": {"mode": "impersonate"}}})
  ));
  assert!(!delegator_impersonates(
      "starrocks",
      &json!({"starrocksConfig": {"delegation": {"mode": "jwt"}}})
  ));
  assert!(!delegator_impersonates("starrocks", &json!({})));
  ```

  `crates/egret-perm-ir/src/lower/infra.rs`, in `mod tests`:

  ```rust
  fn reserved() -> Vec<String> {
      vec!["egret_delegator".into(), "egret_query_log".into(), "root".into()]
  }

  /// Package D, D1b: a StarRocks profile whose engine impersonates with the
  /// delegator gets the delegator's impersonation, with root, the delegator and
  /// the query-log loader excluded, and none of the Trino baseline.
  #[test]
  fn starrocks_impersonate_emits_the_delegator_row_without_the_reserved_users() {
      let s = lower_infra(&inputs(EngineType::StarRocks, &["a"], true, true));
      assert_eq!(
          kinds(&s),
          vec![InfraKind::AdminAllAccess, InfraKind::DelegatorImpersonate]
      );
      assert_eq!(s[1].target, Target::Infra(InfraObject::AnyUserExcept(reserved())));
      assert_eq!(s[1].audience.include, vec![user("egret_delegator")]);
      assert!(s[1].audience.except.is_empty());
      assert_eq!(
          s[1].effect,
          Effect::EnginePrivilege {
              capability: "impersonate".into()
          }
      );
  }

  #[test]
  fn starrocks_with_the_query_log_orders_the_delegator_row_after_it() {
      let s = lower_infra(&with_query_log(inputs(
          EngineType::StarRocks,
          &["a"],
          true,
          true,
      )));
      assert_eq!(
          kinds(&s),
          vec![
              InfraKind::AdminAllAccess,
              InfraKind::QueryLogLoader,
              InfraKind::QueryLogGuard,
              InfraKind::DelegatorImpersonate
          ]
      );
  }

  #[test]
  fn starrocks_without_the_delegator_or_not_delegating_emits_no_impersonation() {
      let s = lower_infra(&inputs(EngineType::StarRocks, &["a"], true, false));
      assert_eq!(kinds(&s), vec![InfraKind::AdminAllAccess]);
      assert!(lower_infra(&inputs(EngineType::StarRocks, &["a"], false, true)).is_empty());
  }
  ```

  `crates/egret-policy-api/src/infra_bodies.rs`, in `mod tests`:

  ```rust
  /// Package D, D1b: the StarRocks body, whole, so any drift shows.
  #[test]
  fn the_starrocks_delegator_body_excludes_the_reserved_users() {
      let excluded: Vec<String> = vec![
          "egret_delegator".into(),
          "egret_query_log".into(),
          "root".into(),
      ];
      assert_eq!(
          starrocks_delegator_impersonate_policy("sr", &excluded),
          json!({
              "name": "egret-delegator-impersonate",
              "service": "sr",
              "serviceType": "starrocks",
              "policyType": 0,
              "isEnabled": true,
              "resources": {
                  "user": {
                      "values": ["egret_delegator", "egret_query_log", "root"],
                      "isExcludes": true,
                      "isRecursive": false
                  }
              },
              "policyItems": [{
                  "users": ["egret_delegator"],
                  "groups": [],
                  "roles": [],
                  "accesses": [{ "type": "impersonate", "isAllowed": true }],
                  "delegateAdmin": false
              }],
              "denyPolicyItems": [],
              "allowExceptions": [],
              "denyExceptions": [],
              "isDenyAllElse": false,
              "policyPriority": 0
          })
      );
  }
  ```

  `crates/egret-perm-compile/src/ranger/infra.rs`, in `mod tests` (Step 3 adds `InfraObject` and
  `Target` to the module's own imports, which the tests' `use super::*` brings in):

  ```rust
  fn sr_delegator(delegating: bool, imp: bool) -> Vec<Statement> {
      lower_infra(&InfraInputs {
          engine: EngineType::StarRocks,
          admins: vec![],
          delegating,
          delegator_impersonates: imp,
          query_log: false,
      })
  }

  /// Package D, D1b.
  #[test]
  fn starrocks_renders_the_delegator_row_on_the_user_resource() {
      let rows: Vec<_> = sr_delegator(true, true)
          .iter()
          .flat_map(|s| render(s, "sr", EngineType::StarRocks).unwrap())
          .collect();
      let excluded: Vec<String> = egret_core::engine_principals::STARROCKS_RESERVED_USERS
          .iter()
          .map(|s| s.to_string())
          .collect();
      assert_eq!(
          rows,
          vec![(
              b::IMPERSONATE_POLICY_NAME.to_string(),
              b::starrocks_delegator_impersonate_policy("sr", &excluded)
          )]
      );
  }

  #[test]
  fn a_delegator_row_whose_object_does_not_fit_the_engine_is_refused() {
      let trino = lower_infra(&InfraInputs {
          engine: EngineType::Trino,
          admins: vec![],
          delegating: true,
          delegator_impersonates: true,
          query_log: false,
      });
      let delegator = trino
          .iter()
          .find(|s| matches!(s.source, Source::Infra { kind: InfraKind::DelegatorImpersonate }))
          .unwrap();
      assert!(render(delegator, "sr", EngineType::StarRocks).is_err());
      let sr = sr_delegator(true, true).remove(0);
      assert!(render(&sr, "svc", EngineType::Trino).is_err());
  }

  #[test]
  fn an_empty_exclusion_list_is_refused() {
      let mut s = sr_delegator(true, true).remove(0);
      s.target = Target::Infra(InfraObject::AnyUserExcept(vec![]));
      assert!(render(&s, "sr", EngineType::StarRocks).is_err());
  }
  ```

  `crates/egret-perm-eval/tests/decide.rs`, after `impersonation_rules`:

  ```rust
  /// Package D, D1b: the StarRocks delegator may become anyone but the
  /// reserved users, matched like Ranger's `ignoreCase` `user` resource.
  #[test]
  fn the_starrocks_delegator_may_become_anyone_but_the_reserved_users() {
      let s = snap(
          vec![infra(
              InfraKind::DelegatorImpersonate,
              who(&["egret_delegator"], &[]),
              InfraObject::AnyUserExcept(vec![
                  "egret_delegator".into(),
                  "egret_query_log".into(),
                  "root".into(),
              ]),
              "impersonate",
          )],
          vec![],
      );
      let op = "ImpersonateUser";
      assert!(allowed(&s.decide(
          "egret_delegator",
          op,
          &Resource::User("ann".into())
      )));
      for target in ["root", "ROOT", "egret_query_log", "egret_delegator"] {
          assert!(
              !allowed(&s.decide("egret_delegator", op, &Resource::User(target.into()))),
              "{target}"
          );
      }
      assert!(!allowed(&s.decide("ann", op, &Resource::User("bob".into()))));
  }
  ```

  `crates/egret-engine-mgmt/src/engines.rs`: in the delegation-parity test's StarRocks loop (about
  line 9276), replace `assert!(!ed::delegator_impersonates("starrocks", &spec), "{spec}");` and the
  two lines after it with:

  ```rust
  let mut e = dummy_engine();
  e.r#type = "starrocks".into();
  e.spec = spec.clone();
  assert_eq!(
      ed::delegator_impersonates("starrocks", &spec),
      e.is_impersonate_delegation(),
      "{spec}"
  );
  ```

- [ ] **Step 2: Run them and see them fail** (compile errors count as failing):

  ```bash
  CARGO_TARGET_DIR=$PWD/target-srd cargo test -p egret-core -p egret-perm-ir -p egret-policy-api --lib
  ```

  Expected: `STARROCKS_RESERVED_USERS`, `AnyUserExcept` and
  `starrocks_delegator_impersonate_policy` are not found.

- [ ] **Step 3: Implement.**

  `crates/egret-core/src/engine_principals.rs`, after `QUERY_LOG_LOADER_PRINCIPAL`:

  ```rust
  /// StarRocks' built-in superuser. Never an impersonation target and never a
  /// user egret creates for a person (package D).
  pub const STARROCKS_ROOT_USER: &str = "root";

  /// The StarRocks users egret's delegator may never become (the D1b Ranger
  /// row's exclusion) and that egret never creates or drops for a person (the
  /// D1a user sync). Sorted, so a rendered body is byte-stable.
  pub const STARROCKS_RESERVED_USERS: [&str; 3] = [
      DELEGATOR_PRINCIPAL,
      QUERY_LOG_LOADER_PRINCIPAL,
      STARROCKS_ROOT_USER,
  ];

  /// Whether `name` is one of [`STARROCKS_RESERVED_USERS`], ignoring ASCII
  /// case like the Ranger `user` resource (`ignoreCase: true`).
  pub fn is_starrocks_reserved_user(name: &str) -> bool {
      STARROCKS_RESERVED_USERS
          .iter()
          .any(|r| r.eq_ignore_ascii_case(name))
  }
  ```

  `crates/egret-core/src/engine_delegation.rs`, replace `delegator_impersonates`:

  ```rust
  /// Whether applying this engine provisions the `egret_delegator` service user
  /// and its impersonation grant: a Trino engine in `impersonate` mode with
  /// auth, or a StarRocks engine in `impersonate` mode, whose post-Ready
  /// bootstrap creates the delegator (package D).
  pub fn delegator_impersonates(engine_type: &str, spec: &Value) -> bool {
      let impersonate = delegation_mode(engine_type, spec) == "impersonate";
      match engine_type {
          "trino" => impersonate && trino_auth_enabled(spec),
          "starrocks" => impersonate,
          _ => false,
      }
  }
  ```

  `crates/egret-perm-ir/src/statement.rs`, in `InfraObject` after `AnyUser`:

  ```rust
      /// Any user except these, as the object of impersonation (StarRocks: the
      /// delegator may not become root or egret's own users). Matched without
      /// regard to ASCII case, like Ranger's `user` resource.
      AnyUserExcept(Vec<String>),
  ```

  `crates/egret-perm-ir/src/lower/infra.rs`:
  - import `STARROCKS_RESERVED_USERS` beside `DELEGATOR_PRINCIPAL`;
  - change the `delegator_impersonates` field doc to "At least one bound engine impersonates with
    `egret_delegator` (`egret_core::engine_delegation::delegator_impersonates`)";
  - update `lower_infra`'s doc order to "`AdminAllAccess`, `QueryLogLoader`, `QueryLogGuard`
    (StarRocks only), `DelegatorImpersonate`, `SelfImpersonate`, `ExecuteQuery`, `QueryTimeout`
    (the last three Trino only)";
  - insert immediately before `if !i.delegating || i.engine != EngineType::Trino {`:

  ```rust
      if i.engine == EngineType::StarRocks && i.delegating && i.delegator_impersonates {
          out.push(statement(
              InfraKind::DelegatorImpersonate,
              vec![user(DELEGATOR_PRINCIPAL)],
              InfraObject::AnyUserExcept(
                  STARROCKS_RESERVED_USERS
                      .iter()
                      .map(|u| u.to_string())
                      .collect(),
              ),
              "impersonate",
          ));
      }
  ```

  `crates/egret-policy-api/src/infra_bodies.rs`, after `delegator_impersonate_policy`:

  ```rust
  /// `egret_delegator` may impersonate every StarRocks user except `excluded`
  /// (package D, D1b): resource `user` with `isExcludes: true`, which the
  /// StarRocks service def allows (`excludesSupported`) and matches without
  /// regard to case (`ignoreCase`). Measured on the fork's plugin in package
  /// D's Task 1 (M1).
  pub fn starrocks_delegator_impersonate_policy(service: &str, excluded: &[String]) -> Value {
      json!({
          "name": IMPERSONATE_POLICY_NAME,
          "service": service,
          "serviceType": "starrocks",
          "policyType": 0,
          "isEnabled": true,
          "resources": {
              "user": { "values": excluded, "isExcludes": true, "isRecursive": false }
          },
          "policyItems": [{
              "users": [DELEGATOR_PRINCIPAL],
              "groups": [],
              "roles": [],
              "accesses": [{ "type": "impersonate", "isAllowed": true }],
              "delegateAdmin": false
          }],
          "denyPolicyItems": [],
          "allowExceptions": [],
          "denyExceptions": [],
          "isDenyAllElse": false,
          "policyPriority": 0
      })
  }
  ```

  `crates/egret-perm-compile/src/ranger/infra.rs`: import
  `egret_perm_ir::statement::{InfraKind, InfraObject, Source, Statement, Target}`, and replace the
  `InfraKind::DelegatorImpersonate => one(…)` arm with:

  ```rust
          InfraKind::DelegatorImpersonate => match (&s.target, engine) {
              (Target::Infra(InfraObject::AnyUser), EngineType::Trino) => one(
                  bodies::IMPERSONATE_POLICY_NAME,
                  bodies::delegator_impersonate_policy(service_name),
              ),
              (Target::Infra(InfraObject::AnyUserExcept(excluded)), EngineType::StarRocks)
                  if !excluded.is_empty() =>
              {
                  one(
                      bodies::IMPERSONATE_POLICY_NAME,
                      bodies::starrocks_delegator_impersonate_policy(service_name, excluded),
                  )
              }
              (target, engine) => Err(CompileError::Db(format!(
                  "ranger::infra::render: no delegator impersonation body for {target:?} on \
                   {engine:?} (an empty exclusion would exclude nothing)"
              ))),
          },
  ```

  (If `EngineType` has no `Debug`, use a `match engine { Trino => "trino", StarRocks => "starrocks" }`
  string in the message.)

  `crates/egret-perm-eval/src/decide.rs`, in the `OpClass::Impersonation` closure, after
  `InfraObject::AnyUser => true,`:

  ```rust
                      InfraObject::AnyUserExcept(excluded) => {
                          !excluded.iter().any(|e| eq_ci(e, target))
                      }
  ```

- [ ] **Step 4: Run the tests and see them pass.**

  ```bash
  CARGO_TARGET_DIR=$PWD/target-srd cargo test -p egret-core -p egret-perm-ir -p egret-policy-api --lib
  CARGO_TARGET_DIR=$PWD/target-srd cargo test -p egret-perm-compile --lib ranger::infra
  CARGO_TARGET_DIR=$PWD/target-srd cargo test -p egret-perm-eval --test decide
  env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
    CARGO_TARGET_DIR=$PWD/target-srd cargo test -p egret-engine-mgmt --lib delegation
  ```

  Expected: all pass. `trino_impersonate_emits_all_five_in_order` and
  `trino_renders_todays_bodies_byte_for_byte` pass unchanged (Trino is byte for byte the same).

- [ ] **Step 5: Mutants** (each must fail a test; restore after each and record it):
  - in `lower_infra`, drop `&& i.delegator_impersonates`: `starrocks_without_the_delegator…` fails;
  - in the body, `"isExcludes": false`: the body test fails;
  - in `decide`, the `AnyUserExcept` arm returns `true`: the decide test fails;
  - in `delegator_impersonates`, `"starrocks" => false`: the core test fails;
  - remove the `if !excluded.is_empty()` guard: `an_empty_exclusion_list_is_refused` fails.

- [ ] **Step 6: Commit.**

  ```bash
  git add crates/egret-core crates/egret-perm-ir crates/egret-policy-api crates/egret-perm-compile/src/ranger/infra.rs crates/egret-perm-eval crates/egret-engine-mgmt/src/engines.rs
  git commit -m "feat(perm-ir,policy): let the StarRocks delegator impersonate everyone but root and egret's users

  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
  ```

---

### Task 3: Convergence writes the row; re-bless and verify the goldens

**Files:**
- Test: `crates/egret-perm-compile/tests/infra_converge.rs`
- Modify: `crates/egret-perm-compile/tests/golden/*.json` (re-blessed only)

**Interfaces:**
- Consumes: Task 2 (`starrocks_delegator_impersonate_policy`, the widened `delegator_impersonates`).
- Produces: nothing new.

- [ ] **Step 1: Write the failing test and fix the one that now under-counts.**

  In `infra_converge.rs`, `a_starrocks_profile_gets_egret_admin_and_the_query_log` binds a StarRocks
  impersonate engine, so it now also gets the delegator row: after `want.extend(query_log_names(&name));`
  add `want.push(IMPERSONATE_POLICY_NAME.to_string());`, and change its message to "a StarRocks
  impersonate profile gets the StarRocks-def egret-admin rows, the query-log rows and the delegator's
  impersonation". Its loop asserting each `egret-admin` row's users is unchanged.

  Add:

  ```rust
  /// Package D, D1b: a StarRocks engine in impersonate mode brings the
  /// delegator's impersonation, StarRocks-shaped, excluding root, the
  /// delegator and the query-log loader; moving it to jwt takes only that row
  /// away.
  #[tokio::test]
  async fn a_starrocks_impersonate_engine_brings_the_delegator_row_and_jwt_takes_it_away() {
      let _serial = SERIAL.lock().await;
      let Some(db) = test_db().await else { return };
      one_admin(&db).await;
      let (svc, name) = profile(&db, "starrocks").await;
      let engine = bind_engine(
          &db,
          svc,
          "starrocks",
          json!({"starrocksConfig": {"delegation": {"mode": "impersonate"}}}),
      )
      .await;
      converge_infra_policy(&db.pool, svc).await.expect("converge");

      let before = infra_rows(&db, svc).await;
      let row = before
          .get(IMPERSONATE_POLICY_NAME)
          .expect("the delegator's impersonation row");
      assert_eq!(row.managed_by, "infra");
      let excluded: Vec<String> = vec![
          "egret_delegator".into(),
          "egret_query_log".into(),
          "root".into(),
      ];
      assert_eq!(
          row.body,
          egret_policy_api::infra_bodies::starrocks_delegator_impersonate_policy(&name, &excluded)
      );

      sqlx::query("UPDATE engine_mgmt.engines SET spec = $2 WHERE id = $1")
          .bind(engine)
          .bind(json!({"starrocksConfig": {"delegation": {"mode": "jwt"}}}))
          .execute(&db.pool)
          .await
          .expect("to jwt");
      let sync = converge_infra_policy(&db.pool, svc).await.expect("converge");

      assert_eq!(sync.removed, vec![IMPERSONATE_POLICY_NAME.to_string()]);
      let after = infra_rows(&db, svc).await;
      assert!(!after.contains_key(IMPERSONATE_POLICY_NAME));
      assert_eq!(after.len(), before.len() - 1, "only the delegator row went");
  }
  ```

  (If `egret-policy-api` is not a dev-dependency of `egret-perm-compile`'s tests, use the
  `egret_policy::infra_bodies` re-export if there is one, else add `egret-policy-api` to
  `[dev-dependencies]` with `.workspace = true`.)

- [ ] **Step 2: Run it.**

  ```bash
  env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
    CARGO_TARGET_DIR=$PWD/target-srd cargo test -p egret-perm-compile --test infra_converge
  ```

  Expected: both pass (Task 2 already did the work); the report gives the duration (a skip finishes
  in milliseconds). Then run the mutant: in `lower_infra`, change `EngineType::StarRocks` in the new
  condition to `EngineType::Trino`; the new test fails. Restore.

- [ ] **Step 3: The golden gate, bless.**

  ```bash
  env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
    EGRET_GOLDEN_BLESS=1 CARGO_TARGET_DIR=$PWD/target-srd \
    cargo test -p egret-perm-compile --features harness --test golden_compile
  git diff --stat crates/egret-perm-compile/tests/golden/
  ```

  Expected: only fixtures that bind a StarRocks engine in impersonate mode change (G22 at least).

- [ ] **Step 4: Verify the diff semantically.** Save as
  `/private/tmp/claude-501/-Users-1pro-l4r8-Documents-github-data-portal/739fff40-b4cd-44a8-8f4d-649cbeb36373/scratchpad/golden_diff.py`:

  ```python
  """Every golden change must be the new StarRocks delegator row and nothing else.

  Run from the worktree root: python3 -I <this file>
  """
  import json
  import pathlib
  import subprocess
  import sys

  NAME = "egret-delegator-impersonate"
  root = pathlib.Path("crates/egret-perm-compile/tests/golden")
  bad = []
  for path in sorted(root.glob("*.json")):
      old_text = subprocess.run(["git", "show", f"HEAD:{path}"], capture_output=True, text=True)
      if old_text.returncode != 0:
          bad.append(f"{path}: not in HEAD")
          continue
      old, new = json.loads(old_text.stdout), json.loads(path.read_text())
      if old == new:
          continue

      def strip(doc, drop_new):
          out = []
          for entry in doc:
              entry = dict(entry)
              for key in ("desired", "served"):
                  rows = []
                  for row in entry.get(key, []):
                      body = row.get("body", row)
                      is_new = body.get("name") == NAME and body.get("serviceType") == "starrocks"
                      if drop_new and is_new:
                          continue
                      row = dict(row)
                      row.pop("id", None)
                      rows.append(row)
                  if key in entry:
                      entry[key] = rows
              out.append(entry)
          return out

      added = sum(
          1
          for entry in new
          for key in ("desired", "served")
          for row in entry.get(key, [])
          if row.get("body", row).get("name") == NAME
          and row.get("body", row).get("serviceType") == "starrocks"
      )
      if strip(new, True) != strip(old, False):
          bad.append(f"{path}: changed beyond the new row")
      elif added == 0:
          bad.append(f"{path}: changed but has no new row")
      else:
          print(f"{path}: +{added} StarRocks delegator row(s), nothing else")
  if bad:
      print("\n".join(bad))
      sys.exit(1)
  ```

  ```bash
  python3 -I /private/tmp/claude-501/-Users-1pro-l4r8-Documents-github-data-portal/739fff40-b4cd-44a8-8f4d-649cbeb36373/scratchpad/golden_diff.py
  ```

  Expected: exit 0, one line per changed fixture. Also read one added row in full and check its
  `resources.user` is `{"values": ["egret_delegator", "egret_query_log", "root"], "isExcludes": true, "isRecursive": false}`
  and its item allows `impersonate` to `egret_delegator` only.

- [ ] **Step 5: The golden gate, unblessed and on OPA.**

  ```bash
  env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
    CARGO_TARGET_DIR=$PWD/target-srd cargo test -p egret-perm-compile --features harness --test golden_compile
  env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
    EGRET_GOLDEN_MECHANISM=opa CARGO_TARGET_DIR=$PWD/target-srd \
    cargo test -p egret-perm-compile --features harness --test golden_compile --no-fail-fast
  ```

  Expected: both pass.

- [ ] **Step 6: Commit.**

  ```bash
  git add crates/egret-perm-compile/tests
  git commit -m "test(perm-compile): converge the StarRocks delegator row, bless the goldens with it

  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
  ```

---

### Task 4: `MYSQL_PWD` for the FE and the CN (D2)

**Files:**
- Modify: `crates/egret-provisioner/src/starrocks.rs` (`starrocks_cluster_cr`, two new private
  helpers, tests)

**Interfaces:**
- Consumes: `root_password(spec)` (existing, `starrocks.rs:306`).
- Produces (private): `const MYSQL_PWD_ENV: &str = "MYSQL_PWD"; fn mysql_pwd_env(spec: &EngineSpec) -> Option<Value>; fn add_env_unless_named(role: &mut Value, env_key: &str, entry: Value)`.

- [ ] **Step 1: Write the failing tests** in `starrocks.rs`'s `mod tests` (`StarRocksPod` derives
  `Default`):

  ```rust
  fn mysql_pwd_entry() -> serde_json::Value {
      serde_json::json!({
          "name": "MYSQL_PWD",
          "valueFrom": { "secretKeyRef": { "name": "sr1-initpwd-secret", "key": "password" } }
      })
  }

  /// Package D, D2 (KI-335): the CN entrypoint (and an FE follower's) logs in
  /// as root with `mysql`, which reads the password only from `MYSQL_PWD`.
  #[test]
  fn a_passworded_engine_hands_mysql_pwd_to_the_fe_and_the_cn() {
      let cr = starrocks_cluster_cr(&spec_with_root_password());
      assert_eq!(
          cr["spec"]["starRocksFeSpec"]["feEnvVars"],
          serde_json::json!([mysql_pwd_entry()])
      );
      assert_eq!(
          cr["spec"]["starRocksCnSpec"]["cnEnvVars"],
          serde_json::json!([mysql_pwd_entry()])
      );
  }

  #[test]
  fn a_passwordless_engine_renders_no_env() {
      let cr = starrocks_cluster_cr(&sample_sr_spec());
      assert!(cr["spec"]["starRocksFeSpec"].get("feEnvVars").is_none());
      assert!(cr["spec"]["starRocksCnSpec"].get("cnEnvVars").is_none());
  }

  #[test]
  fn admin_env_is_kept_and_mysql_pwd_appended() {
      let mut spec = spec_with_root_password();
      spec.starrocks.as_mut().unwrap().pod = Some(StarRocksPod {
          env: vec![serde_json::json!({"name": "TZ", "value": "UTC"})],
          ..Default::default()
      });
      let cr = starrocks_cluster_cr(&spec);
      let want = serde_json::json!([{"name": "TZ", "value": "UTC"}, mysql_pwd_entry()]);
      assert_eq!(cr["spec"]["starRocksFeSpec"]["feEnvVars"], want);
      assert_eq!(cr["spec"]["starRocksCnSpec"]["cnEnvVars"], want);
  }

  #[test]
  fn an_admin_set_mysql_pwd_is_not_overridden() {
      let mut spec = spec_with_root_password();
      let admin = serde_json::json!({"name": "MYSQL_PWD", "value": "admin-chose-this"});
      spec.starrocks.as_mut().unwrap().pod = Some(StarRocksPod {
          env: vec![admin.clone()],
          ..Default::default()
      });
      let cr = starrocks_cluster_cr(&spec);
      assert_eq!(cr["spec"]["starRocksFeSpec"]["feEnvVars"], serde_json::json!([admin.clone()]));
      assert_eq!(cr["spec"]["starRocksCnSpec"]["cnEnvVars"], serde_json::json!([admin]));
  }
  ```

- [ ] **Step 2: Run them; the passworded and appended tests fail.**

  ```bash
  CARGO_TARGET_DIR=$PWD/target-srd cargo test -p egret-provisioner --lib mysql_pwd
  CARGO_TARGET_DIR=$PWD/target-srd cargo test -p egret-provisioner --lib admin_env
  ```

- [ ] **Step 3: Implement.** Above `starrocks_cluster_cr`:

  ```rust
  /// The env var the `mysql` client in the FE and CN entrypoints reads root's
  /// password from (`cn_entrypoint.sh`'s `ALTER SYSTEM ADD COMPUTE NODE`, an FE
  /// follower's registration, the operator's CN scale-in).
  const MYSQL_PWD_ENV: &str = "MYSQL_PWD";

  /// `MYSQL_PWD` from `{name}-initpwd-secret`, as the kube-starrocks helm chart
  /// injects it into every role when `initPassword` is on. `None` for a
  /// passwordless engine. A reference, so no manifest carries the password.
  fn mysql_pwd_env(spec: &EngineSpec) -> Option<Value> {
      root_password(spec)?;
      Some(json!({
          "name": MYSQL_PWD_ENV,
          "valueFrom": { "secretKeyRef": {
              "name": format!("{}-initpwd-secret", spec.name),
              "key": "password",
          } },
      }))
  }

  /// Append `entry` to the role's `env_key` array unless an entry with the same
  /// `name` is already there: an admin's own value wins, as in the helm chart.
  fn add_env_unless_named(role: &mut Value, env_key: &str, entry: Value) {
      let obj = role.as_object_mut().expect("role spec is a JSON object");
      let env = obj.entry(env_key).or_insert_with(|| json!([]));
      let Some(list) = env.as_array_mut() else {
          tracing::warn!(field = env_key, "role env is not an array; MYSQL_PWD not added");
          return;
      };
      if !list.iter().any(|e| e["name"] == entry["name"]) {
          list.push(entry);
      }
  }
  ```

  In `starrocks_cluster_cr`, immediately after the `if let Some(pod) = &sr.pod { … }` block:

  ```rust
      // D2 (package D, KI-335): after `apply_pod_fields`, which writes the admin
      // env over the whole array, so the admin's entries stay and come first.
      if let Some(entry) = mysql_pwd_env(spec) {
          add_env_unless_named(&mut fe_spec, "feEnvVars", entry.clone());
          add_env_unless_named(&mut cn_spec, "cnEnvVars", entry);
      }
  ```

- [ ] **Step 4: Run the provisioner suite.**

  ```bash
  CARGO_TARGET_DIR=$PWD/target-srd cargo test -p egret-provisioner
  ```

  Expected: all pass, including the existing byte-for-byte tests of unmarked and passwordless
  engines.

- [ ] **Step 5: Mutants:** drop the CN call (the first test fails); drop the `any(...)` check (the
  admin test fails); move the block before `apply_pod_fields` (the appended test fails).

- [ ] **Step 6: Commit.**

  ```bash
  git add crates/egret-provisioner/src/starrocks.rs
  git commit -m "fix(provisioner): hand root's password to the StarRocks FE and CN as MYSQL_PWD

  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
  ```

---

### Task 5: Engine delete removes the Job's pods and the FE metadata (D3)

**Files:**
- Modify: `crates/egret-provisioner/src/starrocks.rs` (`starrocks_cluster_cr`,
  `StarRocksProvisioner::delete` at about line 1239, two helpers, tests)

**Interfaces:**
- Consumes: `KubeClient::{delete_background, get_object, delete}`,
  `crate::rollout::delete_with_ranger_secrets`, `FE_META_VOLUME_NAME` (`"fe-meta"`).
- Produces (private): `const FE_META_CLAIM_PROBE_LIMIT: u32 = 16; fn fe_meta_claim_name(engine: &str, ordinal: u32) -> String; async fn delete_fe_meta_claims(kube: &dyn KubeClient, namespace: &str, engine: &str) -> anyhow::Result<()>`.

- [ ] **Step 1: Write the failing tests** in `mod tests`:

  ```rust
  /// Package D, D3: the operator copies this onto the FE StatefulSet, so its
  /// metadata claims go with the engine.
  #[test]
  fn a_meta_persistent_fe_deletes_its_claims_with_the_statefulset() {
      let cr = starrocks_cluster_cr(&sample_sr_spec());
      assert_eq!(
          cr["spec"]["starRocksFeSpec"]["persistentVolumeClaimRetentionPolicy"],
          serde_json::json!({"whenDeleted": "Delete"})
      );
      assert!(cr["spec"]["starRocksCnSpec"]
          .get("persistentVolumeClaimRetentionPolicy")
          .is_none());
  }

  #[test]
  fn an_unmarked_fe_renders_no_retention_policy() {
      let cr = starrocks_cluster_cr(&sample_sr_spec_unmarked());
      assert!(cr["spec"]["starRocksFeSpec"]
          .get("persistentVolumeClaimRetentionPolicy")
          .is_none());
  }

  #[tokio::test]
  async fn delete_takes_the_initpwd_jobs_pods_with_it() {
      use crate::kube_client::FakeKube;
      let kube = Arc::new(FakeKube::default());
      StarRocksProvisioner::new(kube.clone())
          .delete("egret-engines", "sr1")
          .await
          .unwrap();
      assert_eq!(
          *kube.deleted_background.lock().unwrap(),
          vec!["Job/sr1-initpwd".to_string()],
          "Background propagation: a Job's default is orphan, which leaves its pods"
      );
  }

  fn live_claim(kube: &crate::kube_client::FakeKube, name: &str) {
      kube.set_live(serde_json::json!({"kind": "PersistentVolumeClaim", "metadata": {"name": name}}));
  }

  #[tokio::test]
  async fn delete_removes_every_fe_meta_claim_and_probes_no_further() {
      use crate::kube_client::FakeKube;
      let kube = Arc::new(FakeKube::default());
      for i in 0..3 {
          live_claim(&kube, &format!("fe-meta-sr1-fe-{i}"));
      }
      live_claim(&kube, "fe-meta-sr1-fe-5"); // past a gap: not ours to guess at
      StarRocksProvisioner::new(kube.clone())
          .delete("egret-engines", "sr1")
          .await
          .unwrap();
      let deleted = kube.deleted.lock().unwrap();
      for i in 0..3 {
          assert!(
              deleted.contains(&format!("PersistentVolumeClaim/fe-meta-sr1-fe-{i}")),
              "{deleted:?}"
          );
      }
      assert!(!deleted.iter().any(|d| d.ends_with("fe-meta-sr1-fe-5")), "{deleted:?}");
  }

  #[tokio::test]
  async fn delete_with_no_claims_deletes_none() {
      use crate::kube_client::FakeKube;
      let kube = Arc::new(FakeKube::default());
      StarRocksProvisioner::new(kube.clone())
          .delete("egret-engines", "sr1")
          .await
          .unwrap();
      assert!(!kube
          .deleted
          .lock()
          .unwrap()
          .iter()
          .any(|d| d.starts_with("PersistentVolumeClaim/")));
  }

  #[tokio::test]
  async fn a_claim_that_cannot_be_deleted_is_reported_after_the_rest() {
      use crate::kube_client::FakeKube;
      let kube = Arc::new(FakeKube::default());
      live_claim(&kube, "fe-meta-sr1-fe-0");
      kube.fail_delete("PersistentVolumeClaim", "fe-meta-sr1-fe-0", "forbidden");
      let err = StarRocksProvisioner::new(kube.clone())
          .delete("egret-engines", "sr1")
          .await
          .expect_err("a failed claim delete must surface");
      assert!(
          format!("{err:#}").contains("PersistentVolumeClaim/fe-meta-sr1-fe-0: forbidden"),
          "{err:#}"
      );
      let deleted = kube.deleted.lock().unwrap();
      assert!(deleted.contains(&"StarRocksCluster/sr1".to_string()));
      assert!(deleted.contains(&"Job/sr1-initpwd".to_string()));
  }
  ```

  Also, in the existing `provisioner_delete_cleans_up_initpwd`, add after the `Job/sr1-initpwd`
  assertion:

  ```rust
  assert_eq!(
      *kube.deleted_background.lock().unwrap(),
      vec!["Job/sr1-initpwd".to_string()]
  );
  ```

- [ ] **Step 2: Run them; the retention, Job and claim tests fail.**

  ```bash
  CARGO_TARGET_DIR=$PWD/target-srd cargo test -p egret-provisioner --lib delete
  CARGO_TARGET_DIR=$PWD/target-srd cargo test -p egret-provisioner --lib retention
  ```

- [ ] **Step 3: Implement.**

  In `starrocks_cluster_cr`, inside the existing `if sr.fe_meta_persistent { … }` block, after
  `storage_volumes.push(fe_meta);`:

  ```rust
          // D3 (package D): the claim goes when the FE StatefulSet goes. A
          // mutable StatefulSet field, so an existing engine takes it on its
          // next apply without the `volumeClaimTemplates` trap above, and it is
          // not part of the pod template (no roll).
          fe_spec["persistentVolumeClaimRetentionPolicy"] = json!({ "whenDeleted": "Delete" });
  ```

  Beside `FE_META_VOLUME_NAME`:

  ```rust
  /// How many FE ordinals [`delete_fe_meta_claims`] probes at most. Ordinals are
  /// created from 0 and kept from 0 to the highest ever, so the probe stops at
  /// the first one that does not exist long before this.
  const FE_META_CLAIM_PROBE_LIMIT: u32 = 16;

  /// The claim the StatefulSet makes for FE `ordinal`:
  /// `<volumeClaimTemplate>-<statefulset>-<ordinal>`, the StatefulSet being
  /// `<engine>-fe`.
  fn fe_meta_claim_name(engine: &str, ordinal: u32) -> String {
      format!("{FE_META_VOLUME_NAME}-{engine}-fe-{ordinal}")
  }

  /// Delete the engine's FE metadata claims (D3, for engines never re-applied
  /// since the retention policy): probe ordinals from 0, delete each claim that
  /// exists, stop at the first that does not. Every failure is collected.
  async fn delete_fe_meta_claims(
      kube: &dyn KubeClient,
      namespace: &str,
      engine: &str,
  ) -> anyhow::Result<()> {
      let mut failures = Vec::new();
      for ordinal in 0..FE_META_CLAIM_PROBE_LIMIT {
          let claim = fe_meta_claim_name(engine, ordinal);
          match kube
              .get_object("v1", "PersistentVolumeClaim", namespace, &claim)
              .await
          {
              Ok(None) => break,
              Ok(Some(_)) => {
                  if let Err(e) = kube
                      .delete("v1", "PersistentVolumeClaim", namespace, &claim)
                      .await
                  {
                      failures.push(format!("PersistentVolumeClaim/{claim}: {e:#}"));
                  }
              }
              Err(e) => {
                  failures.push(format!("PersistentVolumeClaim/{claim}: read: {e:#}"));
                  break;
              }
          }
      }
      if failures.is_empty() {
          Ok(())
      } else {
          anyhow::bail!("failed to delete {}", failures.join("; "))
      }
  }
  ```

  Replace `StarRocksProvisioner::delete`'s body:

  ```rust
      async fn delete(&self, namespace: &str, name: &str) -> anyhow::Result<()> {
          // The CR first (the operator tears down FE/CN), then the objects egret
          // applies beside it. A 404 is success; every step is attempted and
          // every failure is returned together.
          let objects = [
              ("starrocks.com/v1", "StarRocksCluster", name.to_string()),
              ("v1", "ConfigMap", format!("{name}-fe-cm")),
              ("v1", "ConfigMap", format!("{name}-cn-cm")),
              ("v1", "ConfigMap", format!("{name}-ranger-cm")),
              ("v1", "Secret", format!("{name}-ranger-secret")),
              ("v1", "ConfigMap", format!("{name}-ssl-cm")),
              ("v1", "Secret", format!("{name}-ssl-secret")),
              ("v1", "ConfigMap", format!("{name}-initpwd-shell")),
              ("v1", "Secret", format!("{name}-initpwd-secret")),
              ("networking.k8s.io/v1", "Ingress", format!("{name}-fe")),
              ("v1", "Secret", format!("{name}-ingress-tls")),
          ];
          let mut failures = Vec::new();
          if let Err(e) =
              crate::rollout::delete_with_ranger_secrets(&*self.kube, namespace, name, &objects).await
          {
              failures.push(format!("{e:#}"));
          }
          // D3: Background, or the Job's pods are orphaned (a Job's default).
          let job = format!("{name}-initpwd");
          if let Err(e) = self
              .kube
              .delete_background("batch/v1", "Job", namespace, &job)
              .await
          {
              failures.push(format!("Job/{job}: {e:#}"));
          }
          // D3: the FE metadata, for engines never re-applied since the
          // retention policy. A deleted engine's FE metadata is gone; its table
          // data stays in object storage.
          if let Err(e) = delete_fe_meta_claims(&*self.kube, namespace, name).await {
              failures.push(format!("{e:#}"));
          }
          if failures.is_empty() {
              Ok(())
          } else {
              anyhow::bail!("{}", failures.join("; "))
          }
      }
  ```

- [ ] **Step 4: Run the provisioner suite.** `CARGO_TARGET_DIR=$PWD/target-srd cargo test -p egret-provisioner`.
  Expected: all pass, `provisioner_delete_surfaces_a_failed_cluster_delete` included (still 11
  recorded deletes: ten objects plus the Job through `delete_background`).

- [ ] **Step 5: Mutants:** put the Job back in `objects` and drop the `delete_background` call (the
  Job test fails); change `Ok(None) => break` to `continue` (the probe test fails: `-5` deleted);
  drop the retention line (its test fails).

- [ ] **Step 6: Commit.**

  ```bash
  git add crates/egret-provisioner/src/starrocks.rs
  git commit -m "fix(provisioner): delete a StarRocks engine's initpwd Job pods and FE metadata claims

  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
  ```

---

### Task 6: `POST /engines` refuses a leftover FE metadata claim (decided 2026-11-11, 4)

**Files:**
- Modify: `crates/egret-engine-mgmt/src/http_error.rs` (variant, response, message helper)
- Modify: `crates/egret-engine-mgmt/src/engines.rs` (`refuse_leftover_fe_metadata`, its call in
  `create_starrocks_engine` right after `check_starrocks_engine_name(&b.name)?;`, the create route's
  409 description, tests)
- Modify: `openapi.json`, and `frontend/src/api/generated/**` only through `npm run gen:api`

**Interfaces:**
- Consumes: `CpState::kube` (`Option<Arc<dyn egret_provisioner::KubeClient>>`), `DEFAULT_NAMESPACE`.
- Produces:

  ```rust
  // http_error.rs
  EngineHttpError::LeftoverFeMetadata { name: String, claim: String, namespace: String } // 409
  pub(crate) fn leftover_fe_metadata_message(name: &str, claim: &str, namespace: &str) -> String;
  // engines.rs
  async fn refuse_leftover_fe_metadata(st: &CpState, name: &str) -> Result<(), CpError>;
  ```

- [ ] **Step 1: Write the failing tests** in `engines.rs`'s `mod tests`, beside
  `create_starrocks_engine_mints_root_password`:

  ```rust
  /// Package D, decided on 2026-11-11 (4): a claim a previous engine of this
  /// name left would be rebound by the new FE, with the old root password.
  #[tokio::test]
  async fn a_starrocks_create_refuses_a_leftover_metadata_claim() {
      let Some(mut st) = test_state().await else {
          eprintln!("skipping a_starrocks_create_refuses_a_leftover_metadata_claim: DATABASE_URL not set");
          return;
      };
      st.engine_provisioner = Some(std::sync::Arc::new(NoopProvisioner));
      let kube = std::sync::Arc::new(egret_provisioner::kube_client::FakeKube::default());
      st.kube = Some(kube.clone() as std::sync::Arc<dyn egret_provisioner::KubeClient>);
      let dsn = egret_test_db::url().await.unwrap();
      let db = egret_metastore::Db::connect(&dsn, 5).await.expect("connect");
      let (wh, _secret) = seed_sr_warehouse(&st, &db).await;
      let mut body = sr_update_body(None);
      body.name = format!("sr-{}", Uuid::new_v4().simple());
      let name = body.name.clone();
      let claim = format!("fe-meta-{name}-fe-0");
      kube.set_live(serde_json::json!({"kind": "PersistentVolumeClaim", "metadata": {"name": claim}}));

      let err = create_starrocks_engine(st.clone(), body, wh, Vec::new())
          .await
          .expect_err("the create must be refused");

      assert!(
          matches!(&err, CpError::LeftoverFeMetadata { claim: c, .. } if c == &claim),
          "{err:?}"
      );
      assert!(
          st.engine_mgmt.list_engines().await.unwrap().iter().all(|e| e.name != name),
          "no engine row may exist"
      );
      let resp = axum::response::IntoResponse::into_response(err);
      assert_eq!(resp.status(), StatusCode::CONFLICT);
      let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
      let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
      assert_eq!(v["code"], "engine_metadata_left_over");
      assert_eq!(v["claim"], claim);
      assert!(
          v["error"]
              .as_str()
              .unwrap()
              .contains(&format!("kubectl delete pvc {claim} -n egret-engines")),
          "{v}"
      );
  }

  #[tokio::test]
  async fn a_starrocks_create_without_a_leftover_claim_proceeds() {
      let Some(mut st) = test_state().await else {
          eprintln!("skipping a_starrocks_create_without_a_leftover_claim_proceeds: DATABASE_URL not set");
          return;
      };
      st.engine_provisioner = Some(std::sync::Arc::new(NoopProvisioner));
      let kube = std::sync::Arc::new(egret_provisioner::kube_client::FakeKube::default());
      st.kube = Some(kube as std::sync::Arc<dyn egret_provisioner::KubeClient>);
      let dsn = egret_test_db::url().await.unwrap();
      let db = egret_metastore::Db::connect(&dsn, 5).await.expect("connect");
      let (wh, _secret) = seed_sr_warehouse(&st, &db).await;
      let mut body = sr_update_body(None);
      body.name = format!("sr-{}", Uuid::new_v4().simple());

      let (code, _dto) = create_starrocks_engine(st.clone(), body, wh, Vec::new())
          .await
          .expect("create");

      assert_eq!(code, StatusCode::CREATED);
  }
  ```

  (If `axum::body::to_bytes` is not how this crate's other tests read a response body, use the
  helper they use.)

- [ ] **Step 2: Run them; the first fails** (the variant does not exist yet: a compile failure).

  ```bash
  env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
    CARGO_TARGET_DIR=$PWD/target-srd cargo test -p egret-engine-mgmt --lib leftover
  ```

- [ ] **Step 3: Implement.** `http_error.rs`, in `EngineHttpError` before `Internal`:

  ```rust
      /// A previous StarRocks engine of this name left its FE metadata claim,
      /// which a new FE of the same name would rebind, with the old users,
      /// catalogs and root password (package D, decided on 2026-11-11, 4).
      /// 409, `code: "engine_metadata_left_over"`.
      #[error("a previous engine left its front-end metadata volume {claim}")]
      LeftoverFeMetadata {
          name: String,
          claim: String,
          namespace: String,
      },
  ```

  In `IntoResponse`, beside `EngineIsExternal`:

  ```rust
              EngineHttpError::LeftoverFeMetadata {
                  name,
                  claim,
                  namespace,
              } => (
                  StatusCode::CONFLICT,
                  Json(serde_json::json!({
                      "error": leftover_fe_metadata_message(&name, &claim, &namespace),
                      "code": "engine_metadata_left_over",
                      "claim": claim,
                  })),
              )
                  .into_response(),
  ```

  At module level:

  ```rust
  /// The 409's message: what is left, why it matters, and the command that
  /// removes it.
  pub(crate) fn leftover_fe_metadata_message(name: &str, claim: &str, namespace: &str) -> String {
      format!(
          "A previous StarRocks engine named \"{name}\" left its front-end metadata volume \
           {claim}. A new engine would start with the old engine's users, catalogs and root \
           password. Delete it first: kubectl delete pvc {claim} -n {namespace}"
      )
  }
  ```

  `engines.rs`, beside `check_starrocks_engine_name`:

  ```rust
  /// Package D, decided on 2026-11-11 (4): refuse a StarRocks create whose
  /// name left an FE metadata claim. Without a Kubernetes client (the Docker
  /// provisioner) there is no claim to find. A read failure refuses too: the
  /// create does not guess.
  async fn refuse_leftover_fe_metadata(st: &CpState, name: &str) -> Result<(), CpError> {
      let Some(kube) = st.kube.as_ref() else {
          return Ok(());
      };
      let claim = format!("fe-meta-{name}-fe-0");
      match kube
          .get_object("v1", "PersistentVolumeClaim", DEFAULT_NAMESPACE, &claim)
          .await
      {
          Ok(None) => Ok(()),
          Ok(Some(_)) => Err(CpError::LeftoverFeMetadata {
              name: name.to_string(),
              claim,
              namespace: DEFAULT_NAMESPACE.to_string(),
          }),
          Err(e) => Err(CpError::ClusterApplyFailed(format!(
              "could not check for a leftover front-end metadata volume {claim}: {e:#}"
          ))),
      }
  }
  ```

  In `create_starrocks_engine`, after `check_starrocks_engine_name(&b.name)?;`:
  `refuse_leftover_fe_metadata(&st, &b.name).await?;`. In the create route's `#[utoipa::path]`,
  change the 409 description to "An engine with this name already exists, its same-name policy
  profile is bound to another engine, or (StarRocks, `code` = `engine_metadata_left_over`) a
  previous engine of this name left its front-end metadata volume; the message carries the
  `kubectl` command that removes it". Regenerate `openapi.json` and the client (Global Constraints).

- [ ] **Step 4: Run them; both pass.** Same command as Step 2, plus
  `CARGO_TARGET_DIR=$PWD/target-srd cargo test -p egret-engine-mgmt --lib create_starrocks`.

- [ ] **Step 5: Mutants:** drop the call in `create_starrocks_engine` (the refusal test fails);
  answer `Ok(None)` with the refusal too (the proceed test fails); drop `-n {namespace}` from the
  message (the refusal test fails).

- [ ] **Step 6: Commit.**

  ```bash
  git add crates/egret-engine-mgmt openapi.json frontend/src/api/generated
  git commit -m "feat(engine-mgmt): refuse a StarRocks create whose name left a front-end metadata volume

  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
  ```

---

### Task 7: Migration 0219 and the store

**Files:**
- Create: `migrations/0219_starrocks_users.sql`
- Modify: `crates/egret-engine-mgmt/src/store.rs` (five methods, `UserSyncLock`)
- Test: `crates/egret-auth/tests/username_ledger.rs` (the `root` reservation)
- Create: `crates/egret-engine-mgmt/tests/starrocks_user_sync_triggers.rs`

**Interfaces:**
- Consumes: nothing new.
- Produces:

  ```rust
  // egret_engine_mgmt::store (crate-visible)
  pub(crate) struct UserSyncLock { /* conn, key */ }
  impl UserSyncLock { pub(crate) async fn release(self); }
  impl EngineMgmtStore {
      pub(crate) async fn starrocks_user_candidates(&self, engine_id: Uuid)
          -> Result<Vec<(Uuid, Option<String>)>, EngineMgmtError>;
      pub(crate) async fn recorded_starrocks_users(&self, engine_id: Uuid)
          -> Result<std::collections::BTreeSet<String>, EngineMgmtError>;
      pub(crate) async fn record_starrocks_user(&self, engine_id: Uuid, username: &str)
          -> Result<bool, EngineMgmtError>;  // true: this call inserted it
      pub(crate) async fn forget_starrocks_user(&self, engine_id: Uuid, username: &str)
          -> Result<(), EngineMgmtError>;
      pub(crate) async fn try_lock_user_sync(&self, engine_id: Uuid)
          -> Result<Option<UserSyncLock>, EngineMgmtError>;
  }
  ```

  The channel name `egret_starrocks_user_sync` is fixed here and becomes
  `starrocks_users::USER_SYNC_CHANNEL` in Task 8.

- [ ] **Step 1: Write the migration.** Check the number first (Global Constraints).

  ```sql
  -- Package D (docs/en/specs/2026-11-11-starrocks-delegation-and-lifecycle-design.md, section 2):
  -- egret creates a StarRocks user for each person who may query an impersonate-mode StarRocks
  -- engine, and drops it when they no longer may.

  -- 1. The StarRocks users egret created, per engine. egret drops only these; a StarRocks user it
  --    did not create is never touched. The rows go with the engine.
  CREATE TABLE engine_mgmt.starrocks_users (
      engine_id  uuid NOT NULL REFERENCES engine_mgmt.engines(id) ON DELETE CASCADE,
      username   text NOT NULL,
      created_at timestamptz NOT NULL DEFAULT now(),
      PRIMARY KEY (engine_id, username)
  );

  -- 2. `root` is StarRocks' superuser: no person may be minted that name
  --    (`egret_core::engine_principals::STARROCKS_ROOT_USER`), as 0209 reserved `egret_delegator`
  --    and 0218 `egret_query_log`. A person who already holds it keeps it; the sync refuses it.
  INSERT INTO username_ledger (username, source, retired_at)
  VALUES ('root', 'reserved', now())
  ON CONFLICT DO NOTHING;

  -- 3. Wake the sync (`egret_engine_mgmt::starrocks_users`, channel `egret_starrocks_user_sync`)
  --    after every committed write that can change who may query an engine. Triggers, not Rust
  --    calls, for the reason 0209 gives for the ledger: they cover every writer. Postgres delivers a
  --    NOTIFY only at commit, once per payload per transaction.
  CREATE FUNCTION engine_mgmt.notify_starrocks_user_sync() RETURNS trigger
  LANGUAGE plpgsql AS $$
  BEGIN
      PERFORM pg_notify('egret_starrocks_user_sync', '*');
      RETURN NULL;
  END
  $$;

  CREATE TRIGGER starrocks_user_sync_access_grants
      AFTER INSERT OR UPDATE OR DELETE ON access_grants
      FOR EACH STATEMENT EXECUTE FUNCTION engine_mgmt.notify_starrocks_user_sync();
  CREATE TRIGGER starrocks_user_sync_user_groups
      AFTER INSERT OR UPDATE OR DELETE ON user_groups
      FOR EACH STATEMENT EXECUTE FUNCTION engine_mgmt.notify_starrocks_user_sync();
  CREATE TRIGGER starrocks_user_sync_users
      AFTER UPDATE OF status, username OR DELETE ON users
      FOR EACH STATEMENT EXECUTE FUNCTION engine_mgmt.notify_starrocks_user_sync();
  CREATE TRIGGER starrocks_user_sync_engine_workspaces
      AFTER INSERT OR DELETE ON engine_mgmt.engine_workspaces
      FOR EACH STATEMENT EXECUTE FUNCTION engine_mgmt.notify_starrocks_user_sync();
  -- Platform admins are synced too (decided on 2026-11-11, 1): their grants are `access_grants`
  -- rows (scope 'platform', above); the role that makes them admins is the `roles` row named
  -- `platform_admin`, so a rename or delete of a role wakes the sync as well.
  CREATE TRIGGER starrocks_user_sync_roles
      AFTER UPDATE OF name OR DELETE ON roles
      FOR EACH STATEMENT EXECUTE FUNCTION engine_mgmt.notify_starrocks_user_sync();
  ```

- [ ] **Step 2: Write the failing tests.**

  `crates/egret-auth/tests/username_ledger.rs`, after `the_query_log_loader_name_is_reserved`:

  ```rust
  /// Package D: `root` is StarRocks' superuser.
  #[tokio::test]
  async fn the_starrocks_root_name_is_reserved() {
      let Some(db) = connect().await else { return };
      let held: i64 =
          sqlx::query_scalar("SELECT count(*) FROM username_ledger WHERE username = 'root'")
              .fetch_one(&db.pool)
              .await
              .unwrap();
      assert_eq!(held, 1);
      let got = mint(&db, &format!("root@{}.test", t())).await;
      assert_ne!(got, "root");
      assert!(got.starts_with("root-"), "{got}");
  }
  ```

  `crates/egret-engine-mgmt/tests/starrocks_user_sync_triggers.rs`:

  ```rust
  //! Package D, migration 0219: every write that can change who may query an
  //! engine notifies `egret_starrocks_user_sync` at commit, and a rolled-back
  //! one does not. DB-gated (`DATABASE_URL`, through `egret-test-db`).
  //!
  //!   env -u EGRET_ALLOW_DB_SKIP \
  //!     DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  //!     cargo test -p egret-engine-mgmt --test starrocks_user_sync_triggers

  use std::time::Duration;

  use egret_metastore::Db;
  use sqlx::postgres::PgListener;
  use uuid::Uuid;

  const CHANNEL: &str = "egret_starrocks_user_sync";

  async fn db() -> Option<Db> {
      let dsn = egret_test_db::url().await?;
      let db = Db::connect(&dsn, 5).await.expect("connect");
      db.migrate().await.expect("migrate");
      Some(db)
  }

  async fn listening(db: &Db) -> PgListener {
      let mut l = PgListener::connect_with(&db.pool).await.expect("listener");
      l.listen(CHANNEL).await.expect("listen");
      l
  }

  /// Whether a notification arrives within one second.
  async fn notified(l: &mut PgListener) -> bool {
      matches!(
          tokio::time::timeout(Duration::from_secs(1), l.recv()).await,
          Ok(Ok(n)) if n.channel() == CHANNEL
      )
  }

  async fn new_user(db: &Db) -> Uuid {
      sqlx::query_scalar(
          "INSERT INTO users (provider, email) VALUES ('local', $1) RETURNING id",
      )
      .bind(format!("sync-{}@trigger.test", Uuid::new_v4().simple()))
      .fetch_one(&db.pool)
      .await
      .expect("user")
  }

  async fn new_workspace(db: &Db) -> Uuid {
      let slug = format!("sync-{}", Uuid::new_v4().simple());
      sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1, $1) RETURNING id")
          .bind(slug)
          .fetch_one(&db.pool)
          .await
          .expect("workspace")
  }

  const GRANT_VIEWER: &str = "INSERT INTO access_grants \
      (principal_type, principal_id, role_id, scope_type, scope_id) \
      SELECT 'user', $1, id, 'workspace', $2 FROM roles \
       WHERE scope = 'workspace' AND name = 'ws_viewer'";

  #[tokio::test]
  async fn a_membership_write_notifies_at_commit() {
      let Some(db) = db().await else { return };
      let (user, ws) = (new_user(&db).await, new_workspace(&db).await);
      let mut l = listening(&db).await;
      sqlx::query(GRANT_VIEWER).bind(user).bind(ws).execute(&db.pool).await.unwrap();
      assert!(notified(&mut l).await, "a workspace grant must wake the sync");
  }

  #[tokio::test]
  async fn a_rolled_back_membership_write_does_not_notify() {
      let Some(db) = db().await else { return };
      let (user, ws) = (new_user(&db).await, new_workspace(&db).await);
      let mut l = listening(&db).await;
      let mut tx = db.pool.begin().await.unwrap();
      sqlx::query(GRANT_VIEWER).bind(user).bind(ws).execute(&mut *tx).await.unwrap();
      tx.rollback().await.unwrap();
      assert!(!notified(&mut l).await, "a rollback must notify nothing");
  }

  #[tokio::test]
  async fn disabling_a_user_and_a_group_membership_notify() {
      let Some(db) = db().await else { return };
      let user = new_user(&db).await;
      let mut l = listening(&db).await;
      sqlx::query("UPDATE users SET status = 'disabled' WHERE id = $1")
          .bind(user)
          .execute(&db.pool)
          .await
          .unwrap();
      assert!(notified(&mut l).await, "disable");
      let group: Uuid = sqlx::query_scalar("INSERT INTO groups (name) VALUES ($1) RETURNING id")
          .bind(format!("sync-{}", Uuid::new_v4().simple()))
          .fetch_one(&db.pool)
          .await
          .unwrap();
      sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2)")
          .bind(user)
          .bind(group)
          .execute(&db.pool)
          .await
          .unwrap();
      assert!(notified(&mut l).await, "group membership");
  }

  /// Decided on 2026-11-11 (1): a platform-role grant and a write to the
  /// role table wake the sync, since platform admins are synced.
  #[tokio::test]
  async fn a_platform_admin_grant_and_a_role_write_notify() {
      let Some(db) = db().await else { return };
      let user = new_user(&db).await;
      let mut l = listening(&db).await;
      sqlx::query(
          "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
           SELECT 'user', $1, id, 'platform', NULL FROM roles \
            WHERE scope = 'platform' AND name = 'platform_admin'",
      )
      .bind(user)
      .execute(&db.pool)
      .await
      .unwrap();
      assert!(notified(&mut l).await, "a platform_admin grant");
      sqlx::query("DELETE FROM access_grants WHERE principal_id = $1")
          .bind(user)
          .execute(&db.pool)
          .await
          .unwrap();
      assert!(notified(&mut l).await, "its revoke");
      // A no-op rename still runs the trigger, and changes nothing shared.
      sqlx::query("UPDATE roles SET name = name WHERE name = 'platform_admin'")
          .execute(&db.pool)
          .await
          .unwrap();
      assert!(notified(&mut l).await, "a write to the role table");
  }

  #[tokio::test]
  async fn an_engine_assignment_notifies() {
      let Some(db) = db().await else { return };
      let ws = new_workspace(&db).await;
      let engine = egret_engine_mgmt::EngineMgmtStore::new(db.clone())
          .create_engine(
              &format!("sync-{}", Uuid::new_v4().simple()),
              "starrocks",
              "egret-engines",
              &serde_json::json!({}),
              None,
          )
          .await
          .expect("engine");
      let mut l = listening(&db).await;
      sqlx::query(
          "INSERT INTO engine_mgmt.engine_workspaces (engine_id, workspace_id) VALUES ($1, $2)",
      )
      .bind(engine.id)
      .bind(ws)
      .execute(&db.pool)
      .await
      .unwrap();
      assert!(notified(&mut l).await, "assignment");
  }
  ```

  (Use the crate's existing test-DB helper if the other `tests/*.rs` files use one instead of
  `egret_test_db::url()` directly; keep the assertions.)

- [ ] **Step 3: Run them.** The migrations are embedded at build time, so the red proof for this
  task is Step 5's mutants (each rebuilds and must fail its test), not a run without the file:

  ```bash
  env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
    CARGO_TARGET_DIR=$PWD/target-srd cargo test -p egret-engine-mgmt --test starrocks_user_sync_triggers
  env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
    CARGO_TARGET_DIR=$PWD/target-srd cargo test -p egret-auth --test username_ledger
  ```

  Expected with the migration: all pass, each in more than a few milliseconds.

- [ ] **Step 4: The store.** In `store.rs`, after `username_taken`:

  ```rust
  /// The egret users who may query `engine_id` through a workspace connection
  /// (package D, D1a): active users holding a live `allow` workspace grant,
  /// directly or through a group, on a workspace the engine is assigned to —
  /// the same membership `egret_perm_compile::members::workspace_members_via`
  /// expands. `None` for a user not minted a username yet.
  pub(crate) async fn starrocks_user_candidates(
      &self,
      engine_id: Uuid,
  ) -> Result<Vec<(Uuid, Option<String>)>, EngineMgmtError> {
      Ok(sqlx::query_as(
          "SELECT u.id, u.username::text \
             FROM engine_mgmt.engine_workspaces ew \
             JOIN access_grants ag ON ag.scope_type = 'workspace' AND ag.scope_id = ew.workspace_id \
             JOIN users u ON ag.principal_type = 'user' AND u.id = ag.principal_id \
            WHERE ew.engine_id = $1 AND u.status = 'active' \
              AND ag.effect = 'allow' AND (ag.expires_at IS NULL OR ag.expires_at > now()) \
           UNION \
           SELECT u.id, u.username::text \
             FROM engine_mgmt.engine_workspaces ew \
             JOIN access_grants ag ON ag.scope_type = 'workspace' AND ag.scope_id = ew.workspace_id \
             JOIN user_groups ug ON ag.principal_type = 'group' AND ug.group_id = ag.principal_id \
             JOIN users u ON u.id = ug.user_id \
            WHERE ew.engine_id = $1 AND u.status = 'active' \
              AND ag.effect = 'allow' AND (ag.expires_at IS NULL OR ag.expires_at > now())",
      )
      .bind(engine_id)
      .fetch_all(self.pool())
      .await?)
  }

  /// The StarRocks users egret created on `engine_id` (migration 0219).
  pub(crate) async fn recorded_starrocks_users(
      &self,
      engine_id: Uuid,
  ) -> Result<std::collections::BTreeSet<String>, EngineMgmtError> {
      let rows: Vec<String> = sqlx::query_scalar(
          "SELECT username FROM engine_mgmt.starrocks_users WHERE engine_id = $1",
      )
      .bind(engine_id)
      .fetch_all(self.pool())
      .await?;
      Ok(rows.into_iter().collect())
  }

  /// Record that egret creates `username` on `engine_id`; `true` when this call
  /// inserted the row (the caller forgets it again if its `CREATE USER` fails).
  pub(crate) async fn record_starrocks_user(
      &self,
      engine_id: Uuid,
      username: &str,
  ) -> Result<bool, EngineMgmtError> {
      let r = sqlx::query(
          "INSERT INTO engine_mgmt.starrocks_users (engine_id, username) VALUES ($1, $2) \
           ON CONFLICT DO NOTHING",
      )
      .bind(engine_id)
      .bind(username)
      .execute(self.pool())
      .await?;
      Ok(r.rows_affected() == 1)
  }

  pub(crate) async fn forget_starrocks_user(
      &self,
      engine_id: Uuid,
      username: &str,
  ) -> Result<(), EngineMgmtError> {
      sqlx::query("DELETE FROM engine_mgmt.starrocks_users WHERE engine_id = $1 AND username = $2")
          .bind(engine_id)
          .bind(username)
          .execute(self.pool())
          .await?;
      Ok(())
  }

  /// Take `engine_id`'s StarRocks user sync lock, a session advisory lock on a
  /// connection of its own, or `None` when another session holds it. The
  /// connection closes instead of returning to the pool, so a guard dropped
  /// without [`UserSyncLock::release`] (a panic) still frees the lock.
  pub(crate) async fn try_lock_user_sync(
      &self,
      engine_id: Uuid,
  ) -> Result<Option<UserSyncLock>, EngineMgmtError> {
      let mut conn = self.pool().acquire().await?;
      conn.close_on_drop();
      let key = format!("egret-starrocks-users:{engine_id}");
      let got: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock(hashtextextended($1, 0))")
          .bind(&key)
          .fetch_one(&mut *conn)
          .await?;
      Ok(got.then_some(UserSyncLock { conn, key }))
  }
  ```

  And at module level:

  ```rust
  /// One engine's StarRocks user sync lock ([`EngineMgmtStore::try_lock_user_sync`]).
  pub(crate) struct UserSyncLock {
      conn: sqlx::pool::PoolConnection<sqlx::Postgres>,
      key: String,
  }

  impl UserSyncLock {
      /// Release the lock. A failure is logged: the connection closes on drop,
      /// which releases it anyway.
      pub(crate) async fn release(mut self) {
          if let Err(e) = sqlx::query("SELECT pg_advisory_unlock(hashtextextended($1, 0))")
              .bind(&self.key)
              .execute(&mut *self.conn)
              .await
          {
              tracing::warn!(error = %e, "starrocks users: the sync lock was not released cleanly");
          }
      }
  }
  ```

  Store tests come with Task 8's DB tests, which exercise every method.

- [ ] **Step 5: Mutants:** remove the `access_grants` trigger (the membership and platform-grant
  tests fail); remove the `roles` trigger (the role-write assertion fails); change `'root'` to
  `'roo'` in the ledger insert (the ledger test fails).

- [ ] **Step 6: Commit.**

  ```bash
  git add migrations/0219_starrocks_users.sql crates/egret-engine-mgmt/src/store.rs crates/egret-engine-mgmt/tests/starrocks_user_sync_triggers.rs crates/egret-auth/tests/username_ledger.rs
  git commit -m "feat(engine-mgmt): record egret's StarRocks users, reserve root, notify on membership writes

  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
  ```

---

### Task 8: The StarRocks user sync

**Files:**
- Create: `crates/egret-engine-mgmt/src/starrocks_users.rs`; register `pub(crate) mod starrocks_users;`
  in `crates/egret-engine-mgmt/src/lib.rs`
- Modify: `crates/egret-engine-mgmt/src/starrocks_ddl.rs` (`FakeStarRocksDdl`: users)
- Test: pure tests in `starrocks_users.rs`; DB tests in `engines.rs`'s `mod tests`, next to the
  query-log tests

**Interfaces:**
- Consumes: Task 2 (`is_starrocks_reserved_user`); Task 7 (the store methods, `UserSyncLock`);
  `crate::query_log::authorized_name`; `crate::engines::random_internal_secret`;
  `egret_auth::users::ensure_username_via`; `egret_auth::roles::engine_admin_user_ids_via` (the one
  definition of the platform admins, decided on 2026-11-11, 1);
  `EngineMgmtStore::list_engine_workspace_ids`; Task 1's M2 and M3.
- Produces:

  ```rust
  pub const USER_SYNC_CHANNEL: &str = "egret_starrocks_user_sync";
  pub(crate) const SHOW_USERS_SQL: &str = "SHOW USERS";
  pub(crate) fn is_starrocks_user_name(name: &str) -> bool;
  pub(crate) fn user_identity(name: &str) -> String;
  pub(crate) fn create_user_sql(name: &str, password: &str) -> String;
  pub(crate) fn drop_user_sql(name: &str) -> String;
  pub(crate) fn listed_users(rows: &[Vec<Option<String>>]) -> BTreeSet<String>;
  #[derive(Debug, Default, PartialEq, Eq)]
  pub(crate) struct UserPlan { pub kept: Vec<String>, pub create: Vec<String>, pub drop: Vec<String>,
      pub forget: Vec<String>, pub foreign: Vec<String>, pub refused: Vec<String> }
  pub(crate) fn plan_users(desired: &BTreeSet<String>, recorded: &BTreeSet<String>,
      on_fe: &BTreeSet<String>) -> UserPlan;
  #[derive(Debug, PartialEq, Eq)]
  pub(crate) enum UserSync { NotNeeded, Busy, Done { created: usize, dropped: usize,
      granted: usize, revoked_all: bool, // filled by Task 10; 0 and false until then
      foreign: Vec<String>, refused: Vec<String>, failed: Vec<String> }, Failed(String) }
  pub(crate) async fn wants_user_sync(st: &CpState, engine: &Engine) -> bool;
  pub(crate) async fn sync_starrocks_users(st: &CpState, engine: &Engine, fe: &FeTarget) -> UserSync;
  pub(crate) fn log_outcome(engine_name: &str, outcome: &UserSync);
  // FakeStarRocksDdl
  pub fn seed_user(&self, name: &str);
  pub fn users(&self) -> BTreeSet<String>;   // `root` not included
  ```

- [ ] **Step 1: The fake learns users.** In `FakeStarRocksDdl` add the field
  `users: Arc<Mutex<BTreeSet<String>>>` (in the `Default` derive's struct) and:

  ```rust
  /// A StarRocks user that exists on this FE from now on, as if someone else
  /// had created it (package D: a "foreign" user).
  pub fn seed_user(&self, name: &str) {
      self.users.lock().unwrap().insert(name.to_string());
  }

  /// The users this FE has, `root` not included.
  pub fn users(&self) -> BTreeSet<String> {
      self.users.lock().unwrap().clone()
  }
  ```

  A free function beside `statement_target`:

  ```rust
  /// The user name out of `<prefix>'<name>'@'<host>' …`, `''` read as `'`.
  fn user_target(prefix: &str, sql: &str) -> Option<String> {
      let rest = sql.strip_prefix(prefix)?;
      let identity = rest.split(" IDENTIFIED").next()?.trim();
      crate::query_log::authorized_name(identity)
  }
  ```

  In `execute`, after the `fail_once` block and before `let mut cats = …`:

  ```rust
          if let Some(name) = user_target("CREATE USER IF NOT EXISTS ", sql) {
              self.users.lock().unwrap().insert(name);
              return Ok(());
          }
          if let Some(name) = user_target("CREATE USER ", sql) {
              let mut users = self.users.lock().unwrap();
              if name == "root" || users.contains(&name) {
                  // The text measured in Task 1 (M3) replaces this one.
                  return Err(fe_error(format!(
                      "Operation CREATE USER failed for '{name}'@'%'"
                  )));
              }
              users.insert(name);
              return Ok(());
          }
          if let Some(name) = user_target("DROP USER ", sql) {
              if !self.users.lock().unwrap().remove(&name) {
                  // The text measured in Task 1 (M3) replaces this one.
                  return Err(fe_error(format!(
                      "Operation DROP USER failed for '{name}'@'%'"
                  )));
              }
              return Ok(());
          }
  ```

  In `query`, after the `SHOW PLUGINS` branch (adjust to Task 1's M2 if it differed):

  ```rust
          if sql == crate::starrocks_users::SHOW_USERS_SQL {
              let mut rows = vec![vec![Some("'root'@'%'".to_string())]];
              rows.extend(
                  self.users
                      .lock()
                      .unwrap()
                      .iter()
                      .map(|u| vec![Some(format!("'{}'@'%'", u.replace('\'', "''")))]),
              );
              return Ok(rows);
          }
  ```

- [ ] **Step 2: Write the module, tests first.** Write the file's `mod tests` (below) with only
  the `use` lines and the constants above it, run it and see it fail to compile, then add the code.

  `crates/egret-engine-mgmt/src/starrocks_users.rs`:

  ```rust
  //! The StarRocks users of the people who may query an impersonate-mode
  //! StarRocks engine (package D, D1a; spec
  //! `docs/en/specs/2026-11-11-starrocks-delegation-and-lifecycle-design.md`,
  //! section 2).
  //!
  //! `EXECUTE AS` needs its target user to exist (StarRocks checks it before
  //! any authorizer). egret creates `'<users.username>'@'%'` — exactly the name
  //! egret-query's connector sends — with a random password kept nowhere, for
  //! each active member of a workspace the engine serves, records it in
  //! `engine_mgmt.starrocks_users`, and drops only what it recorded. It runs as
  //! `root` from engine-mgmt, never from the query path.
  //!
  //! Woken by: the `pg_notify` triggers of migration 0219 (membership, groups,
  //! user status and delete, engine assignment), a sweep every
  //! [`SWEEP_INTERVAL`] whose first tick is the boot reconcile, every StarRocks
  //! apply ([`spawn_user_sync`]), and the post-Ready bootstrap.

  use std::collections::{BTreeMap, BTreeSet};
  use std::sync::Arc;
  use std::time::Duration;

  use egret_core::engine_principals::is_starrocks_reserved_user;
  use egret_query_api::{quote_string, Engine as QueryEngine};
  use uuid::Uuid;

  use crate::engines::{engine_still_current, random_internal_secret, wait_for_fe_root_login};
  use crate::starrocks_ddl::{fe_target, FeTarget};
  use crate::EngineMgmtState as CpState;
  use crate::{engine_status, Engine};

  /// The channel migration 0219's triggers notify.
  pub const USER_SYNC_CHANNEL: &str = "egret_starrocks_user_sync";

  /// How often every engine is reconciled whatever was notified: a lapsed
  /// time-boxed membership writes nothing, a notification sent while the
  /// listener reconnected is lost, and an FE can lose its metadata on a
  /// restart outside an apply.
  const SWEEP_INTERVAL: Duration = Duration::from_secs(600);
  const LISTEN_RETRY: Duration = Duration::from_secs(5);

  pub(crate) const SHOW_USERS_SQL: &str = "SHOW USERS";

  /// Whether StarRocks accepts `name` as a user name: `FeNameFormat.checkUserName`,
  /// `^\w{1,64}/?[.\w-]{0,63}$` and at most 64 characters, `\w` being ASCII.
  pub(crate) fn is_starrocks_user_name(name: &str) -> bool {
      let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
      if name.is_empty() || name.len() > 64 {
          return false;
      }
      let head = name.find(|c: char| !word(c)).unwrap_or(name.len());
      if head == 0 {
          return false;
      }
      let rest = &name[head..];
      let rest = rest.strip_prefix('/').unwrap_or(rest);
      rest.chars().all(|c| word(c) || c == '.' || c == '-')
  }

  /// `'<name>'@'%'`, the name quoted as a StarRocks string literal.
  pub(crate) fn user_identity(name: &str) -> String {
      format!("{}@'%'", quote_string(QueryEngine::StarRocks, name))
  }

  /// No `IF NOT EXISTS`: a user that is already there is someone else's, and
  /// the failure is how the sync learns it without adopting it.
  pub(crate) fn create_user_sql(name: &str, password: &str) -> String {
      format!(
          "CREATE USER {} IDENTIFIED BY {}",
          user_identity(name),
          quote_string(QueryEngine::StarRocks, password)
      )
  }

  pub(crate) fn drop_user_sql(name: &str) -> String {
      format!("DROP USER {}", user_identity(name))
  }

  /// The user names `SHOW USERS` lists: the first column, read as `'<name>'@'<host>'`
  /// when it has that form (measured in Task 1, M2), as is otherwise.
  pub(crate) fn listed_users(rows: &[Vec<Option<String>>]) -> BTreeSet<String> {
      rows.iter()
          .filter_map(|r| r.first().cloned().flatten())
          .map(|raw| crate::query_log::authorized_name(&raw).unwrap_or(raw))
          .collect()
  }

  /// What one reconcile does (spec section 2.3's table), every list sorted.
  #[derive(Debug, Default, PartialEq, Eq)]
  pub(crate) struct UserPlan {
      /// Desired, recorded and on the FE: nothing to do (Task 10 grants these).
      pub kept: Vec<String>,
      /// Desired and absent from the FE: record, then `CREATE USER`.
      pub create: Vec<String>,
      /// Recorded, no longer desired, on the FE: `DROP USER`, then forget.
      pub drop: Vec<String>,
      /// Recorded, no longer desired, gone from the FE: forget.
      pub forget: Vec<String>,
      /// Desired, on the FE, not recorded: someone else's user, left alone.
      pub foreign: Vec<String>,
      /// Desired but never created, as `"<name>: <why>"`.
      pub refused: Vec<String>,
  }

  pub(crate) fn plan_users(
      desired: &BTreeSet<String>,
      recorded: &BTreeSet<String>,
      on_fe: &BTreeSet<String>,
  ) -> UserPlan {
      let mut plan = UserPlan::default();
      for name in desired {
          if is_starrocks_reserved_user(name) {
              plan.refused
                  .push(format!("{name}: reserved for StarRocks or egret"));
              continue;
          }
          if !is_starrocks_user_name(name) {
              plan.refused
                  .push(format!("{name}: not a valid StarRocks user name"));
              continue;
          }
          match (recorded.contains(name), on_fe.contains(name)) {
              (true, true) => plan.kept.push(name.clone()),
              (false, true) => plan.foreign.push(name.clone()),
              (_, false) => plan.create.push(name.clone()),
          }
      }
      for name in recorded.difference(desired) {
          if on_fe.contains(name) {
              plan.drop.push(name.clone());
          } else {
              plan.forget.push(name.clone());
          }
      }
      plan
  }

  /// The outcome of one engine's reconcile.
  #[derive(Debug, PartialEq, Eq)]
  pub(crate) enum UserSync {
      /// Not impersonate-mode and nothing recorded: no FE call was made.
      NotNeeded,
      /// Another egret session holds this engine's sync lock.
      Busy,
      Done {
          created: usize,
          dropped: usize,
          /// Native `GRANT IMPERSONATE ON USER` statements sent (Task 10).
          granted: usize,
          /// Whether `ON ALL USERS` was revoked (Task 10).
          revoked_all: bool,
          foreign: Vec<String>,
          refused: Vec<String>,
          /// One scrubbed message per statement or record that failed.
          failed: Vec<String>,
      },
      /// The database or `SHOW USERS` could not be read (scrubbed).
      Failed(String),
  }

  /// Whether `engine` gets a reconcile at all: a provisioned StarRocks engine,
  /// not deleting or failed, whose bootstrap created `egret_delegator`
  /// (`impersonate` or `jwt` mode: Task 10 converges its native grants), or
  /// that still has users egret created.
  pub(crate) async fn wants_user_sync(st: &CpState, engine: &Engine) -> bool {
      if !engine.is_starrocks()
          || crate::external::is_external(engine)
          || engine.status == engine_status::DELETING
          || engine.status == engine_status::FAILED
      {
          return false;
      }
      if matches!(engine.delegation_mode(), "impersonate" | "jwt") {
          return true;
      }
      match st.engine_mgmt.recorded_starrocks_users(engine.id).await {
          Ok(recorded) => !recorded.is_empty(),
          Err(e) => {
              tracing::warn!(engine = %engine.name, error = %e,
                  "starrocks users: the records could not be read");
              false
          }
      }
  }

  /// Reconcile `engine`'s StarRocks users (spec section 2.3) as `root` on `fe`.
  pub(crate) async fn sync_starrocks_users(st: &CpState, engine: &Engine, fe: &FeTarget) -> UserSync {
      if !wants_user_sync(st, engine).await {
          return UserSync::NotNeeded;
      }
      let lock = match st.engine_mgmt.try_lock_user_sync(engine.id).await {
          Ok(Some(lock)) => lock,
          Ok(None) => return UserSync::Busy,
          Err(e) => return UserSync::Failed(format!("the sync lock could not be taken: {e}")),
      };
      let outcome = sync_locked(st, engine, fe).await;
      lock.release().await;
      outcome
  }

  /// The desired names: empty unless impersonate-mode; the members, and the
  /// platform admins when the engine serves at least one workspace (decided on
  /// 2026-11-11, 1). A user with no username yet is minted, as the permission
  /// compiler does.
  async fn desired_users(st: &CpState, engine: &Engine) -> Result<BTreeSet<String>, String> {
      if !engine.is_impersonate_delegation() {
          return Ok(BTreeSet::new());
      }
      let mut candidates = st
          .engine_mgmt
          .starrocks_user_candidates(engine.id)
          .await
          .map_err(|e| e.to_string())?;
      let serves_a_workspace = !st
          .engine_mgmt
          .list_engine_workspace_ids(engine.id)
          .await
          .map_err(|e| e.to_string())?
          .is_empty();
      if serves_a_workspace {
          let mut conn = st
              .engine_mgmt
              .pool()
              .acquire()
              .await
              .map_err(|e| e.to_string())?;
          let admins = egret_auth::roles::engine_admin_user_ids_via(&mut conn)
              .await
              .map_err(|e| e.to_string())?;
          // `None`: `ensure_username_via` below returns the stored name or mints one.
          candidates.extend(admins.into_iter().map(|u| (u.0, None)));
      }
      let mut out = BTreeSet::new();
      for (id, username) in candidates {
          if let Some(u) = username {
              out.insert(u);
              continue;
          }
          let mut conn = st
              .engine_mgmt
              .pool()
              .acquire()
              .await
              .map_err(|e| e.to_string())?;
          match egret_auth::users::ensure_username_via(&mut conn, egret_core::UserId(id)).await {
              Ok(u) => {
                  out.insert(u);
              }
              Err(e) => tracing::warn!(engine = %engine.name, user = %id, error = %e,
                  "starrocks users: a member's username could not be minted; skipped"),
          }
      }
      Ok(out)
  }

  async fn sync_locked(st: &CpState, engine: &Engine, fe: &FeTarget) -> UserSync {
      let desired = match desired_users(st, engine).await {
          Ok(d) => d,
          Err(e) => return UserSync::Failed(e),
      };
      let recorded = match st.engine_mgmt.recorded_starrocks_users(engine.id).await {
          Ok(r) => r,
          Err(e) => return UserSync::Failed(e.to_string()),
      };
      let on_fe = match st.starrocks_ddl.query(fe, SHOW_USERS_SQL).await {
          Ok(rows) => listed_users(&rows),
          Err(e) => return UserSync::Failed(e.scrubbed(fe, &BTreeMap::new())),
      };
      let plan = plan_users(&desired, &recorded, &on_fe);
      let (mut created, mut dropped, mut failed) = (0, 0, Vec::new());
      for name in &plan.create {
          let inserted = match st.engine_mgmt.record_starrocks_user(engine.id, name).await {
              Ok(inserted) => inserted,
              Err(e) => {
                  failed.push(format!("{name}: record: {e}"));
                  continue;
              }
          };
          let password = random_internal_secret();
          let mut secrets = BTreeMap::new();
          secrets.insert(Uuid::from_u128(1), password.clone());
          match st
              .starrocks_ddl
              .execute(fe, &create_user_sql(name, &password))
              .await
          {
              Ok(()) => created += 1,
              Err(e) => {
                  // Never adopt a user that appeared between the read and the
                  // create: forget the record if this call made it.
                  if inserted {
                      if let Err(f) = st.engine_mgmt.forget_starrocks_user(engine.id, name).await {
                          failed.push(format!("{name}: forget: {f}"));
                      }
                  }
                  failed.push(format!("{name}: {}", e.scrubbed(fe, &secrets)));
              }
          }
      }
      for name in &plan.drop {
          match st.starrocks_ddl.execute(fe, &drop_user_sql(name)).await {
              Ok(()) => {
                  dropped += 1;
                  if let Err(e) = st.engine_mgmt.forget_starrocks_user(engine.id, name).await {
                      failed.push(format!("{name}: forget: {e}"));
                  }
              }
              Err(e) => failed.push(format!("{name}: {}", e.scrubbed(fe, &BTreeMap::new()))),
          }
      }
      for name in &plan.forget {
          if let Err(e) = st.engine_mgmt.forget_starrocks_user(engine.id, name).await {
              failed.push(format!("{name}: forget: {e}"));
          }
      }
      UserSync::Done {
          created,
          dropped,
          granted: 0,
          revoked_all: false,
          foreign: plan.foreign,
          refused: plan.refused,
          failed,
      }
  }

  pub(crate) fn log_outcome(engine_name: &str, outcome: &UserSync) {
      match outcome {
          UserSync::NotNeeded => {}
          UserSync::Busy => tracing::debug!(engine = %engine_name,
              "starrocks users: another sync of this engine is running"),
          UserSync::Done { created, dropped, granted, revoked_all, foreign, refused, failed } => {
              if *created > 0 || *dropped > 0 || *granted > 0 || *revoked_all {
                  tracing::info!(engine = %engine_name, created, dropped, granted, revoked_all,
                      "starrocks users synced");
              }
              if !foreign.is_empty() {
                  tracing::warn!(engine = %engine_name, users = ?foreign,
                      "starrocks users: a member's name is held by a StarRocks user egret did not \
                       create; it is left as it is");
              }
              if !refused.is_empty() {
                  tracing::warn!(engine = %engine_name, users = ?refused,
                      "starrocks users: not created");
              }
              if !failed.is_empty() {
                  tracing::warn!(engine = %engine_name, errors = ?failed,
                      "starrocks users: some changes failed; the next sync tries again");
              }
          }
          UserSync::Failed(why) => tracing::warn!(engine = %engine_name, error = %why,
              "starrocks users: the sync failed; the next sync tries again"),
      }
  }
  ```

  (Task 9 adds `spawn_user_sync`, `sync_all_starrocks_users` and `spawn_starrocks_user_sync` to this
  file; until then `Arc`, `Duration` constants, `fe_target`, `engine_still_current` and
  `wait_for_fe_root_login` are unused: add the imports in Task 9 instead if clippy complains.)

  Pure tests at the bottom of the file:

  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;

      fn set(names: &[&str]) -> BTreeSet<String> {
          names.iter().map(|s| s.to_string()).collect()
      }

      #[test]
      fn starrocks_user_names_follow_fe_name_format() {
          for ok in ["alice", "member-sso", "bob-2", "a_b.c", "x1", "a/b"] {
              assert!(is_starrocks_user_name(ok), "{ok}");
          }
          let long = "a".repeat(65);
          for bad in ["", "-lead", ".lead", "a b", "o'brien", "é", "a/b/c", long.as_str()] {
              assert!(!is_starrocks_user_name(bad), "{bad}");
          }
      }

      #[test]
      fn the_statements_quote_the_name_and_the_password() {
          assert_eq!(
              create_user_sql("bob", "p'w\\x"),
              "CREATE USER 'bob'@'%' IDENTIFIED BY 'p''w\\\\x'"
          );
          assert_eq!(drop_user_sql("o'b"), "DROP USER 'o''b'@'%'");
      }

      #[test]
      fn listed_users_reads_the_identity_form_and_a_bare_name() {
          let rows = vec![
              vec![Some("'root'@'%'".to_string())],
              vec![Some("'o''b'@'10.%'".to_string())],
              vec![Some("plain".to_string())],
              vec![None],
          ];
          assert_eq!(listed_users(&rows), set(&["o'b", "plain", "root"]));
      }

      #[test]
      fn the_plan_follows_the_table() {
          let desired = set(&["keep", "new", "lost", "theirs", "root", "egret_query_log", "bad name"]);
          let recorded = set(&["keep", "lost", "gone", "left"]);
          let on_fe = set(&["root", "keep", "theirs", "gone"]);
          assert_eq!(
              plan_users(&desired, &recorded, &on_fe),
              UserPlan {
                  kept: vec!["keep".into()],
                  create: vec!["lost".into(), "new".into()],
                  drop: vec!["gone".into()],
                  forget: vec!["left".into()],
                  foreign: vec!["theirs".into()],
                  refused: vec![
                      "bad name: not a valid StarRocks user name".into(),
                      "egret_query_log: reserved for StarRocks or egret".into(),
                      "root: reserved for StarRocks or egret".into(),
                  ],
              }
          );
      }

      #[test]
      fn nothing_desired_drops_every_recorded_user_and_touches_no_other() {
          let plan = plan_users(&set(&[]), &set(&["a", "b"]), &set(&["root", "a", "other"]));
          assert_eq!(plan.drop, vec!["a".to_string()]);
          assert_eq!(plan.forget, vec!["b".to_string()]);
          assert!(plan.create.is_empty() && plan.foreign.is_empty());
      }
  }
  ```

  Run: `CARGO_TARGET_DIR=$PWD/target-srd cargo test -p egret-engine-mgmt --lib starrocks_users`.
  Expected: they fail before the code above exists and pass after.

- [ ] **Step 3: The DB tests,** in `engines.rs`'s `mod tests`, next to the query-log tests. Fixtures:

  ```rust
  /// `seed_sr_engine` in impersonate mode.
  async fn seed_impersonate_sr_engine(st: &CpState) -> Engine {
      let engine = seed_sr_engine(st).await;
      let mut spec = engine.spec.clone();
      spec["starrocksConfig"]["delegation"] = serde_json::json!({"mode": "impersonate"});
      assert!(st
          .engine_mgmt
          .update_engine(engine.id, &spec, &engine.status)
          .await
          .expect("update spec"));
      st.engine_mgmt
          .get_engine(engine.id)
          .await
          .expect("reload")
          .expect("present")
  }

  /// A user with `username` and `status`, a direct `ws_viewer` of `ws`.
  async fn seed_member(st: &CpState, ws: Uuid, username: &str, status: &str) -> Uuid {
      let pool = st.engine_mgmt.pool();
      let id: Uuid = sqlx::query_scalar(
          "INSERT INTO users (provider, email, username, status) VALUES ('local', $1, $2, $3) \
           RETURNING id",
      )
      .bind(format!("{username}@{}.test", Uuid::new_v4().simple()))
      .bind(username)
      .bind(status)
      .fetch_one(pool)
      .await
      .expect("user");
      sqlx::query(
          "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
           SELECT 'user', $1, id, 'workspace', $2 FROM roles \
            WHERE scope = 'workspace' AND name = 'ws_viewer'",
      )
      .bind(id)
      .bind(ws)
      .execute(pool)
      .await
      .expect("grant");
      id
  }

  /// A workspace assigned to `engine`.
  async fn seed_assigned_workspace(st: &CpState, engine: &Engine) -> Uuid {
      let pool = st.engine_mgmt.pool();
      let ws: Uuid =
          sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1, $1) RETURNING id")
              .bind(format!("ws-{}", Uuid::new_v4().simple()))
              .fetch_one(pool)
              .await
              .expect("workspace");
      sqlx::query(
          "INSERT INTO engine_mgmt.engine_workspaces (engine_id, workspace_id) VALUES ($1, $2)",
      )
      .bind(engine.id)
      .bind(ws)
      .execute(pool)
      .await
      .expect("assign");
      ws
  }

  fn uname(prefix: &str) -> String {
      format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..10])
  }
  ```

  Tests (each starts with the `test_state_with_fake_fe` skip pattern the query-log tests use):

  ```rust
  /// Package D, D1a: active members get a StarRocks user; a disabled one does
  /// not; a second sync changes nothing.
  #[tokio::test]
  async fn the_sync_creates_the_members_users_once() {
      let Some((st, fake, _prov)) = test_state_with_fake_fe().await else {
          eprintln!("skipping the_sync_creates_the_members_users_once: DATABASE_URL not set");
          return;
      };
      let engine = seed_impersonate_sr_engine(&st).await;
      let ws = seed_assigned_workspace(&st, &engine).await;
      let (alice, carol) = (uname("alice"), uname("carol"));
      seed_member(&st, ws, &alice, "active").await;
      seed_member(&st, ws, &carol, "disabled").await;
      let fe = crate::starrocks_ddl::fe_target(&st, &engine).await.expect("fe");

      let first = crate::starrocks_users::sync_starrocks_users(&st, &engine, &fe).await;
      let second = crate::starrocks_users::sync_starrocks_users(&st, &engine, &fe).await;

      // The test database's platform admins are synced too (decided on
      // 2026-11-11, 1), so these assertions name the test's own users only.
      assert!(matches!(first, crate::starrocks_users::UserSync::Done { .. }), "{first:?}");
      assert!(matches!(second, crate::starrocks_users::UserSync::Done { created: 0, dropped: 0, .. }), "{second:?}");
      let users = fake.users();
      assert!(users.contains(&alice) && !users.contains(&carol), "{users:?}");
      let recorded = st.engine_mgmt.recorded_starrocks_users(engine.id).await.unwrap();
      assert!(recorded.contains(&alice) && !recorded.contains(&carol), "{recorded:?}");
      let creates: Vec<_> = fake
          .executed()
          .into_iter()
          .filter(|s| s.starts_with(&format!("CREATE USER '{alice}'@")))
          .collect();
      assert_eq!(creates.len(), 1, "{creates:?}");
      assert!(!creates[0].contains("IF NOT EXISTS"));
  }

  /// A member who leaves is dropped; a same-name user egret did not create is
  /// left alone even when its holder leaves.
  #[tokio::test]
  async fn the_sync_drops_who_left_and_never_touches_a_foreign_user() {
      let Some((st, fake, _prov)) = test_state_with_fake_fe().await else {
          eprintln!("skipping the_sync_drops_who_left_and_never_touches_a_foreign_user: DATABASE_URL not set");
          return;
      };
      let engine = seed_impersonate_sr_engine(&st).await;
      let ws = seed_assigned_workspace(&st, &engine).await;
      let (bob, theirs) = (uname("bob"), uname("theirs"));
      let bob_id = seed_member(&st, ws, &bob, "active").await;
      let theirs_id = seed_member(&st, ws, &theirs, "active").await;
      fake.seed_user(&theirs);
      let fe = crate::starrocks_ddl::fe_target(&st, &engine).await.expect("fe");
      let first = crate::starrocks_users::sync_starrocks_users(&st, &engine, &fe).await;
      assert!(
          matches!(&first, crate::starrocks_users::UserSync::Done { foreign, .. } if foreign == &vec![theirs.clone()]),
          "{first:?}"
      );

      sqlx::query("DELETE FROM access_grants WHERE principal_id = ANY($1)")
          .bind(vec![bob_id, theirs_id])
          .execute(st.engine_mgmt.pool())
          .await
          .unwrap();
      let second = crate::starrocks_users::sync_starrocks_users(&st, &engine, &fe).await;

      assert!(matches!(second, crate::starrocks_users::UserSync::Done { dropped: 1, .. }), "{second:?}");
      let users = fake.users();
      assert!(users.contains(&theirs) && !users.contains(&bob), "bob dropped, theirs kept: {users:?}");
      assert!(!st.engine_mgmt.recorded_starrocks_users(engine.id).await.unwrap().contains(&bob));
  }

  /// A create that fails forgets only the record it made, and the FE's error
  /// reaches the outcome scrubbed of the password it carried.
  #[tokio::test]
  async fn a_failed_create_forgets_its_record_and_is_scrubbed() {
      let Some((st, fake, _prov)) = test_state_with_fake_fe().await else {
          eprintln!("skipping a_failed_create_forgets_its_record_and_is_scrubbed: DATABASE_URL not set");
          return;
      };
      let engine = seed_impersonate_sr_engine(&st).await;
      let ws = seed_assigned_workspace(&st, &engine).await;
      let dan = uname("dan");
      seed_member(&st, ws, &dan, "active").await;
      // The FE rejects the create and quotes the password back (not in the
      // `IDENTIFIED BY` shape, so only the exact-value scrub can hide it).
      // The needle is the whole statement, so each create gets its own echo.
      let echo = fake.clone();
      fake.on_execute(move |sql| {
          if let Some(pw) = sql
              .strip_prefix("CREATE USER '")
              .and_then(|_| sql.rsplit("IDENTIFIED BY '").next())
          {
              let pw = pw.trim_end_matches('\'');
              echo.fail_matching(&sql, &format!("rejected; it was {pw}"));
          }
          Box::pin(std::future::ready(()))
      });
      let fe = crate::starrocks_ddl::fe_target(&st, &engine).await.expect("fe");

      let out = crate::starrocks_users::sync_starrocks_users(&st, &engine, &fe).await;

      let crate::starrocks_users::UserSync::Done { created: 0, failed, .. } = &out else {
          panic!("{out:?}");
      };
      let dans: Vec<&String> = failed.iter().filter(|f| f.starts_with(&format!("{dan}:"))).collect();
      assert_eq!(dans.len(), 1, "{failed:?}");
      assert!(!st.engine_mgmt.recorded_starrocks_users(engine.id).await.unwrap().contains(&dan));
      let sent = fake
          .executed()
          .into_iter()
          .find(|s| s.starts_with(&format!("CREATE USER '{dan}'@")))
          .unwrap();
      let password = sent.rsplit("IDENTIFIED BY '").next().unwrap().trim_end_matches('\'');
      assert!(!dans[0].contains(password), "the password leaked: {dans:?}");
  }

  /// An engine with no delegation (so no delegator) and nothing recorded costs
  /// no FE call.
  #[tokio::test]
  async fn an_engine_without_delegation_or_records_makes_no_fe_call() {
      let Some((st, fake, _prov)) = test_state_with_fake_fe().await else {
          eprintln!("skipping an_engine_without_delegation_or_records_makes_no_fe_call: DATABASE_URL not set");
          return;
      };
      let engine = seed_sr_engine(&st).await;
      let fe = crate::starrocks_ddl::fe_target(&st, &engine).await.expect("fe");
      let out = crate::starrocks_users::sync_starrocks_users(&st, &engine, &fe).await;
      assert_eq!(out, crate::starrocks_users::UserSync::NotNeeded);
      assert!(fake.queried().is_empty() && fake.executed().is_empty());
  }

  /// Another session holding the engine's lock: `Busy`, nothing done.
  #[tokio::test]
  async fn a_held_lock_makes_the_sync_busy() {
      let Some((st, fake, _prov)) = test_state_with_fake_fe().await else {
          eprintln!("skipping a_held_lock_makes_the_sync_busy: DATABASE_URL not set");
          return;
      };
      let engine = seed_impersonate_sr_engine(&st).await;
      let held = st.engine_mgmt.try_lock_user_sync(engine.id).await.unwrap().expect("lock");
      let fe = crate::starrocks_ddl::fe_target(&st, &engine).await.expect("fe");
      let out = crate::starrocks_users::sync_starrocks_users(&st, &engine, &fe).await;
      held.release().await;
      assert_eq!(out, crate::starrocks_users::UserSync::Busy);
      assert!(fake.queried().is_empty());
  }

  /// A member reached through a group is synced; an expired grant is not.
  #[tokio::test]
  async fn group_members_count_and_expired_grants_do_not() {
      let Some((st, fake, _prov)) = test_state_with_fake_fe().await else {
          eprintln!("skipping group_members_count_and_expired_grants_do_not: DATABASE_URL not set");
          return;
      };
      let engine = seed_impersonate_sr_engine(&st).await;
      let ws = seed_assigned_workspace(&st, &engine).await;
      let pool = st.engine_mgmt.pool();
      let (gina, eve) = (uname("gina"), uname("eve"));
      let gina_id: Uuid = sqlx::query_scalar(
          "INSERT INTO users (provider, email, username) VALUES ('local', $1, $2) RETURNING id",
      )
      .bind(format!("{gina}@g.test"))
      .bind(&gina)
      .fetch_one(pool)
      .await
      .unwrap();
      let group: Uuid = sqlx::query_scalar("INSERT INTO groups (name) VALUES ($1) RETURNING id")
          .bind(uname("grp"))
          .fetch_one(pool)
          .await
          .unwrap();
      sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2)")
          .bind(gina_id)
          .bind(group)
          .execute(pool)
          .await
          .unwrap();
      sqlx::query(
          "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
           SELECT 'group', $1, id, 'workspace', $2 FROM roles \
            WHERE scope = 'workspace' AND name = 'ws_viewer'",
      )
      .bind(group)
      .bind(ws)
      .execute(pool)
      .await
      .unwrap();
      let eve_id = seed_member(&st, ws, &eve, "active").await;
      sqlx::query("UPDATE access_grants SET expires_at = now() - interval '1 minute' WHERE principal_id = $1")
          .bind(eve_id)
          .execute(pool)
          .await
          .unwrap();
      let fe = crate::starrocks_ddl::fe_target(&st, &engine).await.expect("fe");

      crate::starrocks_users::sync_starrocks_users(&st, &engine, &fe).await;

      let users = fake.users();
      assert!(users.contains(&gina) && !users.contains(&eve), "{users:?}");
  }

  /// Decided on 2026-11-11 (1): a platform admin who is no member gets a user
  /// on an engine that serves a workspace, and none on one assigned nowhere.
  #[tokio::test]
  async fn platform_admins_are_synced_where_the_engine_serves_a_workspace() {
      let Some((st, fake, _prov)) = test_state_with_fake_fe().await else {
          eprintln!("skipping platform_admins_are_synced_where_the_engine_serves_a_workspace: DATABASE_URL not set");
          return;
      };
      let pool = st.engine_mgmt.pool();
      let admin = uname("padmin");
      let admin_id: Uuid = sqlx::query_scalar(
          "INSERT INTO users (provider, email, username) VALUES ('local', $1, $2) RETURNING id",
      )
      .bind(format!("{admin}@p.test"))
      .bind(&admin)
      .fetch_one(pool)
      .await
      .unwrap();
      sqlx::query(
          "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
           SELECT 'user', $1, id, 'platform', NULL FROM roles \
            WHERE scope = 'platform' AND name = 'platform_admin'",
      )
      .bind(admin_id)
      .execute(pool)
      .await
      .unwrap();
      let serving = seed_impersonate_sr_engine(&st).await;
      seed_assigned_workspace(&st, &serving).await;
      let lonely = seed_impersonate_sr_engine(&st).await;

      let fe = crate::starrocks_ddl::fe_target(&st, &serving).await.expect("fe");
      crate::starrocks_users::sync_starrocks_users(&st, &serving, &fe).await;
      assert!(fake.users().contains(&admin), "{:?}", fake.users());
      let fe = crate::starrocks_ddl::fe_target(&st, &lonely).await.expect("fe");
      crate::starrocks_users::sync_starrocks_users(&st, &lonely, &fe).await;
      assert!(
          !st.engine_mgmt.recorded_starrocks_users(lonely.id).await.unwrap().contains(&admin),
          "an engine assigned nowhere gets no platform admin"
      );

      sqlx::query("DELETE FROM access_grants WHERE principal_id = $1")
          .bind(admin_id)
          .execute(pool)
          .await
          .unwrap();
  }
  ```

  Run:

  ```bash
  env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
    CARGO_TARGET_DIR=$PWD/target-srd cargo test -p egret-engine-mgmt --lib -- sync_ held_lock group_members engine_without_delegation failed_create platform_admins
  ```

  Expected: all pass; report the durations.

- [ ] **Step 4: Mutants:**
  - `CREATE USER IF NOT EXISTS` in `create_user_sql`: `the_sync_drops_who_left…` fails (the foreign
    user is adopted) and the quoting test fails;
  - drop `u.status = 'active'` from both halves of the candidates query: the first test fails
    (`carol` is created);
  - drop the group half of the `UNION`: the group test fails;
  - drop the `if inserted { forget }` block: the failed-create test fails;
  - call `wants_user_sync` after taking the lock and querying `SHOW USERS`: the no-delegation test
    fails;
  - drop the platform-admin block in `desired_users`: the platform-admin test fails; drop its
    `serves_a_workspace` condition: its "assigned nowhere" assertion fails;
  - drop the lock: the busy test fails;
  - replace `&secrets` with `&BTreeMap::new()` in the create branch: the scrub test fails.

- [ ] **Step 5: Commit.**

  ```bash
  git add crates/egret-engine-mgmt/src
  git commit -m "feat(engine-mgmt): sync the StarRocks users of an impersonate-mode engine's members

  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
  ```

---

### Task 9: Wire the sync: the bootstrap, every apply, the listener and the sweep

**Files:**
- Modify: `crates/egret-engine-mgmt/src/starrocks_users.rs` (three functions)
- Modify: `crates/egret-engine-mgmt/src/engines.rs` (`spawn_starrocks_catalog_wire` and its doc;
  `finish_apply`; tests)
- Modify: `crates/egret-engine-mgmt/src/lib.rs` (re-export)
- Modify: `crates/egret-bin/src/main.rs` (about line 3692, beside `setup_query_logs`)

**Interfaces:**
- Consumes: Task 8.
- Produces:

  ```rust
  pub(crate) fn spawn_user_sync(st: CpState, engine_id: Uuid, name: String, namespace: String)
      -> tokio::task::JoinHandle<()>;
  pub(crate) async fn sync_all_starrocks_users(st: &CpState) -> usize;
  pub fn spawn_starrocks_user_sync(st: CpState) -> tokio::task::JoinHandle<()>; // re-exported
  ```

- [ ] **Step 1: Write the failing tests** in `engines.rs`'s `mod tests` (`test_delegation()`
  already builds a `StarRocksDelegationBootstrap`):

  ```rust
  /// Package D: the bootstrap creates the members' StarRocks users right after
  /// the delegation statements, which create the delegator.
  #[tokio::test]
  async fn the_bootstrap_syncs_the_members_after_the_delegation_statements() {
      let Some((st, fake, prov)) = test_state_with_fake_fe().await else {
          eprintln!("skipping the_bootstrap_syncs_the_members_after_the_delegation_statements: DATABASE_URL not set");
          return;
      };
      let engine = seed_impersonate_sr_engine(&st).await;
      prov.set_status(ready_fe_status(&engine.name));
      let ws = seed_assigned_workspace(&st, &engine).await;
      let alice = uname("alice");
      seed_member(&st, ws, &alice, "active").await;

      spawn_starrocks_catalog_wire(
          st.clone(),
          engine.id,
          engine.name.clone(),
          engine.namespace.clone(),
          Some(test_delegation()),
      )
      .await
      .expect("bootstrap task did not panic");

      let sent = fake.executed();
      let delegator = sent
          .iter()
          .position(|s| s.contains("CREATE USER IF NOT EXISTS 'egret_delegator'"))
          .expect("the delegator");
      let create = sent
          .iter()
          .position(|s| s.starts_with(&format!("CREATE USER '{alice}'@'%'")))
          .expect("the member's user");
      assert!(delegator < create, "{sent:?}");
      assert!(fake.users().contains(&alice));
      // Before the query log, whose node wait can take minutes.
      let q = fake.queried();
      let users_at = q
          .iter()
          .position(|s| s == crate::starrocks_users::SHOW_USERS_SQL)
          .expect("SHOW USERS");
      let nodes_at = q
          .iter()
          .position(|s| s.ends_with("SHOW COMPUTE NODES"))
          .expect("the query log's node wait");
      assert!(users_at < nodes_at, "{q:?}");
  }

  /// Checkpoint 7: an engine deleted on the last delegation statement gets no
  /// user sync.
  #[tokio::test]
  async fn a_deleted_engine_gets_no_user_sync_from_the_bootstrap() {
      let Some((st, fake, prov)) = test_state_with_fake_fe().await else {
          eprintln!("skipping a_deleted_engine_gets_no_user_sync_from_the_bootstrap: DATABASE_URL not set");
          return;
      };
      let engine = seed_impersonate_sr_engine(&st).await;
      prov.set_status(ready_fe_status(&engine.name));
      let store = st.engine_mgmt.clone();
      let engine_id = engine.id;
      fake.on_execute(move |sql| {
          let store = store.clone();
          Box::pin(async move {
              // The LAST delegation statement. Task 10 removes the `ON ALL USERS`
              // grant and moves this to the delegator's `CREATE USER`.
              if sql.contains("GRANT IMPERSONATE ON ALL USERS") {
                  store.delete_engine(engine_id).await.unwrap();
              }
          })
      });

      spawn_starrocks_catalog_wire(
          st.clone(),
          engine.id,
          engine.name.clone(),
          engine.namespace.clone(),
          Some(test_delegation()),
      )
      .await
      .expect("bootstrap task did not panic");

      assert!(
          !fake.queried().iter().any(|q| q == crate::starrocks_users::SHOW_USERS_SQL),
          "{:?}",
          fake.queried()
      );
  }

  /// Every StarRocks apply reconciles the users: `apply_built` with
  /// `ApplyOp::Update`, the path update, restart, rebind, catalog apply and
  /// security upgrade all take.
  #[tokio::test]
  async fn an_apply_syncs_the_users() {
      let Some((st, fake, prov)) = test_state_with_fake_fe().await else {
          eprintln!("skipping an_apply_syncs_the_users: DATABASE_URL not set");
          return;
      };
      let dsn = egret_test_db::url().await.unwrap();
      let db = egret_metastore::Db::connect(&dsn, 5).await.expect("connect");
      let (wh, _s) = seed_sr_warehouse(&st, &db).await;
      let engine = seed_impersonate_sr_engine(&st).await;
      attach_primary_warehouse_catalog(&st, &engine, wh.id).await;
      prov.set_status(ready_fe_status(&engine.name));
      let ws = seed_assigned_workspace(&st, &engine).await;
      let erin = uname("erin");
      seed_member(&st, ws, &erin, "active").await;

      let built = build_provision_req_with(&st, &engine, BuildOpts::default())
          .await
          .expect("build");
      let port: std::sync::Arc<dyn egret_engine_mgmt_api::EngineProvisioningPort> = prov.clone();
      apply_built(&st, port, &engine, &built, ApplyOp::Update)
          .await
          .expect("apply");

      let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
      while !fake.users().contains(&erin) {
          assert!(
              std::time::Instant::now() < deadline,
              "no sync after the apply: {:?}",
              fake.executed()
          );
          tokio::time::sleep(std::time::Duration::from_millis(50)).await;
      }
  }

  /// The sweep's pass reconciles every engine that wants it.
  #[tokio::test]
  async fn the_pass_syncs_every_engine_that_wants_it() {
      let Some((st, fake, _prov)) = test_state_with_fake_fe().await else {
          eprintln!("skipping the_pass_syncs_every_engine_that_wants_it: DATABASE_URL not set");
          return;
      };
      let engine = seed_impersonate_sr_engine(&st).await;
      let ws = seed_assigned_workspace(&st, &engine).await;
      let fay = uname("fay");
      seed_member(&st, ws, &fay, "active").await;

      let synced = crate::starrocks_users::sync_all_starrocks_users(&st).await;

      assert!(synced >= 1);
      assert!(fake.users().contains(&fay));
  }
  ```

  `CountingStatusProvisioner` (a test double in the same module) answers `update` with `Ok(())`.
  Run them; they fail (no sync is wired).

- [ ] **Step 2: Implement.** Append to `starrocks_users.rs`:

  ```rust
  /// After a StarRocks apply (`engines::finish_apply`): if the engine wants a
  /// sync, wait for the FE and a real root login exactly as the bootstrap does,
  /// pass a checkpoint, and reconcile. Spawned, never awaited by a request.
  pub(crate) fn spawn_user_sync(
      st: CpState,
      engine_id: Uuid,
      name: String,
      namespace: String,
  ) -> tokio::task::JoinHandle<()> {
      tokio::spawn(async move {
          let Ok(Some(engine)) = st.engine_mgmt.get_engine(engine_id).await else {
              return;
          };
          if !wants_user_sync(&st, &engine).await {
              return;
          }
          let mut errors: u32 = 0;
          let Some((engine, fe)) =
              wait_for_fe_root_login(&st, engine_id, &name, &namespace, &mut errors).await
          else {
              return;
          };
          if !engine_still_current(&st.engine_mgmt, engine_id, &name, &mut errors).await {
              return;
          }
          let outcome = sync_starrocks_users(&st, &engine, &fe).await;
          log_outcome(&name, &outcome);
      })
  }

  /// One pass over every engine that wants a sync, one at a time. Returns how
  /// many reconciled (`Done`).
  pub(crate) async fn sync_all_starrocks_users(st: &CpState) -> usize {
      let engines = match st.engine_mgmt.list_engines().await {
          Ok(engines) => engines,
          Err(e) => {
              tracing::warn!(error = %e, "starrocks users: listing the engines failed");
              return 0;
          }
      };
      let mut synced = 0;
      for engine in engines {
          if !wants_user_sync(st, &engine).await {
              continue;
          }
          let fe = match fe_target(st, &engine).await {
              Ok(fe) => fe,
              Err(e) => {
                  tracing::warn!(engine = %engine.name, error = ?e,
                      "starrocks users: the FE connection could not be resolved");
                  continue;
              }
          };
          let outcome = sync_starrocks_users(st, &engine, &fe).await;
          log_outcome(&engine.name, &outcome);
          if matches!(outcome, UserSync::Done { .. }) {
              synced += 1;
          }
      }
      synced
  }

  /// Start the sync's three tasks: a `LISTEN` on [`USER_SYNC_CHANNEL`], a
  /// sweep every [`SWEEP_INTERVAL`] whose first tick is immediate (the boot
  /// reconcile), and the worker that runs [`sync_all_starrocks_users`] once per
  /// wake-up. Wake-ups coalesce (`Notify` keeps at most one permit), so a burst
  /// of writes during a pass costs one more pass. The listener reconnects on
  /// its own; what it misses meanwhile, the sweep catches.
  pub fn spawn_starrocks_user_sync(st: CpState) -> tokio::task::JoinHandle<()> {
      let wake = Arc::new(tokio::sync::Notify::new());
      {
          let wake = wake.clone();
          let pool = st.engine_mgmt.pool().clone();
          tokio::spawn(async move {
              loop {
                  let mut listener = match sqlx::postgres::PgListener::connect_with(&pool).await {
                      Ok(l) => l,
                      Err(e) => {
                          tracing::warn!(error = %e, "starrocks users: cannot connect the listener");
                          tokio::time::sleep(LISTEN_RETRY).await;
                          continue;
                      }
                  };
                  if let Err(e) = listener.listen(USER_SYNC_CHANNEL).await {
                      tracing::warn!(error = %e, "starrocks users: LISTEN failed");
                      tokio::time::sleep(LISTEN_RETRY).await;
                      continue;
                  }
                  // A (re)connect may have missed notifications.
                  wake.notify_one();
                  loop {
                      match listener.recv().await {
                          Ok(_) => wake.notify_one(),
                          Err(e) => {
                              tracing::warn!(error = %e, "starrocks users: listener error");
                              tokio::time::sleep(LISTEN_RETRY).await;
                              break;
                          }
                      }
                  }
              }
          });
      }
      {
          let wake = wake.clone();
          tokio::spawn(async move {
              let mut tick = tokio::time::interval(SWEEP_INTERVAL);
              loop {
                  tick.tick().await;
                  wake.notify_one();
              }
          });
      }
      tokio::spawn(async move {
          loop {
              wake.notified().await;
              let synced = sync_all_starrocks_users(&st).await;
              tracing::debug!(engines = synced, "starrocks users: sync pass done");
          }
      })
  }
  ```

  `lib.rs`: `pub use starrocks_users::{spawn_starrocks_user_sync, USER_SYNC_CHANNEL};`

  `engines.rs`, `spawn_starrocks_catalog_wire`: replace the comment and checkpoint that start
  "// Checkpoint 7, then the query log, LAST" with:

  ```rust
          // Checkpoint 7, then the StarRocks users of the people who may query
          // this engine (package D, D1a): after the delegation statements,
          // which create the delegator, and before the query log, whose node
          // wait can take minutes.
          if !engine_still_current(
              &st.engine_mgmt,
              engine_id,
              &engine_name,
              &mut consecutive_read_errors,
          )
          .await
          {
              return;
          }
          let users = crate::starrocks_users::sync_starrocks_users(&st, &engine, &fe).await;
          crate::starrocks_users::log_outcome(&engine_name, &users);

          // Checkpoint 8, then the query log, LAST (live check, 2026-11-11):
          // its setup waits up to about five minutes for an alive compute node
          // (a shared-data `CREATE TABLE` needs one), and its step 6 up to three
          // minutes for the first event. The delegation statements and the
          // users above need neither, so nothing of the query log may stand in
          // front of them.
  ```

  (keep the existing `if !engine_still_current(…) { return; }` that followed the old comment). In
  the function's doc, "the seven `engine_still_current` checkpoints" becomes "eight", and the list
  ends "…; then one before the user sync (7, [`crate::starrocks_users::sync_starrocks_users`]); and
  last, one before the query log setup (8, …)".

  `engines.rs`, `finish_apply`, right after `let res = match op { … };`:

  ```rust
      // Package D, D1a: every StarRocks apply reconciles the engine's users
      // once the FE answers (an FE without persistent metadata loses them on a
      // roll). Not awaited.
      if res.is_ok() && engine.is_starrocks() && op == ApplyOp::Update {
          crate::starrocks_users::spawn_user_sync(
              st.clone(),
              engine.id,
              engine.name.clone(),
              engine.namespace.clone(),
          );
      }
  ```

  `crates/egret-bin/src/main.rs`, replace the `if let Some(st) = query_log_state { … }` block with:

  ```rust
      // Every provisioned StarRocks engine gets its Query log set up once at
      // start (an engine created before the Query log existed has none), in the
      // background; each engine logs its own outcome. The StarRocks user sync
      // (package D) starts here too: its first sweep is the boot reconcile.
      if let Some(st) = query_log_state {
          egret_engine_mgmt::spawn_starrocks_user_sync(st.clone());
          tokio::spawn(async move {
              egret_engine_mgmt::setup_query_logs(&st).await;
          });
      }
  ```

- [ ] **Step 3: Run the engine-mgmt suite.**

  ```bash
  env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
    CARGO_TARGET_DIR=$PWD/target-srd cargo test -p egret-engine-mgmt --no-fail-fast
  ```

  Expected: all pass. Engines with no delegation get no FE call from the sync. A delegating test
  engine (`seed_sr_delegating_engine`, jwt mode) now also sees the sync's `SHOW USERS` (and, after
  Task 10, `SHOW GRANTS FOR …`): a test that asserts an exact `queried()` list for one gains those
  entries in its expected list, and nothing else changes.

- [ ] **Step 4: Mutants:** remove checkpoint 7 (the deleted-engine test fails); remove the
  `finish_apply` hook (the apply test fails); move the sync after the query log (the first new
  test's `users_at < nodes_at` fails); make `sync_all_starrocks_users` skip
  engines that are impersonate-mode (the pass test fails).

- [ ] **Step 5: Commit.**

  ```bash
  git add crates/egret-engine-mgmt crates/egret-bin/src/main.rs
  git commit -m "feat(engine-mgmt): run the StarRocks user sync on membership writes, applies, the bootstrap and a sweep

  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
  ```

---

### Task 10: Native access control: per-user IMPERSONATE, `ON ALL USERS` revoked (decided 2026-11-11, 2)

> **Amended by fix round 1 (2026-11-12); these points override the steps below** (spec 2.7 and
> "Decided on 2026-11-12"):
>
> - **Order:** grant each missing per-user grant FIRST, then `REVOKE IMPERSONATE ON ALL USERS`, and
>   only when every grant landed. No read after the revoke (M7a: per-user grants survive it). The
>   unbound test asserts `grant < revoke`; mutant "revoke first" is killed by
>   `a_failed_grant_keeps_all_users` (a failed per-user grant leaves `ON ALL USERS` held).
> - **`DROP USER` keeps the delegator's per-user grant** (measured), and a `REVOKE` for a missing
>   user fails with `cannot find user`. The fake keeps the grant on `DROP USER`. Before each drop the
>   sync revokes the user's listed grant, on bound engines too; one `SHOW GRANTS` read per sync,
>   only when there is something to drop or converge; a user whose grant cannot be read or revoked
>   is not dropped.
> - **Parser:** reads the `Grants` column by header, returns `Result`, and unquotes with
>   `identity_name`, the exact inverse of `quote_string` (`''` and `\\`).
> - **Mode check:** `none`-mode engines are not converged (`a_none_mode_engine_keeps_all_users`).
> - **Orphan grants** on users already gone are a known issue; the sync does not clean them up.

**Files:**
- Modify: `crates/egret-engine-mgmt/src/starrocks_users.rs` (builders, parser,
  `converge_native_impersonation`, its call in `sync_locked`, tests)
- Modify: `crates/egret-engine-mgmt/src/starrocks_ddl.rs` (`FakeStarRocksDdl`: impersonation grants)
- Modify: `crates/egret-engine-mgmt/src/engines.rs`:
  - delete `starrocks_impersonation_grant_sql` (about line 5368), its push in
    `spawn_starrocks_catalog_wire` (about line 5718) and its test
    `starrocks_impersonation_grant_sql_renders_and_escapes` (about line 15548);
  - the comment in `build_starrocks_provision_req` (about line 4271) that says the bootstrap creates
    "the service user + GRANT IMPERSONATE": now "the service user";
  - the existing bootstrap tests that name `GRANT IMPERSONATE` (Step 1);
  - new DB tests.

**Interfaces:**
- Consumes: Task 8 (`sync_locked`, `UserPlan::kept`, `UserSync::Done { granted, revoked_all, .. }`),
  Task 9 (the bootstrap's checkpoint 7), Task 1's M6 and M7.
- Produces:

  ```rust
  pub(crate) fn show_delegator_grants_sql() -> String;
  pub(crate) fn revoke_all_impersonation_sql() -> String;
  /// `None` for a reserved user.
  pub(crate) fn grant_impersonation_sql(name: &str) -> Option<String>;
  /// `(ON ALL USERS granted, users named by ON USER …)`
  pub(crate) fn delegator_impersonation(rows: &[Vec<Option<String>>]) -> (bool, BTreeSet<String>);
  // FakeStarRocksDdl
  pub fn seed_impersonate_all(&self);
  pub fn impersonation(&self) -> (bool, BTreeSet<String>);
  ```

- [ ] **Step 1: The fake learns impersonation grants, and the existing bootstrap tests move.**

  `FakeStarRocksDdl`: fields `impersonate_all: Arc<Mutex<bool>>` and
  `impersonable: Arc<Mutex<BTreeSet<String>>>`, and:

  ```rust
  /// The delegator holds `IMPERSONATE ON ALL USERS`, as every engine
  /// bootstrapped before package D does.
  pub fn seed_impersonate_all(&self) {
      *self.impersonate_all.lock().unwrap() = true;
  }

  /// `(ON ALL USERS, the users named by ON USER …)` the delegator holds.
  pub fn impersonation(&self) -> (bool, BTreeSet<String>) {
      (
          *self.impersonate_all.lock().unwrap(),
          self.impersonable.lock().unwrap().clone(),
      )
  }
  ```

  In `execute`, before the user branches of Task 8 (answers per M7):

  ```rust
          if sql.starts_with("GRANT IMPERSONATE ON ALL USERS") {
              *self.impersonate_all.lock().unwrap() = true;
              return Ok(());
          }
          if sql.starts_with("REVOKE IMPERSONATE ON ALL USERS") {
              // M7: whether per-user grants survive, and what a revoke of
              // nothing answers, replace this behaviour if they differ.
              *self.impersonate_all.lock().unwrap() = false;
              return Ok(());
          }
          if let Some(rest) = sql.strip_prefix("GRANT IMPERSONATE ON USER ") {
              if let Some(name) = rest
                  .split(" TO ")
                  .next()
                  .and_then(|i| crate::query_log::authorized_name(i.trim()))
              {
                  self.impersonable.lock().unwrap().insert(name);
              }
              return Ok(());
          }
  ```

  (Superseded by fix round 1: `DROP USER` keeps the grant, as measured.) In the `DROP USER` branch, after the remove: `self.impersonable.lock().unwrap().remove(&name);`.
  In `query`, beside `SHOW USERS` (rows per M6):

  ```rust
          if sql.starts_with("SHOW GRANTS FOR ") {
              let me = "'egret_delegator'@'%'".to_string();
              let mut rows = Vec::new();
              if *self.impersonate_all.lock().unwrap() {
                  rows.push(vec![
                      Some(me.clone()),
                      None,
                      Some(format!("GRANT IMPERSONATE ON ALL USERS TO USER {me}")),
                  ]);
              }
              let users = self.impersonable.lock().unwrap().clone();
              if !users.is_empty() {
                  let list: Vec<String> = users
                      .iter()
                      .map(|u| format!("'{}'@'%'", u.replace('\'', "''")))
                      .collect();
                  rows.push(vec![
                      Some(me.clone()),
                      None,
                      Some(format!("GRANT IMPERSONATE ON USER {} TO USER {me}", list.join(", "))),
                  ]);
              }
              return Ok(rows);
          }
  ```

  Existing tests in `engines.rs` that name `GRANT IMPERSONATE` (the bootstrap stops sending it):
  - `the_delegation_bootstrap_still_creates_then_repairs` (about line 14169): replace
    `assert!(sent.iter().any(|s| s.contains("GRANT IMPERSONATE")), "{sent:?}");` with
    `assert!(!sent.iter().any(|s| s.contains("ON ALL USERS") && s.starts_with("GRANT")), "{sent:?}");`;
  - `the_bootstrap_stops_before_the_query_log_after_the_delegation_statements` (about line 16270):
    the hook and the premise name `"CREATE USER IF NOT EXISTS 'egret_delegator'"` instead of
    `"GRANT IMPERSONATE"`; add `|| q == crate::starrocks_users::SHOW_USERS_SQL` to the reads that
    must not happen; its doc says "Checkpoints 7 and 8" (the user sync and the query log);
  - `the_delegation_statements_are_sent_while_no_node_is_alive` (about line 16340): drop
    `"GRANT IMPERSONATE"` from the `needed` list and from the hook's condition;
  - Task 9's `a_deleted_engine_gets_no_user_sync_from_the_bootstrap`: the hook condition becomes
    `sql.contains("CREATE USER IF NOT EXISTS 'egret_delegator'")`, now the last delegation statement;
  - `the_remaining_delegation_statements_stop_at_their_own_checkpoint` keeps `"GRANT IMPERSONATE"` in
    its not-sent list: still true.

- [ ] **Step 2: Write the failing tests.** Pure, in `starrocks_users.rs`'s `mod tests`:

  ```rust
  #[test]
  fn the_native_statements_name_the_delegator_and_refuse_reserved_users() {
      assert_eq!(show_delegator_grants_sql(), "SHOW GRANTS FOR 'egret_delegator'@'%'");
      assert_eq!(
          revoke_all_impersonation_sql(),
          "REVOKE IMPERSONATE ON ALL USERS FROM USER 'egret_delegator'@'%'"
      );
      assert_eq!(
          grant_impersonation_sql("o'b").as_deref(),
          Some("GRANT IMPERSONATE ON USER 'o''b'@'%' TO USER 'egret_delegator'@'%'")
      );
      for reserved in ["root", "ROOT", "egret_delegator", "egret_query_log"] {
          assert_eq!(grant_impersonation_sql(reserved), None, "{reserved}");
      }
  }

  #[test]
  fn the_grants_parser_reads_all_users_and_per_user_lists() {
      let row = |g: &str| vec![Some("'egret_delegator'@'%'".to_string()), None, Some(g.to_string())];
      let rows = vec![
          row("GRANT IMPERSONATE ON ALL USERS TO USER 'egret_delegator'@'%'"),
          row("GRANT IMPERSONATE ON USER 'a'@'%', 'o''b'@'%' TO USER 'egret_delegator'@'%'"),
          row("GRANT SELECT ON ALL TABLES IN ALL DATABASES TO USER 'egret_delegator'@'%'"),
      ];
      let (all, users) = delegator_impersonation(&rows);
      assert!(all);
      assert_eq!(users, ["a", "o'b"].iter().map(|s| s.to_string()).collect());
      assert_eq!(delegator_impersonation(&[]), (false, BTreeSet::new()));
  }
  ```

  DB, in `engines.rs`'s `mod tests` (fixtures from Task 8):

  ```rust
  /// Decided on 2026-11-11 (2): on an engine bound to no profile, the sync
  /// (superseded: grants first, then revokes) revokes `ON ALL USERS` first, then grants each user egret manages; a
  /// second sync sends neither.
  #[tokio::test]
  async fn an_unbound_engine_trades_all_users_for_per_user_grants() {
      let Some((st, fake, _prov)) = test_state_with_fake_fe().await else {
          eprintln!("skipping an_unbound_engine_trades_all_users_for_per_user_grants: DATABASE_URL not set");
          return;
      };
      let engine = seed_impersonate_sr_engine(&st).await;
      assert!(engine.authz_service_id.is_none(), "premise: bound to no profile");
      let ws = seed_assigned_workspace(&st, &engine).await;
      let (alice, theirs) = (uname("alice"), uname("theirs"));
      seed_member(&st, ws, &alice, "active").await;
      seed_member(&st, ws, &theirs, "active").await;
      fake.seed_user(&theirs);
      fake.seed_impersonate_all();
      let fe = crate::starrocks_ddl::fe_target(&st, &engine).await.expect("fe");

      let first = crate::starrocks_users::sync_starrocks_users(&st, &engine, &fe).await;

      assert!(
          matches!(first, crate::starrocks_users::UserSync::Done { revoked_all: true, .. }),
          "{first:?}"
      );
      let sent = fake.executed();
      let revoke = sent
          .iter()
          .position(|s| s.starts_with("REVOKE IMPERSONATE ON ALL USERS"))
          .expect("the revoke");
      let grant = sent
          .iter()
          .position(|s| s == &format!(
              "GRANT IMPERSONATE ON USER '{alice}'@'%' TO USER 'egret_delegator'@'%'"
          ))
          .expect("alice's grant");
      assert!(revoke < grant, "{sent:?}");
      let (all, users) = fake.impersonation();
      assert!(!all);
      assert!(users.contains(&alice), "{users:?}");
      assert!(!users.contains(&theirs), "a user egret did not create is never granted");
      assert!(!users.iter().any(|u| egret_core::engine_principals::is_starrocks_reserved_user(u)));

      fake.clear_executed();
      let second = crate::starrocks_users::sync_starrocks_users(&st, &engine, &fe).await;
      assert!(
          matches!(second, crate::starrocks_users::UserSync::Done { granted: 0, revoked_all: false, .. }),
          "{second:?}"
      );
      assert!(!fake.executed().iter().any(|s| s.contains("IMPERSONATE")), "{:?}", fake.executed());
  }

  /// On an engine bound to a profile, Ranger decides: no native grant or
  /// revoke is sent.
  #[tokio::test]
  async fn a_bound_engine_gets_no_native_impersonation_statement() {
      let Some((st, fake, _prov)) = test_state_with_fake_fe().await else {
          eprintln!("skipping a_bound_engine_gets_no_native_impersonation_statement: DATABASE_URL not set");
          return;
      };
      let dsn = egret_test_db::url().await.unwrap();
      let db = egret_metastore::Db::connect(&dsn, 5).await.expect("connect");
      let policy = egret_policy::PolicyStore::new(db.clone());
      egret_policy::seed_starrocks(&policy).await.expect("def");
      let svc = policy
          .create_service(&uname("pkgd_sr"), "starrocks", &serde_json::json!({}))
          .await
          .expect("service");
      let engine = seed_impersonate_sr_engine(&st).await;
      sqlx::query("UPDATE engine_mgmt.engines SET authz_service_id = $2 WHERE id = $1")
          .bind(engine.id)
          .bind(svc.id)
          .execute(st.engine_mgmt.pool())
          .await
          .unwrap();
      let engine = st.engine_mgmt.get_engine(engine.id).await.unwrap().unwrap();
      let ws = seed_assigned_workspace(&st, &engine).await;
      seed_member(&st, ws, &uname("bob"), "active").await;
      fake.seed_impersonate_all();
      let fe = crate::starrocks_ddl::fe_target(&st, &engine).await.expect("fe");

      crate::starrocks_users::sync_starrocks_users(&st, &engine, &fe).await;

      assert!(!fake.executed().iter().any(|s| s.contains("IMPERSONATE")), "{:?}", fake.executed());
      assert!(!fake.queried().iter().any(|q| q.starts_with("SHOW GRANTS")));
  }

  /// A jwt engine (no users to manage) still loses `ON ALL USERS`.
  #[tokio::test]
  async fn a_jwt_engine_loses_all_users_and_gets_no_grant() {
      let Some((st, fake, _prov)) = test_state_with_fake_fe().await else {
          eprintln!("skipping a_jwt_engine_loses_all_users_and_gets_no_grant: DATABASE_URL not set");
          return;
      };
      let engine = seed_sr_delegating_engine(&st, "CERTPEM").await;
      fake.seed_impersonate_all();
      let fe = crate::starrocks_ddl::fe_target(&st, &engine).await.expect("fe");

      let out = crate::starrocks_users::sync_starrocks_users(&st, &engine, &fe).await;

      assert!(
          matches!(out, crate::starrocks_users::UserSync::Done { granted: 0, revoked_all: true, .. }),
          "{out:?}"
      );
      assert_eq!(fake.impersonation(), (false, BTreeSet::new()));
  }

  /// The bootstrap grants no `ON ALL USERS` any more; the sync right after it
  /// grants the members.
  #[tokio::test]
  async fn the_bootstrap_grants_per_user_never_all_users() {
      let Some((st, fake, prov)) = test_state_with_fake_fe().await else {
          eprintln!("skipping the_bootstrap_grants_per_user_never_all_users: DATABASE_URL not set");
          return;
      };
      let engine = seed_impersonate_sr_engine(&st).await;
      prov.set_status(ready_fe_status(&engine.name));
      let ws = seed_assigned_workspace(&st, &engine).await;
      let alice = uname("alice");
      seed_member(&st, ws, &alice, "active").await;

      spawn_starrocks_catalog_wire(
          st.clone(),
          engine.id,
          engine.name.clone(),
          engine.namespace.clone(),
          Some(test_delegation()),
      )
      .await
      .expect("bootstrap task did not panic");

      let sent = fake.executed();
      assert!(
          !sent.iter().any(|s| s.starts_with("GRANT") && s.contains("ON ALL USERS")),
          "{sent:?}"
      );
      assert!(fake.impersonation().1.contains(&alice), "{sent:?}");
  }
  ```

  (Use `std::collections::BTreeSet` in the test module if it is not imported.) Run them; they fail.

  ```bash
  env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
    CARGO_TARGET_DIR=$PWD/target-srd cargo test -p egret-engine-mgmt --lib -- \
    native_statements grants_parser unbound_engine bound_engine jwt_engine_loses bootstrap_grants
  ```

- [ ] **Step 3: Implement.** In `starrocks_users.rs` (import
  `egret_core::engine_principals::DELEGATOR_PRINCIPAL`):

  ```rust
  fn delegator_identity() -> String {
      user_identity(DELEGATOR_PRINCIPAL)
  }

  pub(crate) fn show_delegator_grants_sql() -> String {
      format!("SHOW GRANTS FOR {}", delegator_identity())
  }

  pub(crate) fn revoke_all_impersonation_sql() -> String {
      format!(
          "REVOKE IMPERSONATE ON ALL USERS FROM USER {}",
          delegator_identity()
      )
  }

  /// The delegator may become `name`. `None` for a reserved user: egret never
  /// lets the delegator become root or one of egret's own users.
  pub(crate) fn grant_impersonation_sql(name: &str) -> Option<String> {
      (!is_starrocks_reserved_user(name)).then(|| {
          format!(
              "GRANT IMPERSONATE ON USER {} TO USER {}",
              user_identity(name),
              delegator_identity()
          )
      })
  }

  /// The delegator's IMPERSONATE grants out of `SHOW GRANTS FOR` rows (M6),
  /// read from the last column (`Grants`).
  pub(crate) fn delegator_impersonation(rows: &[Vec<Option<String>>]) -> (bool, BTreeSet<String>) {
      const ON_USER: &str = "IMPERSONATE ON USER ";
      let mut all = false;
      let mut users = BTreeSet::new();
      for grant in rows.iter().filter_map(|r| r.last().cloned().flatten()) {
          if grant.contains("IMPERSONATE ON ALL USERS") {
              all = true;
              continue;
          }
          let Some(at) = grant.find(ON_USER) else {
              continue;
          };
          let list = grant[at + ON_USER.len()..].split(" TO ").next().unwrap_or("");
          for identity in list.split(", ") {
              if let Some(name) = crate::query_log::authorized_name(identity.trim()) {
                  users.insert(name);
              }
          }
      }
      (all, users)
  }

  /// Spec 2.7: on an engine bound to no profile whose bootstrap created the
  /// delegator, the delegator may impersonate exactly the users egret manages
  /// and the FE has. `ON ALL USERS` is revoked first and the grants read again
  /// (M7), so the result is right whether or not the revoke took per-user
  /// grants with it. Returns `(granted, revoked_all)`; failures go to `failed`.
  async fn converge_native_impersonation(
      st: &CpState,
      engine: &Engine,
      fe: &FeTarget,
      managed: &BTreeSet<String>,
      failed: &mut Vec<String>,
  ) -> (usize, bool) {
      if engine.authz_service_id.is_some()
          || !matches!(engine.delegation_mode(), "impersonate" | "jwt")
      {
          return (0, false);
      }
      let none = BTreeMap::new();
      let read = |rows: Result<Vec<Vec<Option<String>>>, crate::FeError>| {
          rows.map(|r| delegator_impersonation(&r))
              .map_err(|e| format!("{DELEGATOR_PRINCIPAL}: grants: {}", e.scrubbed(fe, &none)))
      };
      let mut grants = match read(st.starrocks_ddl.query(fe, &show_delegator_grants_sql()).await) {
          Ok(g) => g,
          Err(e) => {
              failed.push(e);
              return (0, false);
          }
      };
      let mut revoked_all = false;
      if grants.0 {
          if let Err(e) = st
              .starrocks_ddl
              .execute(fe, &revoke_all_impersonation_sql())
              .await
          {
              failed.push(format!(
                  "{DELEGATOR_PRINCIPAL}: revoke on all users: {}",
                  e.scrubbed(fe, &none)
              ));
              return (0, false);
          }
          revoked_all = true;
          grants = match read(st.starrocks_ddl.query(fe, &show_delegator_grants_sql()).await) {
              Ok(g) => g,
              Err(e) => {
                  failed.push(e);
                  return (0, true);
              }
          };
      }
      let mut granted = 0;
      for name in managed.difference(&grants.1) {
          let Some(sql) = grant_impersonation_sql(name) else {
              continue;
          };
          match st.starrocks_ddl.execute(fe, &sql).await {
              Ok(()) => granted += 1,
              Err(e) => failed.push(format!("{name}: grant: {}", e.scrubbed(fe, &none))),
          }
      }
      (granted, revoked_all)
  }
  ```

  In `sync_locked`: collect the created names (`let mut created_names = Vec::new();`, push `name`
  where `created += 1`), and before building `UserSync::Done`:

  ```rust
      let managed: BTreeSet<String> = plan
          .kept
          .iter()
          .chain(created_names.iter())
          .cloned()
          .collect();
      let (granted, revoked_all) =
          converge_native_impersonation(st, engine, fe, &managed, &mut failed).await;
  ```

  and use `granted, revoked_all` in `Done` instead of `0, false`.

  In `engines.rs`: delete `starrocks_impersonation_grant_sql`, the
  `stmts.push(starrocks_impersonation_grant_sql(&d.service_user));` line and the function's test;
  fix the `build_starrocks_provision_req` comment; make the Step 1 test edits.

- [ ] **Step 4: Run the engine-mgmt suite.**

  ```bash
  env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
    CARGO_TARGET_DIR=$PWD/target-srd cargo test -p egret-engine-mgmt --no-fail-fast
  ```

  Expected: all pass, the moved bootstrap tests included.

- [ ] **Step 5: Mutants:**
  - put the `ON ALL USERS` push back in the bootstrap: `the_bootstrap_grants_per_user_never_all_users`
    and `the_delegation_bootstrap_still_creates_then_repairs` fail;
  - drop the `authz_service_id.is_some()` guard: the bound test fails;
  - drop the revoke: the unbound and jwt tests fail;
  - (superseded by fix round 1) grant before the revoke: the unbound test's `revoke < grant` fails;
    now: revoke first: `a_failed_grant_keeps_all_users` and the unbound test's `grant < revoke` fail;
  - drop the reserved check in `grant_impersonation_sql`: the pure test fails;
  - grant `plan.foreign` too: the unbound test's `theirs` assertion fails.

- [ ] **Step 6: Commit.**

  ```bash
  git add crates/egret-engine-mgmt
  git commit -m "fix(engine-mgmt): grant the StarRocks delegator per-user IMPERSONATE, revoke ON ALL USERS

  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
  ```

---

### Task 11: CI parity gate

Run, one at a time, from the worktree root with `CARGO_TARGET_DIR=$PWD/target-srd` and the DB env
for every suite. Record each command, its result and its duration in the report.

- [ ] **Step 1: Format and lint.**

  ```bash
  cargo fmt --all -- --check
  CARGO_TARGET_DIR=$PWD/target-srd cargo clippy --workspace --all-targets -- -D warnings
  ```

- [ ] **Step 2: The crates this package touched.**

  ```bash
  E="env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test CARGO_TARGET_DIR=$PWD/target-srd"
  $E cargo test -p egret-core -p egret-perm-ir -p egret-policy-api -p egret-provisioner --no-fail-fast
  $E cargo test -p egret-engine-mgmt --no-fail-fast
  $E cargo test -p egret-auth --no-fail-fast
  ```

- [ ] **Step 3: The policy suites CI runs.**

  ```bash
  $E cargo test -p egret-perm-compile --features harness --no-fail-fast
  $E EGRET_GOLDEN_MECHANISM=opa cargo test -p egret-perm-compile --features harness --test golden_compile --no-fail-fast
  $E cargo test -p egret-perm-eval --no-fail-fast
  $E cargo test -p egret-policy --no-fail-fast
  $E cargo test -p egret-ranger-eval --no-fail-fast
  ```

- [ ] **Step 4: The Ranger end-to-end gates CI runs** (Docker; they start their own Ranger).

  ```bash
  $E EGRET_TEST_RANGER_E2E=1 cargo test -p egret-policy --test ranger_engine_gate --no-fail-fast
  $E EGRET_TEST_RANGER_E2E=1 cargo test -p egret-ranger-eval --test differential --no-fail-fast -- --nocapture
  $E EGRET_TEST_RANGER_E2E=1 cargo test -p egret-perm-compile --features harness --test opa_differential --no-fail-fast -- --nocapture
  ```

  Expected: everything green. A failure in a file this package did not touch is a hypothesis
  ("pre-existing") until it is reproduced on the base commit `69e789f08` with the same command.

---

### Task 12: Against a real StarRocks (controller)

- [ ] **Step 1: Start the FE.** Your own container, a name you choose:

  ```bash
  docker run -d --name pkgd-sr -p 19330:9030 ghcr.io/egret-data/egret-starrocks-allin1:4.1.7-egret.4
  ```

  Wait until `mysql -h127.0.0.1 -P19330 -uroot -e 'SELECT 1'` answers.

- [ ] **Step 2: Add `crates/egret-engine-mgmt/tests/starrocks_users_live.rs`,** gated on
  `EGRET_TEST_STARROCKS_URL` like `query_log_live.rs` (its `fe_from` and `MysqlDdlRunner` pattern),
  through a `test-util` entry point `egret_engine_mgmt::starrocks_users_live_testing` that exposes
  `sync_starrocks_users`, `UserSync`, `SHOW_USERS_SQL` and `show_delegator_grants_sql` (mirror
  `query_log::live_testing`). With
  the test database for the store:
  - an impersonate engine row with one workspace and two members (`pkgd_live_<suffix>_a`, `_b`),
    and a third name seeded on the FE by the test as root (`CREATE USER … IDENTIFIED BY 'x'`);
  - the first sync: `Done { foreign: [third], revoked_all: true, .. }` with both members created; the
    second: `created: 0, granted: 0, revoked_all: false`;
  - `SHOW USERS` lists both members; logging in as a member with any password fails;
  - remove one member's grant; the next sync drops that user and only that one;
  - **native access control** (the allin1 FE runs it; decided on 2026-11-11, 2): before the first
    sync the test creates `egret_delegator` with a password it chooses and
    `GRANT IMPERSONATE ON ALL USERS TO USER 'egret_delegator'@'%'` (an engine bootstrapped before
    this package); after the sync, `SHOW GRANTS FOR 'egret_delegator'@'%'` has no `ON ALL USERS`
    and names both members, logging in as `egret_delegator` and running
    ``EXECUTE AS `<member>` WITH NO REVERT`` succeeds, and ``EXECUTE AS `root` `` and
    ``EXECUTE AS `<third>` `` (the foreign user) are refused;
  - assertions name the test's own users only: the test database's platform admins are synced too;
  - the test drops what it created at the end (the delegator included).

  Run it with the variable set, and report its duration:

  ```bash
  env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
    EGRET_TEST_STARROCKS_URL=mysql://127.0.0.1:19330 CARGO_TARGET_DIR=$PWD/target-srd \
    cargo test -p egret-engine-mgmt --features test-util --test starrocks_users_live -- --nocapture
  ```

- [ ] **Step 3: The rendered row on the fork's plugin.** Repeat Task 1 step 1 with the body egret
  now renders instead of the hand-written one: print
  `starrocks_delegator_impersonate_policy("e2e", &reserved)` from a scratch test, put that JSON in
  the scratch `policies.py` as a raw dict appended to its policy list, and rerun Task 1 step 2.
  Same expectations.

- [ ] **Step 4: Commit the live test; stop the container.**

  ```bash
  git add crates/egret-engine-mgmt
  git commit -m "test(engine-mgmt): the StarRocks user sync against a real FE

  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
  docker rm -f pkgd-sr
  ```

---

### Task 13: Live check on the kind cluster `egret` (controller)

The existing cluster (`kind-egret`), egret in `egret-system` with OIDC, ingress and Ranger, SPA at
`http://egret.localtest.me`. Record every command and output under
`.superpowers/sdd/2026-11-11-starrocks-delegation-and-lifecycle/task-13/`.

- [ ] **Step 1: Build and load the backend from this branch, proven fresh.**

  ```bash
  find crates -name '*.rs' -newer Cargo.toml -exec touch {} +
  docker build --no-cache-filter builder -f deploy/docker/backend.Dockerfile -t egret-backend:dev .
  docker run --rm --entrypoint sh egret-backend:dev -c 'grep -c egret_starrocks_user_sync /usr/local/bin/egret'
  ```

  Expected: a count above 0 (else the image carries another branch's binary: stop). Then:

  ```bash
  kind load docker-image egret-backend:dev --name egret
  kubectl rollout restart deploy/egret -n egret-system
  kubectl rollout status deploy/egret -n egret-system --timeout=300s
  ```

  No `helm upgrade` (the release carries user-supplied values). Migration 0219 runs at boot: check
  `kubectl logs deploy/egret -n egret-system | grep -i 0219`.

- [ ] **Step 2: Proof 1, a query as `member-sso` with the user auto-created.**
  - Create a StarRocks engine in the UI: default fork image, Ranger on, **Per-user delegation:
    Impersonate**. Do not create any StarRocks user by hand.
  - Assign it to a workspace where `member-sso` is a member.
  - Within a minute of the FE being ready, as root on the FE:
    `kubectl exec -n egret-engines <name>-fe-0 -- mysql -h127.0.0.1 -P9030 -uroot -p"$PW" -e "SHOW USERS"`
    lists `'member-sso'@'%'` (the root password from the engine's `<name>-initpwd-secret`).
  - The profile shows `egret-delegator-impersonate` with the three exclusions (Engine & policy, or
    `GET /ranger/service/plugins/policies/download/<service>`).
  - Sign in as `member-sso`, open SQL Lab on the engine's connection, run
    `SELECT current_user()`. Expected: `'member-sso'@'%'`.
  - As root: `EXECUTE AS root` by the delegator is denied (log in as `egret_delegator` with the
    delegator secret and run `EXECUTE AS \`root\` WITH NO REVERT`).
  - Remove `member-sso` from the workspace; within a minute `SHOW USERS` no longer lists it. Add
    them back.

- [ ] **Step 3: Proof 2, the CN is healthy without the hand patch.**
  - `kubectl get pods -n egret-engines -l app.kubernetes.io/component=cn` for the new engine:
    `Running`, restarts recorded (M4: a restart or two before the initpwd Job wins is expected and
    recorded; a crash loop is a failure).
  - `kubectl get starrocksclusters <name> -n egret-engines -o jsonpath='{.spec.starRocksCnSpec.cnEnvVars}'`
    shows `MYSQL_PWD` from `<name>-initpwd-secret`, put there by egret.
  - `SHOW COMPUTE NODES` as root lists the CN `Alive: true`.
  - Delete the CN pod; it comes back and re-registers without a crash loop.

- [ ] **Step 4: Proof 3, delete leaves no Job pods and no claims.** Before:
  `kubectl get pvc,job,pod -n egret-engines | grep <name>`. Delete the engine in the UI. After two
  minutes, the same command shows nothing for `<name>` (no `<name>-initpwd-*` pod, no
  `fe-meta-<name>-fe-0`).

- [ ] **Step 5: Proof 4, a same-name re-create gets a fresh root (M5).** Create an engine with the
  same name. Once Ready: egret's bootstrap logs no `Access denied for user 'root'`
  (`kubectl logs deploy/egret -n egret-system | grep <name>`), the new `<name>-initpwd-secret`'s
  password logs in as root, and `SHOW USERS` does not list users of the old engine (only `root`,
  `egret_delegator`, `egret_query_log`, and the members if assigned).

- [ ] **Step 6: Native access control on an engine bound to no profile** (decided on 2026-11-11, 2).
  - Create a StarRocks engine in impersonate mode with **no** policy profile (package B's "Not
    governed" choice on create), assign it to the workspace with `member-sso`.
  - As root on its FE: `SHOW GRANTS FOR 'egret_delegator'@'%'` shows no `ON ALL USERS` and names
    `'member-sso'@'%'` (and the platform admins' users); `SHOW USERS` lists the platform admins'
    users too (decided 1).
  - Sign in as `member-sso`, SQL Lab on this engine: `SELECT current_user()` answers
    `'member-sso'@'%'`.
  - As `egret_delegator` (its secret): ``EXECUTE AS `root` WITH NO REVERT`` is refused.
  - If a StarRocks engine created before this image exists on the cluster and is bound to no profile,
    record its `SHOW GRANTS FOR 'egret_delegator'@'%'` before the image and again at most ten minutes
    after (one sweep): `ON ALL USERS` is gone. If none exists, say so in the report.

- [ ] **Step 7: The create refusal** (decided on 2026-11-11, 4). With a name no engine has:

  ```bash
  N=pkgd-leftover
  kubectl apply -n egret-engines -f - <<EOF
  apiVersion: v1
  kind: PersistentVolumeClaim
  metadata: { name: fe-meta-$N-fe-0 }
  spec: { accessModes: [ReadWriteOnce], resources: { requests: { storage: 1Gi } } }
  EOF
  ```

  Create a StarRocks engine named `pkgd-leftover` in the UI: the dialog shows the 409 message with
  `kubectl delete pvc fe-meta-pkgd-leftover-fe-0 -n egret-engines`, and no engine row appears. Run
  that command; the same create then succeeds. Delete the engine.

- [ ] **Step 8: Clean up** the engines this task created. Write the summary (four proofs, native
  access control, the create refusal, M4, M5) into the task report and the PR description.

---

### Task 14: Docs (in `egret-data/docs`)

Branch `docs/starrocks-delegation-and-lifecycle` in a fresh worktree of
`/Users/1pro-l4r8/Documents/github/data-portal/docs`, after the platform PR merges. Find each page
with grep; edit it in place. Every code fence has a language. `npm run build` passes.

- [ ] **Step 1: `platform/deploy-operate/engines-and-ranger.md`** ("The policy service" and
  "Platform admins on the engines"): StarRocks impersonate profiles carry
  `egret-delegator-impersonate` on resource `user`, excluding `root`, `egret_delegator` and
  `egret_query_log`. A new subsection "StarRocks users for impersonation": egret creates
  `'<username>'@'%'` for each active member of each workspace the engine serves, with a password
  nobody knows; drops it when they leave, are disabled or deleted, or the engine is unassigned or
  moved to JWT; never touches a user it did not create (and logs a warning naming it); refuses
  `root` and egret's own users; when (membership writes, every apply, at start, every 10 minutes).
  Platform admins get a user on every impersonate engine assigned to a workspace. On an engine bound
  to no profile, the delegator holds `IMPERSONATE` on exactly those users, never `ON ALL USERS`.
  **Handing over a user egret did not create:** egret leaves it untouched and logs a warning naming
  it; drop it as root, and the next sync creates egret's.
- [ ] **Step 2: `platform/workspace-settings/databases.md`:** the StarRocks text of **Run queries
  as the logged-in user** no longer asks for a hand-configured service user; egret creates the users.
- [ ] **Step 3: `platform/control-plane/query-engines/engine-detail.md`, "Delete":** deleting a
  StarRocks engine destroys its FE metadata (users, catalogs, the root password, the query log);
  table data stays in object storage under the engine's storage path.
- [ ] **Step 4: `platform/deploy-operate/upgrade.md`:** a new section "StarRocks impersonation,
  CN password and engine delete (platform #<PR>)" with the spec's eight upgrade notes, the
  `kubectl get pvc -n egret-engines | grep '^fe-meta-'` command in a `bash` fence.
- [ ] **Step 4b: `platform/reference/rest-api/control-plane-engines.md`:** `POST /engines` (StarRocks)
  can answer 409 `engine_metadata_left_over` with `claim`, and a message carrying the
  `kubectl delete pvc … -n …` command. `platform/control-plane/query-engines/provision.md`: the
  same, as what the create dialog says.
- [ ] **Step 5: `platform/known-issues.md`:** KI-335 gets *Status: fixed in platform #<PR>*. Two
  new rows (numbers above the highest on docs `main` and every open docs PR): impersonate-mode
  StarRocks under Ranger could not run a query (`cannot find user`, then `IMPERSONATE`), and engine
  delete left the initpwd Job pods and the FE metadata claims (a same-name re-create kept the old
  root password); both *fixed in platform #<PR>*.
- [ ] **Step 6: Build and commit.**

  ```bash
  npm run build
  git add platform
  git commit -m "docs(engines): StarRocks users for impersonation, MYSQL_PWD, delete removes FE metadata

  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
  ```

---

## Self-review

- **Spec coverage:**
  - D1b → Tasks 2, 3, 12 (step 3), 13 (step 2).
  - D1a → Tasks 7, 8, 9, 12, 13 (step 2); the ledger decision → Task 7.
  - Decided on 2026-11-11: (1) platform admins → Tasks 7 (the `roles` and platform-grant triggers),
    8, 13 (step 6); (2) native access control → Tasks 1 (M6, M7), 10, 12, 13 (step 6); (3) users
    egret did not create → Tasks 8, 10 (never granted), 14; (4) the create refusal → Tasks 6, 13
    (step 7), 14.
  - D2 → Tasks 4, 13 (step 3). D3 → Tasks 5, 13 (steps 4, 5).
  - M1 to M3, M6, M7 → Task 1; M4, M5 → Task 13. Upgrade notes and docs → Task 14. Golden and CI
    gates → Tasks 3, 11.
- **Names used across tasks:** `STARROCKS_RESERVED_USERS`, `is_starrocks_reserved_user`,
  `InfraObject::AnyUserExcept`, `starrocks_delegator_impersonate_policy`,
  `EngineHttpError::LeftoverFeMetadata`, `leftover_fe_metadata_message`,
  `refuse_leftover_fe_metadata`, the five store methods and `UserSyncLock`, `USER_SYNC_CHANNEL`,
  `SHOW_USERS_SQL`, `UserPlan::kept`, `sync_starrocks_users`,
  `UserSync::Done { granted, revoked_all, .. }`, `spawn_user_sync`, `sync_all_starrocks_users`,
  `spawn_starrocks_user_sync`, `show_delegator_grants_sql`, `revoke_all_impersonation_sql`,
  `grant_impersonation_sql`, `delegator_impersonation`,
  `FakeStarRocksDdl::{seed_user, users, seed_impersonate_all, impersonation}`.
