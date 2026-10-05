import httpx
import pytest

from egret_marimo.client import GatewayError, SessionExpired
from egret_marimo.podapi import DraftConflict, PodApi


@pytest.fixture
def api(egret):
    egret.pod_tokens["pod-t"] = {"mode": "edit", "app_id": "a1"}
    egret.drafts["a1"] = {"source": "x = 1", "revision": 3}
    egret.versions[("a1", "v1")] = {"source": "y = 2", "version_no": 1}
    return PodApi(egret.base_url, "pod-t")


def test_get_then_put_round_trip(api, egret):
    d = api.get_draft("a1")
    assert (d.source, d.revision) == ("x = 1", 3)
    assert api.put_draft("a1", d.revision, "x = 2") == 4
    assert api.get_draft("a1").source == "x = 2"


def test_stale_put_raises_conflict(api):
    with pytest.raises(DraftConflict) as e:
        api.put_draft("a1", 1, "z")
    assert e.value.current_revision == 3


def test_if_match_is_quoted(api, egret):
    api.put_draft("a1", 3, "z")
    assert egret.events[-1]["if_match"] == '"3"'


def test_wrong_pod_token(egret):
    egret.drafts["a1"] = {"source": "", "revision": 1}
    with pytest.raises(GatewayError) as e:
        PodApi(egret.base_url, "nope").get_draft("a1")
    assert e.value.status == 401


def test_get_version_returns_source(api):
    assert api.get_version("a1", "v1").source == "y = 2"


def test_put_body_is_json_source(api, egret):
    api.put_draft("a1", 3, "x = 'é'")
    assert egret.drafts["a1"]["source"] == "x = 'é'"


def test_raw_text_put_rejected_by_fake(api, egret):
    r = api._client.put(f"{egret.base_url}/api/apps/pod/apps/a1/draft", content="x = 9",
                        headers={"authorization": "Bearer pod-t", "if-match": '"3"'})
    assert 400 <= r.status_code < 500
    assert egret.drafts["a1"]["source"] == "x = 1"


def test_source_at_limit_ok_over_limit_413(api):
    assert api.put_draft("a1", 3, "x" * (1024 * 1024)) == 4
    with pytest.raises(GatewayError) as e:
        api.put_draft("a1", 4, "x" * (1024 * 1024 + 1))
    assert e.value.status == 413


def test_non_json_409_is_gateway_error(api):
    def handler(request):
        return httpx.Response(409, text="conflict")
    p = PodApi("http://x", "t", client=httpx.Client(transport=httpx.MockTransport(handler)))
    with pytest.raises(GatewayError) as e:
        p.put_draft("a1", 1, "s")
    assert e.value.status == 409


def test_pod_401_is_not_session_expired(egret):
    with pytest.raises(GatewayError) as e:
        PodApi(egret.base_url, "nope").get_draft("a1")
    assert not isinstance(e.value, SessionExpired)
    assert "reload" not in e.value.detail


def test_unreachable_is_gateway_error():
    with pytest.raises(GatewayError) as e:
        PodApi("http://127.0.0.1:1", "t").get_draft("a1")
    assert e.value.status == 0


def test_missing_if_match_maps_428(egret):
    egret.pod_tokens["t"] = {}
    egret.drafts["a1"] = {"source": "", "revision": 1}
    api = PodApi(egret.base_url, "t", client=httpx.Client())
    r = api._client.put(f"{egret.base_url}/api/apps/pod/apps/a1/draft", content="x",
                        headers={"authorization": "Bearer t"})
    assert r.status_code == 428
