"""egret-apps launcher, run-mode IPC patch, D14 shim and locked config (Task 3)."""

import asyncio
import json
import os
import subprocess
import sys
from pathlib import Path

import marimo
import pytest

from egret_marimo import launcher

ROOT = Path(__file__).resolve().parent.parent
SHIM = ROOT / "venv-shim"
CONFIG = ROOT / "config"
PROBE = "import marimo, msgspec, zmq; print(marimo.__version__)"
APP = "22222222-2222-4222-8222-222222222222"
VERSION = "v-7"


@pytest.fixture
def env(tmp_path, monkeypatch):
    """A clean launcher environment; every variable the launcher touches is restored."""
    home = tmp_path / "home"
    home.mkdir()
    workdir = tmp_path / "work"
    for name in (
        "XDG_CONFIG_HOME", "MARIMO_SKIP_UPDATE_CHECK", "EGRET_REAL_PYTHON", "EGRET_MARIMO_VERSION",
        "EGRET_MARIMO_PORT", "EGRET_APP_ID", "EGRET_VERSION_ID",
    ):
        monkeypatch.delenv(name, raising=False)
    monkeypatch.setenv("HOME", str(home))
    monkeypatch.setenv("EGRET_URL", os.environ.get("EGRET_URL", "http://127.0.0.1:9"))
    monkeypatch.setenv("EGRET_POD_TOKEN", "pod-tok")
    monkeypatch.setenv("EGRET_MARIMO_TOKEN", "marimo-secret")
    monkeypatch.setenv("EGRET_BASE_URL", "/e/pod-1")
    monkeypatch.setenv("APPS_WORKDIR", str(workdir))
    monkeypatch.setenv("EGRET_APPS_CONFIG", str(CONFIG))
    monkeypatch.setenv("EGRET_VENV_SHIM", str(SHIM))
    monkeypatch.chdir(tmp_path)  # the launcher chdirs; this restores the cwd

    calls: dict[str, list] = {"storage": [], "ipc": [], "marimo": []}
    monkeypatch.setattr(launcher, "install_storage_patch", lambda *a: calls["storage"].append(a))
    monkeypatch.setattr(launcher, "install_run_mode_ipc_patch", lambda *a: calls["ipc"].append(a))

    calls["exec"] = []
    calls["marimo_env"] = []

    def fake_marimo(argv):
        calls["marimo"].append(list(argv))
        calls["marimo_env"].append(dict(os.environ))
        return 0

    def fake_exec(path, argv, child_env):
        """Stand-in for os.execve: run the internal phase in-process with the child's env."""
        calls["exec"].append((path, list(argv), dict(child_env)))
        for name in launcher.SECRETS:
            monkeypatch.delenv(name, raising=False)
        monkeypatch.setenv(launcher.SECRETS_FD, child_env[launcher.SECRETS_FD])
        return launcher.main(argv[3:])

    monkeypatch.setattr(launcher, "run_marimo", fake_marimo)
    monkeypatch.setattr(launcher, "_exec", fake_exec)
    monkeypatch.setattr(launcher, "install_config_lock", lambda: None)
    return {"home": home, "workdir": workdir, "calls": calls}


# --- usage and required variables -------------------------------------------


@pytest.mark.parametrize("argv", [[], ["serve"], ["edit", "extra"]])
def test_bad_usage_exits_64(env, argv, capsys):
    assert launcher.main(argv) == 64
    assert "usage: egret-apps edit|run" in capsys.readouterr().err


@pytest.mark.parametrize(
    "mode,name",
    [
        ("edit", "EGRET_URL"), ("edit", "EGRET_POD_TOKEN"), ("edit", "EGRET_MARIMO_TOKEN"),
        ("edit", "EGRET_BASE_URL"), ("edit", "APPS_WORKDIR"),
        ("run", "EGRET_APP_ID"), ("run", "EGRET_VERSION_ID"), ("run", "EGRET_POD_TOKEN"),
    ],
)
def test_missing_required_variable_exits_64_and_names_it(env, monkeypatch, capsys, mode, name):
    monkeypatch.setenv("EGRET_APP_ID", APP)
    monkeypatch.setenv("EGRET_VERSION_ID", VERSION)
    monkeypatch.delenv(name)
    assert launcher.main([mode]) == 64
    assert name in capsys.readouterr().err
    assert env["calls"]["marimo"] == []


