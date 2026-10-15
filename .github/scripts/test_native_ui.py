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


class NativeVersionPhaseTests(unittest.TestCase):
    def test_seed_and_same_version_preservation(self):
        for version in ("0.1.2", "0.1.3"):
            native.verify_phase_version("seed", version, "new-hash", None)
            expected = {"application_version": version, "application_sha256": "new-hash"}
            native.verify_phase_version("preserved", version, "new-hash", expected)
            with self.assertRaises(ValueError):
                native.verify_phase_version("preserved", version, "other-hash", expected)

    def test_upgrade_requires_exact_old_version_and_replaced_executable(self):
        expected = {"application_version": "0.1.2", "application_sha256": "old-hash"}
        native.verify_phase_version("upgrade", "0.1.3", "new-hash", expected)
        for version, digest, record in [("0.1.2", "new-hash", expected), ("0.1.3", "old-hash", expected),
                ("0.1.4", "new-hash", expected), ("0.1.3", "new-hash", None),
                ("0.1.3", "new-hash", {"application_version": "0.1.0", "application_sha256": "old-hash"})]:
            with self.assertRaises(ValueError):
                native.verify_phase_version("upgrade", version, digest, record)

    def test_invalid_version_or_phase_rejected(self):
        for phase, version in [("seed", "latest"), ("seed", "0.1.2\\nextra"), ("seed", "0.1.1"), ("seed", "0.1.4"), ("other", "0.1.2")]:
            with self.assertRaises(ValueError):
                native.verify_phase_version(phase, version, "hash", None)


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


class NativeDriverHelperTests(unittest.TestCase):
    def test_both_reviewed_versions_use_current_connection_and_log_controls(self):
        for version in ("0.1.2", "0.1.3"):
            with self.subTest(version=version):
                driver = mock.Mock()
                driver.button.side_effect = lambda text: text
                native.open_profiles(driver, version)
                native.close_profiles(driver, version)
                native.open_shell_logs(driver, version)
                self.assertEqual(driver.click.call_args_list, [mock.call("打开连接管理"), mock.call("关闭"),
                    mock.call("打开终端日志"), mock.call("//select[@aria-label='日志查看范围']/option[@value='all']")])

    def transport_driver(self, directory):
        driver = native.Driver.__new__(native.Driver)
        driver.application = pathlib.Path("xyzaishell")
        driver.application_processes = frozenset([123])
        driver.evidence = pathlib.Path(directory)
        driver.process = mock.Mock()
        driver.process.poll.return_value = None
        return driver

    def test_read_only_transport_retry_keeps_same_live_application(self):
        with tempfile.TemporaryDirectory() as directory:
            driver = self.transport_driver(directory)
            driver.command = mock.Mock(side_effect=[native.http.client.RemoteDisconnected("fixture"), True])
            with mock.patch.object(native, "application_pids", return_value=[123]), mock.patch.object(native.time, "sleep"):
                self.assertTrue(driver.js("return true"))
            self.assertEqual(driver.command.call_count, 2)
            self.assertTrue((driver.evidence / "read-only-transport-retries.jsonl").is_file())
            driver.process.terminate.assert_not_called()

    def test_read_only_retry_is_bounded_and_refuses_dead_app_or_driver(self):
        for state, expected in [("live", 3), ("app-exited", 1), ("app-replaced", 1), ("driver-exited", 1)]:
            with self.subTest(state=state), tempfile.TemporaryDirectory() as directory:
                driver = self.transport_driver(directory)
                if state == "driver-exited":
                    driver.process.poll.return_value = 1
                driver.command = mock.Mock(side_effect=native.http.client.RemoteDisconnected("fixture"))
                pids = [] if state == "app-exited" else [456] if state == "app-replaced" else [123]
                with mock.patch.object(native, "application_pids", return_value=pids), \
                        mock.patch.object(native.time, "sleep"):
                    with self.assertRaises(native.NativeTransportFailure):
                        driver.js("return true")
                self.assertEqual(driver.command.call_count, expected)

    def test_outer_readiness_wait_cannot_restart_exhausted_transport_retries(self):
        with tempfile.TemporaryDirectory() as directory:
            driver = self.transport_driver(directory)
            driver.command = mock.Mock(side_effect=urllib.error.URLError("fixture"))
            with mock.patch.object(native, "application_pids", return_value=[123]), mock.patch.object(native.time, "sleep"):
                with self.assertRaises(native.NativeTransportFailure):
                    native.wait_for(lambda: driver.js("return true"))
            self.assertEqual(driver.command.call_count, 3)

    def test_clipped_control_is_reached_by_real_tab_then_native_click(self):
        driver = native.Driver.__new__(native.Driver)
        driver.find = mock.Mock(return_value="target")
        calls = []
        def command(method, path, value=None):
            calls.append((method, path, value))
            if path.endswith("/enabled"):
                return True
            if path.endswith("/click") and sum(p.endswith('/click') for _, p, _ in calls) == 1:
                raise native.WebDriverError('{"value":{"error":"element not interactable"}}')
            if path == "/element/active":
                return {native.ELEMENT: "target"}
        driver.command = command
        driver.click("fixture")
        self.assertEqual([path for _, path, _ in calls],
                         ["/element/target/enabled", "/element/target/click", "/actions", "/element/active", "/element/target/click"])
        self.assertEqual(calls[2][2]["actions"][0]["actions"][0]["value"], "\ue004")

    def test_ambiguous_click_errors_are_never_replayed(self):
        driver = native.Driver.__new__(native.Driver)
        driver.find = mock.Mock(return_value="target")
        driver.command = mock.Mock(side_effect=[True, native.WebDriverError('{"value":{"error":"unknown error"}}')])
        with self.assertRaises(native.WebDriverError):
            driver.click("fixture")
        self.assertEqual(driver.command.call_count, 2)

    def test_close_verifies_app_exit_before_session_or_driver_teardown(self):
        driver = native.Driver.__new__(native.Driver)
        driver.application = pathlib.Path("xyzaishell")
        driver.session = "session"
        driver.process = mock.Mock()
        driver.process.poll.return_value = None
        driver.log = mock.Mock()
        driver.connection = mock.Mock()
        events = []
        def command(method, path):
            events.append(path)
            if path == "":
                raise native.WebDriverError('{"value":{"error":"invalid session id"}}')
        driver.command = command
        driver.process.terminate.side_effect = lambda: events.append("terminate-driver")
        with mock.patch.object(native, "application_pids", side_effect=lambda _: events.append("check-app-exited") or []):
            driver.close()
        self.assertEqual(events[:4], ["/window", "check-app-exited", "", "check-app-exited"])
        self.assertIn("terminate-driver", events[4:])

    def test_windows_fixture_file_explicitly_sets_current_user_owner(self):
        path = pathlib.Path("synthetic-model.toml")
        with mock.patch.object(native.subprocess, "check_output", return_value="S-1-5-21-123-456-789-1000\n"), \
                mock.patch.object(native.subprocess, "run") as run:
            native.protect_windows_fixture_file(path)
        self.assertEqual(run.call_args_list[-1].args[0][-2:], ["/setowner", "*S-1-5-21-123-456-789-1000"])

    def test_driver_cleanup_cannot_turn_a_live_application_into_a_pass(self):
        driver = native.Driver.__new__(native.Driver)
        driver.application = pathlib.Path("xyzaishell")
        driver.session = "session"
        driver.process = mock.Mock()
        driver.process.poll.return_value = None
        driver.log = mock.Mock()
        driver.connection = mock.Mock()
        driver.command = mock.Mock()
        with mock.patch.object(native, "wait_for", side_effect=[AssertionError("app still running"), True]):
            with self.assertRaisesRegex(RuntimeError, "shutdown was not clean"):
                driver.close()
        driver.process.terminate.assert_called_once()


