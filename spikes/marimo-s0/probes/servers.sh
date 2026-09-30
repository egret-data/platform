#!/bin/bash
# usage: servers.sh up-support | run <mode> [ENV=..] | stop-run | restart-egret [ENV=..] | down
# S0b: KERNEL_LAUNCHER=pool servers.sh run ipc [S0_POOL_SIZE=3]   (default launcher: direct)
cd "$(dirname "$0")/.."
mkdir -p .work/logs
export S0_TIMING_LOG=$PWD/.work/logs/timing.jsonl
case "$1" in
up-support)
  nohup uv run uvicorn fake_egret:app --host 127.0.0.1 --port 8700 > .work/logs/egret.log 2>&1 &
  nohup uv run uvicorn proxy:app --host 127.0.0.1 --port 8701 > .work/logs/proxy.log 2>&1 &
  for i in $(seq 30); do curl -sf http://127.0.0.1:8700/_events >/dev/null && echo egret up && break; sleep 1; done ;;
run)
  mode=$2; shift 2; launcher=${KERNEL_LAUNCHER:-direct}
  APPS_WORKDIR=$PWD/.work/run nohup env "$@" uv run python entrypoint.py --mode run --process-mode $mode --kernel-launcher $launcher > .work/logs/run-$mode-$launcher.log 2>&1 &
  for i in $(seq 300); do pgrep -f "entrypoint.py --mode run" >/dev/null || { echo run server died; break; }; curl -s -o /dev/null http://127.0.0.1:2719/ && echo run up after ${i}s && break; sleep 1; done ;;
stop-run)
  # LISTEN-only: a bare `lsof -ti :2719` also matches the proxy's pooled client/WS sockets and kills the proxy
  pkill -f "entrypoint.py --mode run"; for i in $(seq 15); do pgrep -f "entrypoint.py --mode run" >/dev/null || break; sleep 1; done; pkill -f kernel_pool.py; pkill -f kernel_worker.py; lsof -ti tcp:2719 -sTCP:LISTEN | xargs kill 2>/dev/null; sleep 1; lsof -i tcp:2719 -sTCP:LISTEN || echo 2719 free ;;
restart-egret)
  # usage: servers.sh restart-egret [ENV=..]   (e.g. COOKIE_PREFIX=__Host-); kills only the :8700 LISTENER
  shift; lsof -ti tcp:8700 -sTCP:LISTEN | xargs kill 2>/dev/null; sleep 2
  nohup env "$@" uv run uvicorn fake_egret:app --host 127.0.0.1 --port 8700 > .work/logs/egret.log 2>&1 &
  for i in $(seq 30); do curl -sf http://127.0.0.1:8700/_events >/dev/null && echo egret up && break; sleep 1; done ;;
down)
  # pool leftovers only AFTER the server has exited: killing kernels under a live server hung its shutdown (S0b T3)
  pkill -f "entrypoint.py"; for i in $(seq 15); do pgrep -f "entrypoint.py" >/dev/null || break; sleep 1; done; pkill -f kernel_pool.py; pkill -f kernel_worker.py; pkill -f "uvicorn fake_egret"; pkill -f "uvicorn proxy"; sleep 2; lsof -i :8700 -i :8701 -i :2718 -i :2719 || echo all free ;;
esac
