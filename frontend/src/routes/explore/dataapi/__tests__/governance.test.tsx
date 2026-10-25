import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useEffect, useRef } from "react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClientProvider, QueryClient } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";

import type { DatabaseConn } from "@/api/databases";
import {
  GOVERNED_BLOCKED_MESSAGE,
  governedWithoutPerPrincipal,
  runsPerPrincipal,
} from "@/routes/explore/dataapi/governance";

/**
 * Package 1's authoring-side warning.
 *
 * The server refuses the publish (`check_governed_connection`) and refuses the
 * request (`governance_allows_serving`); this file is about the third thing,
 * which is that an author should not have to WRITE the endpoint to find out.
 * Nothing here gates anything — a client-side gate is a gate that can be
 * skipped — so what is asserted is only that the sentence appears exactly when
 * the server would refuse, and disappears when it would not.
 */

function conn(over: Partial<DatabaseConn> = {}): DatabaseConn {
  return {
    id: "db-x",
    name: "warehouse",
    engine: "trino",
    host: "h",
    port: 8080,
    options: {},
    has_secret: false,
    status: "ok",
    delegated: false,
    governed: false,
    engine_id: null,
    ...over,
  } as unknown as DatabaseConn;
}

describe("governedWithoutPerPrincipal", () => {
  /**
   * All eight corners, mirroring the Rust test of the same rule. The mutations
   * this predicate can suffer are widenings and narrowings of one expression,
   * and each moves a different row: dropping the `runsPerPrincipal` half turns
   * three permitted rows into warnings, dropping `governed` turns three more,
   * and `false &&` silences the one that matters. A single-case assertion
   * survives two of those.
   */
  it("warns on exactly one of the eight corners", () => {
    for (const governed of [false, true]) {
      for (const delegated of [false, true]) {
        for (const impersonate of [false, true]) {
          const db = conn({ governed, delegated, options: { impersonate } });
          expect(governedWithoutPerPrincipal(db)).toBe(governed && !delegated && !impersonate);
        }
      }
    }
  });

  /**
   * `options.impersonate` alone makes the engine run every query as the caller,
   * independently of `delegated`. A frontend copy of the rule that read
   * `delegated` alone would warn about a connection that publishes perfectly
   * well — and would send the author to change a setting that is already right.
   */
  it("counts impersonation on its own", () => {
    expect(runsPerPrincipal(conn({ options: { impersonate: true } }))).toBe(true);
    expect(governedWithoutPerPrincipal(conn({ governed: true, options: { impersonate: true } }))).toBe(
      false,
    );
  });

  /** A database the page has not loaded is not something to warn about. */
  it("says nothing about an unknown connection", () => {
    expect(governedWithoutPerPrincipal(undefined)).toBe(false);
  });
});

// ── The warning, on the create page ──────────────────────────────────────────

const monacoStub = vi.hoisted(() => ({ reset() {} }));

vi.mock("@/components/monaco/MonacoEditor", () => ({
  default: function MonacoStub({
    value,
    onChange,
    options,
  }: {
    value: string;
    onChange?: (v?: string) => void;
    options?: { ariaLabel?: string };
  }) {
    const latest = useRef(value);
    latest.current = value;
    useEffect(() => {}, []);
    return (
      <textarea
        aria-label={options?.ariaLabel}
        value={value}
        onChange={(e) => onChange?.(e.target.value)}
      />
    );
  },
}));

vi.mock("@/api/dataApi");
vi.mock("@/api/databases");

import * as dataApi from "@/api/dataApi";
import * as databasesApi from "@/api/databases";
import EndpointCreate from "@/routes/explore/dataapi/EndpointCreate";

vi.mock("react-router-dom", async (orig) => ({
  ...(await orig<typeof import("react-router-dom")>()),
  useNavigate: () => vi.fn(),
}));

function renderCreate() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <TooltipProvider>
        <MemoryRouter initialEntries={["/explore/ws1/data-api/endpoints/new"]}>
          <Routes>
            <Route path="/explore/:workspace/data-api/endpoints/new" element={<EndpointCreate />} />
          </Routes>
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

describe("EndpointCreate — the governed-database warning", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    monacoStub.reset();
    // Three connections that differ in ONE field each, so the assertions below
    // are about that field and not about which row happened to be picked.
    vi.mocked(databasesApi.useDatabases).mockReturnValue({
      data: [
        conn({ id: "db-plain", name: "plain" }),
        conn({ id: "db-blocked", name: "blocked", governed: true }),
        conn({ id: "db-ok", name: "delegated", governed: true, delegated: true }),
      ],
    } as never);
    vi.mocked(dataApi.useCreateEndpoint).mockReturnValue({
      mutateAsync: vi.fn(),
      isPending: false,
    } as never);
  });

  async function pick(name: string) {
    await userEvent.click(screen.getByRole("combobox", { name: "Connection" }));
    await userEvent.click(await screen.findByRole("option", { name: new RegExp(name) }));
  }

  it("says nothing until a governed connection is chosen", async () => {
    renderCreate();
    // Anti-triviality FIRST: the sentence is not simply always on screen.
    expect(screen.queryByText(GOVERNED_BLOCKED_MESSAGE)).toBeNull();
    await pick("plain");
    expect(screen.queryByText(GOVERNED_BLOCKED_MESSAGE)).toBeNull();
  });

  it("warns when the chosen connection is governed and runs as the service account", async () => {
    renderCreate();
    await pick("blocked");
    expect(screen.getByText(GOVERNED_BLOCKED_MESSAGE)).toBeInTheDocument();
  });

  /**
   * ONE field apart from the row above, and the warning must be gone. Without
   * this the previous test passes for a page that warns about every governed
   * database — which would be advice to turn on a setting that is already on.
   */
  it("stays quiet on a governed connection that already delegates", async () => {
    renderCreate();
    await pick("delegated");
    expect(screen.queryByText(GOVERNED_BLOCKED_MESSAGE)).toBeNull();
  });
});
