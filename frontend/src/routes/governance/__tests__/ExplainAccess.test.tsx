import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, test, vi } from "vitest";

/**
 * `ExplainAccess` reads the generated perm client directly (`usePermExplain`),
 * matching `Catalogs.test.tsx`'s pattern for the newest list pages. `ScopePicker`
 * is mocked to a dumb controlled stand-in — its own behavior (progressive
 * narrowing, row-filter mode) is covered by `ScopePicker.test.tsx`; this file
 * is about D9: the reason CLASS renders distinguishably, a `grant_id`/`rule_id`
 * links to that row, and the three honest limits are stated on the result.
 */
vi.mock("@/api/generated/perm/perm");
vi.mock("@/api/generated/policy-admin/policy-admin");
vi.mock("@/components/perm/ScopePicker", () => ({
  ScopePicker: ({ value, onChange }: { value: unknown; onChange: (v: unknown) => void }) => (
    <div>
      <div data-testid="scope-value">{JSON.stringify(value)}</div>
      <button type="button" onClick={() => onChange({ catalog_id: "cat-1" })}>
        set-scope
      </button>
    </div>
  ),
}));

import * as permApi from "@/api/generated/perm/perm";
import { policyAdminListPrincipals } from "@/api/generated/policy-admin/policy-admin";
import { ApiError } from "@/api/client";
import type { ExplainResponse } from "@/api/generated/model";
import ExplainAccess from "@/routes/governance/ExplainAccess";

const ALL_KINDS_RESULT: ExplainResponse = {
  allowed: true,
  reasons: [
    { kind: "allowed_by_grant", grant_id: "grant-1" },
    { kind: "explicit_deny", grant_id: "grant-2" },
    { kind: "no_grant_matched" },
    { kind: "masked_by_rule", rule_id: "rule-1", mask_type: "MASK_HASH" },
    { kind: "filtered_by_rule", rule_id: "rule-2", filter_expr: "ws = 'eng'" },
  ],
  tags: ["pii"],
};

let mutateAsync: ReturnType<typeof vi.fn>;

function renderPage() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <MemoryRouter>
      <QueryClientProvider client={qc}>
        <ExplainAccess />
      </QueryClientProvider>
    </MemoryRouter>,
  );
}

async function submit(user: ReturnType<typeof userEvent.setup>) {
  await user.type(screen.getByLabelText("User"), "alice-doe");
  await user.click(screen.getByRole("button", { name: "set-scope" }));
  await user.click(screen.getByRole("button", { name: /Explain/ }));
}

beforeEach(() => {
  vi.clearAllMocks();
  mutateAsync = vi.fn().mockResolvedValue({ status: 200, data: ALL_KINDS_RESULT });
  vi.mocked(permApi.usePermExplain).mockReturnValue({
    mutateAsync,
    isPending: false,
  } as unknown as ReturnType<typeof permApi.usePermExplain>);
  vi.mocked(policyAdminListPrincipals).mockResolvedValue({
    status: 200,
    data: [],
    headers: new Headers(),
  } as Awaited<ReturnType<typeof policyAdminListPrincipals>>);
});

describe("ExplainAccess — the reason class is the answer (D9)", () => {
  test("each of the five reason classes renders with its own distinguishable label", async () => {
    const user = userEvent.setup();
    renderPage();
    await submit(user);

    const result = await screen.findByTestId("explain-result");
    expect(result).toHaveTextContent("Allowed by grant");
    expect(result).toHaveTextContent("Explicit deny");
    expect(result).toHaveTextContent("No grant matched");
    expect(result).toHaveTextContent("Masked by rule");
    expect(result).toHaveTextContent("Filtered by rule");

    // no_grant_matched and explicit_deny are different administrative
    // problems and must not collapse into the same rendered sentence.
    const noGrantHint = screen.getByText(/Nobody granted this/);
    const denyHint = screen.getByText(/Somebody forbade this/);
    expect(noGrantHint.textContent).not.toEqual(denyHint.textContent);
  });

  test("a reason carrying a grant_id links to that grant, and a rule_id to that rule", async () => {
    const user = userEvent.setup();
    renderPage();
    await submit(user);

    await screen.findByTestId("explain-result");

    const grantLink = screen.getByRole("link", { name: "View grant grant-1" });
    expect(grantLink).toHaveAttribute("href", "/governance/data-permissions?grant=grant-1");

    const ruleLink = screen.getByRole("link", { name: "View rule rule-1" });
    expect(ruleLink).toHaveAttribute("href", "/governance/mask-filter-rules?rule=rule-1");
  });

  test("the three honest limits are stated on the result, not tucked in a help page", async () => {
    const user = userEvent.setup();
    renderPage();

    // Not shown before there is a result to attach them to.
    expect(screen.queryByText(/cannot see hand-written Ranger policy/)).not.toBeInTheDocument();

    await submit(user);
    const result = await screen.findByTestId("explain-result");

    expect(result).toHaveTextContent(/cannot see hand-written Ranger policy/);
    expect(result).toHaveTextContent(/seeded admin policy/);
    expect(result).toHaveTextContent(/Tags populate only when the target names both a schema and a table/);
    // SP4b-2 review I-2: the limit that makes this tool too PERMISSIVE, which
    // matters most on the page whose whole job is to be truthful about the
    // engine. Stated on EVERY result, not only when a group was named — this
    // run named none.
    expect(result).toHaveTextContent(
      /A group in the subject is matched here but not by the engine/,
    );
    expect(result).toHaveTextContent(/no\s+group provider or usersync/);
  });

  test("tags populate only when the target names a schema and a table — none renders honestly", async () => {
    mutateAsync.mockResolvedValueOnce({
      status: 200,
      data: { allowed: false, reasons: [{ kind: "no_grant_matched" }], tags: [] },
    });
    const user = userEvent.setup();
    renderPage();
    await submit(user);

    const result = await screen.findByTestId("explain-result");
    expect(result).toHaveTextContent("None.");
  });

  test("a 403 renders as a forbidden message, not a bare error", async () => {
    mutateAsync.mockRejectedValueOnce(new ApiError(403, "forbidden"));
    const user = userEvent.setup();
    renderPage();
    await submit(user);

    expect(await screen.findByText(/policy:read/)).toBeInTheDocument();
  });
});