def test_run_rejects_an_app_id_that_is_not_a_uuid(env, monkeypatch, capsys):
    monkeypatch.setenv("EGRET_APP_ID", "../../etc/x")
    monkeypatch.setenv("EGRET_VERSION_ID", VERSION)
    assert launcher.main(["run"]) == 64
    assert "EGRET_APP_ID" in capsys.readouterr().err


# --- the .marimo.toml guard --------------------------------------------------


@pytest.mark.parametrize("where", ["workdir", "parent", "home"])
def test_a_marimo_toml_that_could_shadow_the_locked_config_exits_65(env, capsys, where):
    env["workdir"].mkdir()
    target = {"workdir": env["workdir"], "parent": env["workdir"].parent, "home": env["home"]}[where]
    (target / ".marimo.toml").write_text("[ai]\nenabled = true\n")
    assert launcher.main(["edit"]) == 65
    assert str(target / ".marimo.toml") in capsys.readouterr().err
    assert (env["calls"]["storage"], env["calls"]["ipc"], env["calls"]["marimo"]) == ([], [], [])


@pytest.mark.parametrize("where", ["workdir", "parent"])
def test_a_pyproject_toml_in_the_workdir_or_a_parent_exits_65(env, capsys, where):
    """marimo's project layer reads the nearest pyproject.toml over the user config."""
    env["workdir"].mkdir()
    target = {"workdir": env["workdir"], "parent": env["workdir"].parent}[where]
    (target / "pyproject.toml").write_text("[tool.marimo.ai]\nenabled = true\n")
    assert launcher.main(["edit"]) == 65
    assert str(target / "pyproject.toml") in capsys.readouterr().err


def test_a_pyproject_toml_beside_the_workdir_is_not_refused(env):
    """The image's own /opt/egret-apps/pyproject.toml is not on the workdir's path."""
    env["workdir"].mkdir()
    sibling = env["workdir"].parent / "opt"
    sibling.mkdir()
    (sibling / "pyproject.toml").write_text("[project]\nname = 'x'\n")
    assert launcher.main(["edit"]) == 0


def test_a_missing_locked_config_exits_65(env, monkeypatch, tmp_path, capsys):
    monkeypatch.setenv("EGRET_APPS_CONFIG", str(tmp_path / "nowhere"))
    assert launcher.main(["edit"]) == 65
    assert "locked marimo config not found" in capsys.readouterr().err


# --- argv per mode -----------------------------------------------------------


def test_edit_argv_installs_storage_patch_only(env):
    assert launcher.main(["edit"]) == 0
    workdir = str(env["workdir"].resolve())
    assert env["calls"]["marimo"] == [[
        "edit", workdir, "--host", "0.0.0.0", "--port", "2718", "--headless",
        "--base-url", "/e/pod-1", "--token-password", "marimo-secret", "--skip-update-check",
    ]]
    assert [Path(a[0]) for a in env["calls"]["storage"]] == [Path(workdir)]
    assert env["calls"]["ipc"] == []
    assert Path.cwd() == Path(workdir)


def test_run_fetches_the_version_once_and_installs_ipc_patch_only(env, egret, monkeypatch):
    monkeypatch.setenv("EGRET_URL", egret.base_url)
    monkeypatch.setenv("EGRET_BASE_URL", f"/r/{VERSION}")
    monkeypatch.setenv("EGRET_APP_ID", APP)
    monkeypatch.setenv("EGRET_VERSION_ID", VERSION)
    egret.pod_tokens["pod-tok"] = {}
    egret.versions[(APP, VERSION)] = {"source": "# version 7\n", "version_no": 7}
    fetched = []
    from egret_marimo.podapi import PodApi

    real = PodApi.get_version
    monkeypatch.setattr(PodApi, "get_version", lambda self, *a: fetched.append(a) or real(self, *a))

    assert launcher.main(["run"]) == 0

    app_file = env["workdir"].resolve() / f"{APP}.py"
    assert app_file.read_text() == "# version 7\n"
    assert fetched == [(APP, VERSION)]
    assert env["calls"]["marimo"] == [[
        "run", str(app_file), "--host", "0.0.0.0", "--port", "2719", "--headless",
        "--base-url", f"/r/{VERSION}", "--token-password", "marimo-secret",
    ]]
    assert env["calls"]["ipc"] == [(SHIM,)]
    assert env["calls"]["storage"] == []


