import { useEffect, useRef } from "react";

/** Is this event's target somewhere the user is typing?
 *
 *  Bare-letter shortcuts and text entry cannot share a keyboard. The dashboard has an inline name
 *  <input> (DashboardView.tsx), an inline heading <input> (HeadingCard.tsx:17-27) and an inline
 *  text <textarea> (TextCard.tsx:16-25); without this guard, typing "s" in a heading saves the
 *  dashboard and typing "e" leaves edit mode mid-word.
 *
 *  contentEditable is checked as well. Nothing on the dashboard uses it today, but it is the third
 *  way a browser says "the user is typing here", it costs one property read, and a card kind that
 *  grows a rich-text editor would otherwise reintroduce the bug silently. */
export function isEditableTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  const tag = target.tagName;
  if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return true;
  // `=== true` rather than a bare return: jsdom does not implement `isContentEditable`, so on an
  // ordinary element the property reads `undefined` — typed `boolean`, absent at runtime. Every
  // caller here treats the result as a boolean, and a predicate that answers `undefined` for "no"
  // is a trap for the next reader.
  return target.isContentEditable === true;
}

export interface DashboardShortcutHandlers {
  editing: boolean;
  /** How many tabs the dashboard has; `1`-`9` select one only when it exists. */
  tabCount: number;
  onEdit: () => void;
  onCancelEdit: () => void;
  onSave: () => void;
  onToggleBookmark: () => void;
  onToggleInfo: () => void;
  onAddFilter: () => void;
  onAddCard: () => void;
  onSelectTab: (pos: number) => void;
}

/** Metabase's dashboard shortcuts (palette/shortcuts/dashboard.ts), ported for the features egret
 *  has: `e` edit / cancel edit, `s` save, `o` bookmark, `]` info sidebar, `f` add filter,
 *  `a` add card, `1`-`9` select tab. Each keeps Metabase's mode — "When viewing" or
 *  "When editing" — so a key never means two things in one mode.
 *
 *  DELIBERATELY NOT PORTED:
 *
 *  - `q` and `n` ("Add query builder question" / "Add native question"). Both open Metabase's
 *    new-question-from-a-dashboard flow, which builds a question, saves it and drops it onto the
 *    board. egret has no such flow: a card is added by picking an ALREADY-SAVED chart
 *    (DashboardView.tsx, ChartPickerList). A key that opened the chart picker under the name
 *    "add native question" would be a different feature wearing Metabase's label, so `a` covers
 *    adding a card and there is nothing for `q`/`n` to do.
 *  - `$mod+backspace` ("Send dashboard to trash"). W4 gives the dashboard page a Move-to-trash
 *    action for the first time, and — matching the list page — it does not confirm. Unconfirmed is
 *    acceptable behind two deliberate clicks through a menu; behind one chord, with no undo
 *    anywhere in egret's dashboard surface, it is not. Design §4 W4 says the same. */
export function useDashboardShortcuts(handlers: DashboardShortcutHandlers): void {
  // The listener is registered ONCE and reads the current handlers through a ref, because the
  // route re-renders often (a card query landing, a filter changing) and rebinding a window
  // listener each time is pure cost. The ref is written in an effect rather than during render so
  // nothing observes a half-committed render.
  const ref = useRef(handlers);
  useEffect(() => { ref.current = handlers; });

  useEffect(() => {
    function onKeyDown(e: KeyboardEvent) {
      // Chords belong to the browser and the OS: Cmd+S saves the page, not the dashboard.
      if (e.ctrlKey || e.metaKey || e.altKey) return;
      // An IME is mid-composition; the keystroke belongs to the text being composed.
      if (e.isComposing) return;
      if (isEditableTarget(e.target)) return;
      const h = ref.current;

      if (e.key >= "1" && e.key <= "9") {
        const pos = Number(e.key) - 1;
        if (pos < h.tabCount) { e.preventDefault(); h.onSelectTab(pos); }
        return;
      }

      switch (e.key) {
        case "e":
          // The one key that means two things — Metabase gives it both "dashboard-edit" (when
          // viewing) and "dashboard-cancel-edit" (when editing).
          e.preventDefault();
          if (h.editing) h.onCancelEdit(); else h.onEdit();
          return;
        case "s":
          if (!h.editing) return;
          e.preventDefault(); h.onSave(); return;
        case "f":
          if (!h.editing) return;
          e.preventDefault(); h.onAddFilter(); return;
        case "a":
          if (!h.editing) return;
          e.preventDefault(); h.onAddCard(); return;
        case "o":
          if (h.editing) return;
          e.preventDefault(); h.onToggleBookmark(); return;
        case "]":
          if (h.editing) return;
          e.preventDefault(); h.onToggleInfo(); return;
        default:
          return;
      }
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);
}

/** Null-rendering child that registers the shortcuts.
 *
 *  DashboardView returns early while the dashboard is loading and every handler above is defined
 *  AFTER that return, so the hook cannot be called from the route body without breaking the Rules
 *  of Hooks. Rendering a child that calls it is the pattern the route already uses for exactly this
 *  reason — see `ParamOptions`, `StaticOptions` and `CardEmptinessProbe`. No JSX is needed, so this
 *  file stays a `.ts` as the plan index names it. */
export function DashboardShortcuts(props: DashboardShortcutHandlers): null {
  useDashboardShortcuts(props);
  return null;
}
