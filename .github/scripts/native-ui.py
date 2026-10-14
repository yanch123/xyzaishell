#!/usr/bin/env python3
"""Installed Tauri UI/IPC smoke. Only the loopback HTTP model is synthetic.

No IPC mock, injected command, production test endpoint, or app code change is
used. All mutations go through real WebDriver clicks/keystrokes. Run only with a
fresh fixture HOME/AppData supplied by the installer smoke scripts.
"""
import argparse
import base64
import contextlib
import hashlib
import http.client
import http.server
import json
import os
import pathlib
import re
import signal
import socket
import sqlite3
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request

ELEMENT = "element-6066-11e4-a52e-4f735466cecf"
PROMPT = "Native fixture history message 中文"
REPLY = "Native fixture streamed reply 中文 <script>window.unsafe=true</script>"
PROFILE = "Native fixture profile 中文"
TOKEN = "synthetic-native-fixture-token"


def wait_for(read, timeout=30):
    until = time.monotonic() + timeout
    last = None
    while time.monotonic() < until:
        try:
            value = read()
            if value:
                return value
        except (AssertionError, RuntimeError, urllib.error.URLError) as error:
            last = error
        time.sleep(0.2)
    raise AssertionError(f"Timed out waiting for native UI: {last}")


def free_port():
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return listener.getsockname()[1]


def application_pids(application):
    application = str(application.resolve())
    if os.name == "nt":
        quoted = application.replace("'", "''")
        script = ("$ErrorActionPreference='Stop'; @(Get-CimInstance Win32_Process -Filter \"Name='xyzaishell.exe'\" | "
                  f"Where-Object {{ $_.ExecutablePath -eq '{quoted}' }} | "
                  "Select-Object -ExpandProperty ProcessId) | ConvertTo-Json -Compress")
        result = subprocess.check_output(["pwsh", "-NoProfile", "-NonInteractive", "-Command", script],
                                         text=True, timeout=15).strip()
        pids = json.loads(result) if result else []
        return pids if isinstance(pids, list) else [pids]
    pids = []
    for entry in pathlib.Path("/proc").iterdir():
        if not entry.name.isdecimal():
            continue
        try:
            if os.readlink(entry / "exe").removesuffix(" (deleted)") == application:
                pids.append(int(entry.name))
        except (FileNotFoundError, PermissionError, ProcessLookupError):
            pass
    return pids


def protect_windows_fixture_file(path):
    # Only called for the newly created synthetic model config inside the
    # marker-validated fixture. Administrator tokens may otherwise create files
    # owned by Administrators; inherited ACLs are also not explicit protection.
    sid = subprocess.check_output(["pwsh", "-NoProfile", "-NonInteractive", "-Command",
                                   "$ErrorActionPreference='Stop'; [Security.Principal.WindowsIdentity]::GetCurrent().User.Value"],
                                  text=True, timeout=15).strip()
    if not re.fullmatch(r"S-1-(?:[0-9]+-)+[0-9]+", sid):
        raise RuntimeError("Could not identify the synthetic fixture owner")
    subprocess.run(["icacls", str(path), "/inheritance:r", "/grant:r", f"*{sid}:F"], check=True)
    subprocess.run(["icacls", str(path), "/setowner", f"*{sid}"], check=True)