def test_run_exits_69_when_egret_cannot_serve_the_version(env, egret, monkeypatch, capsys):
    monkeypatch.setenv("EGRET_URL", egret.base_url)
    monkeypatch.setenv("EGRET_APP_ID", APP)
    monkeypatch.setenv("EGRET_VERSION_ID", "missing")
    egret.pod_tokens["pod-tok"] = {}
    assert launcher.main(["run"]) == 69
    assert "missing" in capsys.readouterr().err
    assert env["calls"]["ipc"] == [] and env["calls"]["marimo"] == []


@pytest.mark.parametrize("mode", ["edit", "run"])
def test_never_watch(env, egret, monkeypatch, mode):
    """--watch would reload sessions on every load, and every load rewrites the app file."""
    monkeypatch.setenv("EGRET_URL", egret.base_url)
    monkeypatch.setenv("EGRET_APP_ID", APP)
    monkeypatch.setenv("EGRET_VERSION_ID", VERSION)
    egret.pod_tokens["pod-tok"] = {}
    egret.versions[(APP, VERSION)] = {"source": "", "version_no": 1}
    assert launcher.main([mode]) == 0
    (argv,) = env["calls"]["marimo"]
    assert not any(a.startswith("--watch") for a in argv)


@pytest.mark.parametrize("mode", ["edit", "run"])
def test_argv_parses_with_marimos_own_cli(env, egret, monkeypatch, mode):
    monkeypatch.setenv("EGRET_URL", egret.base_url)
    monkeypatch.setenv("EGRET_APP_ID", APP)
    monkeypatch.setenv("EGRET_VERSION_ID", VERSION)
    egret.pod_tokens["pod-tok"] = {}
    egret.versions[(APP, VERSION)] = {"source": "", "version_no": 1}
    assert launcher.main([mode]) == 0
    (argv,) = env["calls"]["marimo"]
    from marimo._cli.cli import main as marimo_main

    ctx = marimo_main.make_context("marimo", list(argv), resilient_parsing=False)
    cmd = marimo_main.get_command(ctx, argv[0])
    sub = cmd.make_context(argv[0], list(argv[1:]), parent=ctx)
    assert sub.params["headless"] is True
    assert sub.params["port"] == int(PORTS_FOR[mode])
    assert sub.params["base_url"] == os.environ["EGRET_BASE_URL"]
    assert sub.params["token_password"] == "marimo-secret"
    assert sub.params["watch"] is False


PORTS_FOR = {"edit": "2718", "run": "2719"}


def test_port_override(env, monkeypatch):
    monkeypatch.setenv("EGRET_MARIMO_PORT", "31337")
    assert launcher.main(["edit"]) == 0
    (argv,) = env["calls"]["marimo"]
    assert argv[argv.index("--port") + 1] == "31337"


# --- environment the launcher sets --------------------------------------------


def test_locks_xdg_config_and_skips_update_check(env, monkeypatch):
    monkeypatch.setenv("XDG_CONFIG_HOME", "/somewhere/else")
    assert launcher.main(["edit"]) == 0
    assert os.environ["XDG_CONFIG_HOME"] == str(CONFIG)
    assert os.environ["MARIMO_SKIP_UPDATE_CHECK"] == "1"


def test_reexec_hands_secrets_over_a_pipe_not_the_environment(env):
    assert launcher.main(["edit"]) == 0
    ((path, argv, child_env),) = env["calls"]["exec"]
    assert (path, argv) == (sys.executable, [sys.executable, "-m", "egret_marimo.launcher", "--internal", "edit"])
    assert not set(launcher.SECRETS) & set(child_env)
    assert not any(v in ("pod-tok", "marimo-secret") for v in child_env.values())
    assert child_env["EGRET_URL"]  # kernels still need egret's URL
    # The internal phase got the tokens from the pipe and closed it.
    fd = int(child_env[launcher.SECRETS_FD])
    with pytest.raises(OSError):
        os.fstat(fd)
    (marimo_env,) = env["calls"]["marimo_env"]
    assert not (set(launcher.SECRETS) | {launcher.SECRETS_FD}) & set(marimo_env)
    (argv,) = env["calls"]["marimo"]
    assert argv[argv.index("--token-password") + 1] == "marimo-secret"


def test_internal_phase_without_the_pipe_exits_64(env, monkeypatch, capsys):
    monkeypatch.delenv(launcher.SECRETS_FD, raising=False)
    assert launcher.main(["--internal", "edit"]) == 64
    assert launcher.SECRETS_FD in capsys.readouterr().err


