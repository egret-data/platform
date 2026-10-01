"""Drafts live in egret, not on the pod's disk.

marimo saves a notebook through ``AppFileManager.storage.write``. ``EgretStorage``
sends that save to egret as a conditional PUT (``If-Match`` = the base revision),
so a save made from a stale copy is refused with 409 instead of silently
overwriting another author's draft.

Each editor session gets its own base revision. Every load of an app file
fetches the latest draft and binds a fresh ``EgretStorage`` to that draft's
revision; the storage advances it only on its own successful save. A 409 does
NOT move it, so a stale session stays stale until its author reloads (which
loads a new manager). The S0 spike shared one revision that any load could
advance, letting an editor holding an old copy save over another author's work.
"""

from __future__ import annotations

import re
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


def _gateway_to_http(e: GatewayError) -> HTTPException:
    if e.status == 413:
        return HTTPException(413, e.detail)
    return HTTPException(502, f"egret: {e.detail}")


class EgretStorage(FilesystemStorage):
    """marimo storage for one editor session: saves the draft to egret, then to the local file."""

    def __init__(self, app_id: str, api: PodApi, base_revision: int) -> None:
        super().__init__()
        self.app_id = app_id
        self.api = api
        self.base_revision = base_revision

    def write(self, path: Path, content: str) -> None:
        try:
            new = self.api.put_draft(self.app_id, self.base_revision, content)
        except DraftConflict:
            # Deliberately leave base_revision alone: this session's copy is stale
            # and must never be saved over the newer draft.
            raise HTTPException(409, CONFLICT_MESSAGE) from None
        except GatewayError as e:
            raise _gateway_to_http(e) from e
        self.base_revision = new
        super().write(path, content)


def _app_id_for(workdir: Path, path: Path) -> str | None:
    """The app id if ``path`` is ``<workdir>/<uuid>.py``, else None."""
    if path.resolve().parent != workdir and path.parent.absolute() != workdir:
        return None
    if not _APP_FILE.match(path.name):
        return None
    return path.stem.lower()


def install_storage_patch(workdir: Path, api: PodApi) -> None:
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

        try:
            draft = api.get_draft(app_id)
        except GatewayError as e:
            raise _gateway_to_http(e) from e
        Path(resolved).write_text(draft.source, encoding="utf-8")

        manager = original_load(self, key, defaults)
        manager.storage = EgretStorage(app_id, api, draft.revision)
        return manager

    load._egret_patched = True  # type: ignore[attr-defined]
    NotebookWorkspace._egret_original_load = original_load  # type: ignore[attr-defined]
    NotebookWorkspace.load = load  # type: ignore[method-assign]
