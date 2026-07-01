# Egret end-to-end tests (Playwright)

These drive a **real browser against a running local stack**. They are separate
from the Vitest unit suite (`npm test`) and are **not** run in standard CI —
they need a live backend and a real Jupyter kernel, so they are a manual /
opt-in smoke of the notebook execution money-path (Notebook epic E).

## Prerequisites

1. **Docker services** up:
   ```bash
   docker compose -f ../../deploy/docker-compose.dev.yml up -d
   ```
2. **A Python env with the kernel deps on PATH** (so notebook cells can spawn a
   real kernel — the backend's `SubprocessLauncher` runs `jupyter kernelgateway`):
   ```bash
   python3.12 -m venv /tmp/kernelenv
   /tmp/kernelenv/bin/pip install jupyter_kernel_gateway ipykernel   # + daft for %%egret_sql --target daft
   ```
3. **Backend** running on `:8080` with that venv on PATH + catalog enabled:
   ```bash
   cd ../..   # egret repo root
   PATH="/tmp/kernelenv/bin:$PATH" \
   EGRET_DATABASE__URL="postgres://egret:egret@localhost:5432/egret" \
   EGRET_CATALOG__ENABLED=true EGRET_CATALOG__AUTH_TOKEN=dev \
   cargo run -p egret-bin -- serve
   ```
4. **Frontend dev server** on `:5173` (the Vite proxy forwards `/api` incl. the
   kernel WebSocket — `ws: true` in `vite.config.ts`):
   ```bash
   npm run dev
   ```
5. **A provisioned Iceberg warehouse** for the target workspace. The notebook
   kernel resolves the workspace's warehouse on connect; without one, Run shows
   `KernelError: no warehouse configured for this workspace`. Provision it once
   via the UI: **Storage → Add connection** (S3/MinIO: endpoint
   `http://localhost:9000`, bucket `warehouse`, access key `egret`, secret
   `egretsecret`) → **Catalog → Add warehouse** on that connection.

## Run

```bash
npm run test:e2e
```

Override the target / credentials via env: `EGRET_E2E_BASE_URL`,
`EGRET_E2E_EMAIL`, `EGRET_E2E_PASSWORD`.

## What's covered

- `notebook.spec.ts`:
  - **run a Python cell end-to-end** — login → create notebook → set cell source
    → Run → assert the printed output renders live → reload → assert the output
    **restores** from the persisted `last_output` (E-P2 recorder + E-P3 restore).
  - **toolbar controls** — Run all / Interrupt / Restart / Clear outputs + the
    per-cell Run + Run options are present (E-P4).
