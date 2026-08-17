import { Plus, Trash2 } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type { EngineConfigForm } from "../useEngineConfigForm";
import { ConfigSection } from "@/components/form/ConfigSection";
import { ConfigTable, ConfigTableRow } from "@/components/config/ConfigTable";
import { SELECT_NONE } from "./podEditors";

/**
 * "Image" section — mirrors the chart `image` + `imagePullSecrets`: the full
 * container image string (overrides the default `trinodb/trino:{version}`), the
 * pull policy, and pull secrets. `defaultImage` is shown as the placeholder so a
 * blank field clearly means "use the default image".
 */
export function ImageSection({
  cfg,
  defaultImage,
}: {
  cfg: EngineConfigForm;
  defaultImage: string;
}) {
  const {
    image,
    setImage,
    imagePullPolicy,
    setImagePullPolicy,
    imagePullSecrets,
    setImagePullSecrets,
  } = cfg;

  return (
    <div className="space-y-5">
      <ConfigSection
        title="Image"
        description="Container image for the coordinator and worker. Leave blank to use the default."
      >
        <ConfigTable>
          <ConfigTableRow
            label="Image"
            htmlFor="img-image"
            details={`Full registry/repository:tag. Default: ${defaultImage}.`}
          >
            <Input
              id="img-image"
              className="w-full"
              value={image}
              placeholder={defaultImage}
              onChange={(e) => setImage(e.target.value)}
            />
          </ConfigTableRow>
          <ConfigTableRow
            label="Image pull policy"
            htmlFor="img-pullpolicy"
            details="When the kubelet re-pulls the image. Leave default unless you need Always/Never."
          >
            <Select
              value={imagePullPolicy || SELECT_NONE}
              onValueChange={(v) => setImagePullPolicy(v === SELECT_NONE ? "" : v)}
            >
              <SelectTrigger id="img-pullpolicy" className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value={SELECT_NONE}>(default)</SelectItem>
                <SelectItem value="Always">Always</SelectItem>
                <SelectItem value="IfNotPresent">IfNotPresent</SelectItem>
                <SelectItem value="Never">Never</SelectItem>
              </SelectContent>
            </Select>
          </ConfigTableRow>
        </ConfigTable>

        <div className="space-y-2">
          <Label>Image pull secrets</Label>
          <p className="text-xs text-muted-foreground">
            Names of Kubernetes <code>docker-registry</code> secrets in the engine
            namespace.
          </p>
          {imagePullSecrets.map((s, i) => (
            <div key={i} className="flex items-center gap-2">
              <Input
                value={s}
                placeholder="regcred"
                onChange={(e) =>
                  setImagePullSecrets(
                    imagePullSecrets.map((v, j) => (j === i ? e.target.value : v)),
                  )
                }
              />
              <Button
                type="button"
                variant="ghost"
                size="icon"
                className="shrink-0 text-muted-foreground"
                onClick={() =>
                  setImagePullSecrets(imagePullSecrets.filter((_, j) => j !== i))
                }
                aria-label="Remove secret"
              >
                <Trash2 className="h-4 w-4" />
              </Button>
            </div>
          ))}
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() => setImagePullSecrets([...imagePullSecrets, ""])}
          >
            <Plus className="h-4 w-4" />
            Add secret
          </Button>
        </div>
      </ConfigSection>
    </div>
  );
}
