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

from upgrade_baseline import BASE_VERSION, TARGET_VERSION

ELEMENT = "element-6066-11e4-a52e-4f735466cecf"
PROMPT = "Native fixture history message 中文"
REPLY = "Native fixture streamed reply 中文 <script>window.unsafe=true</script> " + ("Synthetic retained history detail. " * 120).rstrip()
FOLLOWUP = "Native fixture follow-up: retain the previous answer 中文"
FOLLOWUP_REPLY = "Native fixture contextual follow-up reply 中文 " + ("Additional synthetic history detail. " * 120).rstrip()
PROFILE = "Native fixture profile 中文"
TOKEN = "synthetic-native-fixture-token"


def fixture_output_reserve(version, expected=None):
    # Fresh candidate compaction needs room for schema + source coverage. Upgrade
    # must retain the released fixture's exact model settings, including 128.
    if expected is not None:
        return expected.get("max_output_tokens", 128)
    return 4096 if version == TARGET_VERSION else 128


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
        messages = body.get("messages", [])
        compaction = any(m.get("role") == "system" and "You summarize historical data, with no tools." in str(m.get("content")) for m in messages)
        if compaction:
            self.compaction(body)
            return
        probe = body.get("messages") == [{"role": "user", "content": "Reply with OK."}]
        (self.server.test_requests if probe else self.server.requests).append(body)
        valid = (self.path == "/v1/chat/completions"
                 and self.headers.get("Authorization") == f"Bearer {TOKEN}"
                 and body.get("model") == "native-fixture-model"
                 and body.get("stream") is True
                 and not body.get("tools")
                 and (probe or any(m.get("role") == "user" and m.get("content") in (PROMPT, FOLLOWUP)
                                   for m in body.get("messages", []))))
        if not valid:
            self.server.invalid_requests += 1
            self.send_error(400)
            return
        followup = body.get("messages", [])[-1].get("content") == FOLLOWUP
        if followup and not any(m.get("role") == "assistant" and m.get("content") in (REPLY, FOLLOWUP_REPLY) for m in body["messages"]):
            self.server.invalid_requests += 1
            self.send_error(400)
            return
        chunks = [
            {"choices": [{"index": 0, "delta": {"role": "assistant", "content": "OK" if probe else FOLLOWUP_REPLY if followup else REPLY}, "finish_reason": None}]},
            {"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]},
        ]
        stream = "".join("data: " + json.dumps(chunk) + "\n\n" for chunk in chunks)
        data = (stream + "data: [DONE]\n\n").encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def compaction(self, body):
        """Synthetic provider only: accept exact inert source-manifest requests."""
        try:
            payload = json.loads(body["messages"][-1]["content"])
            ids = payload["source_manifest"]
            assert isinstance(ids, list) and ids and all(isinstance(value, str) for value in ids)
            assert len(ids) == len(set(ids))
            assert "sources" in payload or "partial_summaries" in payload
            assert self.path == "/v1/chat/completions" and self.headers.get("Authorization") == f"Bearer {TOKEN}"
            assert body.get("model") == "native-fixture-model" and body.get("stream") is True and not body.get("tools")
        except (AssertionError, ValueError, KeyError, TypeError):
            self.server.invalid_requests += 1
            self.send_error(400)
            return
        self.server.compaction_requests.append(body)
        self.server.compaction_started.set()
        if self.server.hold_compaction:
            if not self.server.release_compaction.wait(timeout=25):
                self.send_error(504)
                return
        summary = {"goal": "Preserve the synthetic conversation", "user_constraints": [], "findings": [],
                   "hypotheses": [], "remaining_steps": [], "uncertainties": ["Synthetic fixture only; source details and prior partial evidence may be omitted"],
                   "coverage": {"source_ids": ids, "omitted_ranges": ids}, "summary_text": "Earlier synthetic history was summarized; no command was executed."}
        chunks = [{"choices": [{"index": 0, "delta": {"role": "assistant", "content": json.dumps(summary)}, "finish_reason": None}]},
                  {"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]}]
        data = ("".join("data: " + json.dumps(chunk) + "\n\n" for chunk in chunks) + "data: [DONE]\n\n").encode()
        with contextlib.suppress(BrokenPipeError, ConnectionResetError):
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


def open_profiles(driver, version):
    # Both reviewed versions use the v0.1.2 connection manager. Baseline means
    # the actual released UI, not the older v0.1.1 fixture layout.
    driver.click(driver.button("打开连接管理"))
    wait_for(lambda: driver.find("//select[@aria-label='选择档案']"))


def close_profiles(driver, version):
    driver.click(driver.button("关闭"))


def enable_history(driver, version):
    driver.click(driver.button("打开设置"))
    driver.click(driver.button("历史记录"))
    button = wait_for(lambda: driver.find(driver.button("启用明文聊天历史")))
    assert driver.command("GET", f"/element/{button}/enabled") is False
    checkbox = driver.find("//label[contains(.,'我理解聊天将以明文保存到本机')]/input")
    assert driver.command("GET", f"/element/{checkbox}/selected") is False
    driver.click("//label[contains(.,'我理解聊天将以明文保存到本机')]/input")
    driver.click(driver.button("启用明文聊天历史"))
    wait_for(lambda: driver.find(driver.button("停止保存聊天历史")))


SETTINGS_BASIC_FIELDS_SCRIPT = """
const visible = element => {
  if (!element.getClientRects().length) return false;
  for (let node = element; node; node = node.parentElement) {
    const style = getComputedStyle(node);
    if (style.display === 'none' || style.visibility === 'hidden' || style.visibility === 'collapse') return false;
    // WebKit/WebView2 may retain geometry for children of closed details.
    // Such descendants are not exposed unless they belong to the summary.
    if (node.tagName === 'DETAILS' && !node.open) {
      const summary = [...node.children].find(child => child.tagName === 'SUMMARY');
      if (!summary?.contains(element)) return false;
    }
  }
  return true;
};
const fields = [...document.querySelectorAll('.settings-modal input')]
  .filter(element => !['checkbox', 'radio'].includes(element.type) && visible(element))
  .map(element => element.closest('label')?.firstChild?.textContent.trim());
const collapsed = ['.settings-advanced-tests', '.settings-key-management', '.settings-context-budget']
  .every(selector => { const nodes = document.querySelectorAll(selector); return nodes.length === 1 && !nodes[0].open; });
return { fields, collapsed };
"""


def verify_basic_model_fields(driver):
    result = driver.js(SETTINGS_BASIC_FIELDS_SCRIPT)
    assert result == {"fields": ["Base URL", "API Key", "模型名"], "collapsed": True}, f"Unexpected visible basic model form: {result}"


def configure_candidate_model(driver, server, version):
    # The fixture file authorizes loopback HTTP and advanced options only. The
    # synthetic key is entered/saved through the actual candidate UI, not IPC.
    driver.click(driver.button("打开设置"))
    wait_for(lambda: driver.js("return document.querySelector('.settings-modal')?.getAttribute('aria-modal') === 'false'"))
    if version == TARGET_VERSION:
        verify_basic_model_fields(driver)
    save = wait_for(lambda: driver.find(driver.button("保存")))
    wait_for(lambda: driver.command("GET", f"/element/{save}/enabled"))
    assert driver.js("return !document.querySelector('.app-shell').hasAttribute('inert')")
    driver.fill("Base URL", f"http://127.0.0.1:{server.server_port}/v1")
    driver.fill("模型名", "native-fixture-model")
    driver.fill("API Key", TOKEN)
    driver.click(driver.button("保存并测试"))
    wait_for(lambda: driver.find("//*[@role='group' and @aria-label='确认测试接收方']"))
    assert driver.js("return [...document.querySelectorAll('input[type=password]')].every(e=>e.value==='')")
    button = driver.find(driver.button("确认并测试"))
    assert driver.command("GET", f"/element/{button}/enabled") is False
    time.sleep(1)
    assert not server.requests and not server.test_requests
    driver.screenshot("model-settings-saved-test-awaits-consent")
    # v0.1.4 names the selected test scope in this label. The same exact
    # recipient-confirmation group still owns the explicit consent checkbox.
    consent = "//*[@role='group' and @aria-label='确认测试接收方']//input[@type='checkbox']"
    assert len(driver.command("POST", "/elements", {"using": "xpath", "value": consent})) == 1
    assert driver.command("GET", f"/element/{driver.find(consent)}/selected") is False
    driver.click(consent)
    driver.click(driver.button("确认并测试"))
    wait_for(lambda: driver.js("return document.querySelector('.settings-test-result')?.textContent.includes('流式聊天测试通过')"))
    assert len(server.test_requests) == 1 and not server.requests and not server.invalid_requests
    driver.screenshot("model-settings-real-ipc-minimal-test")
    driver.click(driver.button("关闭设置"))


def assert_literal_reply(driver):
    expected = json.dumps(REPLY)
    wait_for(lambda: driver.js(
        "return document.querySelector('article[aria-label=\"模型回复\"] p')?.textContent === " + expected))
    assert driver.js("return document.querySelectorAll('article[aria-label=\"模型回复\"] script').length === 0")
    assert driver.js("return window.unsafe === undefined")


def wait_for_chat_idle(driver, version):
    # Both reviewed v0.1.4 and v0.2.0 use the simplified unified composer.
    wait_for(lambda: driver.js("return ![...document.querySelectorAll('button')].some(e=>['停止生成','正在提交…'].includes(e.textContent.trim())) && document.querySelector('textarea[aria-label=\"聊天消息\"]')?.value === ''"))


def rendered_task_identity(driver, version):
    # Backend-rendered identity; no injected IPC or test-only app endpoint.
    value = driver.js("return document.querySelector('.conversation-details')?.getAttribute('data-task-id')")
    assert isinstance(value, str) and value, "Missing rendered candidate task identity"
    return value


def verify_context_controls(driver, server):
    """Real UI/IPC estimator and local compaction; provider is still synthetic."""
    indicator = "//button[starts-with(@aria-label,'可用输入预算占用')]"
    driver.click(indicator)
    panel = "//*[@role='dialog' and @aria-label='上下文占用与压缩']"
    wait_for(lambda: driver.find(panel))
    wait_for(lambda: driver.js("return document.querySelector('.context-total')?.textContent.includes('输入约') && !document.querySelector('.context-total')?.textContent.includes('未知')"))
    assert driver.js("return document.querySelector('.context-popover').getAttribute('aria-modal') === 'false'")
    assert driver.js("return document.querySelector('.context-popover').textContent.includes('回复后 80%，任务中 95%')")
    driver.click(panel + "//summary[normalize-space(.)='详情']")
    assert driver.js("return document.querySelector('.context-popover').textContent.includes('256K = 256000 tokens')")
    assert driver.js("return document.querySelector('.context-popover').textContent.includes('保守估算')")
    assert driver.js("return !/(?:目标|压缩到|压到|降至|降到)\\s*50%|50%\\s*(?:目标|固定)/.test(document.querySelector('.context-popover').textContent)")
    assert driver.js("return document.querySelector('select[aria-label=\"本次压缩方式\"]').value === 'llm'")
    driver.screenshot("native-context-default-budget-and-sources")
    driver.click("//select[@aria-label='本次压缩方式']/option[@value='quick']")
    requests_before = len(server.requests)
    driver.click(driver.button("立即压缩"))
    wait_for(lambda: driver.js("return [...document.querySelectorAll('.context-popover [role=status]')].some(e=>e.textContent.trim()==='上下文已整理')"))
    assert len(server.requests) == requests_before, "Quick compaction unexpectedly called the model"
    driver.screenshot("native-quick-compaction-positive-reduction")
    driver.click(driver.button("关闭上下文"))


def context_control_state(driver):
    return driver.js("""
      const panel = document.querySelector('.context-popover');
      return {
        mode: panel?.querySelector('select[aria-label="本次压缩方式"]')?.value,
        total: panel?.querySelector('.context-total')?.textContent,
        status: [...(panel?.querySelectorAll('[role=status]') ?? [])].map(e=>e.textContent.trim()),
        alerts: [...(panel?.querySelectorAll('[role=alert]') ?? [])].map(e=>e.textContent.trim()),
        actionEnabled: [...(panel?.querySelectorAll('button') ?? [])].some(e=>e.textContent.trim()==='立即压缩' && !e.disabled),
        details: panel?.querySelector('details')?.textContent
      };
    """)


def wait_for_compaction_provider(driver, server):
    try:
        wait_for(lambda: server.compaction_started.is_set())
    except AssertionError as error:
        raise AssertionError("LLM provider request did not start; native UI state: "
                             + json.dumps(context_control_state(driver), ensure_ascii=False)) from error


def verify_llm_compaction_and_cancel(driver, server):
    driver.click("//button[starts-with(@aria-label,'可用输入预算占用')]")
    driver.click("//select[@aria-label='本次压缩方式']/option[@value='llm']")
    # Selection and completed-turn estimate refresh must settle before clicking.
    # Observe only rendered state; never inject IPC or repeat an ambiguous click.
    previous = None
    def settled():
        nonlocal previous
        state = context_control_state(driver)
        ready = (state["mode"] == "llm" and state["actionEnabled"]
                 and not state["alerts"] and not any("更新" in value for value in state["status"]))
        same = state == previous
        previous = state
        if not (ready and same):
            time.sleep(0.25)
            return False
        return True
    wait_for(settled)
    driver.screenshot("native-llm-compaction-ready-before-request")
    server.hold_compaction = True
    server.release_compaction.clear()
    server.compaction_started.clear()
    before = len(server.compaction_requests)
    driver.click(driver.button("立即压缩"))
    wait_for_compaction_provider(driver, server)
    wait_for(lambda: driver.find(driver.button("取消压缩")))
    driver.screenshot("native-llm-compaction-real-http-in-progress")
    driver.click(driver.button("取消压缩"))
    wait_for(lambda: driver.js("return document.querySelector('.context-popover')?.textContent.includes('压缩已取消，原上下文保留')"))
    assert len(server.compaction_requests) == before + 1
    driver.screenshot("native-llm-compaction-cancelled-before-response")
    # Only release the fixture's held HTTP response after native cancellation
    # has reached the terminal state. Retry must make a new real provider call.
    server.hold_compaction = False
    server.release_compaction.set()
    driver.click(driver.button("立即压缩"))
    wait_for(lambda: driver.js("return [...document.querySelectorAll('.context-popover [role=status]')].some(e=>e.textContent.trim()==='上下文已整理')"))
    assert len(server.compaction_requests) > before + 1
    assert not server.invalid_requests
    driver.screenshot("native-llm-compaction-positive-reduction")
    driver.click(driver.button("关闭上下文"))


def verify_archive_page(driver, message_count):
    driver.click(driver.button("翻阅已保存对话"))
    wait_for(lambda: driver.js("return document.querySelector('[aria-label=\"归档历史页\"]')?.textContent.includes(" + json.dumps(PROMPT) + ")"))
    assert driver.js("return document.querySelector('[aria-label=\"归档历史页\"]').textContent.includes(" + json.dumps(REPLY) + ")")
    wait_for(lambda: driver.js("return document.querySelectorAll('[aria-label=\"归档历史页\"] article').length") == message_count)
    older = driver.find(driver.button("更早一页"))
    assert driver.command("GET", f"/element/{older}/enabled") is False
    driver.click(driver.button("最新一页"))
    wait_for(lambda: driver.js("return document.querySelectorAll('[aria-label=\"归档历史页\"] article').length") == message_count)
    driver.screenshot("native-durable-archive-page")
    driver.click(driver.button("收起历史页"))


def seed(driver, server, version):
    configure_candidate_model(driver, server, version)
    if version == TARGET_VERSION:
        assert driver.js("return ![...document.querySelectorAll('[role=tab]')].some(e=>['文字聊天','执行终端任务'].includes(e.textContent.trim())) && !document.querySelector('textarea[aria-label=\"终端任务指令\"]')")
    driver.click(driver.button("新建 SSH 连接"))
    for label, text in [("连接名称", PROFILE), ("主机地址", "127.0.0.1"), ("用户名", "synthetic-user")]:
        driver.fill(label, text)
    driver.click("//label[span[normalize-space(.)='保存连接']]/input")
    driver.click(driver.button("仅保存连接"))
    wait_for(lambda: driver.js("return ![...document.querySelectorAll('[role=dialog] h2')].some(e=>e.textContent==='连接 SSH 服务器')"))
    open_profiles(driver, version)
    driver.click(f"//select[@aria-label='选择档案']/option[starts-with(normalize-space(.),'{PROFILE}')]")
    profile_id = wait_for(lambda: driver.js("return document.querySelector('select[aria-label=\"选择档案\"]').value"))
    driver.screenshot("saved-profile")
    close_profiles(driver, version)
    enable_history(driver, version)
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
    driver.click("//label[contains(.,'我确认此地址')]/input")
    driver.click(driver.button("同意并发送"))
    assert_literal_reply(driver)
    wait_for_chat_idle(driver, version)
    assert len(server.requests) == 1 and server.invalid_requests == 0
    driver.screenshot("actual-ipc-streamed-chat")
    if version == TARGET_VERSION:
        # The engine protects the two newest complete turns. Six real turns
        # provide older eligible sources for separate positive quick and LLM
        # compactions; no source ledger or IPC is injected by this fixture.
        for turn in range(2, 7):
            element = driver.find("//textarea[@aria-label='聊天消息']")
            driver.command("POST", f"/element/{element}/value", {"text": FOLLOWUP, "value": list(FOLLOWUP)})
            driver.click(driver.button("发送"))
            wait_for(lambda: driver.js("return document.querySelectorAll('article[aria-label=\"模型回复\"]').length") == turn)
            wait_for_chat_idle(driver, version)
            assert len(server.requests) == turn and server.invalid_requests == 0
            assert server.requests[-1]["messages"][-1]["content"] == FOLLOWUP
            assert not driver.command("POST", "/elements", {"using": "xpath", "value": "//*[@role='dialog' and @aria-label='确认模型接收方']"})
            if turn == 3:
                verify_context_controls(driver, server)
        driver.screenshot("unified-no-ssh-contextual-followup")
        verify_llm_compaction_and_cancel(driver, server)
        verify_archive_page(driver, 12)
    return profile_id


def verify_preserved(driver, server, version, expected_profile_id, message_count=2):
    open_profiles(driver, version)
    driver.click(f"//select[@aria-label='选择档案']/option[starts-with(normalize-space(.),'{PROFILE}')]")
    assert driver.js("return document.querySelector('select[aria-label=\"选择档案\"]').value") == expected_profile_id
    driver.click(driver.button("编辑 / 重命名"))
    wait_for(lambda: driver.js("return [...document.querySelectorAll('label')].find(e=>e.textContent.startsWith('用户名'))?.querySelector('input')?.value === 'synthetic-user'"))
    assert driver.js("return document.querySelector('input[aria-label=\"主机地址\"]').value === '127.0.0.1'")
    assert driver.js("return document.querySelector('input[aria-label=\"连接名称\"]').value") == PROFILE
    driver.click(driver.button("取消"))
    assert driver.js("return [...document.querySelectorAll('input[type=password]')].every(e=>e.value==='')")
    close_profiles(driver, version)
    enable_history(driver, version)
    # The saved title remains '新任务'; it is not derived from message text.
    # This isolated fixture has exactly one conversation (baseline: two messages;
    # candidate: four, including the contextual follow-up). Require
    # uniqueness, then verify exact reply and persisted task/chat identity below.
    restore = f"//div[@aria-label='本机保存的聊天']/div[span[contains(.,' · {message_count} 条消息')]]/button[normalize-space(.)='仅恢复文字']"
    wait_for(lambda: len(driver.command("POST", "/elements", {"using": "xpath", "value": restore})) == 1)
    driver.click(restore)
    assert_literal_reply(driver)
    assert driver.js("return document.body.textContent.includes('未连接服务器')")
    time.sleep(1)
    assert server.requests == [], "Restore/restart unexpectedly started a model request"
    driver.screenshot("restored-text-no-execution")
    if version == TARGET_VERSION:
        verify_archive_page(driver, message_count)
        assert not server.requests, "Archive pagination unexpectedly called the model"


def stored_identity(history):
    database = history / "history.sqlite3"
    with sqlite3.connect(database.as_uri() + "?mode=ro", uri=True) as connection:
        rows = connection.execute("SELECT chat_id,task_id,payload FROM chats").fetchall()
        if connection.execute("PRAGMA user_version").fetchone()[0] == 3:
            rows = [(chat_id, task_id, "\n".join(entry[0] for entry in connection.execute(
                "SELECT payload FROM entries WHERE chat_id=? ORDER BY sequence", (chat_id,))))
                for chat_id, task_id, _ in rows]
    matches = [(chat_id, task_id) for chat_id, task_id, payload in rows if PROMPT in payload and REPLY in payload]
    assert len(matches) == 1, "Expected one persisted synthetic conversation"
    assert TOKEN.encode() not in database.read_bytes(), "Synthetic API token leaked into persisted history"
    return matches[0]


def history_rows(history):
    """Read exact existing rows; database file bytes may change during migration."""
    database = history / "history.sqlite3"
    with sqlite3.connect(database.as_uri() + "?mode=ro", uri=True) as connection:
        version = connection.execute("PRAGMA user_version").fetchone()[0]
        rows = connection.execute("SELECT chat_id,task_id,revision,listed,updated_ms,payload FROM chats ORDER BY chat_id").fetchall()
    return version, [list(row) for row in rows]


def verify_history_upgrade(history, expected_rows):
    version, rows = history_rows(history)
    assert version == 3, "Candidate did not migrate to the supported history format marker"
    indexed = {row[0]: row for row in rows}
    assert expected_rows, "Upgrade must start with real baseline history rows"
    database = history / "history.sqlite3"
    with sqlite3.connect(database.as_uri() + "?mode=ro", uri=True) as connection:
        for row in expected_rows:
            assert indexed.get(row[0], [])[:5] == row[:5], "Upgrade changed a preexisting history row identity"
            backup = connection.execute("SELECT payload FROM legacy_backups WHERE chat_id=?", (row[0],)).fetchone()
            assert backup and backup[0] == row[5], "Upgrade lost exact preexisting history row backup"
            original = json.loads(row[5])
            entries = connection.execute("SELECT sequence,message_id,payload FROM entries WHERE chat_id=? ORDER BY sequence", (row[0],)).fetchall()
            assert [json.loads(entry[2]) for entry in entries] == original["messages"], "Archive migration lost original message fields"
            assert [entry[0] for entry in entries] == list(range(1, len(entries) + 1)), "Archive sequence is not contiguous"
            assert [entry[1] for entry in entries] == [message["id"] for message in original["messages"]], "Archive message identities changed"
            assert json.loads(indexed[row[0]][5])["messages"] == [], "Archive metadata retained a bounded transcript copy"


def verify_compaction_archive(history):
    database = history / "history.sqlite3"
    with sqlite3.connect(database.as_uri() + "?mode=ro", uri=True) as connection:
        rows = connection.execute("SELECT event_id,payload FROM compactions ORDER BY event_id").fetchall()
        assert len(rows) == 2, "Only the successful quick and LLM compactions should be archived"
        modes = set()
        for event_id, payload in rows:
            event = json.loads(payload)
            assert event["version"] == 1
            record = event["record"]
            assert record["event_id"] == event_id
            modes.add(record["mode"])
            assert record["summary_text"].strip()
            assert record["before_tokens"] >= record["after_tokens"]
            assert record["before_bytes"] >= record["after_bytes"]
            assert record["before_tokens"] > record["after_tokens"] or record["before_bytes"] > record["after_bytes"]
            indexed = connection.execute("SELECT source_id FROM compaction_sources WHERE event_id=? ORDER BY ordinal", (event_id,)).fetchall()
            assert [row[0] for row in indexed] == record["source_ids"], "Durable compaction source index differs from its event"
        assert modes == {"quick", "llm"}


def verify_private_file(path, expected_digest, label):
    assert path.is_file() and not path.is_symlink(), f"Missing protected regular {label}"
    assert hashlib.sha256(path.read_bytes()).hexdigest() == expected_digest, f"{label} differs from baseline bytes"
    if os.name != "nt":
        info = path.stat()
        assert info.st_uid == os.getuid() and info.st_mode & 0o7777 == 0o600, f"{label} is not owner-only"
    else:
        quoted = str(path).replace("'", "''")
        script = ("$ErrorActionPreference='Stop'; "
                  "$sid=[Security.Principal.WindowsIdentity]::GetCurrent().User.Value; "
                  f"$acl=Get-Acl -LiteralPath '{quoted}'; "
                  "$owner=$acl.GetOwner([Security.Principal.SecurityIdentifier]).Value; "
                  "$allowed=@($sid,'S-1-5-18','S-1-5-32-544'); "
                  "$unexpected=@($acl.GetAccessRules($true,$true,[Security.Principal.SecurityIdentifier]) | "
                  "Where-Object { $_.AccessControlType -eq 'Allow' -and $_.IdentityReference.Value -notin $allowed }); "
                  "if ($owner -ne $sid -or -not $acl.AreAccessRulesProtected -or $unexpected.Count -ne 0) { throw 'Unsafe private acceptance file ACL' }")
        # Match the pwsh host used by the Windows fixture. Windows PowerShell
        # cannot reliably load its Security module with inherited pwsh module paths.
        subprocess.run(["pwsh", "-NoProfile", "-NonInteractive", "-Command", script], check=True, timeout=15)


def verify_profile_backup(config, expected_digest):
    # Retain the strict legacy migration helper and regression coverage. The
    # reviewed v0.1.4 baseline already writes v2, so this is not its upgrade gate.
    verify_private_file(config / "connections.v1.backup.toml", expected_digest, "Legacy profile backup")


def profile_schema_version(config):
    path = config / "connections.toml"
    assert path.is_file() and not path.is_symlink(), "Missing regular profile file"
    markers = re.findall(r"^schema_version\s*=\s*([0-9]+)\s*$", path.read_text(encoding="utf-8"), re.MULTILINE)
    assert len(markers) == 1, "Expected one profile schema marker"
    return int(markers[0])


def verify_current_profile_upgrade(config, expected):
    assert expected["profile_schema_version"] == 2, "Upgrade must start with real v0.1.4 profiles"
    verify_private_file(config / "connections.toml", expected["profile_file_sha256"], "Profile file")
    assert profile_schema_version(config) == 2, "Upgrade changed the profile schema marker"
    backup = config / "connections.v1.backup.toml"
    assert not backup.exists() and not backup.is_symlink(), "Fresh v2 baseline unexpectedly created a legacy backup"


def open_shell_logs(driver, version):
    driver.click(driver.button("打开终端日志"))
    wait_for(lambda: driver.find("//select[@aria-label='日志查看范围']"))
    # A disconnected current-session view deliberately does not widen its
    # scope. Explicitly choose the all-records view before querying/export.
    driver.click("//select[@aria-label='日志查看范围']/option[@value='all']")


def verify_empty_shell_log(driver, history, server, version):
    # This native smoke has no SSH connection and must never invent a shell
    # event or create a database just to make export succeed.
    assert not list(history.glob("shell-log*")), "Unexpected shell-log data before consent"
    requests_before = len(server.requests)
    open_shell_logs(driver, version)
    assert driver.js("return document.querySelector('.shell-log-dialog')?.getAttribute('aria-modal') === 'false'")
    wait_for(lambda: driver.js("return document.querySelector('.shell-log-dialog')?.textContent.includes('当前筛选下暂无日志。')"))
    wait_for(lambda: driver.js("return document.querySelector('.shell-log-dialog')?.textContent.includes('记录状态：未启用')"))
    assert driver.js("return document.querySelector('.shell-log-dialog').textContent.includes('不保存终端输出，不自动发送给模型')")
    checks = driver.command("POST", "/elements", {"using": "xpath", "value": "//div[contains(@class,'shell-log-consent')]//input[@type='checkbox']"})
    assert len(checks) == 1
    for checkbox in checks:
        assert driver.command("GET", f"/element/{checkbox[ELEMENT]}/selected") is False
        assert driver.command("GET", f"/element/{checkbox[ELEMENT]}/enabled") is False
    start = driver.find(driver.button("启动终端日志"))
    assert driver.command("GET", f"/element/{start}/enabled") is False
    driver.screenshot("shell-log-disabled-empty")
    driver.click(driver.button("关闭对话框"))
    open_shell_logs(driver, version)
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
    if version not in {BASE_VERSION, TARGET_VERSION}:
        raise ValueError("No reviewed native UI fixture for this application version")
    if phase == "seed":
        if expected is not None:
            raise ValueError("Seed must use fresh fixture state")
        return
    if not expected or not expected.get("application_sha256"):
        raise ValueError("Missing original application identity")
    if phase == "upgrade":
        if expected.get("application_version") != BASE_VERSION or version != TARGET_VERSION:
            raise ValueError(f"Only the reviewed {BASE_VERSION} to {TARGET_VERSION} upgrade is covered")
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
    max_output_tokens = fixture_output_reserve(args.application_version, expected)
    application_hash = hashlib.sha256(args.application.read_bytes()).hexdigest()
    verify_phase_version(args.phase, args.application_version, application_hash, expected)
    server = http.server.ThreadingHTTPServer(("127.0.0.1", expected["port"] if expected else 0), ModelFixture)
    server.requests, server.test_requests, server.invalid_requests = [], [], 0
    server.compaction_requests = []
    server.compaction_started, server.release_compaction = threading.Event(), threading.Event()
    server.hold_compaction = False
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    if expected is None:
        text = f'''schema_version = 1
[model]
protocol = "openai_chat_completions"
base_url = "http://127.0.0.1:{server.server_port}/v1"
api_key = ""
name = "native-fixture-model"
[capabilities]
streaming = "required"
tool_calling = "required"
[request]
connect_timeout_seconds = 5
request_timeout_seconds = 30
stream_idle_timeout_seconds = 10
max_output_tokens = {max_output_tokens}
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
        profile_id = expected["profile_id"] if expected else None
        message_count = expected.get("message_count", 2) if expected else (12 if args.application_version == TARGET_VERSION else 2)
        for phase in phases:
            directory = args.evidence / phase
            directory.mkdir(exist_ok=True)
            driver = Driver(args.application.resolve(strict=True), directory)
            try:
                if phase == "seed":
                    profile_id = seed(driver, server, args.application_version)
                else:
                    verify_preserved(driver, server, args.application_version, profile_id, message_count)
                verify_empty_shell_log(driver, history, server, args.application_version)
                visible_task_identity = rendered_task_identity(driver, args.application_version)
            except BaseException:
                with contextlib.suppress(Exception):
                    driver.screenshot("failure")
                    (directory / "failure.html").write_text(driver.command("GET", "/source"), encoding="utf-8")
                raise
            finally:
                driver.close()
            assert not any((history / ("history.sqlite3" + suffix)).exists() for suffix in ("-wal", "-shm", "-journal")), "History database sidecars survived clean application exit"
            assert len(server.requests) == (message_count // 2 if phase == "seed" else 0), "Unexpected model request before process exit"
            assert len(server.test_requests) == (1 if phase == "seed" else 0), "Unexpected model configuration test"
            assert server.invalid_requests == 0
            if phase == "seed" and args.application_version == TARGET_VERSION:
                assert len(server.compaction_requests) >= 2, "Expected cancelled and successful real compaction requests"
            else:
                assert not server.compaction_requests, "Restoration unexpectedly started compaction"
            actual = list(stored_identity(history))
            assert visible_task_identity == actual[1], "Visible native task identity differs from persisted history"
            if identity is None:
                identity = actual
            assert actual == identity, "Restoration changed the stored task/chat identity"
            model_text = (config / "model.toml").read_text(encoding="utf-8")
            for advanced in ("connect_timeout_seconds = 5", "request_timeout_seconds = 30",
                             "stream_idle_timeout_seconds = 10", f"max_output_tokens = {max_output_tokens}",
                             "allow_loopback_http = true"):
                assert advanced in model_text, "Saving model settings lost an advanced fixture option"
            server.requests.clear()
            server.test_requests.clear()
            server.compaction_requests.clear()
        history_version, preserved_rows = history_rows(history)
        assert history_version == (3 if args.application_version == TARGET_VERSION else 2), "Unexpected native history schema"
        if args.phase == "seed" and args.application_version == TARGET_VERSION:
            verify_compaction_archive(history)
        profile_version = profile_schema_version(config)
        assert profile_version == 2, "Reviewed native fixture must use profile schema v2"
        if args.phase == "upgrade":
            assert expected["history_version"] == 2, "Upgrade must start with real v0.1.4 history"
            verify_history_upgrade(history, expected["history_rows"])
            verify_current_profile_upgrade(config, expected)
            verify_private_file(config / "model.toml", expected["model_file_sha256"], "Model settings")
        if expected is None or args.phase == "upgrade":
            state_file.write_text(json.dumps({"port": server.server_port, "identity": identity, "profile_id": profile_id,
                "application_version": args.application_version, "application_sha256": application_hash, "message_count": message_count, "max_output_tokens": max_output_tokens,
                "history_version": history_version, "history_rows": preserved_rows,
                "profile_schema_version": profile_version,
                "profile_file_sha256": hashlib.sha256((config / "connections.toml").read_bytes()).hexdigest(),
                "model_file_sha256": hashlib.sha256((config / "model.toml").read_bytes()).hexdigest()}), encoding="utf-8")
        (args.evidence / "result.json").write_text(json.dumps({
            "result": "passed", "phase": args.phase, "application": str(args.application),
            "application_sha256": application_hash, "application_version": args.application_version,
            "upgrade_from": expected["application_version"] if args.phase == "upgrade" else None,
            "native_ipc": True, "synthetic_loopback_model": True,
            "native_llm_compaction_cancel_retry_positive_reduction": args.phase == "seed" and args.application_version == TARGET_VERSION,
            "context_estimator_and_quick_positive_reduction": args.phase == "seed" and args.application_version == TARGET_VERSION,
            "durable_archive_page_read": args.application_version == TARGET_VERSION,
            "durable_compaction_event_and_source_index": args.phase == "seed" and args.application_version == TARGET_VERSION,
            "profile_and_history_persisted": True, "restored_task_identity_preserved": True,
            "history_requires_new_process_consent": True, "restore_model_requests": 0,
            "shell_log_empty_export_fail_closed": True,
            "profile_id_preserved": True,
            "history_rows_and_private_v2_profiles_preserved": args.phase == "upgrade",
            "legacy_profile_migration_tested": False,
            "model_settings_saved_and_tested_via_ui": args.phase == "seed",
            "unified_no_ssh_followup_context_tested": args.phase == "seed" and args.application_version == TARGET_VERSION,
            "not_tested": ["real provider", "SSH via UI", "desktop input methods", "Windows 10/11", "native multi-page archive"]
                + ([] if args.phase == "upgrade" else ["version upgrade in this phase"]),
        }, indent=2), encoding="utf-8")
    finally:
        server.release_compaction.set()
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
    print(f"PASS: installed native UI/IPC {args.phase}; profile/history and consent checks")


if __name__ == "__main__":
    main()