class WebDriverTransportTests(unittest.TestCase):
    def setUp(self):
        class Handler(native.http.server.BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"
            def log_message(self, *_args):
                pass
            def do_GET(self):
                self.do_POST()
            def do_POST(self):
                body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
                self.server.observed.append((self.path, self.headers.get("Connection"), self.client_address, body))
                if self.path == "/mutation-drop":
                    self.close_connection = True
                    return
                payload = b'{"value":true}'
                self.send_response(200)
                if self.path.startswith("/chunked"):
                    self.send_header("Transfer-Encoding", "chunked")
                elif self.path == "/eof":
                    self.send_header("Connection", "close")
                else:
                    self.send_header("Content-Length", "100" if self.path == "/truncated" else str(len(payload)))
                self.end_headers()
                if self.path.startswith("/chunked"):
                    self.wfile.write(f"{len(payload):x}\r\n".encode() + payload + b"\r\n")
                    if self.path == "/chunked":
                        self.wfile.write(b"0\r\n\r\n")
                else:
                    self.wfile.write(payload)
                if self.path in ("/truncated", "/chunked-truncated", "/eof"):
                    self.close_connection = True
        self.server = native.http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.server.observed = []
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.driver = native.Driver.__new__(native.Driver)
        self.driver.connection = native.http.client.HTTPConnection("127.0.0.1", self.server.server_port, timeout=2)

    def tearDown(self):
        self.driver.connection.close()
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=2)

    def test_explicit_keepalive_reuses_connection_instead_of_urllib_close(self):
        url = f"http://127.0.0.1:{self.server.server_port}/old-urllib"
        with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(url, timeout=2) as response:
            response.read()
        self.assertTrue(self.driver.call("GET", "/first"))
        self.assertTrue(self.driver.call("POST", "/second", {"synthetic": True}))
        first, second, third = self.server.observed
        self.assertEqual(first[1], "close")
        self.assertEqual((second[1], third[1]), ("keep-alive", "keep-alive"))
        self.assertEqual(second[2], third[2], "Commands should share one HTTP connection")

    def test_uncertain_mutation_is_sent_exactly_once(self):
        with self.assertRaises(urllib.error.URLError):
            self.driver.call("POST", "/mutation-drop", {"click": "synthetic"})
        self.assertEqual(len(self.server.observed), 1)
        self.assertEqual(self.server.observed[0][0], "/mutation-drop")
        self.assertIsNone(self.driver.connection.sock)

    def test_truncated_valid_json_cannot_count_as_command_success(self):
        with self.assertRaises(urllib.error.URLError):
            self.driver.call("POST", "/truncated", {"click": "synthetic"})
        self.assertEqual(len(self.server.observed), 1)
        self.assertIsNone(self.driver.connection.sock)

    def test_complete_chunked_and_close_delimited_json_are_accepted(self):
        self.assertTrue(self.driver.call("GET", "/chunked"))
        self.assertTrue(self.driver.call("GET", "/eof"))

    def test_missing_chunked_terminator_rejects_valid_json_prefix(self):
        with self.assertRaises(urllib.error.URLError):
            self.driver.call("POST", "/chunked-truncated", {"click": "synthetic"})
        self.assertEqual(len(self.server.observed), 1)
        self.assertIsNone(self.driver.connection.sock)


