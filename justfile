# Egret dev commands. Run `just --list` to see all.

set dotenv-load := true

# The database every test recipe below runs against, stated here rather than inherited.
#
# `set dotenv-load` above puts `.env` into every recipe's environment, and `.env` (like
# `.env.example`) points `DATABASE_URL` at the DEV database `egret`. A recipe that simply
# ran `cargo test` therefore pointed the whole workspace suite — including `#[sqlx::test]`,
# which resolves the variable through dotenvy unconditionally — at the database the dev
# server boots from. Naming it explicitly on the recipe line overrides the dotenv value.
test_database_url := "postgres://egret:egret@localhost:5432/egret_test"

# Start the local dev stack (Postgres, RustFS, Trino, StarRocks)
up:
    docker compose -f deploy/docker-compose.dev.yml up -d

# Stop the dev stack
down:
    docker compose -f deploy/docker-compose.dev.yml down

# Scrub the DB-skip opt-out from any recipe that is supposed to actually run the tests.
#
# `EGRET_ALLOW_DB_SKIP` makes every DB-gated suite report `ok` having asserted nothing. An
# opt-out that came from `.env` is refused in `egret_test_db` itself (it reads the file, so
# `set dotenv-load` cannot smuggle one in) — but one exported in a shell profile is
# indistinguishable in-process from one the operator typed, and would make `just check`
# report a full green over ~315 empty tests, silently: the skip's `eprintln!` is swallowed
# by libtest because the test PASSES.
#
# So the recipes that mean "everything ran" unset it. `tests/every_test_recipe_refuses_the
# _skip.rs` in egret-test-db fails if a recipe here runs `cargo test` without either this
# prefix or a deliberate `EGRET_ALLOW_DB_SKIP=` of its own.
no_db_skip := "env -u EGRET_ALLOW_DB_SKIP"
# The ENGINE axis, which is a different one. `crates/egret-workflow/tests/sql_starrocks_
# live.rs` is the only instrument `worker::handlers::sql` has — the execution ratchet
# reaches zero corpus entries for it and can never reach one — and it used to self-skip,
# reporting `7 passed; 0 failed; 0 ignored` in 0.00s with no engine and the identical line
# in 0.88s with one. It now FAILS instead, so the recipes that legitimately have no engine
# say so here, in the command, rather than by producing a green nobody can read.
no_engine := "EGRET_ALLOW_ENGINE_SKIP=1"
# The DOCKER axis, and it is a THIRD one for the reason the second exists: a machine may
# honestly have a Postgres and no daemon, or a daemon and no StarRocks.
# `crates/egret-workflow/tests/container_docker_live.rs` is the only instrument
# `worker::handlers::{container,python}` have against the outside world — zero of the 345
# corpus flows declare a script, container, shell or command task type, measured — so it
# FAILS without a daemon rather than self-skipping. The recipes that legitimately have no
# daemon (CI, and a contributor who has not installed one) say so here, in the command.
# `just test-workflow-docker` is the recipe that runs it for real.
no_docker := "EGRET_ALLOW_DOCKER_SKIP=1"

# The STORE axis, a FOURTH one — and the only one of the four that `check` decides by
# RUNNING the suite rather than by opting out of it. SP4d Task 8.
#
# `check` used to decide the DB, ENGINE and DOCKER axes and leave this one undecided, so
# the workspace run ended `0 passed; 14 failed` on `crates/egret-dataapi/tests/serving_
# shared_store.rs` — measured — and the panic's own remedy text tells the reader to export
# `EGRET_ALLOW_STORE_SKIP=1`. From that export onward every `just check` on that machine
# skipped all fourteen with the skip's `eprintln!` swallowed by libtest. A recipe that
# makes its user disable part of itself is worse than one that forgot a gate: the forced
# export is permanent and silent.
#
# So this axis is NOT an `EGRET_ALLOW_*=1` default like the two above it. Those exist
# because an engine or a Docker daemon is expensive and a verdict about SQL handlers is not
# what the contributor is changing. A redis-protocol server is neither: `valkey` is in
# `deploy/docker-compose.dev.yml` beside the Postgres `check` ALREADY requires, so `just
# up` provides both in one command, and `brew install valkey && valkey-server --port 6399`
# provides it with no Docker daemon at all. `check` therefore supplies the URL and SCRUBS
# the opt-out, so the only way to disable those fourteen is to edit this file, where the
# edit shows up in a diff.
#
# Which of the two does the work, because the distinction is load-bearing and MEASURED:
# the URL does. `store_url()` reads the opt-out only when the URL is missing or blank, so
# while `check` names a store the opt-out is unreachable — with `EGRET_ALLOW_STORE_SKIP=1`
# exported, that line still ran all fourteen in 1.06 s. Drop the URL from the same command
# and the identical line reports `14 passed … finished in 0.00s`, which is the shape of the
# green this axis existed to produce. The scrub is insurance against a future edit that
# drops the URL, and it is what makes the guard in `egret-test-db` see the decision.
#
# Note what `check` demands already before judging this harsh: a Postgres on 5432, and a
# JVM + Maven for `test-ranger-gates`. A Valkey is the cheapest of the three.
no_store_skip := "env -u EGRET_ALLOW_STORE_SKIP"
# Stated here rather than inherited, for the same reason `test_database_url` is: `set
# dotenv-load` would otherwise let a `.env` decide which server the tests measure. Override
# it on the command line when your store is elsewhere — `just test_store_url=redis://
# 127.0.0.1:6400 check` — which can only point the suite at a DIFFERENT store, never make
# it vacuous.
test_store_url := "redis://127.0.0.1:6399"

