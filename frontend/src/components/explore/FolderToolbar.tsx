import { FolderPlus } from "lucide-react";
import { ListToolbarButton } from "@/components/explore/ListToolbarButton";

/**
 * The New-folder control, as a header toolbar button.
 *
 * Deliberately just the button. The dialog it opens needs the folder list and
 * the folder mutations, both of which the page already holds for its grouping —
 * a toolbar that fetched them would own a second copy of the same query, and
 * the two could disagree about which folders exist.
 *
 * Not `active`/`filled`: this is a command, not a filter. The bookmark and bulk
 * toggles beside it stay pressed while they are engaged; "New folder" has no
 * engaged state to report, and passing `active` here — even `false` — would
 * mark it up as a toggle (`aria-pressed`) it isn't. Omitting `active` is what
 * keeps this a plain button rather than a toggle announcing "not pressed".
 */
export function FolderToolbar({ onNewFolder }: { onNewFolder: () => void }) {
  return <ListToolbarButton icon={FolderPlus} label="New folder" onClick={onNewFolder} />;
}
