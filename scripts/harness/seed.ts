/**
 * Rebuild the "ZZ Chart Parity" harness: nine datasets, their metrics, 19 charts, one dashboard.
 *
 *   cd frontend
 *   npx vite-node --config vitest.config.ts ../scripts/harness/seed.ts -- --workspace=demo
 *
 * (`vitest.config.ts` rather than `vite.config.ts`: it is the one that resolves `@/` AND allows
 * reading above `frontend/`, which is where this script lives.)
 *
 * Flags:
 *   --workspace=<slug>   required
 *   --database=<name>    Trino database to build the virtual datasets on; defaults to the
 *                        workspace's only database, and refuses to guess if there are several
 *   --recreate           delete the harness's own datasets, charts and dashboard first.
 *                        DESTRUCTIVE, default off — see "Idempotence" below
 *   --base=<url>         API origin, default http://127.0.0.1:5174
 *   --email= --password= admin credentials, default admin@egret.local / Admin12345!
 *
 * SCOPE: egret's side only. Metabase's own 19 cards and its H2 Sample Database live in a separate
 * store that a reset of egret's Postgres does not touch, and rebuilding them needs Metabase's card
 * API and its own dashboard model. What IS in scope from the Metabase side is the DATA: the nine
 * datasets are `VALUES` literals materialised from Metabase's own query results, so this script
 * needs no live Metabase.
 *
 * IDEMPOTENCE: upsert by name. Running twice does not produce 38 charts, and it does not churn
 * IDs either — an existing chart is updated in place, so links to it and any card referencing it
 * survive. Deleting is available behind `--recreate` and off by default, because the dashboard
 * this rebuilds is one somebody may be looking at. The one thing upsert will not do quietly is
 * repoint a chart at a different dataset: that is a refusal, because it silently changes what
 * every future measurement compares against.
 */
import { Api, HttpError, type CardInput } from "./api";
import { DATASETS, assertMetricsDeclared, assertColumnsMatch } from "./datasets";
import { CHARTS, DASHBOARD, chartParams, metricUses } from "./charts";

interface Options {
  workspace: string;
  database?: string;
  recreate: boolean;
  base: string;
  email: string;
  password: string;
}

function parseArgs(argv: string[]): Options {
  const flags = new Map<string, string>();
  for (const a of argv) {
    if (!a.startsWith("--")) continue;
    const eq = a.indexOf("=");
    if (eq === -1) flags.set(a.slice(2), "true");
    else flags.set(a.slice(2, eq), a.slice(eq + 1));
  }
  const workspace = flags.get("workspace");
  if (!workspace) throw new Error("--workspace=<slug> is required");
  return {
    workspace,
    database: flags.get("database"),
    recreate: flags.get("recreate") === "true",
    base: flags.get("base") ?? "http://127.0.0.1:5174",
    email: flags.get("email") ?? "admin@egret.local",
    password: flags.get("password") ?? "Admin12345!",
  };
}

const log = (msg: string) => console.log(msg);

async function main(): Promise<void> {
  const opts = parseArgs(process.argv.slice(2));

  // Before a single network call. A chart naming an undeclared metric saves with a 200 and only
  // fails at render, so catching it here is the difference between an error message and a
  // half-built workspace nobody notices is wrong until they open the dashboard.
  assertMetricsDeclared(metricUses());
  // Same reasoning for the params themselves: `harnessParams` throws on a config that would
  // produce no settings in its viz's own namespace, and it is better to learn that now.
  for (const c of CHARTS) chartParams(c);
  log(`checked ${CHARTS.length} charts against ${DATASETS.length} datasets`);

  const api = new Api(opts.base, opts.workspace);
  await api.login(opts.email, opts.password); // ONCE. 5 attempts per 5 minutes locks the account.
  log(`logged in to ${opts.base} as ${opts.email}, workspace "${opts.workspace}"`);

  const dbId = await resolveDatabase(api, opts.database);

  if (opts.recreate) await deleteHarness(api);

  const datasetIds = await seedDatasets(api, dbId, opts.recreate);
  const chartIds = await seedCharts(api, datasetIds);
  await seedDashboard(api, chartIds);
  await checkAnchor(api, datasetIds);

  log(`\n${DASHBOARD.name}: ${CHARTS.length} charts on ${DATASETS.length} datasets — done`);
}

