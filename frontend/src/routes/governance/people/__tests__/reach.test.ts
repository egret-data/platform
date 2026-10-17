import { describe, expect, it } from "vitest";
import type { PrincipalSet, StandsFor } from "@/api/generated/model";
import { reachesAtAll, reachGroup, setReach, type Reach } from "@/routes/governance/people/reach";

const WS = "6f1c2a90-0000-4000-8000-00000000000a";

// A user `ana` in group `analysts`, holding role `steward` and through
// `analysts` the role `reader` (egret only), member of workspace WS, and `public`.
const ANA: StandsFor[] = [
  { kind: "user", ref: "ana", via: { kind: "itself" }, onEngines: true },
  { kind: "group", ref: "analysts", via: { kind: "group", name: "analysts" }, onEngines: true },
  { kind: "role", ref: "steward", via: { kind: "role", name: "steward" }, onEngines: true },
  { kind: "role", ref: "reader", via: { kind: "group_role", group: "analysts", role: "reader" }, onEngines: false, notOnEngines: "held_through_group" },
  { kind: "workspace_members", ref: WS, via: { kind: "workspace", id: WS, name: "Sales" }, onEngines: true },
  { kind: "group", ref: "public", via: { kind: "public" }, onEngines: true },
];

const set = (include: PrincipalSet["include"], exclude: PrincipalSet["exclude"] = []): PrincipalSet => ({ include, exclude });

describe("setReach", () => {
  it.each<[string, PrincipalSet, Reach[]]>([
    ["itself", set([{ kind: "user", ref: "ana" }]), [{ position: "include", via: { kind: "itself" }, onEngines: true }]],
    ["a group", set([{ kind: "group", ref: "analysts" }]), [{ position: "include", via: { kind: "group", name: "analysts" }, onEngines: true }]],
    ["a role", set([{ kind: "role", ref: "steward" }]), [{ position: "include", via: { kind: "role", name: "steward" }, onEngines: true }]],
    [
      "a group's role",
      set([{ kind: "role", ref: "reader" }]),
      [{ position: "include", via: { kind: "group_role", group: "analysts", role: "reader" }, onEngines: false, notOnEngines: "held_through_group" }],
    ],
    ["a workspace", set([{ kind: "workspace_members", ref: WS }]), [{ position: "include", via: { kind: "workspace", id: WS, name: "Sales" }, onEngines: true }]],
    ["public", set([{ kind: "group", ref: "public" }]), [{ position: "include", via: { kind: "public" }, onEngines: true }]],
    ["an exclude", set([{ kind: "role", ref: "other" }], [{ kind: "user", ref: "ana" }]), [{ position: "exclude", via: { kind: "itself" }, onEngines: true }]],
  ])("meets %s", (_label, s, expected) => {
    expect(setReach(s, ANA)).toEqual(expected);
  });

  it("lists every way, include and exclude both", () => {
    const r = setReach(set([{ kind: "group", ref: "analysts" }, { kind: "user", ref: "ana" }], [{ kind: "role", ref: "steward" }]), ANA);
    expect(r.map((x) => [x.position, x.via.kind])).toEqual([
      ["include", "group"],
      ["include", "itself"],
      ["exclude", "role"],
    ]);
  });

  it("a ref that differs only by case does not match", () => {
    expect(setReach(set([{ kind: "user", ref: "Ana" }, { kind: "group", ref: "ANALYSTS" }]), ANA)).toEqual([]);
  });

  it("the same ref under another kind does not match", () => {
    expect(setReach(set([{ kind: "role", ref: "ana" }, { kind: "user", ref: "analysts" }]), ANA)).toEqual([]);
  });

  it.each([
    ["upper case", WS.toUpperCase()],
    ["no hyphens", WS.replace(/-/g, "")],
    ["braces", `{${WS}}`],
    ["urn", `urn:uuid:${WS}`],
  ])("a workspace_members ref matches the workspace id written with %s", (_l, ref) => {
    expect(setReach(set([{ kind: "workspace_members", ref }]), ANA)).toHaveLength(1);
  });

  it("a workspace_members ref that is another id, or no uuid, does not match", () => {
    expect(setReach(set([{ kind: "workspace_members", ref: WS.replace("a", "b") }]), ANA)).toEqual([]);
    expect(setReach(set([{ kind: "workspace_members", ref: "sales" }]), ANA)).toEqual([]);
  });

  it("an empty set reaches nobody", () => {
    expect(setReach(set([]), ANA)).toEqual([]);
  });
});

