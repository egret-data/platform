import { useState } from "react";
import { Link } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";
import { Loader2 } from "lucide-react";
import {
  getPolicyAdminListProfilesQueryKey,
  usePolicyAdminListProfiles,
  usePolicyAdminSetTagService,
} from "@/api/generated/policy-admin/policy-admin";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { refusalMessage, usePolicyPermissions } from "@/routes/governance/policyWrites";
import { Section } from "@/routes/governance/tabParts";
import { ENGINES_BASE } from "@/routes/governance/engines/engineUrl";
import type { FrameProfile } from "@/routes/governance/engines/PageFrame";

/** The Select's value for "no tag service" (a Radix item cannot have an empty value). */
export const NO_TAG_SERVICE = "__none__";

export const NO_TAG_SERVICE_SENTENCE =
  "This profile has no tag service, so no engine can be bound to it: classification masks and row filters would reach nothing.";
export const TAG_SERVICE_EXPLAINED =
  "The tag profile that holds this profile's tag-based policies. Only a profile of type Tag can be named.";
export const BOUND_READ_ONLY =
  "An engine downloads this profile, so its tag service is not changed here. Unbind the engine to change it.";
export const GENERATED_READ_ONLY = "egret compiles this profile, so its tag service is not changed here.";

/**
 * Tag service (#786): the `tag` profile a Trino profile's tag-based policies
 * live in. Settable on an unbound, manual profile with `policy:update`
 * (`PUT /policies/services/{service}/tag-service`); the server refuses the
 * change on a profile an engine downloads or egret compiles, so those show it
 * read-only with the reason. Only `tag` profiles are offered.
 */
export function TagServiceSection({ profile }: { profile: FrameProfile }) {
  const can = usePolicyPermissions();
  const qc = useQueryClient();
  const profilesQ = usePolicyAdminListProfiles();
  const current = profile.tagService;
  const [choice, setChoice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);
  const save = usePolicyAdminSetTagService();

  const dto = profile.dto;
  const bound = (dto?.engines.length ?? 0) > 0;
  const editable = !!dto && !bound && dto.origin === "manual" && can("policy:update");
  const tagProfiles =
    profilesQ.data?.status === 200 ? profilesQ.data.data.filter((p) => p.type === "tag").map((p) => p.name) : [];
  const selected = choice ?? current ?? NO_TAG_SERVICE;

  const submit = () => {
    setError(null);
    setDone(null);
    const tagService = selected === NO_TAG_SERVICE ? null : selected;
    save.mutate(
      { service: profile.name, data: { tagService } },
      {
        onSuccess: () => {
          void qc.invalidateQueries({ queryKey: getPolicyAdminListProfilesQueryKey() });
          setChoice(null);
          setDone(tagService ? `Tag service set to ${tagService}.` : "Tag service removed.");
        },
        onError: (e) => setError(refusalMessage(e, "policy:update", "The tag service could not be changed.")),
      },
    );
  };

  let reason: string | null = null;
  if (bound) reason = BOUND_READ_ONLY;
  else if (dto?.origin === "generated") reason = GENERATED_READ_ONLY;

  return (
    <Section title="Tag service">
      <p className="max-w-prose text-sm text-muted-foreground">{TAG_SERVICE_EXPLAINED}</p>
      {!current ? <p className="max-w-prose text-sm">{NO_TAG_SERVICE_SENTENCE}</p> : null}
      {editable ? (
        <div className="space-y-3">
          <div className="flex flex-wrap items-end gap-3">
            <div className="space-y-1.5">
              <Label htmlFor="profile-tag-service">Tag service</Label>
              <Select
                value={selected}
                onValueChange={(v) => {
                  setChoice(v);
                  setDone(null);
                }}
              >
                <SelectTrigger id="profile-tag-service" className="w-72 max-w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value={NO_TAG_SERVICE}>None</SelectItem>
                  {tagProfiles.map((t) => (
                    <SelectItem key={t} value={t}>
                      {t}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            <Button
              size="sm"
              onClick={submit}
              disabled={save.isPending || selected === (current ?? NO_TAG_SERVICE)}
            >
              {save.isPending ? <Loader2 className="mr-1 h-4 w-4 animate-spin" aria-hidden /> : null}
              Save tag service
            </Button>
          </div>
          {tagProfiles.length === 0 && profilesQ.data ? (
            <p className="text-sm text-muted-foreground">
              There is no tag profile yet. Create one with New profile on{" "}
              <Link to={ENGINES_BASE} className="text-primary underline focus-visible:ring-2 focus-visible:ring-ring">
                Engine &amp; policy
              </Link>
              , type Tag.
            </p>
          ) : null}
          <p role="status" className="text-sm">
            {done}
          </p>
          {error ? (
            <Alert variant="destructive">
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          ) : null}
        </div>
      ) : (
        <div className="space-y-1 text-sm">
          <p>
            <span className="text-muted-foreground">Tag service: </span>
            <span className="font-medium">{current ?? "None"}</span>
          </p>
          {reason ? <p className="text-muted-foreground">{reason}</p> : null}
        </div>
      )}
    </Section>
  );
}