async function resolveDatabase(api: Api, wanted?: string): Promise<string> {
  const dbs = await api.listDatabases();
  if (wanted) {
    const hit = dbs.find((d) => d.name === wanted);
    if (!hit) throw new Error(`no database named "${wanted}" — have: ${dbs.map((d) => d.name).join(", ")}`);
    return hit.id;
  }
  if (dbs.length === 0) throw new Error("the workspace has no databases — the datasets need a Trino connection");
  if (dbs.length > 1) {
    throw new Error(
      `the workspace has ${dbs.length} databases (${dbs.map((d) => d.name).join(", ")}); ` +
      `pass --database=<name> rather than letting this guess`,
    );
  }
  log(`database: ${dbs[0].name} (${dbs[0].engine})`);
  return dbs[0].id;
}

/**
 * `--recreate` only. Scoped to the harness's own rows by name — it never touches anything else in
 * the workspace. The DASHBOARD is deliberately not deleted even here: the PUT further down is a
 * full replace, so emptying it is redundant, and a dashboard delete is the one action in this
 * script with no way back.
 */
async function deleteHarness(api: Api): Promise<void> {
  const chartNames = new Set(CHARTS.map((c) => c.name));
  for (const c of await api.listCharts()) {
    if (chartNames.has(c.name)) {
      await api.deleteChart(c.id);
      log(`  --recreate: deleted chart ${c.name}`);
    }
  }
  const dsNames = new Set(DATASETS.map((d) => d.name));
  for (const d of await api.listDatasets()) {
    if (dsNames.has(d.name)) {
      await api.deleteDataset(d.id);
      log(`  --recreate: deleted dataset ${d.name}`);
    }
  }
}

async function seedDatasets(api: Api, dbId: string, recreated: boolean): Promise<Map<string, string>> {
  const existing = new Map((await api.listDatasets()).map((d) => [d.name, d.id]));
  const ids = new Map<string, string>();

  for (const ds of DATASETS) {
    let id = existing.get(ds.name);
    if (id === undefined) {
      id = (await api.createDataset({
        name: ds.name, database_id: dbId, description: ds.description, sql: ds.sql,
      })).id;
      log(`  + dataset ${ds.name}`);
    } else {
      log(`  = dataset ${ds.name} (reused)`);
    }
    ids.set(ds.name, id);

    const detail = await api.getDataset(id);
    // Reusing a dataset whose SQL has drifted would silently change every number on it, which is
    // the single most damaging thing this script could do quietly.
    if (detail.sql !== undefined && detail.sql !== ds.sql) {
      throw new Error(
        `dataset "${ds.name}" already exists with different SQL. Re-run with --recreate to ` +
        `replace it, or rename it — reusing it would change what every chart on it measures.`,
      );
    }
    // `is_temporal` is inferred by the server, and it decides whether monthly grouping fires.
    assertColumnsMatch(ds, (detail.columns ?? []).map((c) => ({
      column_name: c.column_name, data_type: c.data_type, is_temporal: c.is_temporal,
    })));

    const declared = new Set((detail.metrics ?? []).map((m) => m.metric_name));
    for (const m of ds.metrics) {
      if (declared.has(m.metric_name)) continue;
      await api.createMetric(id, m);
      log(`      + metric ${ds.name}.${m.metric_name} = ${m.expression}`);
    }
  }
  if (recreated) log("  (all created fresh)");
  return ids;
}