class ModelFixture(http.server.BaseHTTPRequestHandler):
    """Bound only to loopback; reject unexpected requests instead of relaying."""

    def log_message(self, *_args):
        pass

    def do_POST(self):
        size = int(self.headers.get("Content-Length", "0"))
        if not 0 < size <= 65536:
            self.send_error(400)
            return
        body = json.loads(self.rfile.read(size))
        self.server.requests.append(body)
        valid = (self.path == "/v1/chat/completions"
                 and self.headers.get("Authorization") == f"Bearer {TOKEN}"
                 and body.get("model") == "native-fixture-model"
                 and body.get("stream") is True
                 and not body.get("tools")
                 and any(m.get("role") == "user" and m.get("content") == PROMPT
                         for m in body.get("messages", [])))
        if not valid:
            self.server.invalid_requests += 1
            self.send_error(400)
            return
        chunks = [
            {"choices": [{"index": 0, "delta": {"role": "assistant", "content": REPLY}, "finish_reason": None}]},
            {"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]},
        ]
        stream = "".join("data: " + json.dumps(chunk) + "\n\n" for chunk in chunks)
        data = (stream + "data: [DONE]\n\n").encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


class WebDriverError(RuntimeError):
    def __init__(self, body):
        super().__init__(body)
        try:
            value = json.loads(body)
            self.code = value.get("value", value).get("error")
        except (ValueError, AttributeError):
            self.code = None


class NativeTransportFailure(Exception):
    """Terminal inspection failure; generic readiness waits must not retry it."""


class Driver:
    def __init__(self, application, evidence):
        self.application = application.resolve()
        if application_pids(self.application):
            raise RuntimeError("A prior application process survived; refusing an ambiguous restart")
        self.evidence = evidence
        self.session = None
        self.port, native_port = free_port(), free_port()
        while native_port == self.port:
            native_port = free_port()
        self.base = f"http://127.0.0.1:{self.port}"
        self.connection = http.client.HTTPConnection("127.0.0.1", self.port, timeout=45)
        self.log = (evidence / "driver.log").open("wb")
        command = [os.environ["TAURI_DRIVER"], "--port", str(self.port),
                   "--native-port", str(native_port)]
        if os.environ.get("NATIVE_DRIVER"):
            command += ["--native-driver", os.environ["NATIVE_DRIVER"]]
        self.process = subprocess.Popen(command, stdout=self.log, stderr=subprocess.STDOUT)
        try:
            wait_for(lambda: self.call("GET", "/status").get("ready"), timeout=30)
            response = self.call("POST", "/session", {"capabilities": {"alwaysMatch": {
                "browserName": "wry", "tauri:options": {"application": str(application)},
            }}})
            self.session = response["sessionId"]
            self.application_processes = frozenset(wait_for(lambda: application_pids(self.application)))
            self.command("POST", "/timeouts", {"implicit": 0, "script": 10000, "pageLoad": 30000})
            self.command("POST", "/window/rect", {"width": 1400, "height": 900})
            wait_for(lambda: self.find("//textarea[@aria-label='聊天消息']"))
            assert self.js("return typeof window.__TAURI_INTERNALS__?.invoke === 'function'")
            assert not self.js("return Boolean(window.__SYNTHETIC_IPC_CALLS__)")
        except BaseException:
            self.close()
            raise

    def call(self, method, suffix, value=None):
        payload = None if value is None else json.dumps(value).encode()
        try:
            # urllib injects Connection: close. tauri-driver forwards that
            # hop-by-hop header into its pooled native-driver connection. Keep
            # this sequential HTTP/1.1 loopback connection alive instead.
            # HTTPConnection sends exactly once; never replay a command here.
            self.connection.request(method, suffix, body=payload,
                                    headers={"Content-Type": "application/json", "Connection": "keep-alive"})
            response = self.connection.getresponse()
            limit = 16 * 1024 * 1024
            if response.length is not None and response.length > limit:
                self.connection.close()
                raise NativeTransportFailure("Native driver response exceeded the fixture bound")
            if response.length is not None:
                # Unbounded read of an already bounded Content-Length uses
                # HTTPResponse's exact-length framing check (IncompleteRead).
                body = response.read()
            else:
                body = response.read(limit + 1)
                if len(body) > limit:
                    self.connection.close()
                    raise NativeTransportFailure("Native driver response exceeded the fixture bound")
                # Require chunked termination or actual EOF, not merely a short
                # read whose prefix happened to be syntactically valid JSON.
                if response.read(1) or not response.isclosed():
                    self.connection.close()
                    raise NativeTransportFailure("Native driver response framing was incomplete")
            if not 200 <= response.status < 300:
                raise WebDriverError(body.decode(errors="replace"))
            result = json.loads(body)
        except (OSError, http.client.HTTPException) as error:
            self.connection.close()
            # Read-only js() may retry under its exact-instance bound. Every
            # mutating caller propagates this uncertain transport failure.
            raise urllib.error.URLError(error) from error
        result = result.get("value", result)
        if isinstance(result, dict) and result.get("error"):
            raise WebDriverError(json.dumps(result))
        return result

    def command(self, method, suffix, value=None):
        return self.call(method, f"/session/{self.session}" + suffix, value)

    def find(self, xpath):
        return self.command("POST", "/element", {"using": "xpath", "value": xpath})[ELEMENT]

    def button(self, name):
        # Test labels are static source-controlled strings, never user input.
        return f"//button[@aria-label='{name}' or normalize-space(.)='{name}']"

    def click(self, xpath):
        element = wait_for(lambda: self.find(xpath))
        wait_for(lambda: self.command("GET", f"/element/{element}/enabled"))
        try:
            self.command("POST", f"/element/{element}/click", {})
        except WebDriverError as error:
            if error.code != "element not interactable":
                raise
            # WebKit may leave a newly shifted modal footer below its scroll
            # viewport. Real Tab navigation scrolls the focused control into
            # view. Never mutate CSS/DOM, invoke JS clicks, or replay a request
            # whose dispatch is ambiguous.
            for _ in range(40):
                self.command("POST", "/actions", {"actions": [{"type": "key", "id": "keyboard", "actions": [
                    {"type": "keyDown", "value": "\ue004"}, {"type": "keyUp", "value": "\ue004"}]}]})
                if self.command("GET", "/element/active")[ELEMENT] == element:
                    self.command("POST", f"/element/{element}/click", {})
                    return
            raise RuntimeError("Native keyboard navigation could not reach the control") from error

    def fill(self, label, text):
        element = wait_for(lambda: self.find(f"//label[starts-with(normalize-space(.),'{label}')]/input"))
        self.command("POST", f"/element/{element}/clear", {})
        self.command("POST", f"/element/{element}/value", {"text": text, "value": list(text)})

    def js(self, script):
        # Read-only DOM inspection; never mocks invoke or calls backend directly.
        for attempt in range(3):
            try:
                return self.command("POST", "/execute/sync", {"script": script, "args": []})
            except (http.client.RemoteDisconnected, ConnectionResetError, urllib.error.URLError) as error:
                # WebKit can reset a startup transport connection. Retry only
                # this read-only inspection, in the same live app/session; no
                # click, keystroke, consent, network request or app relaunch.
                if (attempt == 2 or self.process.poll() is not None
                        or frozenset(application_pids(self.application)) != self.application_processes):
                    raise NativeTransportFailure("Read-only transport retry exhausted or original process exited") from error
                with (self.evidence / "read-only-transport-retries.jsonl").open("a", encoding="utf-8") as output:
                    output.write(json.dumps({"attempt": attempt + 1, "error": type(error).__name__}) + "\n")
                time.sleep(0.25)

    def screenshot(self, name):
        (self.evidence / f"{name}.png").write_bytes(base64.b64decode(self.command("GET", "/screenshot")))

    def close(self):
        failure = None
        if self.session:
            try:
                # Close the real window first. Deleting a WebDriver session can
                # merely detach on WebKit; terminating a driver may kill its
                # Windows job. Neither proves ordinary application shutdown.
                self.command("DELETE", "/window")
                wait_for(lambda: not application_pids(self.application), timeout=10)
            except Exception as error:
                failure = error
            try:
                self.command("DELETE", "")
            except WebDriverError as error:
                # W3C closes the session when its final window closes. Accept
                # only that precise protocol result after verified app exit.
                if error.code != "invalid session id" or application_pids(self.application):
                    failure = error
            except Exception as error:
                failure = error
            self.session = None
        if self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=10)
        self.log.close()
        self.connection.close()
        try:
            wait_for(lambda: not application_pids(self.application), timeout=10)
        except AssertionError as error:
            # These paths are exclusive to this disposable test; initial launch
            # refuses a preexisting instance. Cleanup is not a successful exit.
            for pid in application_pids(self.application):
                with contextlib.suppress(ProcessLookupError):
                    os.kill(pid, signal.SIGTERM)
            failure = error
        if failure:
            raise RuntimeError("Native application/session shutdown was not clean") from failure