const reach = (over: Partial<Reach>): Reach => ({ position: "include", via: { kind: "itself" }, onEngines: true, ...over });

describe("reachGroup", () => {
  it.each<[string, Reach[], ReturnType<typeof reachGroup>]>([
    ["directly", [reach({})], "direct"],
    ["through a group", [reach({ via: { kind: "group", name: "g" } })], "group"],
    ["through a role", [reach({ via: { kind: "role", name: "r" } })], "role"],
    ["through a workspace", [reach({ via: { kind: "workspace", id: WS, name: "Sales" } })], "workspace"],
    ["everyone", [reach({ via: { kind: "public" } })], "public"],
  ])("%s", (_l, r, group) => {
    expect(reachGroup(r)).toBe(group);
  });

  it("an exclude that applies on engines beats an include, as Ranger's exclude does", () => {
    expect(reachGroup([reach({}), reach({ position: "exclude", via: { kind: "group", name: "g" } })])).toBe("excluded");
  });

  const heldThroughGroup = { onEngines: false, notOnEngines: "held_through_group" as const };

  it("an exclude engines do not apply excludes nobody: the includes decide", () => {
    // `… except role r`, r held only through group g: Ranger does not exclude the user.
    const egretExclude = reach({ position: "exclude", via: { kind: "group_role", group: "g", role: "r" }, ...heldThroughGroup });
    expect(reachGroup([reach({}), egretExclude])).toBe("direct");
    expect(reachGroup([reach({ via: { kind: "group", name: "g" } }), egretExclude])).toBe("group");
    expect(reachGroup([reach({ via: { kind: "group_role", group: "g", role: "x" }, ...heldThroughGroup }), egretExclude])).toBe("egret_only");
  });

  it("is egret only when no include applies on engines and one is held through a group", () => {
    const egretOnly = reach({ via: { kind: "group_role", group: "g", role: "r" }, ...heldThroughGroup });
    expect(reachGroup([egretOnly])).toBe("egret_only");
    expect(reachGroup([egretOnly, reach({ via: { kind: "group", name: "g" } })])).toBe("group");
  });

  it.each(["expired", "no_username"] as const)("is not applied on engines when every include is %s", (why) => {
    expect(reachGroup([reach({ onEngines: false, notOnEngines: why })])).toBe("not_on_engines");
    expect(reachGroup([reach({ via: { kind: "role", name: "r" }, onEngines: false, notOnEngines: why })])).toBe("not_on_engines");
  });

  it("a grant concerns the principal through an include, or an exclude engines apply", () => {
    expect(reachesAtAll([reach({})])).toBe(true);
    expect(reachesAtAll([reach({ position: "exclude" })])).toBe(true);
    expect(reachesAtAll([reach({ position: "exclude", via: { kind: "group_role", group: "g", role: "r" }, ...heldThroughGroup })])).toBe(false);
    expect(reachesAtAll([])).toBe(false);
  });

  it("the most direct path that applies on engines names the group", () => {
    expect(
      reachGroup([
        reach({ via: { kind: "public" } }),
        reach({ via: { kind: "role", name: "r" } }),
        reach({ via: { kind: "group", name: "g" } }),
      ]),
    ).toBe("group");
    expect(reachGroup([reach({ via: { kind: "role", name: "r" }, onEngines: false, notOnEngines: "expired" }), reach({ via: { kind: "public" } })])).toBe(
      "public",
    );
  });
});
