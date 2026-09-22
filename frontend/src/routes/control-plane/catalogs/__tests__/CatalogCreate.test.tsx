import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes, useParams } from "react-router-dom";
import { beforeEach, describe, expect, test, vi } from "vitest";

/**
 * The create page posts through the GENERATED registry client for every kind
 * but one, and through the hand-written `createWarehouse` for `egret_warehouse`
 * — the registry refuses that kind outright. Both are mocked, and every test
 * that asserts one of them fired asserts the OTHER did not: a page that called
 * both would otherwise pass each single-sided assertion.
 */
vi.mock("@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry");
vi.mock("@/api/controlPlane");
vi.mock("@/api/hooks");

import * as registry from "@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry";
import * as api from "@/api/controlPlane";
import * as hooks from "@/api/hooks";
import { ApiError } from "@/api/client";
import { TooltipProvider } from "@/components/ui/tooltip";
import { secretsQueryKey, storageQueryKey } from "@/routes/control-plane/ControlPlaneStorage";
import CatalogCreate from "@/routes/control-plane/catalogs/CatalogCreate";
import { catalogsQueryKey } from "@/routes/control-plane/catalogs/Catalogs";
import { KIND_OPTIONS, REGISTRY_KINDS } from "@/routes/control-plane/catalogs/kinds/pickers";
import {
  cleanAdvanced,
  cleanConfig,
  emptyConfigFor,
  submitProblem,
} from "@/routes/control-plane/catalogs/kinds/forms";
import { parseAdvancedIssue } from "@/routes/control-plane/catalogs/kinds/AdvancedEditor";

let createFn: ReturnType<typeof vi.fn>;
let qc: QueryClient;

function CatalogStub() {
  const { id } = useParams();
  return <div>catalog page {id}</div>;
}

function renderPage() {
  qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      {/* `AppShell` provides this app-wide; `FieldLabel`'s help tooltips need it. */}
      <TooltipProvider>
      <MemoryRouter initialEntries={["/control-plane/catalogs/new"]}>
        <Routes>
          <Route path="/control-plane/catalogs/new" element={<CatalogCreate />} />
          <Route path="/control-plane/catalogs" element={<div>catalogs list</div>} />
          <Route path="/control-plane/catalogs/:id" element={<CatalogStub />} />
        </Routes>
      </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

type User = ReturnType<typeof userEvent.setup>;

/** Radix `Select`: the trigger is a combobox associated with its label. */
async function choose(user: User, label: string, option: string) {
  await user.click(screen.getByLabelText(label));
  await user.click(await screen.findByRole("option", { name: option }));
}

/**
 * `fireEvent.change`, not `user.type`: these forms re-render the whole page on
 * every keystroke, and a 20-character URI typed a key at a time took a single
 * test past the 5s timeout. One change event exercises the same `onChange`.
 */
function typeIn(_user: User, label: string, value: string) {
  fireEvent.change(screen.getByLabelText(label), { target: { value } });
}

const create = () => screen.getByRole("button", { name: /Create catalog/ });

/**
 * `pointerEventsCheck: 0`: Radix `Select` sets `pointer-events: none` on the
 * body while a menu is open and clears it asynchronously, so a second select
 * opened in the same tick is otherwise reported as un-clickable — a jsdom
 * artefact of the portal, not a property of the page.
 */
const setup = () => userEvent.setup({ pointerEventsCheck: 0 });

/** The body the page posted through the registry client. */
function postedBody() {
  expect(createFn).toHaveBeenCalledTimes(1);
  return createFn.mock.calls[0][0].data as Record<string, unknown>;
}

beforeEach(() => {
  vi.clearAllMocks();
  createFn = vi.fn().mockResolvedValue({ status: 201, data: { id: "c-new" } });
  vi.mocked(registry.useControlPlaneCatalogRegistryCreateRegistryCatalog).mockReturnValue({
    mutateAsync: createFn,
    isPending: false,
  } as unknown as ReturnType<
    typeof registry.useControlPlaneCatalogRegistryCreateRegistryCatalog
  >);
  vi.mocked(hooks.useMyWorkspaces).mockReturnValue({
    data: [{ id: "ws1", slug: "eng", name: "Engineering" }],
  } as ReturnType<typeof hooks.useMyWorkspaces>);
  vi.mocked(api.listStorage).mockResolvedValue([
    { id: "st-1", name: "lake" },
  ] as unknown as Awaited<ReturnType<typeof api.listStorage>>);
  vi.mocked(api.listSecrets).mockResolvedValue([
    { id: "sec-1", name: "db-pass" },
  ] as unknown as Awaited<ReturnType<typeof api.listSecrets>>);
  vi.mocked(api.createWarehouse).mockResolvedValue({
    id: "wh-1",
    name: "analytics",
  } as unknown as Awaited<ReturnType<typeof api.createWarehouse>>);
});

describe("the kind picker", () => {
  test("offers exactly the kinds the API documents, plus egret's own warehouse", () => {
    // The only machine-readable list the frontend has: the generated model's
    // JSDoc, which orval copies from the OpenAPI description of
    // `CreateRegistryCatalogBody.kind`. A kind added to `CatalogKind::parse`
    // and documented there but missing here is a create form that cannot make
    // it; one listed here and not there is a form whose Create button 400s.
    const src = readFileSync(
      resolve(process.cwd(), "src/api/generated/model/createRegistryCatalogBody.ts"),
      "utf8",
    );
    const doc = /\/\*\* ([a-z_ |]+) \*\/\n\s*kind: string;/.exec(src);
    expect(doc, "the generated model no longer documents the kinds").not.toBeNull();
    const documented = doc![1].split(" | ");
    expect(REGISTRY_KINDS).toEqual(documented);
    expect(KIND_OPTIONS[0].kind).toBe("egret_warehouse");
    expect(KIND_OPTIONS).toHaveLength(documented.length + 1);
  });

  test("choosing jdbc renders the database fields and no iceberg metastore", async () => {
    const user = setup();
    renderPage();
    await choose(user, "Kind", "JDBC");

    expect(screen.getByLabelText("Dialect")).toBeInTheDocument();
    expect(screen.getByLabelText("Host")).toBeInTheDocument();
    expect(screen.getByLabelText("Port")).toBeInTheDocument();
    expect(screen.getByLabelText("User")).toBeInTheDocument();
    expect(screen.getByLabelText("Password secret")).toBeInTheDocument();
    expect(screen.queryByLabelText("Metastore")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Storage connection")).not.toBeInTheDocument();
  });

  test("hudi offers no Glue metastore, which the backend refuses for it", async () => {
    const user = setup();
    renderPage();
    await choose(user, "Kind", "Hudi");
    await user.click(screen.getByLabelText("Metastore"));

    expect(await screen.findByRole("option", { name: "Hive Metastore" })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: "AWS Glue" })).not.toBeInTheDocument();
  });
});

