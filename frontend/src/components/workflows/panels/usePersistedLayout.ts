import { useCallback, useEffect, useRef, useState } from "react";
import { normalise, parseLayout, type Layout } from "@/components/workflows/panels/layout";
import { getTabs, useTabsVersion } from "@/components/workflows/panels/registry";

/** `egret.wf.layout.<flow uuid>` (spec §5.1). */
export const LAYOUT_KEY_PREFIX = "egret.wf.layout.";

const WRITE_DEBOUNCE_MS = 300;

/**
 * Read a stored layout. Every way this can go wrong — storage that throws
 * (blocked, private mode), nothing stored, text that is not JSON, JSON of the
 * wrong shape — ends at the defaults; a layout of the right shape is
 * normalised against the tabs registered now (FE-DATA-3).
 */
function read(key: string | null, defaults: Layout): Layout {
  const known = getTabs().map((t) => t.id);
  const fallback = normalise(defaults, known);
  if (key === null) return fallback;
  let raw: string | null;
  try {
    raw = localStorage.getItem(key);
  } catch {
    return fallback;
  }
  if (raw === null) return fallback;
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return fallback;
  }
  const layout = parseLayout(parsed);
  return layout ? normalise(layout, known) : fallback;
}

function write(key: string, layout: Layout): void {
  try {
    localStorage.setItem(key, JSON.stringify(layout));
  } catch {
    // A per-viewer preference: without storage the layout lives in memory for this visit.
  }
}

/**
 * The editor layout of one flow, kept in `localStorage` as a per-viewer
 * preference (FE-DATA-3). It holds no server data: only which tab sits where
 * and how wide each panel is.
 *
 * - `flowKey` is the flow's uuid. An empty key (a flow not created yet) keeps
 *   the layout in memory only.
 * - A change is written 300 ms after the last one, so dragging a gutter is one
 *   write; a change still pending when the flow changes or the editor unmounts
 *   is written then, under the key it was made for.
 */
export function usePersistedLayout(flowKey: string, defaults: Layout): [Layout, (l: Layout) => void] {
  const key = flowKey ? `${LAYOUT_KEY_PREFIX}${flowKey}` : null;
  const version = useTabsVersion();
  const [state, setState] = useState(() => ({ key, version, edited: false, layout: read(key, defaults) }));
  // Another flow: read its layout during render rather than after, so the
  // previous flow's layout is never painted under the new flow's name.
  //
  // A tab registered late (a lazy chunk): `read` dropped it as unknown, so read
  // again now that it is known. Nothing has been written meanwhile unless the
  // viewer changed the layout first; then their layout stands, and a late tab
  // it does not name stays closed until reopened from "+".
  let current = state;
  if (state.key !== key) {
    current = { key, version, edited: false, layout: read(key, defaults) };
    setState(current);
  } else if (state.version !== version) {
    current = { ...state, version, layout: state.edited ? state.layout : read(key, defaults) };
    setState(current);
  }

  const pending = useRef<{ key: string; layout: Layout } | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const flush = useCallback(() => {
    if (timer.current !== null) clearTimeout(timer.current);
    timer.current = null;
    if (pending.current) write(pending.current.key, pending.current.layout);
    pending.current = null;
  }, []);

  useEffect(() => flush, [key, flush]);

  const setLayout = useCallback((layout: Layout) => {
    setState({ key, version, edited: true, layout });
    if (key === null) return;
    pending.current = { key, layout };
    if (timer.current !== null) clearTimeout(timer.current);
    timer.current = setTimeout(flush, WRITE_DEBOUNCE_MS);
  }, [key, version, flush]);

  return [current.layout, setLayout];
}
