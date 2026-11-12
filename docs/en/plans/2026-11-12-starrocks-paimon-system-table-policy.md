# StarRocks fork patch 0010 — Paimon system tables under base-table policy — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add fork patch `0010` so a user under a row access or column masking policy on a Paimon base table `t` cannot read `t`'s system tables (`t$files`, `t$snapshots`, `t$partitions`, …).

**Architecture:** An overlay in `com.starrocks.authorization.ColumnPrivilege`: in the first table loop (the one patch `0001` extended for `MetadataTable`), add a branch that, for a `PaimonTable` whose name is a Paimon system table, refuses the read when the base table is under a policy — reusing `0001`'s `isRestrictedByPolicy` and `ERR_ACCESS_DENIED_METADATA_TABLE_UNDER_POLICY`. It adds no base-table-`SELECT` check (Paimon system tables are already authorised by name) and does not touch the second loop, so the only behaviour change is the policy refusal.

**Tech Stack:** StarRocks FE (Java, `fe/fe-core`), JUnit 5 + the StarRocks `UtFrameUtils` min-cluster, Apache Paimon `org.apache.paimon.catalog.Identifier`, the fork's `git format-patch`-style `patches/*.patch` build, the `tests/e2e/probe.sh` harness.

## Global Constraints

- The fork repo is `egret-starrocks`. Work against the upstream tree at `work/starrocks` (upstream tag `4.1.7`, commit `b81ff9cc465a5f2e5e53104d87a59b349808abd9`), which `scripts/apply.sh` produces by applying `patches/*.patch` in order. Patch `0010` applies **after** `0001`.
- The spec is `egret-src/docs/en/specs/2026-11-12-starrocks-paimon-system-table-policy-design.md`. On any conflict between this plan and the spec, the spec governs.
- Overlay only: **no** base-table-`SELECT` check, **no** change to name-based authorisation, **no** change to the second loop. Reuse `ErrorCode.ERR_ACCESS_DENIED_METADATA_TABLE_UNDER_POLICY` and the existing `isRestrictedByPolicy`. Do not add a new error code.
- Cover **every** Paimon system table via `Identifier.isSystemTable()`, not only the three the probe reads.
- The egret image bumps `EGRET_PATCH_LEVEL` 4 → 5 (tag `4.1.7-egret.5`). The platform pin bump is a **separate** follow-up, out of scope here.
- Gate: probe check `C14-paimon` reads `ENFORCED` on the egret.5 image and still `LEAK` on the official `4.1.7` image; `tests/e2e/run.sh` stays 15/15.

---

### Task 1: The ColumnPrivilege overlay and its unit test

**Files:**
- Modify: `work/starrocks/fe/fe-core/src/main/java/com/starrocks/authorization/ColumnPrivilege.java`
- Create: `work/starrocks/fe/fe-core/src/test/java/com/starrocks/authorization/ColumnPrivilegePaimonSystemTableTest.java`

**Interfaces:**
- Consumes (already present from patch `0001`, in the same class): `private static boolean isRestrictedByPolicy(ConnectContext, TableName)`; `ErrorCode.ERR_ACCESS_DENIED_METADATA_TABLE_UNDER_POLICY`; `ConnectContext.isMetadataContext()`.
- Produces: `public static void refusePaimonSystemTableUnderPolicy(ConnectContext context, TableName tableName, PaimonTable paimonTable)` and `private static boolean isPaimonSystemTable(PaimonTable table)` on `ColumnPrivilege`.

- [ ] **Step 1: Add the two imports to `ColumnPrivilege.java`**

In the import block (after the existing `import com.starrocks.catalog.InternalCatalog;` and near `import com.starrocks.connector.metadata.MetadataTable;`), add:

```java
import com.starrocks.catalog.PaimonTable;
import org.apache.paimon.catalog.Identifier;
```

- [ ] **Step 2: Add the overlay branch in the first loop**

In `check(...)`, the first `for (Map.Entry<TableName, Table> entry : tableNameTableObj.entrySet())` loop, immediately **after** the existing `MetadataTable` branch:

