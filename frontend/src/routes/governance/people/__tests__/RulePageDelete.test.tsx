import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation, useNavigate } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { RuleView } from "@/api/generated/model";
import { AuthContext } from "@/auth/useAuth";
import { TooltipProvider } from "@/components/ui/tooltip";
import ClassificationsRoutes from "@/routes/governance/classifications/ClassificationsRoutes";
import RulePage from "@/routes/governance/people/RulePage";
import { me, ok, serve, type Answer } from "@/routes/governance/engines/__tests__/engineHarness";
import { tag } from "@/routes/governance/classifications/__tests__/harness";

/**
 * Deleting a rule from its page, with the real query cache and router (no
 * mocked hooks): the page it returns to must never show the deleted rule from
 * the cache, and Back must not lead to the deleted rule's page.
 */
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn(), warning: vi.fn() } }));
afterEach(() => vi.unstubAllGlobals());

const T = { timeout: 3000 };
const WRITER = me(["policy:read", "policy:update", "tag:read"]);
const PII = tag({ id: "t-pii", key: "pii", name: "PII" });
const rule = (id: string, name: string): RuleView => ({
  id,
  name,
  kind: { type: "mask", mask_type: "MASK_HASH" },
  selector: { type: "tag", key: "pii" },
  exempt: { include: [], exclude: [] },
  tag_name: "PII",
});
const HASH = rule("r1", "Hash email");
const KEEP = rule("r2", "Keep me");
const TAG_RULES = "/governance/classifications/t-pii?tab=rules";
const RULE_PAGE = "/governance/rules/r1?from=classification:t-pii";

function Where() {
  const loc = useLocation();
  return <output data-testid="where">{loc.pathname + loc.search}</output>;
}
function Back() {
  const navigate = useNavigate();
  return (
    <button type="button" onClick={() => navigate(-1)}>
      history back
    </button>
  );
}

function renderFrom(entries: string[]) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  render(
    <QueryClientProvider client={qc}>
      <AuthContext.Provider value={{ me: WRITER, isLoading: false }}>
        <TooltipProvider>
          <MemoryRouter initialEntries={entries} initialIndex={entries.length - 1}>
            <Routes>
              <Route path="/governance/rules/:id" element={<RulePage />} />
              <Route path="/governance/classifications/*" element={<ClassificationsRoutes />} />
            </Routes>
            <Where />
            <Back />
          </MemoryRouter>
        </TooltipProvider>
      </AuthContext.Provider>
    </QueryClientProvider>,
  );
}

describe("RulePage: deleting with the real cache", () => {
  it("returns to the classification's Rules tab without the deleted rule, and Back skips its page", async () => {
    const user = userEvent.setup();
    const answers: Record<string, Answer> = {
      "/tags": ok([PII]),
      "/perm/rules": ok([HASH, KEEP]),
      "DELETE /perm/rules/r1": { status: 204, body: null },
    };
    serve(answers);
    renderFrom([TAG_RULES, RULE_PAGE]);
    await user.click(await screen.findByRole("button", { name: "Delete" }, T));
    await user.type(await screen.findByRole("textbox"), "Hash email");
    // From here the server's list is never answered again: what the next page shows is the cache.
    answers["/perm/rules"] = "never";
    await user.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(screen.getByTestId("where")).toHaveTextContent(TAG_RULES), T);
    expect(await screen.findByRole("link", { name: "Keep me" }, T)).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Hash email" })).not.toBeInTheDocument();
    expect(screen.queryByText(/No rule has this id/)).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "history back" }));
    expect(screen.getByTestId("where")).not.toHaveTextContent("/governance/rules/r1");
    expect(screen.queryByText(/No rule has this id/)).not.toBeInTheDocument();
  });
});
