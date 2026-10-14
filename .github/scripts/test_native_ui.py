"""Helper-unit coverage only; never claim these as native application tests."""
import importlib.util
import json
import pathlib
import sqlite3
import tempfile
import threading
import types
import unittest
from unittest import mock
import urllib.error
import urllib.request

SPEC = importlib.util.spec_from_file_location("native_ui", pathlib.Path(__file__).with_name("native-ui.py"))
native = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(native)


class ProcessQueryTests(unittest.TestCase):
    def test_windows_query_is_fail_closed(self):
        application = pathlib.Path("xyzaishell.exe").resolve()
        error = native.subprocess.CalledProcessError(1, "pwsh")
        with mock.patch.object(native, "os", types.SimpleNamespace(name="nt")), \
                mock.patch.object(native.subprocess, "check_output", side_effect=error) as query:
            with self.assertRaises(native.subprocess.CalledProcessError):
                native.application_pids(application)
        self.assertIn("$ErrorActionPreference='Stop'", query.call_args.args[0][-1])

    def test_windows_query_handles_zero_one_and_multiple_processes(self):
        application = pathlib.Path("xyzaishell.exe").resolve()
        for output, expected in [("", []), ("123", [123]), ("[123,456]", [123, 456])]:
            with self.subTest(output=output), \
                    mock.patch.object(native, "os", types.SimpleNamespace(name="nt")), \
                    mock.patch.object(native.subprocess, "check_output", return_value=output):
                self.assertEqual(native.application_pids(application), expected)


class NativeHelperTests(unittest.TestCase):
    def setUp(self):
        self.server = native.http.server.ThreadingHTTPServer(("127.0.0.1", 0), native.ModelFixture)
        self.server.requests, self.server.invalid_requests = [], 0
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=2)

    def post(self, body=None, path="/v1/chat/completions", token=native.TOKEN):
        if body is None:
            body = {"model": "native-fixture-model", "stream": True,
                    "messages": [{"role": "user", "content": native.PROMPT}]}
        request = urllib.request.Request(f"http://127.0.0.1:{self.server.server_port}{path}",
                                         data=json.dumps(body).encode(),
                                         headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"})
        with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(request, timeout=2) as response:
            return response.read().decode()

    def test_fixture_stream_is_synthetic_and_terminated(self):
        stream = self.post()
        chunks = stream.split("\n\n")
        self.assertEqual(json.loads(chunks[0][6:])["choices"][0]["delta"]["content"], native.REPLY)
        self.assertIn("data: [DONE]\n\n", stream)
        self.assertEqual(len(self.server.requests), 1)
        self.assertEqual(self.server.invalid_requests, 0)

    def test_fixture_rejects_unexpected_auth(self):
        with self.assertRaises(urllib.error.HTTPError):
            self.post(token="wrong-synthetic-token")
        self.assertEqual(self.server.invalid_requests, 1)

    def test_fixture_rejects_unexpected_path(self):
        with self.assertRaises(urllib.error.HTTPError):
            self.post(path="/unexpected")

    def test_fixture_rejects_tool_execution(self):
        with self.assertRaises(urllib.error.HTTPError):
            self.post({"model": "native-fixture-model", "stream": True, "tools": [{"type": "function"}],
                       "messages": [{"role": "user", "content": native.PROMPT}]})

    def test_stored_identity_requires_one_exact_synthetic_chat(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            with sqlite3.connect(root / "history.sqlite3") as connection:
                connection.execute("CREATE TABLE chats (chat_id TEXT, task_id TEXT, payload TEXT)")
                connection.execute("INSERT INTO chats VALUES (?,?,?)", ("chat-a", "task-a", native.PROMPT + native.REPLY))
            self.assertEqual(native.stored_identity(root), ("chat-a", "task-a"))
            with sqlite3.connect(root / "history.sqlite3") as connection:
                connection.execute("INSERT INTO chats VALUES (?,?,?)", ("chat-b", "task-b", native.PROMPT + native.REPLY))
            with self.assertRaises(AssertionError):
                native.stored_identity(root)

    def test_wait_is_bounded_and_retries_transient_errors(self):
        values = iter([False, True])
        self.assertTrue(native.wait_for(lambda: next(values), timeout=1))
        with self.assertRaises(AssertionError):
            native.wait_for(lambda: False, timeout=0.01)


if __name__ == "__main__":
    unittest.main()
