import { render, screen } from "@testing-library/react";
import { MemoryRouter, NavLink } from "react-router-dom";
import { PageContainer } from "@/components/layout/PageContainer";
import { PageTitle } from "@/components/layout/PageTitle";
import { DetailHeader } from "@/components/layout/DetailHeader";
import { ListPageShell } from "@/components/explore/ListPageShell";
import { StatusBadge, STATUS_TONE } from "@/components/ui/status-badge";
import { EntityLink } from "@/components/ui/entity-link";
import { Tabs, TabsList, TabsTrigger, tabsTriggerClass } from "@/components/ui/tabs";

/**
 * The page-frame components' contracts (KI-214, KI-215, KI-216, KI-222,
 * KI-223, KI-224).
 *
 * These are class and DOM assertions, and jsdom has no layout
 * (`docs/en/ui-conventions.md` §10): the painted result — 20px titles, one
 * left edge, a 960px column — is measured in Chromium by
 * `tests/e2e/page-frame.spec.ts`. What is pinned here is what a refactor could
 * drop silently: which class each variant carries, and that each shared piece
 * renders the element the rulebook says it does.
 */
const classes = (el: Element) => [...el.classList];

describe("PageTitle — one scale", () => {
  test("a page title is the h1 at text-xl, a tool title at text-base", () => {
    render(
      <>
        <PageTitle>Datasets</PageTitle>
        <PageTitle size="compact">SQL Lab</PageTitle>
      </>,
    );
    const page = screen.getByRole("heading", { level: 1, name: "Datasets" });
    const tool = screen.getByRole("heading", { level: 1, name: "SQL Lab" });
    expect(classes(page)).toContain("text-xl");
    expect(classes(page)).not.toContain("text-base");
    expect(classes(tool)).toContain("text-base");
    expect(classes(tool)).not.toContain("text-xl");
  });
});

describe("PageContainer — one left edge, two widths", () => {
  test("full carries no cap; narrow caps the content box at max-w-page and is never centred", () => {
    const { container } = render(
      <>
        <PageContainer>list</PageContainer>
        <PageContainer width="narrow">form</PageContainer>
      </>,
    );
    const [full, narrow] = [...container.children];
    expect(classes(full!)).toContain("p-4");
    expect(classes(full!)).not.toContain("max-w-page");
    expect(classes(narrow!)).toEqual(expect.arrayContaining(["p-4", "max-w-page", "box-content"]));
    expect(classes(narrow!)).not.toContain("mx-auto");
  });

  test("ListPageShell renders through it, with a PageTitle", () => {
    const { container } = render(<ListPageShell title="Charts">rows</ListPageShell>);
    expect(container.firstElementChild).toHaveAttribute("data-page-width", "full");
    expect(screen.getByRole("heading", { level: 1, name: "Charts" })).toHaveAttribute("data-page-title", "page");
  });
});

describe("DetailHeader — one shape", () => {
  test("back link above the h1, status beside it, actions and facts rendered", () => {
    render(
      <MemoryRouter>
        <DetailHeader
          back={{ to: "/explore/w/alerts", label: "Alerts & Reports" }}
          title="Nightly revenue"
          status={<StatusBadge tone="highlight">paused</StatusBadge>}
          actions={<button type="button">Run now</button>}
          meta={<span>Runs daily</span>}
        />
      </MemoryRouter>,
    );
    const back = screen.getByRole("link", { name: "Alerts & Reports" });
    const h1 = screen.getByRole("heading", { level: 1, name: "Nightly revenue" });
    expect(back).toHaveAttribute("href", "/explore/w/alerts");
    // The back link precedes the title in document order.
    expect(back.compareDocumentPosition(h1) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(screen.getByText("Paused")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Run now" })).toBeInTheDocument();
    expect(screen.getByText("Runs daily")).toBeInTheDocument();
  });

  test("no back link when the page has no parent list", () => {
    render(
      <MemoryRouter>
        <DetailHeader title="Workspace settings" />
      </MemoryRouter>,
    );
    expect(screen.queryByRole("link")).toBeNull();
  });
});

describe("StatusBadge — tint, foreground ink, sentence case", () => {
  test("shows a server word in sentence case and wears its tone", () => {
    render(<StatusBadge tone="success">active</StatusBadge>);
    const badge = screen.getByText("Active");
    for (const cls of STATUS_TONE.success.split(" ")) expect(classes(badge)).toContain(cls);
    expect(classes(badge)).toContain("text-foreground");
    // Not the solid fill Control Plane used to paint.
    expect(classes(badge)).not.toContain("bg-success");
  });

  test("every tone inks in --foreground", () => {
    for (const value of Object.values(STATUS_TONE)) expect(value.split(" ")).toContain("text-foreground");
  });
});

describe("EntityLink — one colour for names", () => {
  test("text colour, underline on hover, never the teal action colour", () => {
    render(
      <MemoryRouter>
        <EntityLink to="/control-plane/workspaces/1">default</EntityLink>
      </MemoryRouter>,
    );
    const link = screen.getByRole("link", { name: "default" });
    expect(classes(link)).toEqual(expect.arrayContaining(["text-foreground", "hover:underline"]));
    expect(classes(link)).not.toContain("text-primary");
  });
});

describe("Tabs — two variants", () => {
  test("a trigger takes its list's variant", () => {
    render(
      <Tabs defaultValue="a">
        <TabsList aria-label="pills">
          <TabsTrigger value="a">Pill</TabsTrigger>
        </TabsList>
        <TabsList variant="underline" aria-label="lines">
          <TabsTrigger value="a">Line</TabsTrigger>
        </TabsList>
      </Tabs>,
    );
    expect(classes(screen.getByRole("tablist", { name: "pills" }))).toContain("bg-muted");
    expect(classes(screen.getByRole("tablist", { name: "lines" }))).toContain("border-b");
    expect(classes(screen.getByRole("tab", { name: "Pill" }))).toContain("rounded-sm");
    expect(classes(screen.getByRole("tab", { name: "Line" }))).toContain("border-b-2");
    // The icon gap that used to be missing (the alert editor, the catalog detail).
    expect(classes(screen.getByRole("tab", { name: "Pill" }))).toContain("gap-1.5");
  });

  test("a router-link tab styles aria-current like an active trigger", () => {
    render(
      <MemoryRouter initialEntries={["/a"]}>
        <NavLink to="/a" className={tabsTriggerClass("underline")}>
          Query
        </NavLink>
      </MemoryRouter>,
    );
    const link = screen.getByRole("link", { name: "Query" });
    expect(link).toHaveAttribute("aria-current", "page");
    expect(classes(link)).toContain("aria-[current=page]:border-primary");
  });
});
