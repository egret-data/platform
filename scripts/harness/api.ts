/**
 * A thin egret REST client for the "ZZ Chart Parity" harness seed.
 *
 * Deliberately small: no retries, no pagination, no generated types. It exists so the seed
 * script reads as a list of intentions rather than a list of `fetch` calls, and so the three
 * API-level traps no descriptor can know about live in one place:
 *
 *   1. LOGIN IS RATE-LIMITED, 5 attempts per 5 minutes. A client that logs in per request
 *      locks the admin account out halfway through a 19-chart seed and leaves the workspace
 *      half-built. `login()` therefore refuses to run twice on the same instance.
 *   2. Every mutating request needs the `X-Egret-CSRF` header echoing the `egret_csrf`
 *      cookie. Node's `fetch` does not keep a cookie jar, so this class is the jar.
 *   3. Dataset creation takes `source: {kind: "virtual", sql}`. `kind: "sql"` is rejected 422.
 *
 * House precedent: `scripts/seed_ecommerce_clone.py`, whose `Api` class this mirrors. What it
 * does NOT mirror is that script's hand-written chart `params` — see `params.ts` for why.
 */

/** Column as the server reports it. `is_temporal` is what drives the monthly-grouping default. */
export interface ApiColumn {
  column_name: string;
  data_type: string;
  is_temporal: boolean;
}

export interface ApiMetric {
  metric_name: string;
  expression: string;
}

export interface ApiDataset {
  id: string;
  name: string;
  columns?: ApiColumn[];
  metrics?: ApiMetric[];
}

export interface ApiChart {
  id: string;
  name: string;
  viz_type: string;
}

export interface ApiDashboard {
  id: string;
  name: string;
}

/** A dashboard card, in the shape the full-replace PUT wants. */
export interface CardInput {
  chart_id?: string;
  card_type: string;
  grid_col: number;
  grid_row: number;
  size_x: number;
  size_y: number;
  options?: Record<string, unknown>;
}

export class HttpError extends Error {
  constructor(readonly status: number, readonly method: string, readonly path: string, readonly body: string) {
    super(`${method} ${path} → ${status}: ${body.slice(0, 500)}`);
    this.name = "HttpError";
  }
}

export class Api {
  private cookies = new Map<string, string>();
  private loggedIn = false;

  constructor(private readonly baseUrl: string, private readonly ws: string) {}

  /** The workspace every `ws*` helper below is scoped to. */
  get workspace(): string {
    return this.ws;
  }

  private async req<T>(method: string, path: string, body?: unknown): Promise<T> {
    const headers: Record<string, string> = { "Content-Type": "application/json" };
    if (this.cookies.size > 0) {
      headers["Cookie"] = [...this.cookies].map(([k, v]) => `${k}=${v}`).join("; ");
    }
    // The CSRF token is the cookie's own value echoed in a header — the double-submit pattern.
    const csrf = this.cookies.get("egret_csrf");
    if (csrf) headers["X-Egret-CSRF"] = csrf;

    const res = await fetch(this.baseUrl + path, {
      method,
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
      redirect: "manual",
    });

    // `getSetCookie` keeps the individual Set-Cookie headers apart; a plain `get("set-cookie")`
    // joins them with ", " and a cookie's Expires attribute contains a comma.
    for (const raw of res.headers.getSetCookie?.() ?? []) {
      const [pair] = raw.split(";");
      const eq = pair.indexOf("=");
      if (eq > 0) this.cookies.set(pair.slice(0, eq).trim(), pair.slice(eq + 1).trim());
    }

    const text = await res.text();
    if (!res.ok) throw new HttpError(res.status, method, path, text);
    return (text ? JSON.parse(text) : {}) as T;
  }

  /**
   * Authenticate once. Calling twice is a bug, not a retry: five of these in five minutes
   * locks the account out, so the failure is made loud here rather than at attempt six.
   */
  async login(email: string, password: string): Promise<void> {
    if (this.loggedIn) {
      throw new Error(
        "Api.login called twice — egret rate-limits login to 5 attempts per 5 minutes. " +
        "Reuse the same Api instance for the whole seed.",
      );
    }
    await this.req("POST", "/api/auth/login", { email, password });
    this.loggedIn = true;
  }