describe("the posted body", () => {
  test("switching kind from jdbc to tpch posts an empty config, not a merged one", async () => {
    const user = setup();
    renderPage();
    await choose(user, "Kind", "JDBC");
    await choose(user, "Dialect", "postgresql");
    typeIn(user, "Host", "db.example");
    await choose(user, "Kind", "TPC-H");
    typeIn(user, "Name", "good_name");
    await user.click(create());

    expect(postedBody()).toEqual({ name: "good_name", kind: "tpch", config: {} });
    expect(await screen.findByText("catalog page c-new")).toBeInTheDocument();
  });

  test("switching the iceberg metastore replaces it: no REST key survives", async () => {
    const user = setup();
    renderPage();
    await choose(user, "Kind", "Iceberg");
    typeIn(user, "REST URI", "https://rest.example");
    typeIn(user, "Warehouse", "wh");
    await choose(user, "Authentication", "OAuth 2");
    await choose(user, "Credential secret", "db-pass");

    await choose(user, "Metastore", "Hive Metastore");
    typeIn(user, "Metastore URI", "thrift://hms.example:9083");
    await choose(user, "Storage connection", "lake");
    typeIn(user, "Name", "lake_iceberg");
    await user.click(create());

    expect(postedBody()).toEqual({
      name: "lake_iceberg",
      kind: "iceberg",
      config: {
        metastore: { type: "hive", uri: "thrift://hms.example:9083" },
        storage_connection_id: "st-1",
      },
    });
  });

  test("switching REST auth back to none leaves no credential behind", async () => {
    const user = setup();
    renderPage();
    await choose(user, "Kind", "Iceberg");
    typeIn(user, "REST URI", "https://rest.example");
    typeIn(user, "Warehouse", "wh");
    await choose(user, "Authentication", "OAuth 2");
    await choose(user, "Credential secret", "db-pass");
    await choose(user, "Authentication", "None");
    await choose(user, "Storage connection", "lake");
    typeIn(user, "Name", "rest_cat");
    await user.click(create());

    // `RestAuth` is `deny_unknown_fields`: a `credential_secret_id` surviving
    // the switch to `none` is a 400 about a field with no control on screen.
    expect((postedBody().config as Record<string, unknown>)).toEqual({
      metastore: {
        type: "rest",
        uri: "https://rest.example",
        warehouse: "wh",
        auth: { type: "none" },
      },
      storage_connection_id: "st-1",
    });
  });

  test("verify_full offers no CA field, which validation refuses outright", async () => {
    const user = setup();
    renderPage();
    await choose(user, "Kind", "JDBC");
    await choose(user, "TLS", "verify_full");

    // `config.rs:524-531`: "a custom CA certificate is not supported yet".
    // The field must not exist — a `ca_secret_id` is the one key that could
    // otherwise survive an `ssl` spread.
    expect(screen.queryByLabelText(/CA/)).not.toBeInTheDocument();
  });

  test("a TLS mode change posts only `mode`, and a mysql dialect drops `database`", async () => {
    const user = setup();
    renderPage();
    await choose(user, "Kind", "JDBC");
    await choose(user, "Dialect", "postgresql");
    typeIn(user, "Host", "db.example");
    typeIn(user, "Database", "sales");
    typeIn(user, "User", "egret");
    await choose(user, "Password secret", "db-pass");
    await choose(user, "TLS", "verify_full");
    await choose(user, "TLS", "disable");

    // MySQL refuses a database outright, so the field goes and so must the value.
    await choose(user, "Dialect", "mysql");
    expect(screen.queryByLabelText("Database")).not.toBeInTheDocument();

    typeIn(user, "Name", "crm");
    await user.click(create());

    expect(postedBody().config).toEqual({
      dialect: "mysql",
      host: "db.example",
      port: 5432,
      user: "egret",
      password_secret_id: "sec-1",
      ssl: { mode: "disable" },
    });
  });

  test("advanced properties post per engine, and nothing at all when empty", async () => {
    const user = setup();
    renderPage();
    await choose(user, "Kind", "TPC-H");
    typeIn(user, "Name", "bench");
    await user.click(screen.getByRole("button", { name: /Advanced properties/ }));
    typeIn(user, "trino property name", "tpch.splits-per-node");
    await user.click(screen.getByRole("button", { name: "Add trino property" }));
    typeIn(user, "tpch.splits-per-node", "8");
    await user.click(create());

    expect(postedBody()).toEqual({
      name: "bench",
      kind: "tpch",
      config: {},
      advanced: { trino: { "tpch.splits-per-node": "8" } },
    });
  });
});