class NativeHelperTests(unittest.TestCase):
    def setUp(self):
        self.server = native.http.server.ThreadingHTTPServer(("127.0.0.1", 0), native.ModelFixture)
        self.server.requests, self.server.test_requests, self.server.invalid_requests = [], [], 0
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

    def test_minimal_settings_probe_is_separate_and_has_no_history(self):
        body = {"model": "native-fixture-model", "stream": True,
                "messages": [{"role": "user", "content": "Reply with OK."}]}
        self.assertIn('"content": "OK"', self.post(body))
        self.assertEqual(len(self.server.test_requests), 1)
        self.assertEqual(self.server.requests, [])
        body["messages"].insert(0, {"role": "system", "content": "unexpected history"})
        with self.assertRaises(urllib.error.HTTPError):
            self.post(body)

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

    def test_upgrade_compares_old_history_rows_not_database_file_hash(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            with sqlite3.connect(root / "history.sqlite3") as connection:
                connection.execute("CREATE TABLE chats (chat_id TEXT,task_id TEXT,revision INTEGER,listed INTEGER,updated_ms INTEGER,payload TEXT)")
                connection.execute("INSERT INTO chats VALUES ('old-chat','old-task',3,1,12,'synthetic')")
                connection.execute("PRAGMA user_version=1")
            _, rows = native.history_rows(root)
            with sqlite3.connect(root / "history.sqlite3") as connection:
                connection.execute("PRAGMA user_version=2")
                connection.execute("INSERT INTO chats VALUES ('new-empty-chat','new-task',4,1,13,'empty')")
            native.verify_history_upgrade(root, rows)
            with sqlite3.connect(root / "history.sqlite3") as connection:
                connection.execute("UPDATE chats SET payload='changed' WHERE chat_id='old-chat'")
            with self.assertRaisesRegex(AssertionError, "preexisting history row"):
                native.verify_history_upgrade(root, rows)

    def test_v012_history_upgrade_rejects_schema_change_missing_or_empty_rows(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            with sqlite3.connect(root / "history.sqlite3") as connection:
                connection.execute("CREATE TABLE chats (chat_id TEXT,task_id TEXT,revision INTEGER,listed INTEGER,updated_ms INTEGER,payload TEXT)")
                connection.execute("INSERT INTO chats VALUES ('old-chat','old-task',3,1,12,'synthetic')")
                connection.execute("PRAGMA user_version=2")
            version, rows = native.history_rows(root)
            self.assertEqual(version, 2)
            native.verify_history_upgrade(root, rows)
            with self.assertRaisesRegex(AssertionError, "real baseline history"):
                native.verify_history_upgrade(root, [])
            with sqlite3.connect(root / "history.sqlite3") as connection:
                connection.execute("PRAGMA user_version=3")
            with self.assertRaisesRegex(AssertionError, "history format marker"):
                native.verify_history_upgrade(root, rows)
            with sqlite3.connect(root / "history.sqlite3") as connection:
                connection.execute("PRAGMA user_version=2")
                connection.execute("DELETE FROM chats")
            with self.assertRaisesRegex(AssertionError, "preexisting history row"):
                native.verify_history_upgrade(root, rows)

    @unittest.skipIf(native.os.name == "nt", "Unix permission fixture")
    def test_current_profile_upgrade_requires_v2_exact_bytes_and_private_permissions(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            profile = root / "connections.toml"
            original = b'schema_version = 2\n# real synthetic baseline profile\n'
            profile.write_bytes(original)
            profile.chmod(0o600)
            expected = {"profile_schema_version": 2, "profile_file_sha256": native.hashlib.sha256(original).hexdigest()}
            native.verify_current_profile_upgrade(root, expected)
            with self.assertRaisesRegex(AssertionError, "real v0.1.2 profiles"):
                native.verify_current_profile_upgrade(root, dict(expected, profile_schema_version=1))
            profile.chmod(0o644)
            with self.assertRaisesRegex(AssertionError, "owner-only"):
                native.verify_current_profile_upgrade(root, expected)
            profile.chmod(0o600)
            profile.write_bytes(original + b"# altered\n")
            with self.assertRaisesRegex(AssertionError, "baseline bytes"):
                native.verify_current_profile_upgrade(root, expected)
            profile.write_bytes(original)
            backup = root / "connections.v1.backup.toml"
            backup.write_bytes(original)
            with self.assertRaisesRegex(AssertionError, "legacy backup"):
                native.verify_current_profile_upgrade(root, expected)
            backup.unlink()
            profile.rename(backup)
            profile.symlink_to(backup)
            with self.assertRaisesRegex(AssertionError, "protected regular"):
                native.verify_current_profile_upgrade(root, expected)

    def test_windows_current_profile_checks_actual_file_acl_and_propagates_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            profile = root / "connections.toml"
            profile.write_bytes(b"schema_version = 2\n")
            expected = {"profile_schema_version": 2, "profile_file_sha256": native.hashlib.sha256(profile.read_bytes()).hexdigest()}
            with mock.patch.object(native.os, "name", "nt"), mock.patch.object(native.subprocess, "run") as run:
                native.verify_current_profile_upgrade(root, expected)
                args, kwargs = run.call_args
                self.assertIn(str(profile), args[0][-1])
                self.assertIn("$owner -ne $sid", args[0][-1])
                self.assertIn("AreAccessRulesProtected", args[0][-1])
                self.assertTrue(kwargs["check"])
                run.side_effect = native.subprocess.CalledProcessError(1, "pwsh")
                with self.assertRaises(native.subprocess.CalledProcessError):
                    native.verify_current_profile_upgrade(root, expected)

    @unittest.skipIf(native.os.name == "nt", "Unix permission fixture")
    def test_profile_backup_requires_exact_bytes_and_owner_only_mode(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            backup = root / "connections.v1.backup.toml"
            backup.write_bytes(b"synthetic-v1-profile")
            backup.chmod(0o600)
            digest = native.hashlib.sha256(backup.read_bytes()).hexdigest()
            native.verify_profile_backup(root, digest)
            backup.chmod(0o644)
            with self.assertRaisesRegex(AssertionError, "owner-only"):
                native.verify_profile_backup(root, digest)
            backup.chmod(0o600)
            backup.write_bytes(b"changed")
            with self.assertRaisesRegex(AssertionError, "baseline bytes"):
                native.verify_profile_backup(root, digest)

    def test_windows_backup_acl_uses_fixture_powershell_and_fails_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            backup = root / "connections.v1.backup.toml"
            backup.write_bytes(b"synthetic-v1-profile")
            digest = native.hashlib.sha256(backup.read_bytes()).hexdigest()
            with mock.patch.object(native.os, "name", "nt"), mock.patch.object(native.subprocess, "run") as run:
                native.verify_profile_backup(root, digest)
                args, kwargs = run.call_args
                self.assertEqual(args[0][0], "pwsh")
                self.assertTrue(kwargs["check"])
                self.assertEqual(kwargs["timeout"], 15)
                self.assertIn("Get-Acl", args[0][-1])
                self.assertIn("AreAccessRulesProtected", args[0][-1])
                self.assertIn("$unexpected.Count -ne 0", args[0][-1])
                run.side_effect = native.subprocess.CalledProcessError(1, "pwsh")
                with self.assertRaises(native.subprocess.CalledProcessError):
                    native.verify_profile_backup(root, digest)

    def test_wait_is_bounded_and_retries_transient_errors(self):
        values = iter([False, True])
        self.assertTrue(native.wait_for(lambda: next(values), timeout=1))
        with self.assertRaises(AssertionError):
            native.wait_for(lambda: False, timeout=0.01)


if __name__ == "__main__":
    unittest.main()
