# marimo S0 spike — throwaway, never merged

```bash
uv run uvicorn fake_egret:app --host 127.0.0.1 --port 8700            # terminal 1
uv run uvicorn proxy:app --host 127.0.0.1 --port 8701                 # terminal 2
APPS_WORKDIR=$PWD/.work/edit uv run python entrypoint.py --mode edit  # terminal 3
APPS_WORKDIR=$PWD/.work/run  uv run python entrypoint.py --mode run --process-mode thread  # terminal 4
```

Browser URLs: `http://egret.localhost:8700/login?user=alice`, then `http://egret.localhost:8700/apps/probe/run`.

## Findings
(append one line per surprise, with the file/line in site-packages that explains it)

- T2 smoke (run, thread): the whole chain worked with NO change to `proxy.py` — `PY pid=97346 viewer=alice sql=SELECT 1 AS n` and `SQLCELL viewer=alice`; marimo accepted the proxy's `Authorization: Bearer pod-secret` on HTTP and on the WS upgrade (`marimo/_server/api/auth.py:77-86`), and `--base-url /r` needed no path rewriting.
- T2: `DBAPIEngine.is_compatible` accepted the plain `Connection` unchanged — it only duck-types `cursor/commit/rollback/close` (`marimo/_sql/engines/dbapi.py:95-107`); `mo.sql(..., engine=db)` then ran through egret's gateway as the viewer.
- T2 Step 7a holds: `Session.create` is keyword-only (`*,` at `marimo/_session/session.py:99-101`), and its only caller passes `mode=`/`sandbox=` by name (`marimo/_server/session_manager.py:368-397`), so the kwargs patch is sound.
- T2 Step 7b is FALSE: `IPCKernelManagerImpl` has no `_sandbox` attribute in 0.25.0; without `[venv] path` it always builds a uv `NotebookSandbox` (`marimo/_session/managers/ipc.py:331-431`). Adapted by patching `ipc._get_venv_config` (`ipc.py:65`) to return `path = sys.prefix` in `--process-mode ipc` only — NOT via `marimo.toml`, because a configured venv beats the sandbox (`ipc.py:333`) and would silently turn `--process-mode sandbox` into the ipc variant. (ipc variant not yet exercised; that is Task 3+.)
- T2: in single-file `marimo run` the AppHost path is never taken (`isolate_apps` requires multi-file, `marimo/_server/start.py:307`), so `sandbox=True` on `Session.create` really does select the IPC kernel (`session.py:160`).
- T2: `XDG_CONFIG_HOME` is the LAST place marimo looks for config — a `.marimo.toml` in cwd, any parent, or `~` wins first (`marimo/_config/utils.py:69-120`). None exists on this machine; a pod must guarantee that too.
- T2: `NotebookWorkspace.load` runs twice per edit open — once for the index page (`marimo/_server/api/endpoints/assets.py:387` → `session_manager.py:191`) and once for the session (`session_manager.py:353`) — so the storage patch re-fetches the draft and rewrites the file each time (logged "EgretStorage bound to probe rev 1" ×2).
- T2: the edit launcher asks for draft `xss`, which does not exist yet (fake egret 500s on unknown ids, `fake_egret.py:97`); `entrypoint.py` now skips missing drafts with a log line.
- T2: each WS close revokes the session token (`proxy.py` ws_relay), so one host-page load = one ticket = one token, revoked when the iframe goes away (3 loads → 3 `token_revoked` events).
- T2 (test harness, not marimo): the RED run failed with `AttributeError: module 'egret_marimo' has no attribute 'connect'`, not `ModuleNotFoundError`, because Task 1's `egret_marimo/.gitkeep` made the directory an implicit namespace package.
