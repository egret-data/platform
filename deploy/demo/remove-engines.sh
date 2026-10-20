#!/usr/bin/env bash
# DEMO ONLY. Remove the engine containers egret provisioned for THIS demo project.
# Both labels are required: another stack's managed containers on the same host are left alone.
# EGRET_DEMO_PROJECT overrides the project label (the acceptance test uses a test-only value); the
# just recipes pin it to egret-demo, so a value from .env (dotenv-load) cannot redirect the cleanup.
set -euo pipefail
PROJECT=${EGRET_DEMO_PROJECT:-egret-demo}
ids=$(docker ps -aq --filter label=egret.managed=true --filter "label=egret.project=$PROJECT")
[ -z "$ids" ] || docker rm -f $ids
