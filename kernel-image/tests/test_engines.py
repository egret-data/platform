import json
from egret_kernel_helpers import engines as E


def _fake_http(responses):
    calls = []
    def http(url, body, token):
        calls.append((url, json.loads(body.decode()), token))
        return responses.pop(0)
    http.calls = calls
    return http


def test_engine_token_fetches_and_caches(monkeypatch):
    env = {"EGRET_API_URL": "http://egret", "EGRET_KERNEL_TOKEN": "kt"}
    http = _fake_http([
        {"jwt": "J1", "expires_at": "2999-01-01T00:00:00Z",
         "kind": "trino", "host": "trino", "port": 8080},
    ])
    cache = {}
    tok = E._engine_token("trino", env=env, http=http, cache=cache)
    assert tok["jwt"] == "J1" and tok["host"] == "trino" and tok["port"] == 8080
    # Cached: a second call (far from expiry) does not hit http again.
    tok2 = E._engine_token("trino", env=env, http=http, cache=cache)
    assert tok2["jwt"] == "J1"
    assert len(http.calls) == 1
    assert http.calls[0][0] == "http://egret/api/kernel/engine-token"
    assert http.calls[0][1] == {"engine": "trino"}
