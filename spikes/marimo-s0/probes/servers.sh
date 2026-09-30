#!/bin/bash
# usage: servers.sh up-support | run <mode> [ENV=..] | stop-run | down
cd "$(dirname "$0")/.."
case "$1" in
up-support)
  nohup uv run uvicorn fake_egret:app --host 127.0.0.1 --port 8700 > .work/logs/egret.log 2>&1 &
  nohup uv run uvicorn proxy:app --host 127.0.0.1 --port 8701 > .work/logs/proxy.log 2>&1 &
  for i in $(seq 30); do curl -sf http://127.0.0.1:8700/_events >/dev/null && echo egret up && break; sleep 1; done ;;
run)
  mode=$2; shift 2
  APPS_WORKDIR=$PWD/.work/run nohup env "$@" uv run python entrypoint.py --mode run --process-mode $mode > .work/logs/run-$mode.log 2>&1 &
  for i in $(seq 300); do pgrep -f "entrypoint.py --mode run" >/dev/null || { echo run server died; break; }; curl -s -o /dev/null http://127.0.0.1:2719/ && echo run up after ${i}s && break; sleep 1; done ;;
stop-run)
  pkill -f "entrypoint.py --mode run"; sleep 2; lsof -ti :2719 | xargs kill 2>/dev/null; sleep 1; lsof -i :2719 || echo 2719 free ;;
down)
  pkill -f "entrypoint.py"; pkill -f "uvicorn fake_egret"; pkill -f "uvicorn proxy"; sleep 2; lsof -i :8700 -i :8701 -i :2718 -i :2719 || echo all free ;;
esac