describe("the name rule", () => {
  test("`1bad` blocks Create and shows the rule; `good_name` does not", async () => {
    const user = setup();
    renderPage();
    await choose(user, "Kind", "TPC-H");
    typeIn(user, "Name", "1bad");

    expect(screen.getByText(/\^\[a-z\]\[a-z0-9_\]\{0,62\}\$/)).toBeInTheDocument();
    expect(create()).toBeDisabled();

    typeIn(user, "Name", "good_name");
    expect(screen.queryByText(/\^\[a-z\]\[a-z0-9_\]\{0,62\}\$/)).not.toBeInTheDocument();
    expect(create()).toBeEnabled();
  });
});

describe("the egret-warehouse branch", () => {
  test("posts to createWarehouse and never to the registry", async () => {
    const user = setup();
    renderPage();
    await choose(user, "Kind", "egret warehouse");
    typeIn(user, "Name", "analytics");
    await choose(user, "Workspace", "Engineering");
    await choose(user, "Storage connection", "lake");
    await user.click(create());

    // `mock.calls[0][0]`, not `toHaveBeenCalledWith`: react-query v5 hands a
    // mutation function a second (context) argument of its own.
    expect(vi.mocked(api.createWarehouse).mock.calls[0][0]).toEqual({
      name: "analytics",
      workspaceId: "ws1",
      // `storageConnectionId` going IN; the `Warehouse` coming back spells the
      // same field `storageConnId`. Both are `string`, so only this assertion
      // catches the swap.
      storageConnectionId: "st-1",
    });
    expect(createFn).not.toHaveBeenCalled();
    expect(await screen.findByText("catalogs list")).toBeInTheDocument();
  });

  test("a registry kind posts to the registry and never to createWarehouse", async () => {
    const user = setup();
    renderPage();
    await choose(user, "Kind", "TPC-H");
    typeIn(user, "Name", "bench");
    await user.click(create());

    expect(createFn).toHaveBeenCalledTimes(1);
    expect(api.createWarehouse).not.toHaveBeenCalled();
  });

  test("offers no advanced properties, which the warehouse endpoint cannot take", async () => {
    const user = setup();
    renderPage();
    await choose(user, "Kind", "egret warehouse");

    expect(
      screen.queryByRole("button", { name: /Advanced properties/ }),
    ).not.toBeInTheDocument();
  });
});

