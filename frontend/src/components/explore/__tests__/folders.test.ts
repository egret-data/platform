import { expect, test } from "vitest";
import { groupByFolder } from "@/components/explore/folders";

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
