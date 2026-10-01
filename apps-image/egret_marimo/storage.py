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

A reload does not by itself give a new manager: in edit mode marimo resumes the
orphaned session for the file (``SessionManager.maybe_resume_session``), keeping
its ``AppFileManager``. So a session whose storage has hit a conflict is closed
instead of resumed, and the reconnect creates a new session on the latest draft.
"""

from __future__ import annotations

import re
from pathlib import Path

from marimo._environments.script_metadata import notebook_file_lock
from marimo._server.session_manager import SessionManager
from marimo._server.workspace._base import NotebookWorkspace
from marimo._session.model import ConnectionState
from marimo._session.notebook.file_manager import AppFileManager
from marimo._session.notebook.storage import FilesystemStorage
from marimo._utils.http import HTTPException

from .client import GatewayError
from .podapi import DraftConflict, PodApi

RENAME_MESSAGE = "An app cannot be renamed here: renaming is done in egret."
CONFLICT_MESSAGE = "This draft was saved by someone else — reload to get the latest version."

_APP_FILE = re.compile(
    r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\.py$", re.IGNORECASE
)


def _gateway_to_http(e: GatewayError) -> HTTPException:
    if e.status in (404, 413):  # 404: the app was deleted in egret
        return HTTPException(e.status, e.detail)
    return HTTPException(502, f"egret: {e.detail}")


class EgretStorage(FilesystemStorage):
    """marimo storage for one editor session: saves the draft to egret, then to the local file.

    It is bound to ONE file, ``<workdir>/<app_id>.py``: every write is a PUT of the bound
    app's draft, so a write to any other path (marimo's copy endpoint writes the copy
    through the source notebook's storage) is refused rather than saved over this draft.
    Renaming is refused too (spec §5.1): an app's file name is its id, set by egret.
    """

    def __init__(self, app_id: str, api: PodApi, base_revision: int, app_file: Path) -> None:
        super().__init__()
        self.app_id = app_id
        self.app_file = Path(app_file).resolve()
        self.api = api
        self.base_revision = base_revision
        # Set on the first 409; the session is then closed rather than resumed on reconnect.
        self.conflicted = False

    def write(self, path: Path, content: str) -> None:
        if Path(path).resolve() != self.app_file:
            raise HTTPException(403, f"only this app's draft can be saved here, not {Path(path).name}")
        try:
            new = self.api.put_draft(self.app_id, self.base_revision, content)
        except DraftConflict:
            # Deliberately leave base_revision alone: this session's copy is stale
            # and must never be saved over the newer draft.
            self.conflicted = True
            raise HTTPException(409, CONFLICT_MESSAGE) from None
        except GatewayError as e:
            raise _gateway_to_http(e) from e
        self.base_revision = new
        super().write(path, content)

    def rename(self, old_path: Path, new_path: Path) -> None:
        raise HTTPException(403, RENAME_MESSAGE)


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

        # The same lock marimo's save holds around storage.write, so a load that
        # fetched revision N cannot write the file after a concurrent save of N+1.
        with notebook_file_lock(str(resolved)):
            try:
                draft = api.get_draft(app_id)
            except GatewayError as e:
                raise _gateway_to_http(e) from e
            Path(resolved).write_text(draft.source, encoding="utf-8")

        manager = original_load(self, key, defaults)
        manager.storage = EgretStorage(app_id, api, draft.revision, Path(resolved))
        return manager

    NotebookWorkspace._egret_original_load = original_load  # type: ignore[attr-defined]
    NotebookWorkspace.load = load  # type: ignore[method-assign]
    _install_resume_patch()


def _is_conflicted(session) -> bool:
    storage = getattr(session.app_file_manager, "storage", None)
    return isinstance(storage, EgretStorage) and storage.conflicted


def _install_resume_patch() -> None:
    """Close, rather than resume, an orphaned session whose save hit a conflict."""
    original_resume = getattr(
        SessionManager, "_egret_original_resume", SessionManager.maybe_resume_session
    )

    def maybe_resume_session(self: SessionManager, new_session_id, file_key):
        for session in self._repository.get_all_by_file_key(
            file_key, resolved_path=self._resolve_file_key(file_key)
        ):
            if session.connection_state() == ConnectionState.ORPHANED and _is_conflicted(session):
                session_id = self._repository.get_session_id(session)
                if session_id is not None:
                    self.close_session(session_id)
        return original_resume(self, new_session_id, file_key)

    SessionManager._egret_original_resume = original_resume  # type: ignore[attr-defined]
    SessionManager.maybe_resume_session = maybe_resume_session  # type: ignore[method-assign]
