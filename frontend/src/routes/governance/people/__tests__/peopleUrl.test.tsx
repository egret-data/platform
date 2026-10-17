import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes, useParams, useSearchParams } from "react-router-dom";
import {
  principalLinkFor,
  principalParam,
  principalUrl,
  type PrincipalLink,
} from "@/routes/governance/people/peopleUrl";

/**
 * The round trip goes through a REAL router with the area's own route
 * patterns (`PeopleRoutes.tsx`), as `objectPath.test.tsx` does: whether a `/`,
 * a `%` or a dot-only name survives is decided by how react-router splits and
 * decodes the pathname, which hand-made params would never meet.
 */
function Probe({ kind }: { kind: "user" | "username" | "group" | "role" }) {
  const params = useParams();
  const [search] = useSearchParams();
  let link: PrincipalLink;
  if (kind === "user") link = { kind: "user", userId: principalParam(params.userId) };
  else if (kind === "username") link = { kind: "user", username: principalParam(params.username) };
  else link = { kind, name: principalParam(params.name) };
  return <output data-testid="link">{JSON.stringify({ link, tab: search.get("tab") })}</output>;
}

function roundTrip(p: PrincipalLink, tab?: string): { link: PrincipalLink; tab: string | null } {
  render(
    <MemoryRouter initialEntries={[principalUrl(p, tab)]}>
      <Routes>
        <Route path="/governance/people/users/by-name/:username" element={<Probe kind="username" />} />
        <Route path="/governance/people/users/:userId" element={<Probe kind="user" />} />
        <Route path="/governance/people/groups/:name" element={<Probe kind="group" />} />
        <Route path="/governance/people/roles/:name" element={<Probe kind="role" />} />
      </Routes>
    </MemoryRouter>,
  );
  return JSON.parse(screen.getByTestId("link").textContent ?? "null") as { link: PrincipalLink; tab: string | null };
}

describe("principalUrl", () => {
  it("builds each kind's URL", () => {
    expect(principalUrl({ kind: "user", userId: "u-1" })).toBe("/governance/people/users/u-1");
    expect(principalUrl({ kind: "user", username: "ana" })).toBe("/governance/people/users/by-name/ana");
    expect(principalUrl({ kind: "group", name: "analysts" }, "permissions")).toBe(
      "/governance/people/groups/analysts?tab=permissions",
    );
    expect(principalUrl({ kind: "role", name: "steward" })).toBe("/governance/people/roles/steward");
  });

  it("encodes a space, a % and a / inside a name, so it stays one segment", () => {
    expect(principalUrl({ kind: "group", name: "a/b 50%" })).toBe("/governance/people/groups/a%2Fb%2050%25");
  });

  it.each<[string, PrincipalLink, string | undefined]>([
    ["user id", { kind: "user", userId: "6f1c2a90-0000-4000-8000-000000000001" }, undefined],
    ["username", { kind: "user", username: "ana.s" }, "overview"],
    ["group", { kind: "group", name: "analysts" }, "permissions"],
    ["role", { kind: "role", name: "data steward" }, undefined],
    ["group with a /, a %, a space and a .", { kind: "group", name: "eu/fin 50% a.b" }, "clearances"],
    ["role with a /", { kind: "role", name: "a/b" }, undefined],
    ["username with a % and a /", { kind: "user", username: "x%y/z" }, undefined],
    ["group named .", { kind: "group", name: "." }, undefined],
    ["role named ..", { kind: "role", name: ".." }, "permissions"],
    ["username ...", { kind: "user", username: "..." }, undefined],
  ])("round-trips a %s through the area's routes", (_label, p, tab) => {
    expect(roundTrip(p, tab)).toEqual({ link: p, tab: tab ?? null });
  });

  it("keeps a dot-only name through the browser's URL parser", () => {
    const url = principalUrl({ kind: "group", name: ".." });
    expect(new URL(url, "http://egret.local").pathname).toBe(url);
  });
});

describe("principalLinkFor", () => {
  it("links a user ref by its username, which the by-name route resolves", () => {
    expect(principalLinkFor({ kind: "user", ref: "ana" })).toEqual({ kind: "user", username: "ana" });
  });

  it("links a group and a role by name, public included", () => {
    expect(principalLinkFor({ kind: "group", ref: "analysts" })).toEqual({ kind: "group", name: "analysts" });
    expect(principalLinkFor({ kind: "group", ref: "public" })).toEqual({ kind: "group", name: "public" });
    expect(principalLinkFor({ kind: "role", ref: "steward" })).toEqual({ kind: "role", name: "steward" });
  });

  it("has no page for a workspace's members", () => {
    expect(principalLinkFor({ kind: "workspace_members", ref: "w1" })).toBeNull();
  });
});