def open_profiles(driver):
    driver.click(driver.button("新建 SSH 连接"))
    driver.click(driver.button("管理已保存的 SSH 档案"))
    wait_for(lambda: driver.find("//select[@aria-label='选择档案']"))


def close_profiles(driver):
    driver.click(driver.button("返回"))


def enable_history(driver):
    driver.click(driver.button("打开设置"))
    button = wait_for(lambda: driver.find(driver.button("启用明文聊天历史")))
    assert driver.command("GET", f"/element/{button}/enabled") is False
    checkbox = driver.find("//label[contains(.,'我理解聊天将以明文保存到本机')]/input")
    assert driver.command("GET", f"/element/{checkbox}/selected") is False
    driver.click("//label[contains(.,'我理解聊天将以明文保存到本机')]/input")
    driver.click(driver.button("启用明文聊天历史"))
    wait_for(lambda: driver.find(driver.button("停止保存聊天历史")))


def assert_literal_reply(driver):
    expected = json.dumps(REPLY)
    wait_for(lambda: driver.js(
        "return document.querySelector('article[aria-label=\"模型回复\"] p')?.textContent === " + expected))
    assert driver.js("return document.querySelectorAll('article[aria-label=\"模型回复\"] script').length === 0")
    assert driver.js("return window.unsafe === undefined")


