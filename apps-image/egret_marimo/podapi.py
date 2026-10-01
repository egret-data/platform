"""Client for egret's pod API (draft and published-version source)."""

from dataclasses import dataclass

import httpx

from .client import error_for


@dataclass
class Draft:
    source: str
    revision: int


class DraftConflict(Exception):
    def __init__(self, current_revision: int) -> None:
        super().__init__(f"draft is at revision {current_revision}")
        self.current_revision = current_revision


class PodApi:
    def __init__(self, base_url: str, pod_token: str, client: httpx.Client | None = None) -> None:
        self._base = base_url.rstrip("/")
        self._headers = {"authorization": f"Bearer {pod_token}"}
        self._client = client or httpx.Client(timeout=30)

    def _url(self, app_id: str, tail: str) -> str:
        return f"{self._base}/api/apps/pod/apps/{app_id}/{tail}"

    def _send(self, method: str, url: str, **kw) -> httpx.Response:
        r = self._client.request(method, url, headers=self._headers | kw.pop("headers", {}), **kw)
        if r.status_code == 409:
            raise DraftConflict(int(r.json()["current_revision"]))
        if not r.is_success:
            raise error_for(r)
        return r

    def get_draft(self, app_id: str) -> Draft:
        d = self._send("GET", self._url(app_id, "draft")).json()
        return Draft(source=d["source"], revision=int(d["revision"]))

    def put_draft(self, app_id: str, expected_revision: int, source: str) -> int:
        r = self._send(
            "PUT",
            self._url(app_id, "draft"),
            content=source.encode(),
            headers={"if-match": f'"{expected_revision}"', "content-type": "text/x-python"},
        )
        return int(r.json()["revision"])

    def get_version(self, app_id: str, version_id: str) -> str:
        return self._send("GET", self._url(app_id, f"versions/{version_id}")).json()["source"]