describe("when the server refuses", () => {
  test("shows the server's own sentence — `.message`, not `.error`", async () => {
    createFn.mockRejectedValue(
      new ApiError(400, "invalid jdbc configuration: host is required"),
    );
    const user = setup();
    renderPage();
    await choose(user, "Kind", "TPC-H");
    typeIn(user, "Name", "bench");
    await user.click(create());

    expect(
      await screen.findByText("invalid jdbc configuration: host is required"),
    ).toBeInTheDocument();
  });

  test("a refused advanced key is shown against that key, not just at the top", async () => {
    createFn.mockRejectedValue(
      new ApiError(
        400,
        "advanced.trino.connector.name collides with a property the tpch form sets",
      ),
    );
    const user = setup();
    renderPage();
    await choose(user, "Kind", "TPC-H");
    typeIn(user, "Name", "bench");
    await user.click(screen.getByRole("button", { name: /Advanced properties/ }));
    typeIn(user, "trino property name", "connector.name");
    await user.click(screen.getByRole("button", { name: "Add trino property" }));
    await user.click(create());

    const field = await screen.findByLabelText("connector.name");
    expect(field).toHaveAttribute("aria-invalid", "true");
    expect(screen.getAllByText(/collides with a property/)).toHaveLength(2);
  });
});

describe("cache keys", () => {
  test("the create invalidates the list the Catalogs page reads", async () => {
    const user = setup();
    renderPage();
    const spy = vi.spyOn(qc, "invalidateQueries");
    await choose(user, "Kind", "TPC-H");
    typeIn(user, "Name", "bench");
    await user.click(create());

    expect(spy).toHaveBeenCalledWith({ queryKey: catalogsQueryKey });
  });

  test("the pickers read the same storage and secret caches the other pages do", async () => {
    const user = setup();
    renderPage();
    await choose(user, "Kind", "JDBC");
    await user.click(screen.getByLabelText("Password secret"));
    await screen.findByRole("option", { name: "db-pass" });

    expect(qc.getQueryCache().find({ queryKey: [...secretsQueryKey] })).toBeDefined();
    expect(qc.getQueryCache().find({ queryKey: [...storageQueryKey] })).toBeUndefined();

    await choose(user, "Kind", "Hive");
    await user.click(screen.getByLabelText("Storage connection"));
    await screen.findByRole("option", { name: "lake" });
    expect(qc.getQueryCache().find({ queryKey: [...storageQueryKey] })).toBeDefined();
  });
});

describe("emptyConfigFor", () => {
  test("has an arm for every registry kind and throws for anything else", () => {
    for (const k of REGISTRY_KINDS) expect(() => emptyConfigFor(k)).not.toThrow();
    expect(() => emptyConfigFor("egret_warehouse")).toThrow(/no config skeleton/);
    expect(() => emptyConfigFor("nonsense")).toThrow(/no config skeleton/);
  });

  test("seeds iceberg with the storage field its validator requires", () => {
    expect(emptyConfigFor("iceberg")).toEqual({
      metastore: { type: "rest", uri: "", warehouse: "", auth: { type: "none" } },
      storage_connection_id: "",
    });
  });
});

