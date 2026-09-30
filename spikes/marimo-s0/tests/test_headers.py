from headers import forward_headers


def test_strips_client_egret_headers_cookies_auth_and_hop_by_hop():
    incoming = {
        "X-Egret-App-Token": "forged",
        "x-egret-anything": "forged",
        "Cookie": "apps_sid=abc",
        "Authorization": "Bearer forged",
        "Connection": "keep-alive",
        "Host": "apps.egret.localhost:8701",
        "Marimo-Server-Token": "skew-123",
        "Accept": "text/html",
    }
    out = forward_headers(incoming, session_token="real", pod_token="pod-secret")
    assert out["x-egret-app-token"] == "real"
    assert out["authorization"] == "Bearer pod-secret"
    assert out["marimo-server-token"] == "skew-123"  # skew protection must pass through
    assert out["accept"] == "text/html"
    for gone in ("cookie", "connection", "host", "x-egret-anything"):
        assert gone not in out
    assert "forged" not in out.values()
