"""Validated, recoverable replacement of generated dependency directories."""
from __future__ import annotations

import os
from pathlib import Path, PurePosixPath
import shutil
import uuid


def snapshot_file(root: Path, name: str) -> Path:
    """Reject absolute, escaping and aliased paths in source receipts."""
    parts = PurePosixPath(name).parts
    if not parts or "\\" in name or ":" in name or any(p in (".", "..") for p in parts) or name.startswith("/"):
        raise ValueError(f"Invalid snapshot path: {name}")
    path = root / name
    if path.resolve() != root.resolve().joinpath(*parts):
        raise ValueError(f"Snapshot path follows a link: {name}")
    return path


def replace_directory(staged: Path, destination: Path) -> None:
    """Publish a sibling staging directory, restoring the old tree on failure."""
    parent = destination.parent.resolve(strict=True)
    staged = staged.absolute()
    destination = destination.absolute()
    backup = parent / (".dependency-backup-" + uuid.uuid4().hex)
    for path in (staged, destination, backup):
        if path.parent != parent or path.resolve() != path or path == parent:
            raise ValueError(f"Dependency replacement target is outside its parent: {path}")
    if staged == destination or not staged.is_dir():
        raise ValueError("Dependency staging directory is invalid")
    existed = destination.exists()
    if existed:
        os.replace(destination, backup)
    try:
        os.replace(staged, destination)
    except BaseException:
        if existed:
            os.replace(backup, destination)
        raise
    if existed:
        shutil.rmtree(backup)