def seed(driver, server):
    open_profiles(driver)
    for label, text in [("档案名称", PROFILE), ("档案主机", "127.0.0.1"), ("档案用户名", "synthetic-user")]:
        driver.fill(label, text)
    driver.click(driver.button("保存档案"))
    wait_for(lambda: driver.js("return document.querySelector('select[aria-label=\"选择档案\"]').value"))
    driver.screenshot("saved-profile")
    close_profiles(driver)
    enable_history(driver)
    driver.click(driver.button("关闭设置"))
    element = driver.find("//textarea[@aria-label='聊天消息']")
    driver.command("POST", f"/element/{element}/value", {"text": PROMPT, "value": list(PROMPT)})
    driver.click(driver.button("确认并发送"))
    button = wait_for(lambda: driver.find(driver.button("同意并发送")))
    assert driver.command("GET", f"/element/{button}/enabled") is False
    # Give an accidental async pre-consent dispatch time to reach loopback.
    time.sleep(1)
    assert server.requests == []
    driver.screenshot("actual-ipc-recipient-consent")
    driver.click("//label[contains(.,'我确认此地址和模型')]/input")
    driver.click(driver.button("同意并发送"))
    assert_literal_reply(driver)
    wait_for(lambda: driver.js("return document.body.textContent.includes('无生成中的聊天')"))
    assert len(server.requests) == 1 and server.invalid_requests == 0
    driver.screenshot("actual-ipc-streamed-chat")


def verify_preserved(driver, server):
    open_profiles(driver)
    driver.click(f"//select[@aria-label='选择档案']/option[starts-with(normalize-space(.),'{PROFILE}')]")
    wait_for(lambda: driver.js("return [...document.querySelectorAll('label')].find(e=>e.textContent.startsWith('档案用户名'))?.querySelector('input')?.value === 'synthetic-user'"))
    assert driver.js("return [...document.querySelectorAll('input[type=password]')].every(e=>e.value==='')")
    close_profiles(driver)
    enable_history(driver)
    # The saved title remains '新任务'; it is not derived from message text.
    # This isolated fixture has exactly one two-message conversation. Require
    # uniqueness, then verify exact reply and persisted task/chat identity below.
    restore = "//div[@aria-label='本机保存的聊天']/div[span[contains(.,' · 2 条消息')]]/button[normalize-space(.)='仅恢复文字']"
    wait_for(lambda: len(driver.command("POST", "/elements", {"using": "xpath", "value": restore})) == 1)
    driver.click(restore)
    assert_literal_reply(driver)
    assert driver.js("return document.body.textContent.includes('未连接服务器')")
    time.sleep(1)
    assert server.requests == [], "Restore/restart unexpectedly started a model request"
    driver.screenshot("restored-text-no-execution")


def stored_identity(history):
    database = history / "history.sqlite3"
    with sqlite3.connect(database.as_uri() + "?mode=ro", uri=True) as connection:
        rows = connection.execute("SELECT chat_id,task_id,payload FROM chats").fetchall()
    matches = [(chat_id, task_id) for chat_id, task_id, payload in rows if PROMPT in payload and REPLY in payload]
    assert len(matches) == 1, "Expected one persisted synthetic conversation"
    assert TOKEN.encode() not in database.read_bytes(), "Synthetic API token leaked into persisted history"
    return matches[0]


