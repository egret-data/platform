#!/usr/bin/env bash
# DEMO ONLY. Remove the engine containers egret provisioned for THIS demo project.
# Both labels are required: another stack's managed containers on the same host are left alone.
# The project label follows the compose project: EGRET_DEMO_PROJECT (which the just recipes set
# from COMPOSE_PROJECT_NAME) wins, else COMPOSE_PROJECT_NAME, else egret-demo. The engines carry
# whatever `EGRET_ENGINES__DOCKER__PROJECT` the stack ran with, which the compose file derives the
# same way — so a stack started with COMPOSE_PROJECT_NAME=egret-p6 labels and removes its own
# engines, not egret-demo's. Set EGRET_DEMO_PROJECT explicitly only to target a specific project.
set -euo pipefail
PROJECT=${EGRET_DEMO_PROJECT:-${COMPOSE_PROJECT_NAME:-egret-demo}}
ids=$(docker ps -aq --filter label=egret.managed=true --filter "label=egret.project=$PROJECT")
[ -z "$ids" ] || docker rm -f $ids
