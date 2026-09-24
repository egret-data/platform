import { render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { describe, expect, test, vi } from "vitest";

import type { Engine } from "@/api/controlPlane";
import { TooltipProvider } from "@/components/ui/tooltip";
import { EngineSecurity } from "@/routes/control-plane/EngineSecurity";
import {
  type DelegationMode,
  useEngineConfigForm,
} from "@/routes/control-plane/useEngineConfigForm";

vi.mock("@/api/controlPlane");

const BASE: Omit<Engine, "status"> = {
  id: "t1",
  name: "trino-one",
  type: "trino",
  namespace: "engines",
  spec: {
    coordinatorReplicas: 1,
    workerReplicas: 1,
    cpuMin: "1",
    cpuMax: "2",
    memory: "4Gi",
    imageVersion: "477",
    trinoConfig: { coordinator: {}, worker: {} },
  },
};

/** Build a Trino engine whose stored `trinoConfig.delegationMode` is `mode`. */
function engineWithStoredMode(
  mode: DelegationMode | undefined,
): Omit<Engine, "status"> {
  return {
    ...BASE,
    spec: {
      ...BASE.spec,
      trinoConfig: {
        coordinator: {},
        worker: {},
        ...(mode === undefined || mode === "" ? {} : { delegationMode: mode }),
      },
    },
  };
}

/**
 * Drive the real `useEngineConfigForm` (as EngineDetail's SecuritySection does)
 * and render `EngineSecurity` against it, so the `storedDelegationMode` ratchet
 * is exercised end-to-end from the stored spec.
 */
function Harness({ engine }: { engine: Omit<Engine, "status"> }) {
  const cfg = useEngineConfigForm(engine);
  return (
    <EngineSecurity
      passwordUsers={cfg.passwordUsers}
      setPasswordUsers={cfg.setPasswordUsers}
      jwtEnabled={cfg.jwtEnabled}
      setJwtEnabled={cfg.setJwtEnabled}
      delegationMode={cfg.delegationMode}
      setDelegationMode={cfg.setDelegationMode}
      storedDelegationMode={cfg.storedDelegationMode}
      jwksUri={cfg.jwksUri}
      setJwksUri={cfg.setJwksUri}
      requiredAudience={cfg.requiredAudience}
      setRequiredAudience={cfg.setRequiredAudience}
      ingressEnabled={cfg.ingressEnabled}
      setIngressEnabled={cfg.setIngressEnabled}
      ingressHost={cfg.ingressHost}
      setIngressHost={cfg.setIngressHost}
      ingressClassName={cfg.ingressClassName}
      setIngressClassName={cfg.setIngressClassName}
      ingressTlsSecret={cfg.ingressTlsSecret}
      setIngressTlsSecret={cfg.setIngressTlsSecret}
      ingressTlsCertSecretRef={cfg.ingressTlsCertSecretRef}
      setIngressTlsCertSecretRef={cfg.setIngressTlsCertSecretRef}
      ingressAnnotations={cfg.ingressAnnotations}
      setIngressAnnotations={cfg.setIngressAnnotations}
    />
  );
}

function renderSecurity(engine: Omit<Engine, "status">) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter>
        <TooltipProvider>
          <Harness engine={engine} />
        </TooltipProvider>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

const NONE_NOTE =
  /Removing delegation from a running engine isn't supported yet/i;

describe("EngineSecurity — Trino delegation None ratchet", () => {
  test.each(["jwt", "impersonate"] as const)(
    "stored %s: None is DISABLED + the recreate note shows; other modes stay switchable",
    (mode) => {
      renderSecurity(engineWithStoredMode(mode));

      // Downgrading a running jwt/impersonate engine to open isn't backend-
      // supported (auth.jwt persists, no rematerialize), so "None" is disabled and
      // the recreate note explains why.
      expect(screen.getByText(NONE_NOTE)).toBeInTheDocument();
      expect(
        screen.getByRole("radio", { name: /None \(open engine\)/ }),
      ).toBeDisabled();

      // JWT / Impersonate remain switchable.
      expect(
        screen.getByRole("radio", { name: /JWT \(per-user\)/ }),
      ).not.toBeDisabled();
      expect(
        screen.getByRole("radio", { name: /Impersonate \(service user\)/ }),
      ).not.toBeDisabled();
    },
  );

  test("new engine (no stored delegationMode): None is ENABLED, no note", () => {
    renderSecurity(engineWithStoredMode(undefined));

    expect(screen.queryByText(NONE_NOTE)).not.toBeInTheDocument();
    expect(
      screen.getByRole("radio", { name: /None \(open engine\)/ }),
    ).not.toBeDisabled();
  });

  test("currently-open engine (stored 'none'): None stays ENABLED, no note", () => {
    // A stored "none" is open, not delegated — the ratchet does not apply.
    renderSecurity(engineWithStoredMode("none"));

    expect(screen.queryByText(NONE_NOTE)).not.toBeInTheDocument();
    expect(
      screen.getByRole("radio", { name: /None \(open engine\)/ }),
    ).not.toBeDisabled();
  });
});
