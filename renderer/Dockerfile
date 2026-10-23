# egret-renderer: headless-Chromium print/PDF renderer for scheduled reports.
#
# Base image on Microsoft's own Playwright image, pinned to the SAME version
# as the `playwright` npm dependency below (1.62.1) — it already carries a
# matching Chromium build AND the system fonts print pages need. A slim
# `node:*` base silently renders boxes instead of text (missing fonts), which
# no test in this repo would catch, so don't swap this for something smaller.
FROM mcr.microsoft.com/playwright:v1.62.1-jammy

WORKDIR /app

# The base image's global npm 11.16.0 bundles tar 7.5.15 (CVE-2026-59873, CRITICAL); npm 11.19.0
# is the first release that bundles a fixed tar (7.5.19). Drop this line once the Playwright image
# ships a newer npm.
RUN npm install -g npm@11.19.0 && npm cache clean --force

COPY package.json package-lock.json* ./
RUN npm install --omit=dev

COPY server.js ./

ENV NODE_ENV=production
EXPOSE 8080

# The service holds no credentials of its own and never writes secrets to
# disk, so no extra hardening is needed here beyond running as the base
# image's existing non-root `pwuser`.
USER pwuser

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s \
  CMD node -e "fetch('http://127.0.0.1:'+(process.env.PORT||8080)+'/healthz').then(r=>process.exit(r.ok?0:1)).catch(()=>process.exit(1))"

CMD ["node", "server.js"]
