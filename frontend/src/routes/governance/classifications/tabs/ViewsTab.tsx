import type { Tag } from "@/api/tags";
import ViewExposureSection from "@/routes/governance/ViewExposureSection";

/**
 * View exposure (spec §5.4): `ViewExposureSection` as on the old page, read
 * with `tag:read`. Only a required role reaches an engine (a groups-only gate
 * compiles to nothing), so only a role makes the clearance engine-gated.
 */
export function ViewsTab({ tag }: { tag: Tag }) {
  return <ViewExposureSection tagId={tag.id} engineGated={tag.required_roles.length > 0} />;
}