# Format, lint, and test everything
check:
    # Cheap (0.2s) and visible: cargo never prunes deps/, and past ~100k files every test
    # binary takes seconds just to reach main. See deploy/prune-target.sh for the numbers.
    ./deploy/prune-target.sh
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings
    just test-shipping-resolution
    {{ no_db_skip }} {{ no_store_skip }} {{ no_engine }} {{ no_docker }} \
      DATABASE_URL={{ test_database_url }} EGRET_TEST_SHARED_STORE_URL={{ test_store_url }} \
      cargo test --workspace
    # ⚠ LAST, and it is in `check` rather than beside it ON PURPOSE — open item 0b.
    # The line above RUNS both Ranger gates and they SELF-SKIP without the flag,
    # reporting `1 passed … 0.00s` each, so their contribution to that green was
    # zero. The instrument with the best hit rate in the package — seven defects in
    # code that 127 unit tests and three review rounds had passed — was reached by
    # nothing that actually runs: GitHub CI is out of quota on this project, so
    # `just check` IS the pre-merge gate, and the flag appeared nowhere in it.
    # Expensive (~2-3 min) so it runs after everything cheap, and it is a named
    # recipe so a failure is attributable to it rather than lost in a workspace run.
    just test-ranger-gates
    # The reach of the green above, printed where the reader of the green is looking.
    # Three of the four service axes are decided by opting OUT, and a skip reports `ok`
    # having asserted nothing with its `eprintln!` swallowed by libtest — so the only
    # place a human can learn what this run did not cover is here. No counts: a figure
    # in an echo beside a growing suite is a stale number waiting to happen.
    @echo ""
    @echo "── just check: what this green covers, and what it does not"
    @echo "   RAN  database-gated suites        ({{ test_database_url }})"
    @echo "   RAN  shared-store suites          ({{ test_store_url }} — crates/egret-dataapi/tests/serving_shared_store.rs)"
    @echo "   RAN  the two Ranger gates against the real 2.5.0 + 2.8.0 jars"
    @echo "   SKIPPED engine-gated suites       (EGRET_ALLOW_ENGINE_SKIP=1) → just test-workflow-starrocks, test-engine-mgmt-starrocks, test-dataapi-starrocks"
    @echo "   SKIPPED docker-gated suites       (EGRET_ALLOW_DOCKER_SKIP=1) → just test-workflow-docker"

