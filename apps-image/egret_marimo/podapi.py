"""Client for egret's pod API (draft and published-version source)."""

from dataclasses import dataclass

import httpx

from .client import GatewayError, error_for, transport_error


@dataclass
class Draft:
    source: str
    revision: int
    layout: dict | None = None


@dataclass
class Version:
    source: str
    layout: dict | None = None


class DraftConflict(Exception):
    def __init__(self, current_revision: int | None) -> None:
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
        try:
            r = self._client.request(method, url, headers=self._headers | kw.pop("headers", {}), **kw)
        except httpx.TransportError as e:  # includes timeouts
            raise transport_error(e) from e
        if r.status_code == 409:
            try:
                current = int(r.json()["current_revision"])
            except (ValueError, KeyError, TypeError):
                raise GatewayError(409, r.text) from None
            raise DraftConflict(current)
        if r.status_code == 401:
            raise GatewayError(401, "pod token rejected")
        if not r.is_success:
            raise error_for(r)
        return r

    def get_draft(self, app_id: str) -> Draft:
        d = self._send("GET", self._url(app_id, "draft")).json()
        return Draft(source=d["source"], revision=int(d["revision"]), layout=d.get("layout"))

    def put_draft(
        self, app_id: str, expected_revision: int, source: str, layout: dict | None = None
    ) -> int:
        r = self._send(
            "PUT",
            self._url(app_id, "draft"),
            json={"source": source, "layout": layout},
            headers={"if-match": f'"{expected_revision}"'},
        )
        return int(r.json()["revision"])

    def get_version(self, app_id: str, version_id: str) -> Version:
        d = self._send("GET", self._url(app_id, f"versions/{version_id}")).json()
        return Version(source=d["source"], layout=d.get("layout"))
