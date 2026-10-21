import { describe, it, expect, vi, beforeEach } from "vitest";
import { useState } from "react";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";

const secretsMock = vi.fn();

vi.mock("@/api/secrets", () => ({
  useSecrets: () => secretsMock(),
}));

import { SecretPicker, type SecretSelection } from "@/components/SecretPicker";

// Drive the underlying Radix Select via its hidden native <select>.
function selectOption(value: string) {
  const native = Array.from(
    document.querySelectorAll<HTMLSelectElement>("select"),
  ).find((s) => Array.from(s.options).some((o) => o.value === value));
  if (!native) throw new Error(`option ${value} not found`);
  fireEvent.change(native, { target: { value } });
}

// Controlled wrapper so the picker's emitted selection feeds back as its value,
// matching how the connection forms drive it.
// Radix Select only renders its hidden native <select> (which the tests drive)
// when it lives inside a <form>, matching how the connection dialogs mount it.
function Harness({
  hasExisting,
  onEmit,
}: {
  hasExisting?: boolean;
  onEmit: (v: SecretSelection) => void;
}) {
  const [val, setVal] = useState<SecretSelection>({});
  return (
    <form>
      <SecretPicker
        ws="eng"
        value={val}
        hasExisting={hasExisting}
        onChange={(v) => {
          setVal(v);
          onEmit(v);
        }}
      />
    </form>
  );
}

beforeEach(() => {
  secretsMock.mockReset();
  secretsMock.mockReturnValue({
    data: [
      { id: "sec-1", name: "prod-secret", used_by: 1, created_at: "2025-01-01T00:00:00Z" },
      { id: "sec-2", name: "staging-secret", used_by: 0, created_at: "2025-01-01T00:00:00Z" },
    ],
    isLoading: false,
    isError: false,
    error: null,
  });
});

describe("SecretPicker", () => {
  it("lists the named secrets and a '＋ New secret' item", () => {
    render(<Harness onEmit={vi.fn()} />);
    const opts = Array.from(document.querySelectorAll("option")).map((o) => o.value);
    expect(opts).toContain("sec-1");
    expect(opts).toContain("sec-2");
    expect(opts).toContain("__new__");
  });

  it("emits { secret_id } when an existing secret is picked", () => {
    const onEmit = vi.fn();
    render(<Harness onEmit={onEmit} />);

    selectOption("sec-2");

    expect(onEmit).toHaveBeenCalledWith({ secret_id: "sec-2" });
  });

  it("reveals name + value inputs and emits { new_secret } when creating", async () => {
    const onEmit = vi.fn();
    render(<Harness onEmit={onEmit} />);

    selectOption("__new__");

    await waitFor(() => screen.getByLabelText(/new secret name/i));

    fireEvent.change(screen.getByLabelText(/new secret name/i), {
      target: { value: "fresh-secret" },
    });
    fireEvent.change(screen.getByLabelText(/new secret value/i), {
      target: { value: "p@ss" },
    });

    expect(onEmit).toHaveBeenLastCalledWith({
      new_secret: { name: "fresh-secret", value: "p@ss" },
    });
  });

  it("offers a 'Keep current secret' item when hasExisting and emits {} for it", () => {
    const onEmit = vi.fn();
    render(<Harness hasExisting onEmit={onEmit} />);

    const opts = Array.from(document.querySelectorAll("option")).map((o) => o.value);
    expect(opts).toContain("__keep__");

    // Pick an existing secret first, then switch back to "Keep current".
    selectOption("sec-1");
    selectOption("__keep__");
    expect(onEmit).toHaveBeenLastCalledWith({});
  });
});

describe("SecretPicker and system secrets (KI-207)", () => {
  it("does not offer a secret egret manages itself", () => {
    secretsMock.mockReturnValue({
      data: [
        { id: "sec-1", name: "prod-secret", used_by: 1, system_managed: false },
        { id: "sys-1", name: "engine-internal-x", used_by: 0, system_managed: true },
      ],
      isLoading: false,
      isError: false,
      error: null,
    });
    render(<Harness onEmit={() => {}} />);
    const values = Array.from(document.querySelectorAll("option")).map((o) => o.value);
    expect(values).toContain("sec-1");
    expect(values).not.toContain("sys-1");
  });
});
