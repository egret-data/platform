import { useState } from "react";

import { useSecrets } from "@/api/secrets";
import { pickableSecrets } from "@/lib/systemSecrets";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

export interface SecretSelection {
  secret_id?: string;
  new_secret?: { name: string; value: string };
}

interface SecretPickerProps {
  ws: string;
  value: SecretSelection;
  /** True when the connection being edited already has a secret on the server. */
  hasExisting?: boolean;
  onChange: (v: SecretSelection) => void;
}

// Sentinel select values — must not collide with a real secret id.
const KEEP = "__keep__";
const NEW = "__new__";

/**
 * Controlled secret picker for connection forms. Lets the user reference an
 * existing named workspace secret, create a new one inline, or (on edit) keep
 * the current secret untouched.
 *
 * Emitted selection:
 *   - keep current  → `{}` (no secret fields → backend keeps the existing one)
 *   - pick existing → `{ secret_id }`
 *   - new secret    → `{ new_secret: { name, value } }`
 */
export function SecretPicker({ ws, value, hasExisting, onChange }: SecretPickerProps) {
  const secrets = useSecrets(ws);
  const items = pickableSecrets(secrets.data ?? []);

  // Local fields for the inline "new secret" form.
  const [newName, setNewName] = useState(value.new_secret?.name ?? "");
  const [newValue, setNewValue] = useState(value.new_secret?.value ?? "");

  // Derive the current Select value from the controlled selection.
  let selectValue: string;
  if (value.secret_id) {
    selectValue = value.secret_id;
  } else if (value.new_secret) {
    selectValue = NEW;
  } else {
    // No secret fields set: keep-current (edit) or unselected (create).
    selectValue = hasExisting ? KEEP : "";
  }

  function handleSelect(next: string) {
    if (next === KEEP) {
      onChange({});
    } else if (next === NEW) {
      onChange({ new_secret: { name: newName, value: newValue } });
    } else {
      onChange({ secret_id: next });
    }
  }

  function handleNewName(name: string) {
    setNewName(name);
    onChange({ new_secret: { name, value: newValue } });
  }

  function handleNewValue(val: string) {
    setNewValue(val);
    onChange({ new_secret: { name: newName, value: val } });
  }

  const showNew = value.new_secret !== undefined;

  return (
    <div className="space-y-2">
      <Label htmlFor="secret-picker">Secret</Label>
      <Select value={selectValue} onValueChange={handleSelect}>
        <SelectTrigger id="secret-picker" className="w-full">
          <SelectValue placeholder="Select a secret" />
        </SelectTrigger>
        <SelectContent>
          {hasExisting && <SelectItem value={KEEP}>Keep current secret</SelectItem>}
          {items.map((s) => (
            <SelectItem key={s.id} value={s.id}>
              {s.name}
            </SelectItem>
          ))}
          <SelectItem value={NEW}>＋ New secret</SelectItem>
        </SelectContent>
      </Select>

      {showNew && (
        <div className="space-y-3 pt-1">
          <div className="space-y-1.5">
            <Label htmlFor="new-secret-name">New secret name</Label>
            <Input
              id="new-secret-name"
              className="w-full"
              value={newName}
              onChange={(e) => handleNewName(e.target.value)}
              placeholder="e.g. prod-s3-secret"
            />
          </div>
          <div className="space-y-1.5">
            <Label htmlFor="new-secret-value">New secret value</Label>
            <Input
              id="new-secret-value"
              type="password"
              className="w-full"
              value={newValue}
              onChange={(e) => handleNewValue(e.target.value)}
              placeholder="Enter a value"
              autoComplete="new-password"
            />
          </div>
        </div>
      )}
    </div>
  );
}

export default SecretPicker;