def test_overrides_are_test_only(monkeypatch):
    monkeypatch.delenv("PYTEST_CURRENT_TEST")
    monkeypatch.setenv("EGRET_APPS_CONFIG", str(CONFIG))
    with pytest.raises(launcher.LaunchError) as e:
        launcher._image_path("EGRET_APPS_CONFIG", launcher.DEFAULT_CONFIG)
    assert e.value.code == 65 and "EGRET_APPS_CONFIG" in str(e.value)
    monkeypatch.setenv("EGRET_APPS_CONFIG", "/opt/egret-apps/config")
    assert launcher._image_path("EGRET_APPS_CONFIG", launcher.DEFAULT_CONFIG) == Path("/opt/egret-apps/config")
    monkeypatch.setenv("EGRET_MARIMO_PORT", "1234")
    with pytest.raises(launcher.LaunchError) as e:
        launcher._port("edit")
    assert e.value.code == 64
    monkeypatch.delenv("EGRET_MARIMO_PORT")
    assert launcher._port("edit") == "2718"


def test_run_points_the_shim_at_this_interpreter_and_marimo(env, egret, monkeypatch):
    monkeypatch.setenv("EGRET_URL", egret.base_url)
    monkeypatch.setenv("EGRET_APP_ID", APP)
    monkeypatch.setenv("EGRET_VERSION_ID", VERSION)
    egret.pod_tokens["pod-tok"] = {}
    egret.versions[(APP, VERSION)] = {"source": "", "version_no": 1}
    assert launcher.main(["run"]) == 0
    assert os.environ["EGRET_REAL_PYTHON"] == sys.executable
    assert os.environ["EGRET_MARIMO_VERSION"] == marimo.__version__


# --- the D14 shim ------------------------------------------------------------


def _shim(args, real_python):
    return subprocess.run(
        [str(SHIM / "bin" / "python"), *args],
        env={"PATH": os.environ["PATH"], "EGRET_REAL_PYTHON": real_python, "EGRET_MARIMO_VERSION": "0.25.0"},
        capture_output=True, text=True, timeout=30,
    )


def test_shim_answers_the_exact_probe_without_an_interpreter():
    # /usr/bin/false as the "real" Python: the answer can only come from the shim.
    r = _shim(["-c", PROBE], "/usr/bin/false")
    assert (r.returncode, r.stdout) == (0, "0.25.0\n")


def test_shim_passes_everything_else_to_the_real_python():
    r = _shim(["-c", "print(1)"], sys.executable)
    assert (r.returncode, r.stdout) == (0, "1\n")
    near_miss = _shim(["-c", PROBE + " "], "/usr/bin/false")
    assert near_miss.returncode != 0


def test_shim_layout_and_probe_match_marimo(monkeypatch):
    from marimo._session._venv import get_configured_venv_python, has_marimo_installed

    assert os.access(SHIM / "bin" / "python", os.X_OK)
    cfg = (SHIM / "pyvenv.cfg").read_text().splitlines()
    assert cfg == ["home = /usr/local/bin", "include-system-site-packages = true", "version = 3.12"]
    python = get_configured_venv_python({"path": str(SHIM)})
    assert python == str((SHIM / "bin" / "python").absolute())
    # marimo's own probe, through the shim, with no real interpreter behind it.
    monkeypatch.setenv("EGRET_REAL_PYTHON", "/usr/bin/false")
    monkeypatch.setenv("EGRET_MARIMO_VERSION", marimo.__version__)
    assert asyncio.run(has_marimo_installed(python)) is True


# --- the run-mode IPC patch ----------------------------------------------------


@pytest.fixture
def isolated_session_patch(monkeypatch):
    """Install the IPC patch over recorders; monkeypatch undoes every attribute afterwards."""
    from marimo._session import session as session_mod
    from marimo._session.managers import ipc as ipc_mod

    async def recorded_create(cls, **kwargs):
        return kwargs

    monkeypatch.setattr(session_mod.SessionImpl, "create", session_mod.SessionImpl.create)
    monkeypatch.setattr(session_mod.SessionImpl, "_egret_original_create", recorded_create, raising=False)
    monkeypatch.setattr(ipc_mod, "_get_venv_config", ipc_mod._get_venv_config)
    monkeypatch.setattr(
        ipc_mod, "_egret_original_get_venv_config", lambda cm: {"path": "x", "writable": True}, raising=False
    )
    from egret_marimo.patches import install_run_mode_ipc_patch

    install_run_mode_ipc_patch(SHIM)
    return session_mod.SessionImpl, ipc_mod


