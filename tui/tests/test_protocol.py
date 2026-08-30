from __future__ import annotations

import base64
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from sshmgr_tui import protocol


TOKEN = "a" * 64


def _envelope(osc: bytes) -> dict:
    prefix = b"\x1b]1337;SetUserVar=sshmgr="
    first, clear = osc.split(b"\x07", 1)
    assert clear == prefix + b"\x07"
    return json.loads(base64.b64decode(first.removeprefix(prefix)))


class ProtocolTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory(prefix="wezterm-sshmgr-test-")
        os.chmod(self.temp.name, 0o700)
        protocol.configure(TOKEN, self.temp.name)

    def tearDown(self) -> None:
        self.temp.cleanup()

    def test_emit_keeps_secret_out_of_user_var(self) -> None:
        writes: list[bytes] = []
        with mock.patch.object(protocol, "_write_terminal", side_effect=lambda data: (writes.append(data), True)[1]):
            protocol.emit({"op": "upsert", "raw": {"options": {"password": "s3cret"}}})

        self.assertEqual(len(writes), 1)
        self.assertNotIn(b"s3cret", writes[0])
        envelope = _envelope(writes[0])
        self.assertEqual(envelope["v"], 2)
        self.assertEqual(envelope["token"], TOKEN)
        self.assertEqual(envelope["seq"], 1)

        request = Path(self.temp.name, envelope["request"])
        self.assertEqual(request.parent, Path(self.temp.name))
        self.assertEqual(request.stat().st_mode & 0o777, 0o600)
        body = json.loads(request.read_text(encoding="utf-8"))
        self.assertEqual(body["raw"]["options"]["password"], "s3cret")
        self.assertEqual(body["_session"], TOKEN)
        self.assertEqual(body["_seq"], 1)

    def test_sequence_increases_and_names_are_unique(self) -> None:
        writes: list[bytes] = []
        with mock.patch.object(protocol, "_write_terminal", side_effect=lambda data: (writes.append(data), True)[1]):
            protocol.emit({"op": "reload"})
            protocol.emit({"op": "hide"})
        first, second = map(_envelope, writes)
        self.assertEqual((first["seq"], second["seq"]), (1, 2))
        self.assertNotEqual(first["request"], second["request"])

    def test_failed_terminal_write_removes_request(self) -> None:
        with mock.patch.object(protocol, "_write_terminal", return_value=False):
            protocol.emit({"op": "reload"})
        self.assertEqual(list(Path(self.temp.name).iterdir()), [])

    def test_explicit_cleanup_removes_owned_runtime_files(self) -> None:
        directory = Path(self.temp.name)
        snapshot = directory / "snapshot.json"
        snapshot.write_text("{}", encoding="utf-8")
        request = directory / f"request-1-{'b' * 32}.json"
        request.write_text("{}", encoding="utf-8")

        protocol.cleanup_runtime(str(snapshot), self.temp.name)

        self.assertFalse(directory.exists())

    def test_rejects_bad_token_and_non_private_runtime(self) -> None:
        with self.assertRaises(ValueError):
            protocol.configure("predictable", self.temp.name)
        if os.name != "nt":
            os.chmod(self.temp.name, 0o755)
            with self.assertRaises(ValueError):
                protocol.configure(TOKEN, self.temp.name)


if __name__ == "__main__":
    unittest.main()
