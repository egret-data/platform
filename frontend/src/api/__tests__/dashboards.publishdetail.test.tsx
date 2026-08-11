import { describe, it, expect, vi, beforeEach } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createElement, type ReactNode } from "react";

/**
 * SAVING A DASHBOARD MINTS NEW CARD IDS, and the cache must carry them before the caller can act.
 *
 * The backend replaces the whole layout on save, so every card is recreated with a fresh id.
 * `DashboardView.onSave` leaves edit mode the moment `mutateAsync` resolves; if the detail cache
 * still held the pre-save ids at that point, the view-mode cards would mount against ids that no
 * longer exist and each would `POST …/cards/{old_id}/data` into a 404. That is exactly what was
 * measured on the deployed cluster — five cards, five 404s, on one ordinary save.
 *
 * The assertion is therefore about TIMING, not just contents: the cache has to be correct at the
 * instant `mutateAsync` resolves, which is why it is read immediately after the await and not in
 * a `waitFor`. A `waitFor` would also pass against the old invalidate-and-refetch behaviour,
 * making the test green and worthless.
 */

const NEW_DETAIL = {
  id: "d1",
  name: "test",
  width: "full",
  cards: [{ id: "new-card-1" }, { id: "new-card-2" }],
};

const apiFetch = vi.fn().mockResolvedValue({
  status: 200,
  headers: new Headers({ "content-type": "application/json" }),
  json: () => Promise.resolve(NEW_DETAIL),
});
vi.mock("../client", () => ({
  apiJson: vi.fn(),
  apiFetch: (...a: unknown[]) => apiFetch(...a),
  ApiError: class ApiError extends Error {},
  UnauthorizedError: class UnauthorizedError extends Error {},
}));

import { useDashboardMutations } from "../dashboards";
import { getDashboardsDbGetQueryKey } from "../generated/dashboards/dashboards";

/** The orval envelope the detail cache actually holds — `useDashboard` reads it via
 *  `select: (r) => r.data`, so a seed that wrote the bare detail would make `select` read
 *  `undefined` and blank the dashboard. Shaped here so that mistake fails the test. */
const envelope = (cards: { id: string }[]) => ({
  status: 200,
  headers: new Headers(),
  data: { id: "d1", name: "test", width: "fixed", cards },
});

const savePayload = {
  id: "d1", name: "test", parameters: [], tabs: [], width: "full" as const, cards: [],
};

describe("useDashboardMutations — publishing the saved detail", () => {
  beforeEach(() => apiFetch.mockClear());

  it("has the new card ids in the detail cache the moment a save resolves", async () => {
    const qc = new QueryClient();
    qc.setQueryData(getDashboardsDbGetQueryKey("eng", "d1"), envelope([{ id: "old-card-1" }, { id: "old-card-2" }]));
    const wrapper = ({ children }: { children: ReactNode }) =>
      createElement(QueryClientProvider, { client: qc }, children);

    const { result } = renderHook(() => useDashboardMutations("eng"), { wrapper });
    await act(async () => { await result.current.update.mutateAsync(savePayload); });

    const cached = qc.getQueryData(getDashboardsDbGetQueryKey("eng", "d1")) as { data?: typeof NEW_DETAIL };
    expect(cached?.data?.cards.map((c) => c.id)).toEqual(["new-card-1", "new-card-2"]);
  });

  it("keeps the envelope so `select: (r) => r.data` still finds the detail", async () => {
    const qc = new QueryClient();
    qc.setQueryData(getDashboardsDbGetQueryKey("eng", "d1"), envelope([{ id: "old-card-1" }]));
    const wrapper = ({ children }: { children: ReactNode }) =>
      createElement(QueryClientProvider, { client: qc }, children);

    const { result } = renderHook(() => useDashboardMutations("eng"), { wrapper });
    await act(async () => { await result.current.update.mutateAsync(savePayload); });

    const cached = qc.getQueryData(getDashboardsDbGetQueryKey("eng", "d1")) as { status?: number; data?: unknown };
    expect(cached?.status).toBe(200);
    expect(cached?.data).toMatchObject({ id: "d1", width: "full" });
  });

  it("leaves a cold cache alone rather than inventing an envelope", async () => {
    const qc = new QueryClient();
    const wrapper = ({ children }: { children: ReactNode }) =>
      createElement(QueryClientProvider, { client: qc }, children);

    const { result } = renderHook(() => useDashboardMutations("eng"), { wrapper });
    await act(async () => { await result.current.update.mutateAsync(savePayload); });

    // Nothing was cached, so there is no envelope to patch — writing a half-built one here would
    // hand `select` a shape the server never produced. The invalidate fetches it instead.
    expect(qc.getQueryData(getDashboardsDbGetQueryKey("eng", "d1"))).toBeUndefined();
  });
});