```java
            if (table instanceof MetadataTable metadataTable) {
                checkMetadataTablePrivilege(context, tableName, metadataTable);
                continue;
            }
```

insert:

```java
            if (table instanceof PaimonTable paimonTable && isPaimonSystemTable(paimonTable)) {
                refusePaimonSystemTableUnderPolicy(context, tableName, paimonTable);
                continue;
            }
```

- [ ] **Step 3: Add the two helper methods**

Place them next to `checkMetadataTablePrivilege` / `isRestrictedByPolicy` (so they share the helper):

```java
    /**
     * A Paimon system table ({@code t$files}, {@code t$snapshots}, {@code t$partitions}, ...) is resolved by the
     * Paimon connector as a {@link PaimonTable} named {@code t$...}, not as a {@link MetadataTable}, so it skips
     * {@link #checkMetadataTablePrivilege}. It is already authorised by its own name by the Authorizer; this adds
     * only the policy refusal patch 0001 gives a native metadata table: a user whom a row access policy or a
     * column masking policy on the base table {@code t} applies to cannot read it, because those policies cannot
     * be applied to the file-level statistics, partition values and paths the system table exposes.
     */
    private static boolean isPaimonSystemTable(PaimonTable table) {
        return new Identifier(table.getCatalogDBName(), table.getCatalogTableName()).isSystemTable();
    }

    public static void refusePaimonSystemTableUnderPolicy(ConnectContext context, TableName tableName,
                                                          PaimonTable paimonTable) {
        // As in checkMetadataTablePrivilege: a collect job runs as root on behalf of an already-checked query.
        if (context.isMetadataContext()) {
            return;
        }
        Identifier identifier = new Identifier(paimonTable.getCatalogDBName(), paimonTable.getCatalogTableName());
        String catalog = tableName.getCatalog() == null ? paimonTable.getCatalogName() : tableName.getCatalog();
        TableName baseTableName = new TableName(catalog, paimonTable.getCatalogDBName(), identifier.getTableName());
        if (isRestrictedByPolicy(context, baseTableName)) {
            throw ErrorReportException.report(ErrorCode.ERR_ACCESS_DENIED_METADATA_TABLE_UNDER_POLICY,
                    tableName.getTbl(), baseTableName.getTbl());
        }
    }
```

