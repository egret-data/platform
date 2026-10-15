import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, test, vi } from "vitest";

import TagDetail from "@/routes/governance/TagDetail";
import * as tagsApi from "@/api/tags";
import * as accessApi from "@/api/access";
import { useAuth } from "@/auth/useAuth";
import type { Me } from "@/api/hooks";

vi.mock("@/api/tags");
vi.mock("@/api/access");
vi.mock("@/auth/useAuth");
vi.mock("@/routes/governance/ViewExposureSection", () => ({
  default: ({ tagId }: { tagId: string }) => <div data-testid="view-exposure">{tagId}</div>,
}));
vi.mock("react-router-dom", async () => {
  const actual = await vi.importActual<typeof import("react-router-dom")>("react-router-dom");
  return { ...actual, useParams: () => ({ id: "t1" }), useNavigate: () => vi.fn() };
});

const ADMIN_ME = {
  user_id: "1",
  email: "admin@example.com",
  platform_roles: ["platform_admin"],
  permissions: [],
} as unknown as Me;

const TAG: tagsApi.Tag = {
  id: "t1",
  key: "pii",
  name: "PII",
  description: "Personally identifiable information",
  color: "#e11d48",
  assignment_count: 3,
  created_at: "2026-07-01T00:00:00Z",
  required_roles: [],
  required_groups: [],
};

const ROLES = [
  { id: "r1", scope: "platform", name: "finance_cleared", description: "", permissionCount: 0, assigneeCount: 0, inheritedPermissions: [] },
] as unknown as accessApi.AccessRole[];
const GROUPS = [
  { id: "g1", name: "Security Team", source: "local", memberCount: 4, roles: [] },
] as unknown as accessApi.AccessGroup[];

const setMutate = vi.fn();

