import type { EngineConfigForm } from "../useEngineConfigForm";
import { ContainersMountsSection } from "./ContainersMountsSection";
import { PlacementSection } from "./PlacementSection";
import { PodSection } from "./PodSection";

/**
 * "Scheduling" merged section: the three existing pod editors stacked — Pod
 * (service account, image pull, security contexts, deployment), Placement
 * (per-role node selector / tolerations / affinity), and Containers & Mounts
 * (init/sidecar containers, envFrom, config/secret mounts). Each is already a
 * `ConfigSection`. Composes existing editors — no new fields.
 */
export function SchedulingSection({ cfg }: { cfg: EngineConfigForm }) {
  const {
    serviceAccount,
    setServiceAccount,
    imagePullSecrets,
    setImagePullSecrets,
    imagePullPolicy,
    setImagePullPolicy,
    podSecurityContext,
    setPodSecurityContext,
    containerSecurityContext,
    setContainerSecurityContext,
    deploymentStrategy,
    setDeploymentStrategy,
    deploymentAnnotations,
    setDeploymentAnnotations,
    revisionHistoryLimit,
    setRevisionHistoryLimit,
    progressDeadlineSeconds,
    setProgressDeadlineSeconds,
    shareProcessNamespace,
    setShareProcessNamespace,
    coordinator,
    setCoordinator,
    worker,
    setWorker,
    initContainers,
    setInitContainers,
    sidecarContainers,
    setSidecarContainers,
    envFrom,
    setEnvFrom,
    configMounts,
    setConfigMounts,
    secretMounts,
    setSecretMounts,
  } = cfg;

  return (
    <div className="space-y-5">
      <PodSection
        serviceAccount={serviceAccount}
        setServiceAccount={setServiceAccount}
        imagePullSecrets={imagePullSecrets}
        setImagePullSecrets={setImagePullSecrets}
        imagePullPolicy={imagePullPolicy}
        setImagePullPolicy={setImagePullPolicy}
        podSecurityContext={podSecurityContext}
        setPodSecurityContext={setPodSecurityContext}
        containerSecurityContext={containerSecurityContext}
        setContainerSecurityContext={setContainerSecurityContext}
        deploymentStrategy={deploymentStrategy}
        setDeploymentStrategy={setDeploymentStrategy}
        deploymentAnnotations={deploymentAnnotations}
        setDeploymentAnnotations={setDeploymentAnnotations}
        revisionHistoryLimit={revisionHistoryLimit}
        setRevisionHistoryLimit={setRevisionHistoryLimit}
        progressDeadlineSeconds={progressDeadlineSeconds}
        setProgressDeadlineSeconds={setProgressDeadlineSeconds}
        shareProcessNamespace={shareProcessNamespace}
        setShareProcessNamespace={setShareProcessNamespace}
      />
      <PlacementSection
        coordinator={coordinator}
        setCoordinator={setCoordinator}
        worker={worker}
        setWorker={setWorker}
      />
      <ContainersMountsSection
        initContainers={initContainers}
        setInitContainers={setInitContainers}
        sidecarContainers={sidecarContainers}
        setSidecarContainers={setSidecarContainers}
        envFrom={envFrom}
        setEnvFrom={setEnvFrom}
        configMounts={configMounts}
        setConfigMounts={setConfigMounts}
        secretMounts={secretMounts}
        setSecretMounts={setSecretMounts}
      />
    </div>
  );
}
