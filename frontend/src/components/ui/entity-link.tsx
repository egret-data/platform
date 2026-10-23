import { Link, type LinkProps } from "react-router-dom";
import { cn } from "@/lib/utils";

/**
 * An object's name in a list, as a link to that object (FE-VISUAL-7, KI-223).
 *
 * One colour everywhere — Explore, Governance and Control Plane: the text
 * colour, `font-medium`, underlined on hover. Teal (`--primary`) is the action
 * colour; a name is not an action, and a list where some names were teal and
 * others black read as two kinds of link.
 *
 * `ENTITY_LINK_CLASS` is for a name inside a larger link (a whole-row anchor):
 * put `group` on the row and use `ENTITY_LINK_IN_GROUP_CLASS` on the name.
 */
export const ENTITY_LINK_CLASS = "font-medium text-foreground hover:underline";
export const ENTITY_LINK_IN_GROUP_CLASS = "font-medium text-foreground group-hover:underline";

export function EntityLink({ className, ...props }: LinkProps) {
  return <Link className={cn(ENTITY_LINK_CLASS, className)} {...props} />;
}
