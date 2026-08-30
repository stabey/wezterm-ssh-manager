"""Send authenticated, one-shot requests to the wezterm Lua plugin.

OSC user variables are visible to every client attached to a wezterm mux and
remain associated with the pane. They are therefore only used as a wake-up
signal here. The command itself (which may contain a password) is written to
a private per-TUI runtime directory and removed by Lua before it is handled.
"""
from __future__ import annotations

import base64
import json
import os
import re
import secrets
import sys
from pathlib import Path
from typing import Final


_TOKEN_RE: Final = re.compile(r"^[0-9a-f]{64}$")
_REQUEST_RE: Final = re.compile(r"^request-[1-9][0-9]*-[0-9a-f]{32}\.json$")
_MAX_REQUEST_BYTES: Final = 4 * 1024 * 1024

_session_token: str | None = None
_runtime_dir: Path | None = None
_sequence = 0


def configure(session_token: str, runtime_dir: str) -> None:
    """Configure the protocol once, from the trusted ``__main__`` entrypoint."""
    global _session_token, _runtime_dir, _sequence

    if not isinstance(session_token, str) or not _TOKEN_RE.fullmatch(session_token):
        raise ValueError("invalid sshmgr TUI session token")

    directory = Path(runtime_dir).expanduser().resolve(strict=True)
    if not directory.is_dir() or not directory.name.startswith("wezterm-sshmgr-"):
        raise ValueError("sshmgr TUI runtime path is not a directory")

    # Lua creates this directory with tempfile.mkdtemp and narrows it to 0700.
    # Verify rather than chmod an arbitrary caller-supplied parent directory.
    if os.name != "nt":
        stat = directory.stat()
        if stat.st_uid != os.getuid() or stat.st_mode & 0o077:
            raise ValueError("sshmgr TUI runtime directory is not private")

    _session_token = session_token
    _runtime_dir = directory
    _sequence = 0


def _write_terminal(data: bytes) -> bool:
    try:
        os.write(1, data)
        return True
    except OSError:
        pass

    out = getattr(sys, "__stdout__", None)
    if out is None:
        return False
    try:
        if hasattr(out, "buffer"):
            out.buffer.write(data)
            out.buffer.flush()
        else:
            out.write(data.decode("ascii"))
            out.flush()
        return True
    except (OSError, UnicodeError):
        return False


def _new_request(payload: dict, sequence: int) -> tuple[str, Path]:
    if _session_token is None or _runtime_dir is None:
        raise RuntimeError("sshmgr TUI protocol is not configured")

    body = dict(payload)
    body["_session"] = _session_token
    body["_seq"] = sequence
    raw = json.dumps(body, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
    if len(raw) > _MAX_REQUEST_BYTES:
        raise ValueError("sshmgr TUI request is too large")

    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
    flags |= getattr(os, "O_CLOEXEC", 0)
    flags |= getattr(os, "O_NOFOLLOW", 0)

    # O_EXCL plus a random basename prevents clobbering or following a file
    # planted in the runtime directory. The file is complete before OSC is
    # emitted, so Lua can never observe partially written JSON.
    for _ in range(8):
        name = f"request-{sequence}-{secrets.token_hex(16)}.json"
        path = _runtime_dir / name
        try:
            fd = os.open(path, flags, 0o600)
        except FileExistsError:
            continue
        try:
            with os.fdopen(fd, "wb") as request_file:
                request_file.write(raw)
                request_file.flush()
                os.fsync(request_file.fileno())
            try:
                path.chmod(0o600)
            except OSError:
                if os.name != "nt":
                    raise
            return name, path
        except BaseException:
            # fdopen may already have closed fd; either outcome is harmless.
            try:
                os.close(fd)
            except OSError:
                pass
            try:
                path.unlink()
            except OSError:
                pass
            raise
    raise FileExistsError("cannot allocate a unique sshmgr TUI request")


def emit(msg: dict) -> None:
    """Emit a command without placing its contents in a persistent UserVar."""
    global _sequence

    if not isinstance(msg, dict):
        raise TypeError("sshmgr TUI message must be a dict")
    if _session_token is None or _runtime_dir is None:
        raise RuntimeError("sshmgr TUI protocol is not configured")

    _sequence += 1
    sequence = _sequence
    name, path = _new_request(msg, sequence)
    envelope = {
        "v": 2,
        "token": _session_token,
        "seq": sequence,
        "request": name,
    }
    raw = json.dumps(envelope, separators=(",", ":")).encode("ascii")
    encoded = base64.b64encode(raw).decode("ascii")

    # The second sequence clears the variable immediately. The first value
    # contains only an authenticated file reference; passwords never transit
    # OSC or remain in pane metadata.
    osc = (
        f"\033]1337;SetUserVar=sshmgr={encoded}\007"
        "\033]1337;SetUserVar=sshmgr=\007"
    ).encode("ascii")
    if not _write_terminal(osc):
        try:
            path.unlink()
        except OSError:
            pass


def cleanup_runtime(snapshot_path: str, runtime_dir: str | None = None) -> None:
    """Remove only files owned by this protocol, then the unique directory."""
    directory = _runtime_dir
    if runtime_dir is not None:
        try:
            candidate = Path(runtime_dir).expanduser().resolve(strict=True)
            stat = candidate.stat()
            private = os.name == "nt" or (stat.st_uid == os.getuid() and not stat.st_mode & 0o077)
            if candidate.is_dir() and candidate.name.startswith("wezterm-sshmgr-") and private:
                directory = candidate
            else:
                return
        except OSError:
            return
    if directory is None:
        return

    snapshot = Path(snapshot_path).resolve(strict=False)
    try:
        if snapshot.parent == directory and snapshot.name == "snapshot.json":
            snapshot.unlink(missing_ok=True)
    except OSError:
        pass

    try:
        entries = list(directory.iterdir())
    except OSError:
        entries = []
    for entry in entries:
        name = entry.name
        if _REQUEST_RE.fullmatch(name) or name.startswith("snapshot.json.tmp-"):
            try:
                entry.unlink()
            except OSError:
                pass
    try:
        directory.rmdir()
    except OSError:
        pass