function renderPage() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter>
        <TagDetail />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("TagDetail", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(useAuth).mockReturnValue({ me: ADMIN_ME, isLoading: false } as ReturnType<typeof useAuth>);
    vi.mocked(tagsApi.useTags).mockReturnValue({
      data: [TAG],
      isLoading: false,
      isError: false,
      error: null,
    } as unknown as ReturnType<typeof tagsApi.useTags>);
    vi.mocked(accessApi.listRoles).mockResolvedValue(ROLES);
    vi.mocked(accessApi.listGroups).mockResolvedValue(GROUPS);
    const stub = (m: unknown) => ({ mutate: m, isPending: false }) as never;
    vi.mocked(tagsApi.useUpdateTag).mockReturnValue(stub(vi.fn()));
    vi.mocked(tagsApi.useSetTagAccessPolicy).mockReturnValue(stub(setMutate));
    vi.mocked(tagsApi.useClearTagAccessPolicy).mockReturnValue(stub(vi.fn()));
    vi.mocked(tagsApi.useDeleteTag).mockReturnValue(stub(vi.fn()));
  });

  test("renders the tag identity + immutable key", async () => {
    renderPage();
    expect(await screen.findByRole("heading", { name: "PII" })).toBeInTheDocument();
    expect(screen.getByText("pii")).toBeInTheDocument();
    // Key input is present and read-only.
    expect(screen.getByLabelText("Key")).toHaveAttribute("readonly");
  });

  test("shows the existing-materialized-views section for this tag", async () => {
    renderPage();
    expect(await screen.findByTestId("view-exposure")).toHaveTextContent("t1");
  });

  test("links back to Governance → Classifications", async () => {
    renderPage();
    await screen.findByRole("heading", { name: "PII" });
    expect(screen.getByRole("link", { name: "Classifications" })).toHaveAttribute("href", "/governance/classifications");
  });

  test("picking a role and clicking Add persists the clearance", async () => {
    const user = userEvent.setup();
    renderPage();
    await screen.findByRole("heading", { name: "PII" });

    await user.click(screen.getByRole("combobox", { name: "Role to add" }));
    await user.click(await screen.findByText("finance_cleared"));
    await user.click(screen.getByRole("button", { name: "Add clearance" }));

    expect(setMutate).toHaveBeenCalledWith(
      { id: "t1", required_roles: ["finance_cleared"], required_groups: [] },
      expect.anything(),
    );
  });

  // SP4c Task 5/D2, corrected and extended by Task 7: four facts the UI must
  // say where the clearance is set, because none of them is enforced by the
  // engine and an operator who is not told will assume otherwise. Each
  // assertion below is on the STATEMENT's content, not merely that "a
  // warning" renders — a wording change is exactly what a later refactor
  // would drop silently, and a shape-only test would pass against that.
  //
  // Two of these statements were MEASURED FALSE before Task 7
  // (`docs/en/plans/2026-09-28-sp4c-live-result.md`): the write sentence
  // claimed Trino withheld writes and called the difference an asymmetry,
  // and nothing said a view bypasses the clearance at all. Both now describe
  // something the system CANNOT do, which is precisely the kind of sentence
  // a refactor drops without noticing.
  describe("what the engine does not enforce", () => {
    test("an ungated tag shows none of the clearance warnings", async () => {
      renderPage();
      await screen.findByRole("heading", { name: "PII" });

      expect(screen.queryByText(/not enforced in the engine/i)).not.toBeInTheDocument();
      expect(screen.queryByText(/withholds reads only/i)).not.toBeInTheDocument();
      expect(screen.queryByText(/a view over a classified column/i)).not.toBeInTheDocument();
      expect(screen.queryByText(/500 of this tag.s columns/i)).not.toBeInTheDocument();
    });

    test("a group-only gate says neither a group nor a role name reaches the engine", async () => {
      vi.mocked(tagsApi.useTags).mockReturnValue({
        data: [{ ...TAG, required_groups: ["g1"] }],
        isLoading: false,
        isError: false,
        error: null,
      } as unknown as ReturnType<typeof tagsApi.useTags>);
      renderPage();
      await screen.findByRole("heading", { name: "PII" });

      expect(
        screen.getByText(
          /not enforced in the engine: no group provider or usersync means a group name never reaches trino or starrocks, and a role name does not either/i,
        ),
      ).toBeInTheDocument();
    });

    // SP4c Task 7. The assertion is on BOTH halves of the corrected sentence:
    // that the clearance withholds reads only on both engines, and that
    // NEITHER engine withholds writes. The second half is the one the gate
    // falsified — the old wording said Trino withheld both — so a test that
    // only matched "withholds reads only" would have passed against the false
    // sentence too.
    test("a role-gated tag says NEITHER engine withholds writes", async () => {
      vi.mocked(tagsApi.useTags).mockReturnValue({
        data: [{ ...TAG, required_roles: ["finance_cleared"] }],
        isLoading: false,
        isError: false,
        error: null,
      } as unknown as ReturnType<typeof tagsApi.useTags>);
      renderPage();
      await screen.findByRole("heading", { name: "PII" });

      expect(
        screen.getByText(
          /this clearance withholds reads only, on trino and on starrocks alike: a user without it can still overwrite a column they cannot read/i,
        ),
      ).toBeInTheDocument();
      expect(screen.getByText(/neither engine withholds writes/i)).toBeInTheDocument();
      // And the asymmetry the live gate falsified must not come back.
      expect(screen.queryByText(/on a starrocks engine this clearance/i)).not.toBeInTheDocument();
      expect(screen.queryByText(/trino withholds both reads and writes/i)).not.toBeInTheDocument();
      // The group-only warning must NOT appear here — this gate has a role.
      expect(screen.queryByText(/not enforced in the engine/i)).not.toBeInTheDocument();
    });

    // View governance Task 12 (live finding C4). This box used to say a view
    // returns a classified column "in full to a user without clearance" on both
    // engines. Measured after view governance (Task 11): Trino refuses the
    // uncleared reader through a view, StarRocks returns NULL (and refuses
    // through an INVOKER view). The DEFINER owner direction still holds on Trino.
    test("a role-gated tag states what a view over a classified column returns on each engine", async () => {
      vi.mocked(tagsApi.useTags).mockReturnValue({
        data: [{ ...TAG, required_roles: ["finance_cleared"] }],
        isLoading: false,
        isError: false,
        error: null,
      } as unknown as ReturnType<typeof tagsApi.useTags>);
      renderPage();
      await screen.findByRole("heading", { name: "PII" });

      expect(screen.queryByText(/returns it in full to a user without clearance/i)).not.toBeInTheDocument();
      expect(screen.queryByText(/not covered on either engine/i)).not.toBeInTheDocument();
      const box = screen.getByText(/a view over a classified column carries the classification/i);
      expect(box).toHaveTextContent(/on trino a user without clearance is refused through the view/i);
      expect(box).toHaveTextContent(/on starrocks they read null/i);
      expect(box).toHaveTextContent(/an uncleared owner.s definer view blocks everybody, the cleared included/i);
    });

    // The ceiling is PER PROFILE, not installation-wide: the compiler counts
    // `tagged_columns_for_service_via`, which joins the profile's own attached
    // catalogs, so a profile serving unrelated tables sees none of this tag's
    // columns and is not affected. The earlier wording said every StarRocks
    // profile's recompile fails "including profiles that serve entirely
    // unrelated tables", which overstates the blast radius of a limit an
    // operator plans capacity against.
    test("a role-gated tag says the expansion ceiling is counted PER StarRocks profile", async () => {
      vi.mocked(tagsApi.useTags).mockReturnValue({
        data: [{ ...TAG, required_roles: ["finance_cleared"] }],
        isLoading: false,
        isError: false,
        error: null,
      } as unknown as ReturnType<typeof tagsApi.useTags>);
      renderPage();
      await screen.findByRole("heading", { name: "PII" });

      expect(
        screen.getByText(
          /if more than 500 of this tag.s columns are visible to one starrocks profile, that profile.s recompile fails outright rather than truncating the clearance/i,
        ),
      ).toBeInTheDocument();
      expect(screen.getByText(/profiles that see fewer are unaffected/i)).toBeInTheDocument();
      expect(
        screen.queryByText(/including profiles that serve entirely unrelated tables/i),
      ).not.toBeInTheDocument();
    });

    // **I2.** A groups-only gate compiles to NOTHING (`clearance.rs` skips it:
    // a Ranger item naming a group matches nobody), so every one of the three
    // engine statements would be describing a control that does not exist —
    // claiming withholding where there is none, inside the very statements
    // written to prevent that. The four presence tests above all render a
    // ROLE-gated tag, so none of them could see this.
    test("a groups-only gate shows the group warning and NONE of the engine statements", async () => {
      vi.mocked(tagsApi.useTags).mockReturnValue({
        data: [{ ...TAG, required_roles: [], required_groups: ["g1"] }],
        isLoading: false,
        isError: false,
        error: null,
      } as unknown as ReturnType<typeof tagsApi.useTags>);
      renderPage();
      await screen.findByRole("heading", { name: "PII" });

      // The one statement that IS true of a groups-only gate.
      expect(screen.getByText(/not enforced in the engine/i)).toBeInTheDocument();
      // And the three that are not.
      expect(screen.queryByText(/withholds reads only/i)).not.toBeInTheDocument();
      expect(screen.queryByText(/neither engine withholds writes/i)).not.toBeInTheDocument();
      expect(screen.queryByText(/a view over a classified column/i)).not.toBeInTheDocument();
      expect(screen.queryByText(/500 of this tag.s columns/i)).not.toBeInTheDocument();
      // It is still gated for OBJECT access, which the header badge reports.
      expect(screen.getByText("Gated")).toBeInTheDocument();
    });

    // **SP4c review I5.** The write now reports what it could NOT compile, and
    // a success toast that ignored it would be the same "saved, therefore
    // enforced" claim this whole surface exists to avoid. The author is told
    // where the clearance is authored, not only in the server log.
    test("a clearance the recompile could not express is reported, not reported as saved", async () => {
      const user = userEvent.setup();
      // The mutate stub resolves through its onSuccess, the way the real hook
      // does, with a skip on the wire.
      setMutate.mockImplementation(
        (
          _vars: unknown,
          opts: {
            onSuccess: (r: {
              ok: boolean;
              // The whole SkipReport, not just `reason`: narrowing it here made
              // `tsc` reject the literal below while vitest still passed, so the
              // gate failed on a shape the runtime never minded.
              skipped: { source_kind: string; id: string; reason: string }[];
            }) => void;
          },
        ) =>
          opts.onSuccess({
            ok: true,
            skipped: [
              {
                source_kind: "clearance",
                id: "t1",
                reason:
                  "this Trino profile has no usable `tagService`, and a Trino clearance is a TAG policy",
              },
            ],
          }),
      );
      renderPage();
      await screen.findByRole("heading", { name: "PII" });

      await user.click(screen.getByRole("combobox", { name: "Role to add" }));
      await user.click(await screen.findByText("finance_cleared"));
      await user.click(screen.getByRole("button", { name: "Add clearance" }));

      expect(
        await screen.findByText(/saved, but not compiled into any engine/i),
      ).toBeInTheDocument();
      expect(screen.getByText(/has no usable .?tagService/i)).toBeInTheDocument();
    });
  });
});
