import type { Tag } from "@/api/tags";
import ViewExposureSection from "@/routes/governance/ViewExposureSection";

/**
 * Views (spec §5.4; "View exposure" before the glossary): `ViewExposureSection` as on the old page, read
 * with `tag:read`. A required role or a required group is enforced on the
 * engines, so either makes the clearance engine-gated.
 */
export function ViewsTab({ tag }: { tag: Tag }) {
  return <ViewExposureSection tagId={tag.id} engineGated={tag.required_roles.length + tag.required_groups.length > 0} />;
}
