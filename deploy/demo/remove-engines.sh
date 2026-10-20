#!/usr/bin/env bash
# DEMO ONLY. Remove the engine containers egret provisioned for THIS demo project.
# Both labels are required: another stack's managed containers on the same host are left alone.
set -euo pipefail
ids=$(docker ps -aq --filter label=egret.managed=true --filter label=egret.project=egret-demo)
[ -z "$ids" ] || docker rm -f $ids
