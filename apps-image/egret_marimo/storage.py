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

Grid layout (D23): ``AppFileManager.save`` writes the ``layouts/<app>.<type>.json``
companion file with a raw ``open()`` call (``save_layout_config``), entirely
outside the ``Storage`` abstraction -- patching ``EgretStorage.write`` alone never
sees it. Instead ``install_file_manager_save_patch`` wraps ``save`` itself to stash
``request.layout`` on the bound ``EgretStorage`` right before marimo's own save
runs; ``EgretStorage.write`` then forwards it to egret in the SAME ``put_draft``
call as the source, so both advance on the same revision. marimo's own disk write
still happens underneath, which is what lets the CURRENT session read it straight
back; ``load`` separately restores it from egret's draft onto a fresh pod's disk,
since that pod never ran the save that produced it.
"""

from __future__ import annotations

import json
import re
from pathlib import Path
from typing import Any

from marimo._environments.script_metadata import notebook_file_lock
from marimo._server.session_manager import SessionManager
from marimo._server.workspace._base import NotebookWorkspace
from marimo._session.model import ConnectionState
from marimo._session.notebook.file_manager import AppFileManager
from marimo._session.notebook.storage import FilesystemStorage
from marimo._utils.http import HTTPException

from .client import GatewayError
from .podapi import DraftConflict, PodApi


def layout_file_path(workdir: Path, app_id: str, layout: dict[str, Any]) -> Path:
    """Where marimo's `save_layout_config`/`read_layout_config` put this app's
    layout: `<workdir>/layouts/<app_id>.<type>.json`.
    """
    return workdir / "layouts" / f"{app_id}.{layout['type']}.json"


def write_layout_file(workdir: Path, app_id: str, layout: dict[str, Any] | None) -> None:
    """Materialize a layout dict restored from egret onto this pod's local disk,
    in the exact shape marimo's own `save_layout_config` writes it, so
    `read_layout_config` finds it as if this pod had produced it itself. A no-op
    for `None` (no layout saved yet).
    """
    if layout is None:
        return
    path = layout_file_path(workdir, app_id, layout)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(layout, indent=2), encoding="utf-8")

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

    def __init__(
        self,
        app_id: str,
        api: PodApi,
        base_revision: int,
        app_file: Path,
        layout: dict[str, Any] | None = None,
    ) -> None:
        super().__init__()
        self.app_id = app_id
        self.app_file = Path(app_file).resolve()
        self.api = api
        self.base_revision = base_revision
        # Set on the first 409; the session is then closed rather than resumed on reconnect.
        self.conflicted = False
        # The layout to send on the NEXT write, kept in sync with egret rather than
        # read fresh each time: seeded from the draft `load` fetched, and
        # overwritten by `install_file_manager_save_patch` right before every
        # `AppFileManager.save` (marimo's `request.layout`, `None` included --
        # that is how `save` itself represents "the user removed the layout").
        # A write that reaches here WITHOUT going through `save` first (e.g.
        # `save_app_config`) resends whatever this already was, which is correct:
        # that save never touched the layout, so echoing it back is a no-op.
        self.layout = layout

    def write(self, path: Path, content: str) -> None:
        if Path(path).resolve() != self.app_file:
            raise HTTPException(403, f"only this app's draft can be saved here, not {Path(path).name}")
        try:
            new = self.api.put_draft(self.app_id, self.base_revision, content, self.layout)
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
    if not _APP_FILE.fullmatch(path.name):
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
            # This pod never ran the save that produced the layout file `draft.source`
            # references (`layout_file=...` in its header) -- restore it before
            # marimo parses that header, or `read_layout_config` finds nothing.
            write_layout_file(workdir, app_id, draft.layout)

        manager = original_load(self, key, defaults)
        manager.storage = EgretStorage(app_id, api, draft.revision, Path(resolved), draft.layout)
        return manager

    NotebookWorkspace._egret_original_load = original_load  # type: ignore[attr-defined]
    NotebookWorkspace.load = load  # type: ignore[method-assign]
    _install_resume_patch()
    install_file_manager_save_patch()


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


def install_file_manager_save_patch() -> None:
    """`AppFileManager.save` is the one place `request.layout` (the raw dict the
    browser sent) is available -- `save_layout_config`, which marimo calls from
    inside `save` moments later, only has a derived `LayoutConfig` and writes
    straight to the pod's local disk, never through `Storage`. Stashing
    `request.layout` on the bound `EgretStorage` here, before `save` runs, is what
    lets `EgretStorage.write` forward it in the same `put_draft` call as the
    source a few lines later in the same call stack.
    """
    original_save = getattr(AppFileManager, "_egret_original_save", AppFileManager.save)

    def save(self: AppFileManager, request, **kwargs):
        if isinstance(self.storage, EgretStorage):
            self.storage.layout = request.layout
        return original_save(self, request, **kwargs)

    AppFileManager._egret_original_save = original_save  # type: ignore[attr-defined]
    AppFileManager.save = save  # type: ignore[method-assign]
