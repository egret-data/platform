# ---- Stage 1: Build React App with Vite ----
FROM node:22-alpine AS builder

WORKDIR /app
COPY package*.json ./

RUN npm ci
COPY . .

ENV NODE_ENV=production
RUN npm run build

# ---- Stage 2: Serve with Nginx (root, port 80) ----
FROM mirror.gcr.io/library/nginx:alpine

# A TEMPLATE, not conf.d/default.conf: the official image's entrypoint renders
# /etc/nginx/templates/*.template into conf.d at start. Only variables matching the filter are
# substituted, so nginx's own `$uri`, `$host`, … stay as they are. `APPS_ORIGIN` is the origin of
# the Data Apps iframe, allowed in the CSP `frame-src`; defined-but-empty means "none".
COPY nginx-prod.conf /etc/nginx/templates/default.conf.template
ENV NGINX_ENVSUBST_FILTER=^APPS_ \
    APPS_ORIGIN=
COPY --from=builder /app/dist /usr/share/nginx/html

EXPOSE 80
# Use 127.0.0.1 instead of `localhost` — Alpine's wget tries IPv6 first
# and nginx only binds IPv4 (no `listen [::]:80;` in the conf), so
# `wget http://localhost/` returns "connection refused" and marks the
# container unhealthy even though it serves traffic fine on IPv4.
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s CMD wget -qO- http://127.0.0.1/ || exit 1

CMD ["nginx", "-g", "daemon off;"]
