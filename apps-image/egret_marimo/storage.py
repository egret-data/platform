"""Drafts live in egret, not on the pod's disk.

marimo saves a notebook through ``AppFileManager.storage.write``. ``EgretStorage``
sends that save to egret as a conditional PUT (``If-Match`` = the base revision),
so a save made from a stale copy is refused with 409 instead of silently
overwriting another author's draft.

The base revision is held once per app, in a process-wide ``BaseRevisions``,
not per manager and not re-read on every load. marimo calls
``NotebookWorkspace.load`` on many requests (``SessionManager.app_manager``),
and the S0 spike re-fetched the draft and re-bound the base revision on every
one of them. That let an editor holding an old copy pick up the newer revision
without its content, and its next save overwrote the other author's work. Here
a load only fetches when the base is unknown or a save has already hit a
conflict (the app is "stale").
"""

from __future__ import annotations

import re
import threading
from pathlib import Path

from marimo._server.workspace._base import NotebookWorkspace
from marimo._session.notebook.file_manager import AppFileManager
from marimo._session.notebook.storage import FilesystemStorage
from marimo._utils.http import HTTPException

from .client import GatewayError
from .podapi import DraftConflict, PodApi

CONFLICT_MESSAGE = "This draft was saved by someone else — reload to get the latest version."

_APP_FILE = re.compile(
    r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\.py$", re.IGNORECASE
)


class BaseRevisions:
    """The draft revision each app's local copy was taken from. One per pod; thread-safe."""

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._revisions: dict[str, int] = {}
        self._stale: set[str] = set()

    def get(self, app_id: str) -> int | None:
        with self._lock:
            return self._revisions.get(app_id)

    def set(self, app_id: str, revision: int) -> None:
        """Record a new base revision; this also clears the stale mark."""
        with self._lock:
            self._revisions[app_id] = revision
            self._stale.discard(app_id)

    def mark_stale(self, app_id: str) -> None:
        with self._lock:
            self._stale.add(app_id)

    def is_stale(self, app_id: str) -> bool:
        with self._lock:
            return app_id in self._stale


def _gateway_to_http(e: GatewayError) -> HTTPException:
    if e.status == 413:
        return HTTPException(413, e.detail)
    return HTTPException(502, f"egret: {e.detail}")


class EgretStorage(FilesystemStorage):
    """marimo storage that saves an app's draft to egret, then to the local file."""

    def __init__(self, app_id: str, api: PodApi, revisions: BaseRevisions) -> None:
        super().__init__()
        self.app_id = app_id
        self.api = api
        self.revisions = revisions

    def write(self, path: Path, content: str) -> None:
        base = self.revisions.get(self.app_id)
        if base is None:
            # No known base: we cannot make a conditional save. Force a re-fetch.
            self.revisions.mark_stale(self.app_id)
            raise HTTPException(409, CONFLICT_MESSAGE)
        try:
            new = self.api.put_draft(self.app_id, base, content)
        except DraftConflict:
            self.revisions.mark_stale(self.app_id)
            raise HTTPException(409, CONFLICT_MESSAGE) from None
        except GatewayError as e:
            raise _gateway_to_http(e) from e
        self.revisions.set(self.app_id, new)
        super().write(path, content)


def _app_id_for(workdir: Path, path: Path) -> str | None:
    """The app id if ``path`` is ``<workdir>/<uuid>.py``, else None."""
    if path.resolve().parent != workdir and path.parent.absolute() != workdir:
        return None
    if not _APP_FILE.match(path.name):
        return None
    return path.stem.lower()


def install_storage_patch(workdir: Path, api: PodApi, revisions: BaseRevisions) -> None:
    """Patch ``NotebookWorkspace.load`` so app files are backed by egret drafts."""
    workdir = Path(workdir).resolve()
    original_load = getattr(NotebookWorkspace, "_egret_original_load", NotebookWorkspace.load)

    def load(self: NotebookWorkspace, key: str, defaults=None) -> AppFileManager:
        try:
            resolved = self.resolve(key)
        except HTTPException as e:
            # A fresh pod has no local copy yet; DirectoryWorkspace.resolve 404s on it.
            if e.status_code != 404:
                raise
            candidate = Path(key)
            if not candidate.is_absolute():
                candidate = Path(self.directory or workdir) / candidate
            if _app_id_for(workdir, candidate) is None:
                raise
            resolved = str(candidate)
        app_id = _app_id_for(workdir, Path(resolved)) if resolved else None
        if app_id is None:
            return original_load(self, key, defaults)

        if revisions.get(app_id) is None or revisions.is_stale(app_id):
            try:
                draft = api.get_draft(app_id)
            except GatewayError as e:
                raise _gateway_to_http(e) from e
            Path(resolved).write_text(draft.source, encoding="utf-8")
            revisions.set(app_id, draft.revision)

        manager = original_load(self, key, defaults)
        manager.storage = EgretStorage(app_id, api, revisions)
        return manager

    load._egret_patched = True  # type: ignore[attr-defined]
    NotebookWorkspace._egret_original_load = original_load  # type: ignore[attr-defined]
    NotebookWorkspace.load = load  # type: ignore[method-assign]