describe("submitProblem", () => {
  test("iceberg needs a storage connection, exactly as validate_config does", () => {
    const base = {
      metastore: { type: "rest", uri: "https://r.example", warehouse: "w", auth: { type: "none" } },
    };
    expect(submitProblem("iceberg", { ...base, storage_connection_id: "" }, {})).toMatch(
      /storage_connection_id is required/,
    );
    expect(submitProblem("iceberg", { ...base, storage_connection_id: "st-1" }, {})).toBeNull();
  });

  test("a glue access key and its secret go together", () => {
    const glue = (m: Record<string, unknown>) =>
      submitProblem("hive", { metastore: { type: "glue", ...m }, storage_connection_id: "s" }, {});
    expect(glue({ region: "eu-west-1", access_key: "AK" })).toMatch(/go together/);
    expect(glue({ region: "eu-west-1", secret_key_secret_id: "sec" })).toMatch(/go together/);
    expect(glue({ region: "eu-west-1" })).toBeNull();
    expect(glue({ region: "eu-west-1", access_key: "AK", secret_key_secret_id: "sec" })).toBeNull();
  });

  test("jdbc needs a database only for the dialects that demand one", () => {
    const j = (over: Record<string, unknown>) =>
      submitProblem(
        "jdbc",
        {
          dialect: "postgresql",
          host: "h",
          port: 5432,
          user: "u",
          password_secret_id: "sec",
          ssl: { mode: "require" },
          ...over,
        },
        {},
      );
    expect(j({})).toMatch(/database is required for postgresql/);
    expect(j({ database: "sales" })).toBeNull();
    expect(j({ dialect: "mysql" })).toBeNull();
    expect(j({ port: 0 })).toMatch(/port must be 1-65535/);
  });

  test("a generic catalog needs its engines' required advanced key", () => {
    const cfg = { engines: ["trino"], secret_properties: { trino: {} } };
    expect(submitProblem("generic", { engines: [] }, {})).toMatch(/engines must name/);
    expect(submitProblem("generic", cfg, {})).toMatch(/advanced\.trino\.connector\.name/);
    expect(submitProblem("generic", cfg, { trino: { "connector.name": "hive" } })).toBeNull();
  });

  test("a secret property may not collide with an advanced key", () => {
    const cfg = { engines: ["trino"], secret_properties: { trino: { password: "sec-1" } } };
    expect(
      submitProblem("generic", cfg, { trino: { "connector.name": "hive", password: "x" } }),
    ).toMatch(/collides/);
  });

  test("elasticsearch pairs its user with a password secret", () => {
    const es = (over: Record<string, unknown>) =>
      submitProblem("elasticsearch", { hosts: ["a"], port: 9200, tls: true, ...over }, {});
    expect(es({})).toBeNull();
    expect(es({ user: "elastic" })).toMatch(/go together/);
    expect(es({ user: "elastic", password_secret_id: "sec" })).toBeNull();
    expect(submitProblem("elasticsearch", { hosts: [], port: 9200 }, {})).toMatch(/at least one/);
  });
});

describe("cleanConfig", () => {
  test("drops blank optionals rather than posting an empty string", () => {
    expect(
      cleanConfig("iceberg", {
        metastore: {
          type: "rest",
          uri: "https://r",
          warehouse: "w",
          auth: { type: "oauth2", credential_secret_id: "sec", scope: "", token_uri: "" },
        },
        storage_connection_id: "st",
      }),
    ).toEqual({
      metastore: {
        type: "rest",
        uri: "https://r",
        warehouse: "w",
        auth: { type: "oauth2", credential_secret_id: "sec" },
      },
      storage_connection_id: "st",
    });
    expect(cleanConfig("hive", { metastore: { type: "glue", region: "r", access_key: "" } })).toEqual(
      { metastore: { type: "glue", region: "r" } },
    );
  });

  test("a generic catalog keeps secret properties only for the engines it names", () => {
    expect(
      cleanConfig("generic", {
        engines: ["trino"],
        secret_properties: { trino: { p: "sec" }, starrocks: { q: "sec" } },
      }),
    ).toEqual({ engines: ["trino"], secret_properties: { trino: { p: "sec" } } });
  });
});

describe("cleanAdvanced", () => {
  test("drops an engine a generic catalog no longer names, and empty maps always", () => {
    expect(
      cleanAdvanced("generic", { engines: ["trino"] }, { trino: { a: "1" }, starrocks: { b: "2" } }),
    ).toEqual({ trino: { a: "1" } });
    expect(cleanAdvanced("jdbc", {}, { trino: {}, starrocks: { b: "2" } })).toEqual({
      starrocks: { b: "2" },
    });
  });
});

describe("parseAdvancedIssue", () => {
  test("reads the engine and key out of the two refusals that name one", () => {
    expect(
      parseAdvancedIssue("advanced.trino.connector.name collides with a property the jdbc form sets"),
    ).toEqual({
      engine: "trino",
      key: "connector.name",
      text: "advanced.trino.connector.name collides with a property the jdbc form sets",
    });
    expect(
      parseAdvancedIssue(
        "advanced.starrocks.password looks like a secret; store it as a secret and reference its id",
      )?.key,
    ).toBe("password");
    expect(parseAdvancedIssue("invalid jdbc configuration: host is required")).toBeNull();
  });
});