# The two gates whose other side is Apache Ranger ITSELF rather than this repository's
# reading of the jars: the enforcement gate (`crates/egret-policy`) and the differential
# corpus (`crates/egret-ranger-eval`, ~4000 generated cases at plugin versions 2.5.0 and
# 2.8.0 — the two the live Trino and StarRocks plugins run).
#
# **A FAILURE HERE ON A MACHINE WITH NO JVM OR MAVEN IS THE CORRECT OUTCOME**, and it is
# why this is not another `EGRET_ALLOW_*_SKIP` axis. Those exist because a contributor can
# honestly have a Postgres and no StarRocks, and a verdict about SQL handlers is not what
# they are changing. This is different: these two are the only instruments on the branch
# whose other side is the real engine, and somebody about to merge needs to know whether
# they ran. A green `just check` that silently did not run them is the vacuous green item
# 0b is about.
#
# `EGRET_TEST_RANGER_E2E=1` is what makes them real; without it both self-skip to
# `1 passed … 0.00s`. No `-q` anywhere: both assert the per-class surefire summary BY NAME,
# because `mvn` exits 0 having run nothing when a test class is not on the pom's
# allow-list, and `-q` suppresses the very lines they read.
#
# The jars come from Maven Central on first run and live in `~/.m2` after that.
# `tests/ranger/README.md` is the runbook, including how to re-run a single seed.
#
# `{{ no_db_skip }}` on both lines, and the reason is narrower than it first looks —
# recorded here because the commit that added it (7b78b854) claimed more. MEASURED:
# `crates/egret-test-db/tests/every_test_recipe_refuses_the_skip.rs` failed naming these
# two lines, and the rule it enforces is that no recipe may leave EGRET_ALLOW_DB_SKIP
# undecided. NOT measured, and not true: that an exported opt-out made THESE two suites
# vacuous. Neither `ranger_engine_gate.rs` nor `differential.rs` mentions
# `egret_test_db`, `DATABASE_URL`, `sqlx::test` or any `EGRET_ALLOW_*` — they gate on
# `EGRET_TEST_RANGER_E2E` alone, so the opt-out could not have reached them. The scrub is
# the policy holding, not a vacuous green being fixed.
#
# SP4d-3 Task 8: the THIRD line is the migration differential — every hand-written policy the
# grammar accepts, compiled by egret's real compiler, asked of the real 2.5.0 and 2.8.0 engines
# before and after. It lives in the same test binary as the second line, so the second line
# `--skip`s it and the third names it `--exact`, so each runs once and the slow one (minutes,
# not seconds) is its own visible step. If a rename made `--exact` match nothing, cargo would
# print `0 passed` for that line — read the count.
test-ranger-gates:
    @echo "── Ranger gates (real jars, 2.5.0 + 2.8.0). Needs a JVM and Maven; a failure"
    @echo "   here means the differential did NOT run, which is the thing to know."
    {{ no_db_skip }} EGRET_TEST_RANGER_E2E=1 cargo test -p egret-policy --test ranger_engine_gate --no-fail-fast
    {{ no_db_skip }} EGRET_TEST_RANGER_E2E=1 cargo test -p egret-ranger-eval --test differential --no-fail-fast -- --nocapture --skip a_migration_the_grammar_accepts_decides_identically_on_the_real_engine
    {{ no_db_skip }} EGRET_TEST_RANGER_E2E=1 cargo test -p egret-ranger-eval --test differential --no-fail-fast -- --nocapture --exact a_migration_the_grammar_accepts_decides_identically_on_the_real_engine

# The Kestra plugin host's Java tests (`plugin-host/`, W5b), including the pins of its refusal
# messages (`EgretPluginHostTest`: `namespaceKv()`'s since W9c Task 7).
#
# The plugin jars are NOT Maven dependencies: `fetch-plugins.sh` downloads them into
# `plugin-host/target/plugins`, checked against `plugins.lock` (a jar already there is
# re-verified, not downloaded again), and surefire puts them on the tests' classpath. On a
# fresh checkout `mvn verify` alone cannot load a plugin, so the fetch comes first. Needs JDK 25
# (`JAVA_HOME`) and Maven. No GitHub CI job runs it, so run it before merging a change under
# `plugin-host/` or to the plugin handler's messages.
#
# Fetch the plugin jars, then run the plugin host's Java tests.
test-plugin-host:
    plugin-host/fetch-plugins.sh plugin-host/target/plugins
    mvn -f plugin-host/pom.xml verify

# egret-workflow's tests, compiled under the resolution the BINARY ships.
#
# Cargo unifies features across the packages a build SELECTS, so
# `cargo test -p egret-workflow` and `egret-bin` can compile the same crate against
# different dependencies. Measured: `starlark` 0.14.2 turns on
# `serde_json/arbitrary_precision`, `egret-dataapi` depends on starlark, and `egret-bin`
# pulls `egret-dataapi` and `egret-workflow` together — so the feature was ON in
# everything that ships and OFF under `-p egret-workflow`. It changes how serde_json
# hands numbers to a `Visitor` (every float becomes a one-entry map under a private
# sentinel key), so the crate rendered `{"$serde_json::private::Number":"1.0"}` where it
# meant `1.0`: 13 lib tests and both differential-oracle tests were red in the only
# configuration that ships, while the single-crate command their authors ran all day was
# green for five tasks.
#
# `-p egret-bin` is what makes this the SHIPPING resolution rather than some other one.
# `--lib --test oracle` limits it to three test binaries, so beyond the dependency build
# it costs almost nothing. `just check` runs it BEFORE the workspace suite, which would
# also fail — but as a value mismatch three modules from the cause rather than by name.
test-shipping-resolution:
    {{ no_db_skip }} DATABASE_URL={{ test_database_url }} \
      cargo test -p egret-bin -p egret-workflow --lib --test oracle --no-fail-fast