describe("ExplainAccess — a disabled Explain button says what it is waiting for", () => {
  /**
   * SP4b-2 finding 2, the button half. `Explain` starts disabled and, with
   * `policy:read` alone, can never enable — the target picker has no options
   * (it names the missing `catalog_registry:read` itself; see
   * `ScopePicker.test.tsx`). Either way the button must not sit there inert
   * with nothing said.
   */
  test("names both missing inputs, then stops explaining once they are supplied", async () => {
    const user = userEvent.setup();
    renderPage();

    expect(screen.getByRole("button", { name: /Explain/ })).toBeDisabled();
    expect(screen.getByText("Explain needs a user and a target catalog.")).toBeInTheDocument();

    await user.type(screen.getByLabelText("User"), "alice-doe");
    expect(screen.getByText("Explain needs a target catalog.")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "set-scope" }));
    expect(screen.getByRole("button", { name: /Explain/ })).toBeEnabled();
    expect(screen.queryByText(/^Explain needs/)).not.toBeInTheDocument();
  });
});

describe("ExplainAccess — a group in the subject is a permissive-direction lie (D3, review I-2)", () => {
  /**
   * **THE DISCRIMINATING CASE.** `explain` evaluates the stored model, where a
   * group principal matches; Trino has no group provider, egret writes none and
   * there is no usersync, so the engine cannot see that membership. The page
   * therefore answers `Allowed by grant` for access the engine refuses. The
   * warning is at the point of choice — beside the field, the way
   * `PrincipalPicker` carries the same fact — and appears only once a group is
   * actually named, so an unused field is not nagging.
   */
  test("warns beside the Groups field, but only once a group is named", async () => {
    const user = userEvent.setup();
    renderPage();

    expect(screen.queryByText(/no group provider or usersync/)).not.toBeInTheDocument();

    await user.type(screen.getByLabelText("Groups"), "contractors{Enter}");

    expect(screen.getByText(/no group provider or usersync/)).toBeInTheDocument();
    expect(screen.getByText(/can read Allowed while the engine refuses/)).toBeInTheDocument();
  });

  /** Roles must NOT carry it: Ranger's access items do have a roles bucket, so
   *  a role-matched answer is not subject to this blind spot, and a warning
   *  shown for everything teaches an operator to ignore it. */
  test("does not warn about Roles", async () => {
    const user = userEvent.setup();
    renderPage();

    await user.type(screen.getByLabelText("Roles"), "analyst{Enter}");

    expect(screen.getByText("analyst")).toBeInTheDocument();
    expect(screen.queryByText(/no group provider or usersync/)).not.toBeInTheDocument();
  });
});

describe("ExplainAccess — the subject's ref is the allocated username (fact 5/5)", () => {
  /**
   * **THE DISCRIMINATING CASE (review M-2).** The engine sees a bare
   * `username`; `list_principals` returns an email as well, and a page that
   * stored the email would get `no_grant_matched` — a wrong answer that looks
   * exactly like a real one, on the page built to explain why. The suggestion
   * shows `stored as …` so the operator can see which value is kept, and the
   * explain request carries that value.
   */
  test("selecting a suggestion sends the username, not the email", async () => {
    vi.mocked(policyAdminListPrincipals).mockResolvedValue({
      status: 200,
      data: [
        {
          kind: "user",
          name: "alice@egret.local",
          label: "Alice Doe",
          username: "alice-doe-2",
        },
      ],
      headers: new Headers(),
    } as Awaited<ReturnType<typeof policyAdminListPrincipals>>);

    const user = userEvent.setup();
    renderPage();
    await user.click(screen.getByLabelText("User"));

    const option = await screen.findByRole("button", { name: /Alice Doe/ });
    expect(option).toHaveTextContent("stored as alice-doe-2");
    await user.click(option);

    await user.click(screen.getByRole("button", { name: "set-scope" }));
    await user.click(screen.getByRole("button", { name: /Explain/ }));

    expect(mutateAsync).toHaveBeenCalled();
    expect(mutateAsync.mock.calls[0][0].data.subject.user).toBe("alice-doe-2");
  });

  /** A principal with no allocated username has no ref the engine could ever
   *  see, so it is shown and NOT selectable — never an invented slug (the
   *  collision suffix `-2` above is unreproducible client-side). */
  test("a principal with no allocated username is not offered", async () => {
    vi.mocked(policyAdminListPrincipals).mockResolvedValue({
      status: 200,
      data: [{ kind: "user", name: "bob@egret.local", label: "Bob Roe", username: null }],
      headers: new Headers(),
    } as Awaited<ReturnType<typeof policyAdminListPrincipals>>);

    const user = userEvent.setup();
    renderPage();
    await user.click(screen.getByLabelText("User"));

    expect(await screen.findByText(/no allocated username yet/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Bob Roe/ })).not.toBeInTheDocument();
  });
});