def test_ipc_patch_forces_sandbox_for_run_sessions_only(isolated_session_patch):
    from marimo._session.model import SessionMode

    session_cls, _ = isolated_session_patch
    run = asyncio.run(session_cls.create(mode=SessionMode.RUN, sandbox=False, initialization_id="a"))
    edit = asyncio.run(session_cls.create(mode=SessionMode.EDIT, sandbox=False, initialization_id="b"))
    assert run == {"mode": SessionMode.RUN, "sandbox": True, "initialization_id": "a"}
    assert edit == {"mode": SessionMode.EDIT, "sandbox": False, "initialization_id": "b"}


def test_ipc_patch_points_the_venv_at_the_shim_read_only(isolated_session_patch):
    _, ipc_mod = isolated_session_patch
    assert ipc_mod._get_venv_config(None) == {"path": str(SHIM), "writable": False}


# --- the locked config, read by marimo itself -----------------------------------


def test_locked_config_as_marimo_reads_it(tmp_path):
    code = (
        "import json\n"
        "from marimo._config.manager import get_default_config_manager\n"
        "from marimo._config.utils import get_user_config_path\n"
        "c = get_default_config_manager(current_path=None).get_config()\n"
        "print(json.dumps([get_user_config_path(), c['experimental'].get('rtc_v2'), c['ai'].get('enabled')]))\n"
    )
    clean = {k: v for k, v in os.environ.items() if not k.startswith(("MARIMO_", "XDG_"))}
    r = subprocess.run(
        [sys.executable, "-c", code],
        env={**clean, "HOME": str(tmp_path), "XDG_CONFIG_HOME": str(CONFIG)},
        cwd=tmp_path, capture_output=True, text=True, timeout=60, check=True,
    )
    path, rtc, ai = json.loads(r.stdout.strip().splitlines()[-1])
    assert Path(path) == CONFIG / "marimo" / "marimo.toml"
    assert rtc is False
    assert ai is False


# --- the security-layer lock: holds against project and script layers ------------


def test_security_layer_lock_beats_project_and_script_config(tmp_path):
    """Read with marimo's own config manager; the pyproject guard is bypassed on purpose."""
    work = tmp_path / "work"
    work.mkdir()
    (work / "pyproject.toml").write_text("[tool.marimo.ai]\nenabled = true\n")
    app = work / "app.py"
    app.write_text(
        "# /// script\n# [tool.marimo.experimental]\n# rtc_v2 = true\n# ///\n"
        "import marimo\napp = marimo.App()\n"
    )
    code = (
        "import json, sys\n"
        "from marimo._config.manager import get_default_config_manager\n"
        "from marimo._config.manager import ScriptConfigManager\n"
        "def read():\n"
        "    m = get_default_config_manager(current_path=sys.argv[1]).with_partial(ScriptConfigManager(sys.argv[1]))\n"
        "    c = m.get_config()\n"
        "    return [c['experimental'].get('rtc_v2'), c['ai'].get('enabled')]\n"
        "before = read()\n"
        "from egret_marimo.patches import install_config_lock\n"
        "install_config_lock()\n"
        "print(json.dumps([before, read()]))\n"
    )
    clean = {k: v for k, v in os.environ.items() if not k.startswith(("MARIMO_", "XDG_"))}
    r = subprocess.run(
        [sys.executable, "-c", code, str(app)],
        env={**clean, "HOME": str(tmp_path), "XDG_CONFIG_HOME": str(CONFIG)},
        cwd=work, capture_output=True, text=True, timeout=60, check=True,
    )
    before, after = json.loads(r.stdout.strip().splitlines()[-1])
    assert before == [True, True], "control: the header and pyproject DO override the user layer"
    assert after == [False, False]


def test_config_lock_is_installed_in_both_modes(env, egret, monkeypatch):
    installed = []
    monkeypatch.setattr(launcher, "install_config_lock", lambda: installed.append(1))
    monkeypatch.setenv("EGRET_URL", egret.base_url)
    monkeypatch.setenv("EGRET_APP_ID", APP)
    monkeypatch.setenv("EGRET_VERSION_ID", VERSION)
    egret.pod_tokens["pod-tok"] = {}
    egret.versions[(APP, VERSION)] = {"source": "", "version_no": 1}
    assert launcher.main(["edit"]) == 0
    monkeypatch.setenv("EGRET_POD_TOKEN", "pod-tok")  # the fake exec removed them, as execve would
    monkeypatch.setenv("EGRET_MARIMO_TOKEN", "marimo-secret")
    assert launcher.main(["run"]) == 0
    assert installed == [1, 1]
