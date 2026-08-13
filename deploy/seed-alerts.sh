#!/usr/bin/env bash
#
# Seed Alerts & Reports demo data for a frontend check. See deploy/seed-alerts.sql
# for what it creates and why. Idempotent — re-run it as often as you like.
#
# Usage: just seed-alerts [workspace-slug] [smtp-host]
#   workspace-slug  default: demo
#   smtp-host       where the seeded SMTP row points. Default `localhost` for
#                   the docker-compose stack; pass `mailpit` when seeding a
#                   cluster, where localhost is the egret pod itself.
set -euo pipefail

cd "$(dirname "$0")/.."
if [ -f .env ]; then set -a; . ./.env; set +a; fi

WS="${1:-demo}"
SMTP_HOST="${2:-${SEED_SMTP_HOST:-localhost}}"
PGURL="${DATABASE_URL:-postgres://egret:egret@localhost:5432/egret}"

psql "$PGURL" -v ON_ERROR_STOP=1 -v "ws=$WS" -v "smtp_host=$SMTP_HOST" -f deploy/seed-alerts.sql

cat <<EOF

Seeded workspace "$WS". To look at it:
  just serve                       # if it is not already running
  open http://localhost:5173/explore/$WS/alerts

The 'alerts' entitlement was switched on for this workspace — without it the nav
item does not render. Mail goes to mailpit at ${SMTP_HOST}:1025; its inbox is on :8025.
EOF
