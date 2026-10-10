import { beforeEach, describe, expect, it, vi } from "vitest";

const update = vi.fn();
const configureMonacoYaml = vi.fn((..._a: unknown[]) => ({ update, dispose: vi.fn(), getOptions: vi.fn() }));
vi.mock("monaco-yaml", () => ({ configureMonacoYaml: (...a: unknown[]) => configureMonacoYaml(...a) }));

const lowLevelCreateWebWorker = vi.fn((opts: unknown) => ({ opts }));
const monaco = { tag: "monaco", editor: { createWebWorker: lowLevelCreateWebWorker, other: "kept" } };
const schemaA = { title: "A" };
const schemaB = { title: "B" };

beforeEach(() => {
  vi.resetModules();
  lowLevelCreateWebWorker.mockClear();
  configureMonacoYaml.mockClear();
  update.mockClear();
});

/** A fresh module per test: the instance is module state, as in the app. */
const load = () => import("@/components/workflows/editor/monacoYaml");

describe("configureFlowYaml", () => {
  it("configures monaco-yaml once, for the flow models, with the served schema", async () => {
    const { configureFlowYaml } = await load();
    configureFlowYaml(monaco as never, { schema: schemaA, etag: '"a"', workspace: "ws1" });
    expect(configureMonacoYaml).toHaveBeenCalledTimes(1);
    const [m, opts] = configureMonacoYaml.mock.calls[0] as [typeof monaco, Record<string, unknown>];
    expect(m.tag).toBe("monaco");
    expect(m.editor.other).toBe("kept");
    expect(opts).toMatchObject({ enableSchemaRequest: false, completion: true, hover: true, validate: true });
    expect(opts.schemas).toEqual([
      {
        uri: `${window.location.origin}/api/ws/ws1/workflows/schema`,
        fileMatch: ["inmemory://egret/flow/*.yaml"],
        schema: schemaA,
      },
    ]);
  });

  it("does nothing again for the same ETag, and updates in place for a new one", async () => {
    const { configureFlowYaml } = await load();
    configureFlowYaml(monaco as never, { schema: schemaA, etag: '"a"', workspace: "ws1" });
    configureFlowYaml(monaco as never, { schema: { ...schemaA }, etag: '"a"', workspace: "ws1" });
    expect(configureMonacoYaml).toHaveBeenCalledTimes(1);
    expect(update).not.toHaveBeenCalled();

    configureFlowYaml(monaco as never, { schema: schemaB, etag: '"b"', workspace: "ws1" });
    expect(configureMonacoYaml).toHaveBeenCalledTimes(1);
    expect(update).toHaveBeenCalledTimes(1);
    expect((update.mock.calls[0][0] as { schemas: { schema: unknown }[] }).schemas[0].schema).toBe(schemaB);
  });

  it("without a schema, still gives YAML checks, with no schema attached", async () => {
    const { configureFlowYaml } = await load();
    configureFlowYaml(monaco as never, { schema: null, etag: null, workspace: "ws1" });
    expect((configureMonacoYaml.mock.calls[0][1] as { schemas: unknown[] }).schemas).toEqual([]);
    configureFlowYaml(monaco as never, { schema: schemaA, etag: '"a"', workspace: "ws1" });
    expect(update).toHaveBeenCalledTimes(1);
  });
});

describe("the worker monaco-yaml starts", () => {
  it("comes from MonacoEnvironment by its label and is primed with its createData (monaco 0.56)", async () => {
    const posted: unknown[] = [];
    const worker = { postMessage: (m: unknown) => posted.push(m) };
    const getWorker = vi.fn(() => worker);
    (self as unknown as { MonacoEnvironment: unknown }).MonacoEnvironment = { getWorker };
    const { configureFlowYaml } = await load();
    configureFlowYaml(monaco as never, { schema: null, etag: null, workspace: "ws1" });
    const passed = configureMonacoYaml.mock.calls[0][0] as { editor: { createWebWorker: (o: unknown) => unknown } };
    const createData = { completion: true };

    // monaco-worker-manager's call, in its pre-0.55 shape.
    passed.editor.createWebWorker({ label: "yaml", moduleId: "monaco-yaml/yaml.worker", createData });

    expect(getWorker).toHaveBeenCalledWith("monaco-yaml/yaml.worker", "yaml");
    // monaco 0.56's own signature: `{ worker }`, nothing else.
    const arg = lowLevelCreateWebWorker.mock.calls[0][0] as { worker: Promise<unknown> };
    expect(Object.keys(arg)).toEqual(["worker"]);
    await expect(arg.worker).resolves.toBe(worker);
    expect(posted).toEqual(["ignore", createData]);
  });
});