# The workspace suite with NO database, skipping every DB-gated test on purpose.
#
# `EGRET_ALLOW_DB_SKIP` is the only thing that makes a database-less run green: without it
# `egret_test_db::url()` panics rather than letting ~315 tests in `egret-dataapi` alone
# report `ok` having asserted nothing (see docs/en/testing/db-gated-tests.md). Use this
# when you want the compile + the database-free tests and nothing more — and read the
# result knowing what it did not cover.
#
# `env -u DATABASE_URL` is load-bearing, not tidiness. `set dotenv-load` at the top of this
# file puts `.env` into every recipe's environment, and `.env` points DATABASE_URL at the
# DEV database — so without the `-u` this recipe would not skip anything. It would run the
# whole DB-gated suite against the database `just serve` boots from, which is the precise
# accident the rest of this package exists to make impossible.
#
# Caveat: this silences the suites gated on `egret_test_db::url()`. Tests using
# `#[sqlx::test]` never call it — they resolve DATABASE_URL through dotenvy themselves —
# so they are not covered by the opt-out and may still fail here.
# `EGRET_ALLOW_STORE_SKIP=1` for the same reason and with the same caveat: the fourteen
# tests in `crates/egret-dataapi/tests/serving_shared_store.rs` panic without a
# redis-protocol server, and this recipe's whole premise is that there are no services. It
# is set HERE, on the line, rather than left undecided — undecided is what SP4d Task 8 was
# about — and the echo below says so afterwards, because the skip itself reports `ok`.
test-no-db:
    env -u DATABASE_URL EGRET_ALLOW_DB_SKIP=1 EGRET_ALLOW_STORE_SKIP=1 {{ no_engine }} {{ no_docker }} cargo test --workspace
    @echo ""
    @echo "── just test-no-db: this green asserted nothing about any service."
    @echo "   SKIPPED database-gated suites  (EGRET_ALLOW_DB_SKIP=1)"
    @echo "   SKIPPED shared-store suites    (EGRET_ALLOW_STORE_SKIP=1)"
    @echo "   SKIPPED engine-gated suites    (EGRET_ALLOW_ENGINE_SKIP=1)"
    @echo "   SKIPPED docker-gated suites    (EGRET_ALLOW_DOCKER_SKIP=1)"
    @echo "   The compile and the service-free tests ran. For the rest: just check"

# The shared limiter + response cache against a real Redis/Valkey, on its own, with the
# `--nocapture` the workspace run cannot give it.
#
# `just check` runs this suite too — this recipe exists for when it is the thing you are
# changing, because the fourteen cases include two that drive FORTY concurrent callers
# across two independent `SharedStore` handles and you want their output. Prereq: `just up`,
# or any redis-protocol server at {{ test_store_url }}.
#
# No `EGRET_ALLOW_STORE_SKIP` decision on the line and that is not an oversight: supplying
# the URL decides the axis, because `store_url()` consults the opt-out only when the URL is
# missing or blank. `no_db_skip` is still here — nothing in this suite touches Postgres
# today, and "today" is exactly the word that makes a scrub cheaper than a judgement.
test-dataapi-shared-store:
    {{ no_db_skip }} EGRET_TEST_SHARED_STORE_URL={{ test_store_url }} \
      cargo test -p egret-dataapi --test serving_shared_store --no-fail-fast -- --nocapture

# The StarRocks catalog DDL this branch builds, against a real FE — the only
# instrument that judges the property names in `render/starrocks.rs`.
# Opt-in behind `EGRET_TEST_STARROCKS_URL`; CI has no engine.
# Prereq: `just up`.
test-engine-mgmt-starrocks:
    {{ no_db_skip }} DATABASE_URL={{ test_database_url }} EGRET_TEST_STARROCKS_URL=mysql://localhost:9030 \
      cargo test -p egret-engine-mgmt --test starrocks_ddl_live -- --nocapture