(`PaimonTable.getCatalogName()` exists — it is the `catalogName` field's getter, used as the fallback when the `TableName` carries no catalog, exactly as `checkMetadataTablePrivilege` falls back to `table.getCatalogName()`.)

- [ ] **Step 4: Write the unit test**

`ColumnPrivilegePaimonSystemTableTest.java`. It reuses `0001`'s stub-controller shape and a min-cluster base table `t1`, and a tiny `PaimonTable` subclass that only has to answer the two name getters (the method never touches the native table):

```java
// Copyright 2021-present StarRocks, Inc. All rights reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     https://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

package com.starrocks.authorization;

import com.starrocks.catalog.Column;
import com.starrocks.catalog.InternalCatalog;
import com.starrocks.catalog.PaimonTable;
import com.starrocks.catalog.TableName;
import com.starrocks.catalog.UserIdentity;
import com.starrocks.common.ErrorCode;
import com.starrocks.common.ErrorReportException;
import com.starrocks.qe.ConnectContext;
import com.starrocks.sql.analyzer.Authorizer;
import com.starrocks.sql.ast.expression.BoolLiteral;
import com.starrocks.sql.ast.expression.Expr;
import com.starrocks.sql.ast.expression.NullLiteral;
import com.starrocks.utframe.StarRocksAssert;
import com.starrocks.utframe.UtFrameUtils;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.Assertions;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Map;

/**
 * A Paimon system table (t$files, t$snapshots, t$partitions, ...) exposes partition values, file paths and
 * per-file statistics of its base table t. Patch 0010 refuses it to a user whom a row access or column masking
 * policy on t applies to; it adds no base-SELECT requirement (the system table is already name-authorised).
 */
public class ColumnPrivilegePaimonSystemTableTest {
    private static final String CATALOG = InternalCatalog.DEFAULT_INTERNAL_CATALOG_NAME;

    private StubAccessController controller;
    private ConnectContext ctx;

    /** An external access controller whose policy answers each test sets. */
    private static class StubAccessController extends ExternalAccessController {
        Expr rowAccessPolicy = null;
        Map<String, Expr> maskingPolicy = null;
        final List<TableName> tableChecked = new ArrayList<>();

        @Override
        public void checkTableAction(ConnectContext context, TableName tableName, PrivilegeType privilegeType) {
            tableChecked.add(tableName);
        }

        @Override
        public void checkColumnAction(ConnectContext context, TableName tableName, String column,
                                      PrivilegeType privilegeType) {
        }

        @Override
        public Expr getRowAccessPolicy(ConnectContext context, TableName tableName) {
            return rowAccessPolicy;
        }

        @Override
        public Map<String, Expr> getColumnMaskingPolicy(ConnectContext context, TableName tableName,
                                                        List<Column> columns) {
            return maskingPolicy;
        }
    }

    /** A PaimonTable that answers only the two name getters the check reads. */
    private static class FakePaimonSystemTable extends PaimonTable {
        @Override
        public String getCatalogName() {
            return CATALOG;
        }

        @Override
        public String getCatalogDBName() {
            return "db";
        }

        @Override
        public String getCatalogTableName() {
            return "t1$files";
        }
    }

    @BeforeAll
    public static void beforeClass() throws Exception {
        UtFrameUtils.createMinStarRocksCluster();
        ConnectContext root = UtFrameUtils.createDefaultCtx();
        new StarRocksAssert(root).withDatabase("db").useDatabase("db").withTable("CREATE TABLE `t1` (\n" +
                "  `id` bigint NULL,\n" +
                "  `email` varchar(64) NULL\n" +
                ") ENGINE=OLAP\n" +
                "DUPLICATE KEY(`id`)\n" +
                "DISTRIBUTED BY HASH(`id`) BUCKETS 1\n" +
                "PROPERTIES (\"replication_num\" = \"1\");");
    }

    @BeforeEach
    public void setUp() {
        controller = new StubAccessController();
        Authorizer.getInstance().setAccessControl(CATALOG, controller);
        ctx = new ConnectContext();
        ctx.setCurrentUserIdentity(new UserIdentity("frank", "%"));
        ctx.setDatabase("db");
        ctx.setThreadLocalInfo();
    }

    @AfterEach
    public void tearDown() {
        Authorizer.getInstance().setAccessControl(CATALOG, new NativeAccessController());
    }

    private static TableName systemTableName() {
        return new TableName(CATALOG, "db", "t1$files");
    }

    private static ErrorCode errorCodeOf(Runnable check) {
        ErrorReportException e = Assertions.assertThrows(ErrorReportException.class, check::run);
        return e.getErrorCode();
    }

    @Test
    public void testRefusedWhenRowAccessPolicyApplies() {
        controller.rowAccessPolicy = new BoolLiteral(false);
        ErrorCode code = errorCodeOf(() ->
                ColumnPrivilege.refusePaimonSystemTableUnderPolicy(ctx, systemTableName(), new FakePaimonSystemTable()));
        Assertions.assertEquals(ErrorCode.ERR_ACCESS_DENIED_METADATA_TABLE_UNDER_POLICY, code);
    }

    @Test
    public void testRefusedWhenColumnMaskingPolicyApplies() {
        controller.maskingPolicy = Map.of("email", new NullLiteral());
        ErrorCode code = errorCodeOf(() ->
                ColumnPrivilege.refusePaimonSystemTableUnderPolicy(ctx, systemTableName(), new FakePaimonSystemTable()));
        Assertions.assertEquals(ErrorCode.ERR_ACCESS_DENIED_METADATA_TABLE_UNDER_POLICY, code);
    }

    @Test
    public void testAllowedWhenNoPolicy() {
        Assertions.assertDoesNotThrow(() ->
                ColumnPrivilege.refusePaimonSystemTableUnderPolicy(ctx, systemTableName(), new FakePaimonSystemTable()));
    }

    @Test
    public void testEmptyMaskingPolicyDoesNotRefuse() {
        controller.maskingPolicy = Collections.emptyMap();
        Assertions.assertDoesNotThrow(() ->
                ColumnPrivilege.refusePaimonSystemTableUnderPolicy(ctx, systemTableName(), new FakePaimonSystemTable()));
    }

    @Test
    public void testMetadataContextIsNotRefused() {
        controller.rowAccessPolicy = new BoolLiteral(false);
        ctx.setMetadataContext(true);
        Assertions.assertDoesNotThrow(() ->
                ColumnPrivilege.refusePaimonSystemTableUnderPolicy(ctx, systemTableName(), new FakePaimonSystemTable()));
    }
}
```

- [ ] **Step 5: Run the test (must pass)**

Run:

```bash
cd work/starrocks/fe
mvn -q -pl fe-core test -Dtest=ColumnPrivilegePaimonSystemTableTest
```

Expected: 5 tests, all green. If the min-cluster start is slow, that is normal for `UtFrameUtils`.

- [ ] **Step 6: Confirm the base-table assertion is faithful (mutation check)**

Temporarily change `isRestrictedByPolicy(context, baseTableName)` in `refusePaimonSystemTableUnderPolicy` to `false`, re-run the test, and confirm `testRefusedWhenRowAccessPolicyApplies` and `testRefusedWhenColumnMaskingPolicyApplies` FAIL. Restore the line and confirm green again. (This proves the refusal is driven by the policy check, not by something else throwing.)

- [ ] **Step 7: Commit (in the fork repo, recording the work-tree change is not needed — the patch file is generated in Task 2)**

No commit yet: `work/starrocks` is the scratch build tree, not committed. Task 2 turns this change into `patches/0010-*.patch` and commits that.

---

### Task 2: Generate patch 0010, bump the patch level, prove a clean re-apply

**Files:**
- Create: `egret-starrocks/patches/0010-Check-a-Paimon-base-table-policy-for-its-system-table.patch`
- Modify: `egret-starrocks/UPSTREAM` (`EGRET_PATCH_LEVEL=4` → `5`)

**Interfaces:**
- Consumes: the modified `work/starrocks` tree from Task 1 (the `ColumnPrivilege.java` change + the new test file).
- Produces: `patches/0010-*.patch` that `scripts/apply.sh` applies after `0001`–`0009`.

- [ ] **Step 1: Stage the two changed files inside the work tree and build a patch**

From `egret-starrocks`:

```bash
cd work/starrocks
git add fe/fe-core/src/main/java/com/starrocks/authorization/ColumnPrivilege.java \
        fe/fe-core/src/test/java/com/starrocks/authorization/ColumnPrivilegePaimonSystemTableTest.java
git -c user.name=egret -c user.email=noreply@egret.local \
    commit -q -m "Check a Paimon base-table policy for its system tables

A Paimon system table (t\$files, t\$snapshots, t\$partitions, ...) is a PaimonTable, not a
MetadataTable, so patch 0001's refuse-under-policy check never reaches it. Refuse it to a
user whom a row access or column masking policy on the base table applies to, reusing 0001's
isRestrictedByPolicy and ERR_ACCESS_DENIED_METADATA_TABLE_UNDER_POLICY. No base-SELECT check:
Paimon system tables are already authorised by name."
git format-patch -1 --no-signature --zero-commit --stdout > \
    "$OLDPWD/patches/0010-Check-a-Paimon-base-table-policy-for-its-system-table.patch"
cd "$OLDPWD"
```

- [ ] **Step 2: Bump the patch level**

Edit `UPSTREAM`: change `EGRET_PATCH_LEVEL=4` to `EGRET_PATCH_LEVEL=5`.

- [ ] **Step 3: Prove a clean re-apply from scratch**

Reset the work tree and re-apply every patch in order — `0010` must apply without a reject:

```bash
rm -rf work/starrocks
scripts/apply.sh
```

Expected: the output lists `applying 0001-…` through `applying 0010-Check-a-Paimon-base-table-policy-for-its-system-table.patch` and ends `patched tree ready: …/work/starrocks`, exit 0.

- [ ] **Step 4: Re-run the unit test on the freshly applied tree**

```bash
cd work/starrocks/fe && mvn -q -pl fe-core test -Dtest=ColumnPrivilegePaimonSystemTableTest && cd -
```

Expected: 5 tests green (the patch, not a stray edit, carries the change).

- [ ] **Step 5: Commit the patch and the level bump**

```bash
git add patches/0010-Check-a-Paimon-base-table-policy-for-its-system-table.patch UPSTREAM
git commit -m "patch(0010): refuse Paimon system tables under a base-table policy

$(printf 'A Paimon system table bypassed the base table'\''s Ranger row filter / column mask\n(H7, probe check C14-paimon). Add patch 0010 (overlay in ColumnPrivilege) and bump\nEGRET_PATCH_LEVEL to 5.')

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Build egret.5, measure the e2e gate, update the README

**Files:**
- Modify: `egret-starrocks/README.md` (the Patches table and a new e2e result line)

**Interfaces:**
- Consumes: `patches/0010-*.patch` and `EGRET_PATCH_LEVEL=5` from Task 2.
- Produces: the measured `C14-paimon = ENFORCED` result on `4.1.7-egret.5`, recorded in the README.

- [ ] **Step 1: Build the egret.5 FE and allin1 images**

```bash
scripts/build-fe.sh
scripts/build-images.sh
```

Expected: the images tag `4.1.7-egret.5` are built (the build is long; it compiles the FE). Note the local allin1 image ref it prints for the next step.

- [ ] **Step 2: Run the probe's catalog group on the new egret image (the gate)**

```bash
PROBE_SKIP=stream-load,routine-load,in-namespace tests/e2e/probe.sh <the egret.5 allin1 image ref> \
  > /tmp/probe-egret5-cat.txt 2>&1
grep -E 'C14-paimon|ENFORCED [0-9]+  LEAK' /tmp/probe-egret5-cat.txt | tail
```

Expected: `C14-paimon` on all three connectors reads `ENFORCED` (bob and eve DENIED on `orders$files`/`$snapshots`/`$partitions`); the run has 0 `BLIND`, 0 `ERROR`; the `C14-paimon` sight control (alice) still passes (so it is not a blanket refusal). Compared with the pre-patch egret.4 run, catalog `LEAK` drops by one (the single C14-paimon leak is gone).

- [ ] **Step 3: Confirm the official image still leaks (the check detects what the patch fixes)**

```bash
PROBE_SKIP=stream-load,routine-load,in-namespace tests/e2e/probe.sh starrocks/allin1-ubuntu:4.1.7 \
  > /tmp/probe-official-cat.txt 2>&1
grep 'C14-paimon' /tmp/probe-official-cat.txt
```

Expected: `C14-paimon` reads `LEAK` on the official image (no patch) — the evidence the check detects exactly what `0010` fixes.

- [ ] **Step 4: Run the existing security e2e to prove no regression**

```bash
tests/e2e/run.sh <the egret.5 allin1 image ref> > /tmp/run-egret5.txt 2>&1
tail -3 /tmp/run-egret5.txt
```

Expected: 15/15 pass (the overlay refuses only policy-bound users on Paimon system tables; it must not refuse a plain `SELECT` user and must not touch the Iceberg metadata-table path `0001` already guards).

- [ ] **Step 5: Update the README**

In `README.md`'s Patches table, add a row for `0010` (one line, same shape as the other rows, describing the Paimon system-table policy refusal and citing probe check C14-paimon / H7). In the Probe section, add a result line for `4.1.7-egret.5`: `C14-paimon` now `ENFORCED`, so the catalog `LEAK` count drops by one from the egret.4 run.

- [ ] **Step 6: Commit**

```bash
git add README.md
git commit -m "docs(readme): record patch 0010 and its egret.5 e2e result

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Out of scope (separate follow-ups)

- The platform pin bump to `ghcr.io/egret-data/egret-starrocks-{fe,allin1}:4.1.7-egret.5` and pushing/tagging the images to GHCR.
- Closing the probe report's FU-1 row and the C14-paimon known-issue once egret.5 is the default image.
- The Java/Flink-written column-value-statistics variant of the leak (covered by the refusal, unmeasured — needs Spark/Flink, excluded by the probe's open question 4).
