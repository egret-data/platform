import { Suspense, lazy } from "react";
import type { EditorProps } from "@monaco-editor/react";

/**
 * The single lazy boundary for Monaco, and the only module allowed to reach
 * `@/lib/monaco`.
 *
 * Monaco is ~3.8MB minified — roughly the size of the entire rest of the app.
 * `App.tsx` has no lazy routes, so anything statically reachable from it lands
 * in the one eager chunk that the login screen downloads. Putting the boundary
 * at the *component* rather than at the route means a page cannot pull Monaco
 * into that chunk by accident: a future editor-hosting page gets the lazy
 * behaviour for free just by rendering this, with no route wiring to remember.
 *
 * It also keeps the ordering the loader needs. `@/lib/monaco` calls
 * `loader.config({ monaco })` while the chunk evaluates; `<Editor>` calls
 * `loader.init()` when it mounts, which cannot happen until the promise
 * `lazy()` is waiting on has already resolved. No race, no CDN fallback.
 *
 * Editor components import THIS, not `@monaco-editor/react` — importing the
 * real package from a component would drag Monaco into jsdom for every test
 * that renders an editor, so the component tests stub this module instead.
 */
const Editor = lazy(() => import("@/lib/monaco"));

export default function MonacoEditor(props: EditorProps) {
  // The fallback fills the slot the editor is about to occupy, so the
  // surrounding layout does not jump when the chunk lands.
  return (
    <Suspense fallback={<div className="h-full w-full" aria-hidden="true" />}>
      <Editor {...props} />
    </Suspense>
  );
}