# The one Data API test executed by a REAL engine, rather than a fake `QueryExecPort`.
#
# Every other suite in `egret-dataapi` drives `serving_common::FakePort`, so nothing
# engine-side had ever been RUN by a test there and every claim about StarRocks behaviour
# rested on reading the connector. This publishes an endpoint and serves it end to end
# through `egret_query::EngineState`, asserting rows StarRocks computed.
#
# Opt-in behind `EGRET_TEST_STARROCKS_URL`, the same convention `egret-query`'s connector
# suites use — CI has no engine, so this cannot be part of `just check`.
# Prereq: `just up` (StarRocks takes ~30-60s to become healthy).
test-dataapi-starrocks:
    ./deploy/seed-starrocks-dataapi.sh
    {{ no_db_skip }} DATABASE_URL={{ test_database_url }} EGRET_TEST_STARROCKS_URL=mysql://localhost:9030 \
      cargo test -p egret-dataapi --test serving_starrocks_live -- --nocapture

# `egret.sql` end to end against a real StarRocks — the ONLY instrument that handler has.
#
# `worker::handlers::sql` reaches ZERO entries of the Kestra execution corpus (measured;
# `no_corpus_entry_can_exercise_the_egret_sql_handler` derives it on every run), so the
# execution ratchet says nothing about it and a green ratchet must not be read as coverage.
# This publishes nothing — it drives the handler, and the shipping worker, against rows
# StarRocks computed.
#
# Opt-in behind `EGRET_TEST_STARROCKS_URL`, same convention and same reason as above.
# Prereq: `just up` (StarRocks takes ~30-60s to become healthy).
test-workflow-starrocks:
    ./deploy/seed-starrocks-workflow.sh
    {{ no_db_skip }} DATABASE_URL={{ test_database_url }} EGRET_TEST_STARROCKS_URL=mysql://localhost:9030 \
      cargo test -p egret-workflow --test sql_starrocks_live -- --nocapture

# `egret.container` and `egret.script.python` against a real Docker daemon — the ONLY
# instrument either handler has that judges the daemon rather than a fake.
# (`egret.python` is the NOTEBOOK task in `spec::registry::TASK_TYPES`; the container-backed
# script handler was renamed away from that name in W4 Task 5 fix round 1.)
#
# Both reach ZERO entries of the Kestra execution corpus: 345 flows, none of which declares
# a script, container, shell or command task type (measured — see that file's module doc for
# the command). So the execution ratchet says nothing about them in either direction, and a
# green ratchet must not be read as coverage.
#
# No `DATABASE_URL`: nothing here touches Postgres. It needs a daemon and the two images
# pulled — it refuses rather than pulling inside a test, so its duration stays a measurement
# of the code and not of the network.
test-workflow-docker:
    docker pull alpine:3.20
    docker pull python:3.13-slim
    # `{{ no_db_skip }}` even though nothing here touches Postgres: the rule
    # `egret-test-db`'s `no_just_recipe_runs_cargo_test_with_the_skip_inherited`
    # enforces is that a recipe DECIDE what happens to an exported skip rather
    # than inherit one, and "this suite has no DB-gated test today" is a fact
    # about today. It caught this recipe on its first run.
    # `env -u EGRET_ALLOW_DOCKER_SKIP` is fix round 1's reviewer plan item 3, and it is the
    # same rule as `no_db_skip` one axis over: this recipe means "the daemon tests ran", so
    # an opt-out exported in somebody's shell profile must not be able to turn it into a
    # green that ignored all fifteen. The suite reports `15 ignored` rather than
    # `15 passed` when it skips, so the two are distinguishable — but a recipe whose whole
    # point is the daemon should not be leaving that to the reader. (Measured from the
    # binary, not remembered: 16 cases, of which the pure gate test needs no daemon and
    # runs. A count in a comment beside a growing suite is a stale figure waiting to
    # happen — if it disagrees with the suite, the suite is right.)
    {{ no_db_skip }} env -u EGRET_ALLOW_DOCKER_SKIP \
      cargo test -p egret-workflow --test container_docker_live -- --nocapture

# Apply migrations against the running Postgres
migrate:
    cargo run -p egret-bin -- migrate

# Reset local test data (keeps users + demo/eng; wipes catalog, test workspaces, RustFS)
reset-testdata:
    ./deploy/reset-testdata.sh

# Seed a ready-to-test demo catalog (warehouse + role + client + demo_ns.events). Needs a running serve.
seed-demo:
    ./deploy/seed-demo-catalog.sh

# Seed Alerts & Reports demo data (10 schedules covering every UI state) into a workspace.
seed-alerts ws="demo" smtp_host="localhost":
    ./deploy/seed-alerts.sh {{ws}} {{smtp_host}}