def verify_empty_shell_log(driver, history, server):
    # This native smoke has no SSH connection and must never invent a shell
    # event or create a database just to make export succeed.
    assert not list(history.glob("shell-log*")), "Unexpected shell-log data before consent"
    requests_before = len(server.requests)
    driver.click(driver.button("打开终端日志"))
    wait_for(lambda: driver.js("return document.querySelector('.shell-log-dialog')?.textContent.includes('当前筛选下暂无日志。')"))
    wait_for(lambda: driver.js("return document.querySelector('.shell-log-dialog')?.textContent.includes('记录状态：未启用')"))
    assert driver.js("return document.querySelector('.shell-log-dialog').textContent.includes('不保存终端输出，不自动发送给模型')")
    checks = driver.command("POST", "/elements", {"using": "xpath", "value": "//div[contains(@class,'shell-log-consent')]//input[@type='checkbox']"})
    assert len(checks) == 2
    for checkbox in checks:
        assert driver.command("GET", f"/element/{checkbox[ELEMENT]}/selected") is False
        assert driver.command("GET", f"/element/{checkbox[ELEMENT]}/enabled") is False
    start = driver.find(driver.button("启动终端日志"))
    assert driver.command("GET", f"/element/{start}/enabled") is False
    driver.screenshot("shell-log-disabled-empty")
    driver.click(driver.button("关闭对话框"))
    driver.click(driver.button("打开终端日志"))
    for format_name in ("JSON", "CSV"):
        driver.click(driver.button("筛选 / 刷新"))
        wait_for(lambda: driver.js("return document.querySelector('.shell-log-entries')?.getAttribute('aria-busy') === 'false' && !document.querySelector('.shell-log-dialog [role=alert]')"))
        driver.click(driver.button("导出 " + format_name))
        wait_for(lambda: driver.js("return [...document.querySelectorAll('.shell-log-dialog [role=alert]')].some(e=>e.textContent === 'No shell log database exists to export.')"))
        assert not list(history.glob("shell-log*")), "Empty export created a database, lock or export file"
        assert driver.js("return !document.querySelector('input[aria-label=\"导出文件完整路径\"]')")
    driver.screenshot("shell-log-empty-export-rejected")
    driver.click(driver.button("关闭对话框"))
    assert len(server.requests) == requests_before, "Shell-log UI unexpectedly contacted a model"


def verify_phase_version(phase, version, application_hash, expected):
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version):
        raise ValueError("Exact installed application version required")
    if phase == "seed":
        if expected is not None:
            raise ValueError("Seed must use fresh fixture state")
        return
    if not expected or not expected.get("application_sha256"):
        raise ValueError("Missing original application identity")
    if phase == "upgrade":
        if expected.get("application_version") != "0.1.0" or version != "0.1.1":
            raise ValueError("Only the reviewed 0.1.0 to 0.1.1 upgrade is covered")
        if expected["application_sha256"] == application_hash:
            raise ValueError("Upgrade did not replace the old executable")
    elif phase == "preserved":
        if expected.get("application_version") != version or expected["application_sha256"] != application_hash:
            raise ValueError("Same-version lifecycle changed the executable identity")
    else:
        raise ValueError("Unknown native acceptance phase")


