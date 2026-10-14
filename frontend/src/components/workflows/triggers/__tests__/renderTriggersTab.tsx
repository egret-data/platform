import { render } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import FlowTriggersTab from "@/components/workflows/triggers/FlowTriggersTab";
import { FakeWorkflowServer, FLOW_ID, WS } from "@/routes/workflows/__tests__/fakeWorkflowServer";
import { TRIGGERS_SOURCE } from "./triggerFixtures";

export const TAB_PATH = `/explore/${WS}/workflows/flows/${FLOW_ID}/triggers`;

function Where() {
  const l = useLocation();
  return <output data-testid="where">{`${l.pathname}${l.search}`}</output>;
}

/**
 * The flow's Triggers tab on its route, against `server` (its real orval hooks,
 * mutator and `apiFetch` over the stubbed `fetch`), with the URL shown in
 * `where`.
 */
export function renderTriggersTab(server: FakeWorkflowServer, { manage = true, at = TAB_PATH, source = TRIGGERS_SOURCE } = {}) {
  server.install();
  const onEdit = vi.fn();
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const utils = render(
    <QueryClientProvider client={client}>
      <TooltipProvider>
        <MemoryRouter initialEntries={[at]}>
          <Routes>
            <Route
              path="/explore/:workspace/workflows/flows/:id/:tab"
              element={<FlowTriggersTab ws={WS} flowId={FLOW_ID} source={source} manage={manage} onEditTrigger={onEdit} />}
            />
          </Routes>
          <Where />
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
  return { ...utils, onEdit, client };
}
