import React, { Suspense, lazy } from "react";
import type { DiffEditorProps, EditorProps } from "@monaco-editor/react";

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
/** Monaco's diff editor, from the same chunk: the same `import()`, so no second Monaco. */
const DiffEditor = lazy(() => import("@/lib/monaco").then((m) => ({ default: m.DiffEditor })));

/**
 * Catches a failed chunk fetch so it costs the editor, not the page.
 *
 * A rejected `lazy()` import throws during render, and with no boundary the
 * whole React tree unmounts — `#root` goes empty and the user gets a blank
 * white page, not a broken editor pane. Measured that way before this existed.
 *
 * The trigger is routine, not exotic: a deploy replaces the content-hashed
 * `monaco-*.js` while someone still has the SPA open, and their next visit to
 * an editor page asks for a filename that no longer exists.
 *
 * This is also a behaviour the branch would otherwise have LOST. Monaco used to
 * arrive through `loader.init()`, whose rejection `@monaco-editor/react`
 * handles internally — so even a CDN outage left the surrounding page alive.
 * Moving the fetch to a `lazy()` boundary moved that responsibility here.
 *
 * Modelled on `OutputErrorBoundary` in `CellOutputRenderer.tsx`, the codebase's
 * only other `React.lazy` guard.
 */
class MonacoErrorBoundary extends React.Component<
  { children: React.ReactNode },
  { error: Error | null }
> {
  state = { error: null as Error | null };
  static getDerivedStateFromError(error: Error) {
    return { error };
  }
  componentDidCatch(error: Error, info: React.ErrorInfo) {
    console.error("Monaco failed to load:", error, info.componentStack);
  }
  render() {
    if (this.state.error) {
      return (
        <div
          role="alert"
          className="flex h-full w-full items-center justify-center p-4 text-center text-sm text-muted-foreground"
        >
          The code editor could not be loaded. Reload the page to try again.
        </div>
      );
    }
    return this.props.children;
  }
}

export default function MonacoEditor(props: EditorProps) {
  // The fallback fills the slot the editor is about to occupy, so the
  // surrounding layout does not jump when the chunk lands.
  return (
    <MonacoErrorBoundary>
      <Suspense fallback={<div className="h-full w-full" aria-hidden="true" />}>
        <Editor {...props} />
      </Suspense>
    </MonacoErrorBoundary>
  );
}

/**
 * Monaco's diff editor behind the same lazy boundary and error boundary as
 * {@link MonacoEditor}. Components import this, never `DiffEditor` from
 * `@monaco-editor/react`, for the same reasons.
 */
export function MonacoDiffEditor(props: DiffEditorProps) {
  return (
    <MonacoErrorBoundary>
      <Suspense fallback={<div className="h-full w-full" aria-hidden="true" />}>
        <DiffEditor {...props} />
      </Suspense>
    </MonacoErrorBoundary>
  );
}