async function seedCharts(api: Api, datasetIds: Map<string, string>): Promise<Map<string, string>> {
  const existing = new Map((await api.listCharts()).map((c) => [c.name, c]));
  const ids = new Map<string, string>();

  for (const entry of CHARTS) {
    const datasetId = datasetIds.get(entry.dataset);
    if (!datasetId) throw new Error(`${entry.name}: dataset "${entry.dataset}" was not created`);
    const body = {
      dataset_id: datasetId,
      name: entry.name,
      viz_type: entry.viz,
      params: chartParams(entry) as unknown,
    };

    const prior = existing.get(entry.name);
    if (prior) {
      // A chart that moved dataset is not an upsert, it is a different chart wearing the same
      // name. Repointing it would make every future comparison quietly meaningless, so this is
      // the one case the upsert refuses instead of resolving.
      if (prior.dataset_id !== datasetId) {
        throw new Error(
          `chart "${entry.name}" already exists on a different dataset ` +
          `(${prior.dataset_id}, not ${entry.dataset} = ${datasetId}). Refusing to repoint it: ` +
          `re-run with --recreate, or rename whichever chart is not the harness's.`,
        );
      }
      await api.updateChart(prior.id, body);
      ids.set(entry.name, prior.id);
      log(`  ~ ${entry.viz.padEnd(12)} ${entry.name} (updated in place)`);
    } else {
      const created = await api.createChart(body);
      ids.set(entry.name, created.id);
      log(`  + ${entry.viz.padEnd(12)} ${entry.name}`);
    }
  }
  return ids;
}

async function seedDashboard(api: Api, chartIds: Map<string, string>): Promise<void> {
  const cards: CardInput[] = CHARTS.map((e) => ({
    chart_id: chartIds.get(e.name)!,
    card_type: "chart",
    grid_col: e.col,
    grid_row: e.row,
    size_x: e.sizeX,
    size_y: e.sizeY,
    options: {},
  }));

  const found = (await api.listDashboards()).find((d) => d.name === DASHBOARD.name);
  const id = found ? found.id : (await api.createDashboard({
    name: DASHBOARD.name, description: DASHBOARD.description,
  })).id;

  // FULL REPLACE — `name` is required alongside `cards`, and there is no add-a-card endpoint.
  // This is also what makes the layout idempotent: the card list IS the dashboard.
  await api.putDashboard(id, {
    name: DASHBOARD.name,
    description: DASHBOARD.description,
    width: DASHBOARD.width,
    cards,
  });
  log(`  dashboard ${DASHBOARD.name} — ${cards.length} cards — id=${id}`);
}

/**
 * The seed's own acceptance check. Every chart-parity measurement in PRs 01–06 was taken against
 * `MB Monthly`'s first row; if the datasets landed wrong, this is the cheapest place to find out,
 * and it exercises the whole path — Trino, the dataset, the declared metrics — rather than the
 * literals in this repo.
 */
async function checkAnchor(api: Api, datasetIds: Map<string, string>): Promise<void> {
  const id = datasetIds.get("MB Monthly")!;
  const res = await api.runSpec(id, {
    metrics: ["revenue", "orders"], dimensions: ["order_month"], columns: [], filters: [],
    orderby: [{ field: "order_month", desc: false }], rollup_groups: [], row_limit: 1,
    temporal_units: { order_month: "month" },
  });
  const row = res.rows[0];
  const got = JSON.stringify(row);
  const want = JSON.stringify(["2026-01-01", 11094.7, 205]);
  if (got !== want) {
    throw new Error(`anchor check failed: MB Monthly's first row is ${got}, expected ${want}`);
  }
  log(`  anchor ok: 2026-01 → 205 orders / 11,094.7`);
}

main().catch((e) => {
  if (e instanceof HttpError) console.error(`\nAPI error: ${e.message}`);
  else console.error(`\n${e instanceof Error ? e.message : String(e)}`);
  process.exit(1);
});
