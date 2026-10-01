import pytest

import fake_egret


@pytest.fixture
def egret(monkeypatch):
    fake_egret.STATE.reset()
    server, port = fake_egret.serve()
    base = f"http://127.0.0.1:{port}"
    monkeypatch.setenv("EGRET_URL", base)
    fake_egret.STATE.base_url = base
    yield fake_egret.STATE
    server.should_exit = True
