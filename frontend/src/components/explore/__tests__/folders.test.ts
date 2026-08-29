import { expect, test } from "vitest";
import { folderCollapseKey, groupByFolder, showFolderSections } from "@/components/explore/folders";

const folders = [{ id: "f2", name: "Beta" }, { id: "f1", name: "Alpha" }];
const rows = [
  { id: "a", folder_id: "f1" }, { id: "b", folder_id: null }, { id: "c", folder_id: "f1" },
];

test("groups A→Z with Uncategorized last; hideEmpty drops empty folders", () => {
  const g = groupByFolder(rows, folders, false);
  expect(g.map((x) => x.folder?.name ?? "Uncategorized")).toEqual(["Alpha", "Beta", "Uncategorized"]);
  expect(g[0].rows.map((r) => r.id)).toEqual(["a", "c"]);

  const g2 = groupByFolder(rows, folders, true);          // hideEmpty: drop "Beta" (0 rows)
  expect(g2.map((x) => x.folder?.name ?? "Uncategorized")).toEqual(["Alpha", "Uncategorized"]);
});

test("keeps a row whose folder is missing, instead of dropping it", () => {
  // A folder can vanish from `folders` while a row still points at it: it was
  // deleted, or the folders query has not resolved, or it belongs to another
  // entity_type. Losing the row is the worst available outcome — it cannot be
  // seen, opened, or moved out, and nothing on screen says it exists.
  const orphaned = [{ id: "a", folder_id: "gone" }, { id: "b", folder_id: null }];
  const groups = groupByFolder(orphaned, [], false);
  const all = groups.flatMap((g) => g.rows.map((r) => r.id));
  expect(all).toContain("a");
  expect(all).toContain("b");
});

test("puts an orphan row in Uncategorized rather than inventing a folder for it", () => {
  const orphaned = [{ id: "a", folder_id: "gone" }];
  const groups = groupByFolder(orphaned, [], false);
  // One group, the null-folder one — not a synthesised folder named after an id
  // the user has never seen.
  expect(groups).toHaveLength(1);
  expect(groups[0].folder).toBeNull();
  expect(groups[0].rows.map((r) => r.id)).toEqual(["a"]);
});

test("sends an orphan to Uncategorized even when real folders exist", () => {
  // The configuration the bug was ever visible in: a workspace that HAS folders,
  // and a row pointing at one that is not among them. Every other orphan case
  // here passes an empty folder list, where "is this id known?" and "are there
  // any folders?" cannot disagree — so they cannot see a fix that keys off the
  // folder count instead of the id.
  const g = groupByFolder(
    [{ id: "a", folder_id: "gone" }, { id: "b", folder_id: "f1" }],
    [{ id: "f1", name: "Alpha" }],
    false,
  );
  const alpha = g.find((x) => x.folder?.id === "f1");
  const uncat = g.find((x) => x.folder === null);

  // Present at all: the orphan is not dropped.
  expect(g.flatMap((x) => x.rows.map((r) => r.id))).toContain("a");
  // In the null-folder group.
  expect(uncat?.rows.map((r) => r.id)).toEqual(["a"]);
  // And NOT swept into the first real folder — a fix that did that would satisfy
  // both assertions above.
  expect(alpha?.rows.map((r) => r.id)).toEqual(["b"]);
});

test("still hides empty named folders when hideEmpty is set", () => {
  // The orphan fix must not resurrect the empty-folder behaviour hideEmpty
  // exists to suppress during a search.
  const oneRow = [{ id: "a", folder_id: "f1" }];
  const groups = groupByFolder(oneRow, [{ id: "f1", name: "One" }, { id: "f2", name: "Two" }], true);
  expect(groups.map((g) => g.folder?.name ?? null)).toEqual(["One"]);
});

test("keeps the caller's order in Uncategorized when one of the rows is an orphan", () => {
  // Every consumer sorts BEFORE grouping, so grouping must not reorder. An
  // orphan is folderless like any other folderless row and takes its sorted
  // place among them — it is not appended after them.
  const sortedRows = [
    { id: "alpha", folder_id: "gone" },
    { id: "middle", folder_id: null },
    { id: "zulu", folder_id: null },
  ];
  const groups = groupByFolder(sortedRows, [], false);
  expect(groups[0].rows.map((r) => r.id)).toEqual(["alpha", "middle", "zulu"]);
});

test("emits each row exactly once across all groups", () => {
  // A row belongs to one bucket. Asserted over the flattened output rather than
  // a group index, so it still holds however the buckets are built.
  const all = groupByFolder(rows, folders, false).flatMap((g) => g.rows.map((r) => r.id));
  expect([...all].sort()).toEqual(["a", "b", "c"]);
});

// ── showFolderSections / folderCollapseKey ───────────────────────────────────

test("showFolderSections is false with no folders, true with one", () => {
  // `groupByFolder` always returns at least the folderless bucket, so "is it
  // grouped" is not the same question and cannot be used as the gate.
  expect(showFolderSections([])).toBe(false);
  expect(showFolderSections(folders.slice(0, 1))).toBe(true);
});

test("showFolderSections is false in the trash however many folders exist", () => {
  // The trash is never sectioned: a trashed row's folder is where it will go
  // BACK to, not where it is, and this is the one view whose whole job is to
  // let the author find something they deleted.
  expect(showFolderSections(folders, true)).toBe(false);
  expect(showFolderSections(folders, false)).toBe(true);
});

test("folderCollapseKey separates workspace from kind", () => {
  // Two lists in one workspace, and one list across two workspaces, must not
  // share a collapse set — a shared key would collapse Charts by opening
  // Datasets. Both halves are varied, so a key built from only one of them
  // fails here rather than in a browser.
  expect(folderCollapseKey("eng", "charts")).toBe("folders:eng:charts");
  expect(folderCollapseKey("eng", "datasets")).not.toBe(folderCollapseKey("eng", "charts"));
  expect(folderCollapseKey("ops", "charts")).not.toBe(folderCollapseKey("eng", "charts"));
});