  // --- workspace-scoped resources -------------------------------------------------------

  private wsPath(suffix: string): string {
    return `/api/ws/${encodeURIComponent(this.ws)}${suffix}`;
  }

  /** The API returns a bare array for these collections; tolerate an `{items}` envelope too. */
  private static asList<T>(v: T[] | { items: T[] }): T[] {
    return Array.isArray(v) ? v : (v.items ?? []);
  }

  async listDatabases(): Promise<{ id: string; name: string; engine: string }[]> {
    return Api.asList(await this.req(`GET`, this.wsPath("/databases")));
  }

  async listDatasets(): Promise<ApiDataset[]> {
    return Api.asList(await this.req(`GET`, this.wsPath("/datasets")));
  }

  /** Full detail: `list` omits `sql`, `columns` and `metrics`. */
  async getDataset(id: string): Promise<ApiDataset & { sql?: string }> {
    return this.req("GET", this.wsPath(`/datasets/${id}`));
  }

  /**
   * `kind: "virtual"` — the trap. `kind: "sql"` reads more naturally and is rejected 422,
   * which the seed only discovers after it has already created the workspace's other rows.
   */
  async createDataset(input: {
    name: string;
    database_id: string;
    description?: string;
    sql: string;
  }): Promise<ApiDataset> {
    const { sql, ...rest } = input;
    return this.req("POST", this.wsPath("/datasets"), { ...rest, source: { kind: "virtual", sql } });
  }

  async deleteDataset(id: string): Promise<void> {
    await this.req("DELETE", this.wsPath(`/datasets/${id}`));
  }

  /**
   * Declare a metric. A chart may only name metrics declared this way — a chart whose spec
   * names a bare column comes back `unknown metric` at render time, not at save time, so
   * nothing catches it until someone opens the dashboard.
   */
  async createMetric(datasetId: string, metric: ApiMetric): Promise<void> {
    await this.req("POST", this.wsPath(`/datasets/${datasetId}/metrics`), metric);
  }

  async listCharts(): Promise<ApiChart[]> {
    return Api.asList(await this.req(`GET`, this.wsPath("/charts")));
  }

  async createChart(input: {
    dataset_id: string;
    name: string;
    description?: string;
    viz_type: string;
    params: unknown;
  }): Promise<ApiChart> {
    return this.req("POST", this.wsPath("/charts"), input);
  }

  async updateChart(id: string, input: { name: string; viz_type: string; params: unknown }): Promise<void> {
    await this.req("PUT", this.wsPath(`/charts/${id}`), input);
  }

  async deleteChart(id: string): Promise<void> {
    await this.req("DELETE", this.wsPath(`/charts/${id}`));
  }

  async listDashboards(): Promise<ApiDashboard[]> {
    return Api.asList(await this.req(`GET`, this.wsPath("/dashboards")));
  }

  async createDashboard(input: { name: string; description?: string }): Promise<ApiDashboard> {
    return this.req("POST", this.wsPath("/dashboards"), input);
  }

  /**
   * FULL REPLACE. Omitting `name` is a 422 and omitting `cards` empties the dashboard — this
   * is not a PATCH, and there is no endpoint that adds a single card.
   */
  async putDashboard(id: string, input: {
    name: string;
    description?: string;
    width?: string;
    cards: CardInput[];
  }): Promise<void> {
    await this.req("PUT", this.wsPath(`/dashboards/${id}`), { parameters: [], tabs: [], ...input });
  }

  /** Run a query spec against a dataset — used by the seed's post-hoc data check. */
  async runSpec(datasetId: string, spec: unknown): Promise<{ columns: { name: string }[]; rows: unknown[][] }> {
    return this.req("POST", this.wsPath("/charts/run"), { dataset_id: datasetId, query_spec: spec });
  }
}