# Run the server
serve:
    cargo run -p egret-bin -- serve

# Opt-in Trino-on-k8s e2e harness: kind cluster + target namespace (NOT in CI). See deploy/k8s/README.md
k8s-up:
    kind create cluster --name egret --config deploy/k8s/kind.yaml
    # Trino provisions as plain Deployments egret applies directly — no operator.
    # StarRocks/RisingWave phases will add their own operators here later.
    kubectl create namespace egret-engines

# Tear down the kind e2e cluster
k8s-down:
    kind delete cluster --name egret

# Build all egret images (backend + frontend + apps + renderer).
images-build:
    docker build -f deploy/docker/backend.Dockerfile -t egret-backend:dev .
    docker build -f frontend/Dockerfile -t egret-frontend:dev frontend
    docker build -t egret-apps:dev apps-image
    docker build -t egret-renderer:dev renderer

# Build images, load them into the kind `egret` cluster, and (re)install the chart.
# Force fresh pods onto the just-loaded `:dev` images. A `helm upgrade` with an
# unchanged image tag does NOT restart pods, so a rebuilt `:dev` image is ignored
# without this — the classic same-tag gotcha. NOTE: never run two image builds
# concurrently — both tag `:dev` and the last to finish wins, which can deploy
# stale code.
_kind-refresh:
    kubectl rollout restart deploy/egret deploy/egret-frontend -n egret-system
    kubectl rollout status deploy/egret -n egret-system --timeout=180s

deploy-kind: images-build
    kind load docker-image egret-backend:dev egret-frontend:dev egret-renderer:dev egret-apps:dev --name egret
    helm upgrade --install egret deploy/helm/egret -n egret-system --create-namespace --wait --timeout 10m
    just _kind-refresh
    @echo "egret SPA: http://localhost:30080  (login admin@egret.local / Admin12345!)"

# Like deploy-kind but OIDC-on + ingress (Milestone B) — required for the data-plane e2e.
deploy-kind-oidc: images-build
    kind load docker-image egret-backend:dev egret-frontend:dev egret-apps:dev --name egret
    helm upgrade --install egret deploy/helm/egret -n egret-system --create-namespace --wait --timeout 10m \
      --set oidc.enabled=true --set ingress.enabled=true
    just _kind-refresh
    @echo "egret SPA: http://egret.localtest.me  (login admin@egret.local / Admin12345!)"

# ── Data Apps on kind (P4e) ──────────────────────────────────────────────────────────────
# A SEPARATE release, `egret-verify` in namespace `egret-verify`, on hosts `verify.localtest.me` and
# `apps.verify.localtest.me`, OIDC off, TLS self-signed, with its own Postgres, engine and app-pod
# namespaces and image tag `p4e`. Never a re-deploy of the live `egret` release: that one carries
# four user-supplied values, runs migrations on restart, and serves plain http — and the `__Host-`
# cookies the apps origin depends on exist only over HTTPS.
#
# Re-running this after a code change is safe and is what ROLLS the pods: the tag never changes, so
# Kubernetes sees nothing to roll — hence the explicit `rollout restart`. Check pod AGE, not helm.
apps_host := "verify.localtest.me"

deploy-kind-apps:
    #!/usr/bin/env bash
    set -euo pipefail
    tls=$(mktemp -d); trap 'rm -rf "$tls"' EXIT
    openssl req -x509 -newkey rsa:2048 -nodes -days 7 -keyout "$tls/tls.key" -out "$tls/tls.crt" \
      -subj "/CN={{apps_host}}" -addext "subjectAltName=DNS:{{apps_host}},DNS:apps.{{apps_host}}"
    docker build -f deploy/docker/backend.Dockerfile -t egret-backend:p4e .
    docker build -f frontend/Dockerfile -t egret-frontend:p4e frontend
    docker build -t egret-apps:p4e apps-image
    # The chart renders the renderer Deployment unconditionally: without its image `--wait` below
    # sits on ImagePullBackOff until the timeout and the post-install hooks never run.
    docker build -t egret-renderer:p4e renderer
    kind load docker-image egret-backend:p4e egret-frontend:p4e egret-apps:p4e egret-renderer:p4e --name egret
    helm upgrade --install egret-verify deploy/helm/egret -n egret-verify --create-namespace --wait --timeout 10m \
      --set backend.image.tag=p4e --set frontend.image.tag=p4e --set frontend.nodePort=30081 \
      --set renderer.image.tag=p4e \
      --set ingress.enabled=true --set ingress.host={{apps_host}} --set ingress.tls.enabled=true \
      --set-file ingress.tls.cert="$tls/tls.crt" --set-file ingress.tls.key="$tls/tls.key" \
      --set apps.origin=https://apps.{{apps_host}} --set apps.egretOrigin=https://{{apps_host}} \
      --set apps.pod.launcher=kubernetes --set apps.pod.namespace=egret-verify-apps --set apps.pod.image=egret-apps:p4e \
      --set engineNamespace=egret-verify-engines --set createEngineNamespace=true
    kubectl -n egret-verify rollout restart deploy/egret deploy/egret-frontend
    kubectl -n egret-verify rollout status deploy/egret --timeout=300s
    kubectl -n egret-verify rollout status deploy/egret-frontend --timeout=120s
    echo "egret (apps): https://{{apps_host}}  — login admin@egret.local / Admin12345! (self-signed certificate)"

