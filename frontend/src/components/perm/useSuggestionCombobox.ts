import { useEffect, useId, useRef, useState, type FocusEvent, type KeyboardEvent, type RefObject } from "react";

/** How long typing pauses before a suggestion search is sent. */
export const SEARCH_DEBOUNCE_MS = 200;

/** `value`, once it has stopped changing for `SEARCH_DEBOUNCE_MS`. */
export function useDebounced<T>(value: T): T {
  const [settled, setSettled] = useState(value);
  useEffect(() => {
    const t = setTimeout(() => setSettled(value), SEARCH_DEBOUNCE_MS);
    return () => clearTimeout(t);
  }, [value]);
  return settled;
}

/**
 * The keyboard and focus behaviour of a suggestion field (ARIA 1.2 combobox
 * with a listbox popup), shared by every subject and principal search in
 * governance: `SubjectUserField` (Check access, Who can read?),
 * `SubjectNameField` and the PrincipalPicker's search (DG redesign #2, live
 * check D1/D2).
 *
 * - Arrow keys move through the `count` options with `aria-activedescendant`;
 *   focus stays in the field, and options never take it.
 * - Enter on the active option picks it (`onPick`) instead of submitting the
 *   form around the field.
 * - Escape closes the list and nothing else. An enclosing Radix dialog listens
 *   for Escape in the document's capture phase, before any handler on the
 *   field, and dismisses itself unless the event is already
 *   `defaultPrevented`; a window capture listener runs first and prevents it.
 *   With the list closed, Escape is the dialog's again.
 * - Focus leaving the box (`boxRef`, the caller's: the field and its popup) closes the list,
 *   so Tab moves on to the next control; focus moving inside it keeps it open.
 * - `aria-controls` names the listbox only while it is rendered: the caller
 *   renders `<ul role="listbox" id={listId}>` whenever `open`, even with no
 *   option, and keeps status text outside it.
 */
export function useSuggestionCombobox({
  count,
  onPick,
  closeOnPick = true,
  boxRef,
  inputRef,
}: {
  /** How many pickable options the listbox holds. */
  count: number;
  /** Picks the option at `index`. */
  onPick: (index: number) => void;
  /** False keeps the list open after a pick, for several picks in a row. */
  closeOnPick?: boolean;
  /** The element around the field and its popup (`ref` on it is the caller's). */
  boxRef: RefObject<HTMLDivElement | null>;
  /** The field (`ref` on it is the caller's); Escape hands focus back to it. */
  inputRef: RefObject<HTMLInputElement | null>;
}) {
  const listId = `${useId()}-list`;
  const skipNextFocusOpen = useRef(false);
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(-1);
  const activeRow = open && active >= 0 && active < count ? active : -1;

  const close = () => {
    setOpen(false);
    setActive(-1);
  };

  useEffect(() => {
    if (!open) return;
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (e.key !== "Escape" || !(e.target instanceof Node) || !boxRef.current?.contains(e.target)) return;
      e.preventDefault();
      setOpen(false);
      setActive(-1);
      inputRef.current?.focus();
    };
    window.addEventListener("keydown", onKey, { capture: true });
    return () => window.removeEventListener("keydown", onKey, { capture: true });
  }, [open, boxRef, inputRef]);

  const pick = (index: number) => {
    onPick(index);
    if (closeOnPick) close();
  };

  const closeUnlessInside = (e: FocusEvent) => {
    const next = e.relatedTarget;
    if (next instanceof Node && boxRef.current?.contains(next)) return;
    close();
  };

  return {
    open,
    activeRow,
    listId,
    close,
    pick,
    /** The id of option `index`, for its `<li>`. */
    optionId: (index: number) => `${listId}-${index}`,
    /** The field changed: reopen the list with no option active. */
    typed: () => {
      setOpen(true);
      setActive(-1);
    },
    /** The next focus of the field (focus handed back from a dialog) leaves the list closed. */
    skipNextFocusOpen: () => {
      skipNextFocusOpen.current = true;
    },
    /** Spread on the field's `<Input>`, which also takes `ref={inputRef}`. */
    inputProps: {
      role: "combobox" as const,
      "aria-expanded": open,
      "aria-controls": open ? listId : undefined,
      "aria-autocomplete": "list" as const,
      "aria-activedescendant": activeRow >= 0 ? `${listId}-${activeRow}` : undefined,
      autoComplete: "off",
      onFocus: () => {
        if (skipNextFocusOpen.current) skipNextFocusOpen.current = false;
        else setOpen(true);
      },
      onClick: () => setOpen(true),
      onBlur: closeUnlessInside,
      onKeyDown: (e: KeyboardEvent<HTMLInputElement>) => {
        if (e.key === "ArrowDown" || e.key === "ArrowUp") {
          e.preventDefault();
          setOpen(true);
          if (count === 0) return;
          const step = e.key === "ArrowDown" ? 1 : -1;
          setActive((i) => (i < 0 || i >= count ? (step > 0 ? 0 : count - 1) : (i + step + count) % count));
        } else if (e.key === "Enter" && activeRow >= 0) {
          e.preventDefault();
          pick(activeRow);
        }
      },
    },
    /** Spread on the popup around the listbox: focus leaving it closes the list. */
    popupProps: { onBlur: closeUnlessInside },
  };
}
