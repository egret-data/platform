#!/usr/bin/env bash
set -euo pipefail

# Start the Jupyter Kernel Gateway with the same flags D1b's SubprocessLauncher
# uses and D1c proxies to. Auth is disabled at the gateway (it is reached only
# via egret's authenticated proxy on localhost / the pod network).
exec jupyter kernelgateway \
  --KernelGatewayApp.ip=0.0.0.0 \
  --KernelGatewayApp.port="${KERNEL_GATEWAY_PORT:-8888}" \
  --KernelGatewayApp.allow_origin='*' \
  --KernelGatewayApp.allow_headers='*' \
  --KernelGatewayApp.allow_methods='*' \
  --KernelGatewayApp.auth_token='' \
  --JupyterWebsocketPersonality.list_kernels=True