def main():
    if sys.flags.optimize:
        raise RuntimeError("Native acceptance assertions must not be disabled with Python optimization")
    parser = argparse.ArgumentParser()
    parser.add_argument("--application", required=True, type=pathlib.Path)
    parser.add_argument("--fixture", required=True, type=pathlib.Path)
    parser.add_argument("--evidence", required=True, type=pathlib.Path)
    parser.add_argument("--phase", choices=["seed", "preserved", "upgrade"], required=True)
    parser.add_argument("--application-version", required=True)
    args = parser.parse_args()
    root = args.fixture.resolve(strict=True)
    assert (root / ".xyz-native-fixture").read_text() == "synthetic disposable fixture\n"
    config_parent = pathlib.Path(os.environ["APPDATA"] if os.name == "nt" else os.environ["XDG_CONFIG_HOME"]).resolve()
    assert config_parent.is_relative_to(root), "Refusing a non-fixture configuration path"
    config = config_parent / "xyzaishell"
    config.mkdir(mode=0o700, parents=True, exist_ok=True)
    history_parent = pathlib.Path(os.environ["LOCALAPPDATA"] if os.name == "nt" else os.environ["XDG_STATE_HOME"]).resolve()
    assert history_parent.is_relative_to(root), "Refusing a non-fixture history path"
    history = history_parent / "xyzaishell"
    if os.name == "nt":
        history = history / "state"
    args.evidence.mkdir(parents=True, exist_ok=True)
    state_file = root / "expected.json"
    expected = json.loads(state_file.read_text()) if args.phase != "seed" else None
    application_hash = hashlib.sha256(args.application.read_bytes()).hexdigest()
    verify_phase_version(args.phase, args.application_version, application_hash, expected)
    server = http.server.ThreadingHTTPServer(("127.0.0.1", expected["port"] if expected else 0), ModelFixture)
    server.requests, server.invalid_requests = [], 0
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    if expected is None:
        text = f'''schema_version = 1
[model]
protocol = "openai_chat_completions"
base_url = "http://127.0.0.1:{server.server_port}/v1"
api_key = "{TOKEN}"
name = "native-fixture-model"
[capabilities]
streaming = "required"
tool_calling = "required"
[request]
connect_timeout_seconds = 5
request_timeout_seconds = 30
stream_idle_timeout_seconds = 10
max_output_tokens = 128
[transport]
allow_loopback_http = true
'''
        with (config / "model.toml").open("x", encoding="utf-8") as output:
            output.write(text)
        if os.name != "nt":
            (config / "model.toml").chmod(0o600)
        else:
            protect_windows_fixture_file(config / "model.toml")
    try:
        # First phase includes a real process restart. Preserved phase is called
        # after installer reinstallation and again after uninstall/reinstall.
        phases = ["seed", "restart"] if expected is None else [args.phase]
        identity = expected["identity"] if expected else None
        for phase in phases:
            directory = args.evidence / phase
            directory.mkdir(exist_ok=True)
            driver = Driver(args.application.resolve(strict=True), directory)
            try:
                if phase == "seed":
                    seed(driver, server)
                else:
                    verify_preserved(driver, server)
                if args.application_version != "0.1.0":
                    verify_empty_shell_log(driver, history, server)
                visible_task_identity = driver.js("return document.querySelector('.chat-status span[title]')?.getAttribute('title')")
            except BaseException:
                with contextlib.suppress(Exception):
                    driver.screenshot("failure")
                    (directory / "failure.html").write_text(driver.command("GET", "/source"), encoding="utf-8")
                raise
            finally:
                driver.close()
            assert len(server.requests) == (1 if phase == "seed" else 0), "Unexpected model request before process exit"
            assert server.invalid_requests == 0
            actual = list(stored_identity(history))
            assert visible_task_identity == f"任务 ID: {actual[1]}", "Visible native task identity differs from persisted history"
            if identity is None:
                identity = actual
            assert actual == identity, "Restoration changed the stored task/chat identity"
            server.requests.clear()
        if expected is None or args.phase == "upgrade":
            state_file.write_text(json.dumps({"port": server.server_port, "identity": identity,
                "application_version": args.application_version, "application_sha256": application_hash}), encoding="utf-8")
        (args.evidence / "result.json").write_text(json.dumps({
            "result": "passed", "phase": args.phase, "application": str(args.application),
            "application_sha256": application_hash, "application_version": args.application_version,
            "upgrade_from": expected["application_version"] if args.phase == "upgrade" else None,
            "native_ipc": True, "synthetic_loopback_model": True,
            "profile_and_history_persisted": True, "restored_task_identity_preserved": True,
            "history_requires_new_process_consent": True, "restore_model_requests": 0,
            "shell_log_empty_export_fail_closed": args.application_version != "0.1.0",
            "not_tested": ["real provider", "SSH via UI", "desktop input methods", "Windows 10/11"]
                + ([] if args.phase == "upgrade" else ["version upgrade in this phase"]),
        }, indent=2), encoding="utf-8")
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
    print(f"PASS: installed native UI/IPC {args.phase}; profile/history and consent checks")


if __name__ == "__main__":
    main()
