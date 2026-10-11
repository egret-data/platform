/**
 * `monaco-yaml` for the flow editor: schema completion, hover and structural
 * errors as you type (spec §5.2).
 *
 * This module is reached ONLY through the `import()` in `CodeTab.tsx`, after
 * Monaco itself has loaded, so `monaco-yaml` gets its own lazy chunk and no
 * page but the flow editor downloads it. Its worker is wired where Monaco's
 * own is, in `src/lib/monaco.ts` (`label === "yaml"`).
 * `scripts/assert-monaco-lazy.mjs` fails if either reaches the entry chunk or
 * the Workflows area's own chunk.
 *
 * The setup follows Kestra's `yamlLanguageConfigurator.ts`: one
 * `configureMonacoYaml` instance for the app, updated in place (Kestra notes
 * that configuring again stacks a second instance that cannot be turned off).
 */
import type { Monaco } from "@monaco-editor/react";
import { configureMonacoYaml, type JSONSchema, type MonacoYaml } from "monaco-yaml";
import { FLOW_MODEL_GLOB } from "@/components/workflows/editor/flowModel";

let instance: MonacoYaml | null = null;
/** The ETag the instance was last configured with; `undefined` before the first call. */
let configured: string | null | undefined;

type Input = {
  schema: unknown;
  /** The schema response's `ETag`; null when there is no schema (its fetch failed). */
  etag: string | null;
  workspace: string;
};

function schemas({ schema, workspace }: Input) {
  if (!schema) return [];
  return [
    {
      // Shown as the source in hover cards; never fetched (`enableSchemaRequest: false`).
      uri: `${window.location.origin}/api/ws/${encodeURIComponent(workspace)}/workflows/schema`,
      fileMatch: [FLOW_MODEL_GLOB],
      schema: schema as JSONSchema,
    },
  ];
}

type WorkerRequest = { label: string; moduleId: string; createData: unknown };

/**
 * Monaco as `monaco-yaml` 5.5.1 expects it, with one call changed.
 *
 * `monaco-yaml`'s worker manager (`monaco-worker-manager` 2.0.1) starts its
 * worker with `monaco.editor.createWebWorker({ label, moduleId, createData })`,
 * the pre-0.55 signature. At monaco-editor 0.56 that function takes
 * `{ worker }` (`IInternalWebWorkerOptions`) and, given anything else, falls
 * back to an in-page worker with no YAML methods: every request then fails with
 * "Missing requestHandler or method: doValidation" and there is no completion,
 * hover or schema error at all. Measured in Chromium on this editor.
 *
 * So the call is answered the way monaco 0.56's own language workers do it
 * (`createWebWorker` in `monaco-editor/esm/vs/internal/common/workers.js`):
 * the worker comes from `MonacoEnvironment.getWorker` (`src/lib/monaco.ts`,
 * label `yaml`) and is primed with an "ignore" message, then `createData`.
 * Kestra, on the same versions, patches `monaco-worker-manager` to the same
 * effect (`kestra/ui/patches/monaco-worker-manager+2.0.1.patch`).
 */
function forMonacoYaml(monaco: Monaco): Monaco {
  const createWebWorker = ({ label, moduleId, createData }: WorkerRequest) => {
    const getWorker = self.MonacoEnvironment?.getWorker;
    if (!getWorker) throw new Error("MonacoEnvironment.getWorker is not set; src/lib/monaco.ts sets it.");
    const worker = Promise.resolve(getWorker(moduleId, label)).then((w) => {
      w.postMessage("ignore");
      w.postMessage(createData);
      return w;
    });
    return monaco.editor.createWebWorker({ worker });
  };
  return { ...monaco, editor: { ...monaco.editor, createWebWorker } };
}

/** Configure `monaco-yaml` for the flow models, once per schema ETag. */
export function configureFlowYaml(monaco: Monaco, input: Input): void {
  if (!instance) {
    instance = configureMonacoYaml(forMonacoYaml(monaco) as never, {
      enableSchemaRequest: false,
      completion: true,
      hover: true,
      validate: true,
      schemas: schemas(input),
    });
    configured = input.etag;
    return;
  }
  if (input.etag === configured) return;
  instance.update({ schemas: schemas(input) });
  configured = input.etag;
}