# The browser half of spec §10 for edit mode, against the release above. The login endpoint allows 5
# attempts per 5 minutes and the file logs in ONCE; re-run no more than a few times in that window.
e2e-kind-apps:
    cd frontend && EGRET_E2E_APPS=1 EGRET_E2E_BASE_URL=https://{{apps_host}} EGRET_E2E_INSECURE_TLS=1 \
      npx playwright test apps-edit-kind

# Remove the verification release and its three namespaces (the live `egret` release is untouched).
undeploy-kind-apps:
    helm uninstall egret-verify -n egret-verify
    kubectl delete namespace egret-verify egret-verify-apps egret-verify-engines --ignore-not-found

# Opt-in data-plane e2e against the in-cluster kind deploy (egret.localtest.me).
# One-time prereq: just k8s-up && just ingress-install && just coredns-rewrite
e2e-kind: deploy-kind-oidc
    ./deploy/k8s/wait-ready.sh
    cd frontend && EGRET_E2E_KIND=1 EGRET_E2E_BASE_URL=http://egret.localtest.me \
      npx playwright test dataplane-kind

# Like deploy-kind-oidc but ALSO Ranger-on — provisioned Trino engines bind Ranger
# (egret embeds the Ranger admin at /service). For the governance enforcement e2e.
deploy-kind-governance: images-build
    kind load docker-image egret-backend:dev egret-frontend:dev egret-apps:dev --name egret
    helm upgrade --install egret deploy/helm/egret -n egret-system --create-namespace --wait --timeout 10m \
      --set oidc.enabled=true --set ingress.enabled=true --set ranger.enabled=true
    just _kind-refresh
    @echo "egret (ranger-on): http://egret.localtest.me"

# Opt-in governance enforcement e2e (mask + row-filter on a live Trino query).
e2e-kind-governance: deploy-kind-governance
    ./deploy/k8s/wait-ready.sh
    cd frontend && EGRET_E2E_KIND=1 EGRET_E2E_BASE_URL=http://egret.localtest.me \
      EGRET_E2E_BUCKET=e2e \
      EGRET_E2E_S3_ENDPOINT=http://rustfs.egret-system.svc.cluster.local:9000 \
      npx playwright test governance-kind

# Opt-in auto-admin e2e: admin queries a governed engine with NO hand-seeded policy
# (proves the backend auto-seeds the egret-admin all-access Ranger policy on provision).
e2e-kind-ranger-auto: deploy-kind-governance
    ./deploy/k8s/wait-ready.sh
    cd frontend && EGRET_E2E_KIND=1 EGRET_E2E_BASE_URL=http://egret.localtest.me \
      EGRET_E2E_BUCKET=e2e \
      EGRET_E2E_S3_ENDPOINT=http://rustfs.egret-system.svc.cluster.local:9000 \
      npx playwright test ranger-auto-admin-kind

# Opt-in per-user-enforcement e2e: drives two OIDC identities (admin-sso full vs
# member-sso masked+row-filtered) against the same table via SQL Lab, dataset
# preview, chart data, and dashboard tile, proving per-user Ranger enforcement
# is consistent across all four surfaces.
e2e-kind-per-user: deploy-kind-governance
    ./deploy/k8s/wait-ready.sh
    cd frontend && EGRET_E2E_KIND=1 EGRET_E2E_BASE_URL=http://egret.localtest.me \
      EGRET_E2E_BUCKET=e2e \
      EGRET_E2E_S3_ENDPOINT=http://rustfs.egret-system.svc.cluster.local:9000 \
      npx playwright test per-user-enforcement-kind

