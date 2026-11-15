import { describe, expect, it } from "vitest";
import { insideWorkingDirectory, workingDirectoryTaskIds } from "@/components/workflows/executions/workingDirectory";

const SOURCE = `id: hello
namespace: company.team
tasks:
  - id: wd
    type: io.kestra.plugin.core.flow.WorkingDirectory
    tasks:
      - id: inner
        type: io.kestra.plugin.core.log.Log
        message: hi
  - id: seq
    type: io.kestra.plugin.core.flow.Sequential
    tasks:
      - id: short
        type: core.workingdirectory
        tasks:
          - id: deep
            type: io.kestra.plugin.core.log.Log
            message: hi
errors:
  - id: after
    type: io.kestra.plugin.core.log.Log
    message: hi
`;

const run = (task_id: string, run_ix: number, parent_ix: number | null = null) => ({ task_id, run_ix, parent_ix });

describe("workingDirectoryTaskIds", () => {
  it("finds every WorkingDirectory task, at any depth and in either spelling the server accepts", () => {
    expect([...workingDirectoryTaskIds(SOURCE)].sort()).toEqual(["short", "wd"]);
  });

  it("finds none in a source that does not parse", () => {
    expect(workingDirectoryTaskIds("tasks: [").size).toBe(0);
  });
});

describe("insideWorkingDirectory", () => {
  const runs = [run("wd", 0), run("inner", 1, 0), run("seq", 2), run("short", 3, 2), run("deep", 4, 3), run("after", 5)];
  const wd = workingDirectoryTaskIds(SOURCE);

  it("is true for a run with a WorkingDirectory ancestor, however deep", () => {
    expect(insideWorkingDirectory(runs[1], runs, wd)).toBe(true);
    expect(insideWorkingDirectory(runs[4], runs, wd)).toBe(true);
  });

  it("is false for the block itself and for runs outside every block", () => {
    for (const r of [runs[0], runs[2], runs[3], runs[5]]) expect(insideWorkingDirectory(r, runs, wd)).toBe(false);
  });
});