# Opt-in STS token-broker e2e: a break-glass local admin (NO Keycloak refresh token)
# queries a governed StarRocks engine via SQL Lab purely off an egret-minted
# token, and admin-sso/member-sso Ranger enforcement (full vs masked) still holds
# under that same egret-minted-token path.
e2e-kind-sts: deploy-kind-governance
    ./deploy/k8s/wait-ready.sh
    cd frontend && EGRET_E2E_KIND=1 EGRET_E2E_BASE_URL=http://egret.localtest.me \
      EGRET_E2E_BUCKET=e2e \
      EGRET_E2E_S3_ENDPOINT=http://rustfs.egret-system.svc.cluster.local:9000 \
      npx playwright test sts-broker-kind

# Opt-in: default provision (no delegationMode) => both engines come up jwt-delegated.
e2e-kind-delegation-default: deploy-kind-governance
    ./deploy/k8s/wait-ready.sh
    cd frontend && EGRET_E2E_KIND=1 EGRET_E2E_BASE_URL=http://egret.localtest.me \
      EGRET_E2E_BUCKET=e2e \
      EGRET_E2E_S3_ENDPOINT=http://rustfs.egret-system.svc.cluster.local:9000 \
      npx playwright test delegation-default-kind

# Opt-in local user-management e2e: admin creates a local user, the user logs in and
# holds a live session, then admin disables them — proving the disabled user's live
# session is revoked IMMEDIATELY (not just blocked on their next login). Also covers
# password-reset session invalidation and hard-delete.
e2e-kind-local-user: deploy-kind-governance
    ./deploy/k8s/wait-ready.sh
    cd frontend && EGRET_E2E_KIND=1 EGRET_E2E_BASE_URL=http://egret.localtest.me \
      EGRET_E2E_BUCKET=e2e \
      EGRET_E2E_S3_ENDPOINT=http://rustfs.egret-system.svc.cluster.local:9000 \
      npx playwright test local-user-mgmt-kind

# Opt-in back-channel logout e2e: Keycloak force-logout → egret session revoked.
# Port-forwards Keycloak to loopback so the admin API (used to wire the client +
# force-logout) is reachable over HTTP — Keycloak's sslRequired=external rejects
# admin-cli over plain HTTP from non-loopback but exempts 127.0.0.1 (weakens no config).
e2e-kind-backchannel-logout: deploy-kind-governance
    #!/usr/bin/env bash
    set -euo pipefail
    ./deploy/k8s/wait-ready.sh
    kubectl port-forward -n egret-system svc/keycloak 18080:8080 >/tmp/egret-kc-pf.log 2>&1 &
    PF=$!
    trap 'kill $PF 2>/dev/null || true' EXIT
    sleep 3
    cd frontend && EGRET_E2E_KIND=1 EGRET_E2E_BASE_URL=http://egret.localtest.me \
      EGRET_E2E_KC_ADMIN_URL=http://127.0.0.1:18080 \
      EGRET_E2E_BUCKET=e2e EGRET_E2E_S3_ENDPOINT=http://rustfs.egret-system.svc.cluster.local:9000 \
      npx playwright test backchannel-logout-kind

# Uninstall the egret release (leaves the kind cluster + operators up).
kind-undeploy:
    helm uninstall egret -n egret-system || true

# Install the kind-flavored ingress-nginx controller (needs a cluster created with the ingress-ready label + :80 map).
ingress-install:
    kubectl apply -f https://raw.githubusercontent.com/kubernetes/ingress-nginx/main/deploy/static/provider/kind/deploy.yaml
    kubectl wait --namespace ingress-nginx --for=condition=ready pod --selector=app.kubernetes.io/component=controller --timeout=600s

# Make egret.localtest.me resolve cluster-wide to the ingress controller (so in-cluster
# pods — incl. provisioned engines fetching the OIDC JWKS — reach it). Run AFTER ingress-install.
coredns-rewrite:
    ./deploy/k8s/coredns-rewrite.sh

# Cap cargo's deps/ directory before its size starts costing 30+ seconds per test binary.
# See deploy/prune-target.sh for the measurement that motivated it.
prune-target:
    ./deploy/prune-target.sh

# apps-image: unit/contract tests of egret_marimo (own venv, Python 3.12).
apps-image-test:
    cd apps-image && python3.12 -m venv .venv && .venv/bin/pip install -q -r requirements.txt -r requirements-dev.txt -c constraints.txt -e . && .venv/bin/pytest -q
