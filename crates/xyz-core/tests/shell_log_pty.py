#!/usr/bin/env python3
"""Real CURRENT-Bash PTY contract tests (synthetic shells only).
Run: python3 crates/xyz-core/tests/shell_log_pty.py
Requires Rust/Cargo, an explicitly supported Bash runtime, and Python pexpect.
Set XYZ_TEST_BASH to exercise an alternate real runtime (not a mocked version). No network, rc changes, or user history.
This fixture configures its own test shell; production preserves/rejects settings.
"""
import base64
import hashlib
import pathlib
import re
import unittest
import subprocess
import shlex
import sys
import os
import tempfile
import pexpect

ROOT = pathlib.Path(__file__).resolve().parents[3]
BASH = str(pathlib.Path(os.environ.get('XYZ_TEST_BASH', '/bin/bash')).resolve())
BASH_VERSION = subprocess.check_output(
    [BASH, '--noprofile', '--norc', '-c', 'printf "%s" "$BASH_VERSION"'],
    env=dict(os.environ, BASH_ENV=''),
).decode('ascii')
BASH_MAJOR_MINOR = tuple(int(n) for n in BASH_VERSION.split('(')[0].split('.')[:2])
if BASH_MAJOR_MINOR not in [(4, 2), (5, 1), (5, 2)]:
    raise SystemExit('This acceptance suite requires an implemented adapter: ' + BASH_VERSION)
EXPECTED_VERSION = os.environ.get('XYZ_TEST_BASH_VERSION')
if EXPECTED_VERSION and not BASH_VERSION.startswith(EXPECTED_VERSION + '('):
    raise SystemExit(f'Expected real Bash {EXPECTED_VERSION}, got {BASH_VERSION} at {BASH}')
print(f'Real shell runtime: {BASH_VERSION} ({BASH})', flush=True)
HAS_PS0 = BASH_MAJOR_MINOR >= (4, 4)
HAS_PROMPT_ARRAY = BASH_MAJOR_MINOR >= (5, 1)
NONCE = '0123456789abcdef0123456789abcdef'
SOURCE = (ROOT / 'crates/xyz-core/src/shell_log_bootstrap.bash').read_text().replace('@NONCE@', NONCE)
BOOTSTRAP = subprocess.check_output(
    ['cargo', 'run', '-p', 'xyz-core', '--locked', '--quiet', '--example', 'shell_log_bootstrap'] + (['--', '--legacy'] if BASH_MAJOR_MINOR == (4, 2) else []), cwd=ROOT,
).decode('utf-8')
STARTUP = subprocess.check_output(
    ['cargo', 'run', '-p', 'xyz-core', '--locked', '--quiet', '--example', 'shell_log_bootstrap', '--', '--startup'], cwd=ROOT,
).decode('utf-8')
FRAME = re.compile(rb'\x1b\]777;xyzlog;2;' + NONCE.encode() + rb';([^\x07]*)\x07')
PROMPT = b'XYZ_PTY> '


def frames(output):
    return [body.split(b';') for body in FRAME.findall(output)]


class Shell:
    def __init__(self, hooks=''):
        self.p = pexpect.spawn(BASH, ['--noprofile', '--norc', '-i'], encoding=None, timeout=5, echo=False)
        self.p.expect(rb'bash-[0-9.]+[$#] ')
        setup = "HISTFILE=; HISTCONTROL=; HISTIGNORE=; HISTSIZE=1000; shopt -s cmdhist lithist promptvars; PS1='XYZ_PTY> '; PS2='CONT> '"
        if hooks:
            setup += '; ' + hooks
        self.run(setup)

    def run(self, command):
        if isinstance(command, str):
            command = command.encode()
        self.p.sendline(command)
        self.p.expect_exact(PROMPT)
        return self.p.before

    def install(self):
        self.p.send(BOOTSTRAP.encode())
        self.p.expect_exact(PROMPT)
        return self.p.before

    def close(self):
        self.p.close(force=True)


class CurrentShellContract(unittest.TestCase):
    def shell(self, hooks=''):
        shell = Shell(hooks)
        self.addCleanup(shell.close)
        return shell

    def assert_lifecycle(self, output, command, code):
        events = frames(output)
        if not HAS_PS0:
            self.assertEqual([f[0] for f in events], [b'G', b'R'], output)
            self.assertEqual(events[0], [b'G', b'0', b'legacy_unobserved_command'])
            return events
        self.assertEqual([f[0] for f in events], [b'B', b'E', b'R'], output)
        self.assertEqual(base64.b64decode(events[0][4]).rstrip(b'\n'), command.encode() if isinstance(command, str) else command)
        self.assertEqual(events[1][3], str(code).encode())
        self.assertEqual(events[0][1], events[1][1])
        self.assertEqual(events[0][1], events[2][1])
        return events

    def test_current_pid_alias_function_prompt_string_hooks_and_status(self):
        s = self.shell("PS0='PREEXISTING0'; PROMPT_COMMAND='builtin printf \"PC:%s\\n\" \"$?\"'; alias zz='printf ALIAS_OK'; ff(){ return 23; }; saved_pid=$$")
        hello = frames(s.install())
        self.assertEqual([f[0] for f in hello], [b'H', b'R'])
        output = s.run('false')
        self.assert_lifecycle(output, 'false', 1)
        if HAS_PS0:
            self.assertIn(b'PREEXISTING0', output)
        else:
            self.assertNotIn(b'PREEXISTING0', output)
        self.assertIn(b'PC:1', output)
        self.assertLess(output.index(b';E;' if HAS_PS0 else b';G;'), output.index(b'PC:1'))
        self.assertLess(output.index(b'PC:1'), output.index(b';R;'))
        output = s.run('printf "STATUS:%s\\n" "$?"; test "$$" = "$saved_pid"; zz; ff')
        self.assert_lifecycle(output, 'printf "STATUS:%s\\n" "$?"; test "$$" = "$saved_pid"; zz; ff', 23)
        self.assertIn(b'STATUS:1', output)
        self.assertIn(b'ALIAS_OK', output)
        names = base64.b64decode(frames(output)[-1][3]).splitlines()
        self.assertIn(b'zz', names)
        self.assertIn(b'ff', names)

    def test_ubuntu_defaults_prompt_and_repeated_ai_commands_keep_history_settings(self):
        s = self.shell("HISTCONTROL=ignoreboth; shopt -u lithist; debian_chroot=fixture; PS1='${debian_chroot:+($debian_chroot)}\\u@\\h:\\w\\$ XYZ_PTY> '; before_hist=$(declare -p HISTCONTROL HISTIGNORE HISTSIZE); before_shopt=$(shopt -p cmdhist lithist)")
        self.assertEqual([f[0] for f in frames(s.install())], [b'H', b'R'])
        for action in ['one', 'two', 'three']:
            command = 'pwd # xyz-action:' + action
            self.assert_lifecycle(s.run(command), command, 0)
        output = s.run('test "$before_hist" = "$(declare -p HISTCONTROL HISTIGNORE HISTSIZE)" && test "$before_shopt" = "$(shopt -p cmdhist lithist)"; printf "PRESERVED:%s\\n" "$?" # xyz-action:check')
        self.assertIn(b'PRESERVED:0', output)
        # A real ignored manual line has no claimed command text/completion.
        self.assertEqual([f[0] for f in frames(s.run(' hidden_manual_command'))][0], b'G' if HAS_PS0 else b'R')

    def test_interrupt_reports_foreground_130_and_never_retries(self):
        s = self.shell(); s.install()
        # The actual foreground helper confirms its own signal disposition.
        # PS0 command substitutions can temporarily own another process group;
        # neither a Begin prefix nor tcgetpgrp != Bash PID identifies the job.
        helper = 'import signal,time; signal.signal(signal.SIGINT, signal.SIG_DFL); print("XYZ_INTERRUPT_READY", flush=True); time.sleep(30)'
        command = shlex.quote(sys.executable) + ' -c ' + shlex.quote(helper) + ' # xyz-action:interrupt'
        s.p.sendline(command.encode())
        s.p.expect_exact(b'XYZ_INTERRUPT_READY\r\n')
        started = frames(s.p.before)
        self.assertEqual([f[0] for f in started], [b'B'] if HAS_PS0 else [])
        if HAS_PS0:
            self.assertEqual(base64.b64decode(started[0][4]).rstrip(b'\n'), command.encode())
        s.p.sendcontrol('c')
        s.p.expect_exact(PROMPT)
        events = frames(s.p.before)
        self.assertEqual([f[0] for f in events], [b'E', b'R'] if HAS_PS0 else [b'G', b'R'])
        if HAS_PS0:
            self.assertEqual(events[0][3], b'130')
            self.assertEqual(events[0][1], started[0][1])
            self.assertEqual(events[1][1], started[0][1])
        self.assertIn(b'INTERRUPT_STATUS:130', s.run('printf "INTERRUPT_STATUS:%s" "$?"'))

    def test_array_prompt_hooks_keep_order_and_original_values_and_restore(self):
        s = self.shell("PS0='ORIGINAL0'; PROMPT_COMMAND=('builtin printf \"ONE:%s\\n\" \"$?\"' 'builtin printf \"TWO:%s\\n\" \"$?\"'); before=$(declare -p PROMPT_COMMAND PS0); trap 'printf EXIT_OK' EXIT")
        installed = s.install()
        if not HAS_PROMPT_ARRAY:
            self.assertEqual([f[0] for f in frames(installed)], [b'U'])
            output = s.run('test "$before" = "$(declare -p PROMPT_COMMAND PS0)"; printf "UNCHANGED:%s\\n" "$?"')
            self.assertIn(b'UNCHANGED:0', output)
            self.assertEqual(frames(output), [])
            return
        output = s.run('false')
        self.assert_lifecycle(output, 'false', 1)
        self.assertLess(output.index(b'ONE:1'), output.index(b'TWO:1'))
        self.assertIn(b'declare -a PROMPT_COMMAND', s.run('declare -p PROMPT_COMMAND'))
        s.run('__xyz_uninstall')
        output = s.run('test "$before" = "$(declare -p PROMPT_COMMAND PS0)"; printf "RESTORED:%s\\n" "$?"; trap -p EXIT')
        self.assertIn(b'RESTORED:0', output)
        self.assertIn(b'EXIT_OK', output)
        self.assertEqual(frames(output), [])

    def test_multiline_heredoc_pipeline_cwd_background_and_nonzero_status(self):
        s = self.shell(); s.install()
        for command, code in [('printf a | cat', 0), ('set -o pipefail; false | true', 1)]:
            self.assert_lifecycle(s.run(command), command, code)
        events = self.assert_lifecycle(s.run('cd /tmp; pwd'), 'cd /tmp; pwd', 0)
        if HAS_PS0:
            self.assertNotEqual(base64.b64decode(events[0][3]), b'/tmp')

    def test_async_manual_gap_preserves_native_shell_status(self):
        for installed in [False, True]:
            with self.subTest(installed=installed):
                s = self.shell()
                if installed:
                    s.install()
                s.run('false')
                output = s.run('sleep 2 &')
                if installed and not HAS_PS0:
                    self.assertEqual([f[0] for f in frames(output)], [b'G', b'R'])
                    self.assertEqual(frames(output)[0][-1], b'legacy_unobserved_command')
                elif installed:
                    self.assert_lifecycle(output, 'sleep 2 &', 0)
                self.assertIn(b'STATUS:0', s.run('printf "STATUS:%s" "$?"'))
                self.assertIn(b'Running', s.run('jobs'))

    def test_manual_multiline_history_is_exact_or_explicitly_unavailable(self):
        for command in ["printf '%s\\n' 'one\ntwo'", "cat <<'END'\nhello\nEND"]:
            with self.subTest(command=command):
                s = self.shell(); s.install()
                output = s.run(command)
                if HAS_PS0:
                    self.assert_lifecycle(output, command, 0)
                else:
                    self.assertEqual([f[0] for f in frames(output)], [b'G', b'R'])
                    self.assertEqual(frames(output)[0], [b'G', b'0', b'legacy_unobserved_command'])
                    self.assert_lifecycle(s.run('printf STILL_MANUAL'), 'printf STILL_MANUAL', 0)
        if not HAS_PS0:
            native = self.shell()
            native.run("cat <<'END'\nhello\nEND")
            history = native.run('history 2')
            self.assertIn(b"hello\r\n\r\nEND", history)

    def test_standalone_subshell_is_observed_or_fails_closed_by_adapter(self):
        s = self.shell(); s.install()
        output = s.run('(cd /; pwd)')
        if HAS_PS0:
            self.assert_lifecycle(output, '(cd /; pwd)', 0)
        else:
            self.assertEqual([f[0] for f in frames(output)], [b'G', b'R'])
            self.assertEqual(frames(output)[0], [b'G', b'0', b'legacy_unobserved_command'])
            self.assert_lifecycle(s.run('printf STILL_MANUAL'), 'printf STILL_MANUAL', 0)

    def test_interactive_input_not_recorded_and_ready_after_blocking_prompt_hook(self):
        s = self.shell("PROMPT_COMMAND='if [[ ${block_prompt-} == yes ]]; then read -r -p HOOK_WAIT value; block_prompt=no; fi'")
        s.install()
        s.p.sendline(b'block_prompt=yes; false')
        s.p.expect_exact(b'HOOK_WAIT')
        output = s.p.before
        self.assertEqual([f[0] for f in frames(output)], [b'B', b'E'] if HAS_PS0 else [b'G'])
        s.p.sendline(b'continue')
        s.p.expect_exact(PROMPT)
        self.assertEqual([f[0] for f in frames(s.p.before)], [b'R'])
        s.p.sendline(b'read -s -p SECRET_WAIT secret')
        s.p.expect_exact(b'SECRET_WAIT')
        output = s.p.before
        s.p.sendline(b'SYNTHETIC_PASSWORD_NOT_A_COMMAND')
        s.p.expect_exact(PROMPT)
        output += s.p.before
        self.assertNotIn(b'SYNTHETIC_PASSWORD_NOT_A_COMMAND', output)
        self.assert_lifecycle(output, 'read -s -p SECRET_WAIT secret', 0)

    def test_unsupported_err_trap_and_dynamic_prompt_leave_hooks_unchanged(self):
        for setup in ["trap 'true' DEBUG", "trap 'true' RETURN", "trap 'true' ERR", "trap 'read -r value' WINCH", "trap 'read -r value' ALRM", "PS1='$(printf dynamic)XYZ_PTY> '", "shopt -u promptvars", "shopt -u interactive_comments", "HISTIGNORE='*'"]:
            with self.subTest(setup=setup):
                s = self.shell("PS0='KEPT'; PROMPT_COMMAND='true'; " + setup + "; before=$(declare -p PS0 PROMPT_COMMAND); old_traps=$(trap -p)")
                output = s.install()
                self.assertEqual([f[0] for f in frames(output)], [b'U'])
                output = s.run('test "$before" = "$(declare -p PS0 PROMPT_COMMAND)" && test "$old_traps" = "$(trap -p)"; printf "UNCHANGED:%s\\n" "$?"')
                self.assertIn(b'UNCHANGED:0', output)
                self.assertEqual(frames(output), [])
                s.close()

    def test_changed_hooks_emit_gap_never_ready_and_cleanup_does_not_overwrite(self):
        s = self.shell(); s.install()
        mutation = "PS0='USER_EDIT'" if HAS_PS0 else "trap 'printf USER_EDIT' DEBUG"
        output = s.run(mutation)
        self.assertEqual([f[0] for f in frames(output)], [b'B', b'E', b'G'] if HAS_PS0 else [b'G', b'G'])
        self.assertNotIn(b';R;', s.run('printf STILL_MANUAL'))
        s.run('__xyz_uninstall')
        self.assertIn(b'USER_EDIT', s.run('declare -p PS0' if HAS_PS0 else 'trap -p DEBUG'))

    def test_runtime_async_trap_and_disabled_comments_stop_before_next_ready(self):
        for command in ["trap 'read -r value' WINCH", 'shopt -u interactive_comments']:
            with self.subTest(command=command):
                s = self.shell(); s.install()
                output = s.run(command)
                self.assertEqual([f[0] for f in frames(output)], [b'B', b'E', b'G'] if HAS_PS0 else [b'G', b'G'])
                self.assertNotIn(b';R;', output)
                s.close()

    def test_errexit_change_never_rearms_automatic_execution(self):
        s = self.shell(); s.install()
        s.p.sendline(b'set -e')
        ended = s.p.expect_exact([PROMPT, pexpect.EOF])
        events = frames(s.p.before)
        self.assertTrue(events and events[0][0] == (b'B' if HAS_PS0 else b'G'), s.p.before)
        self.assertNotIn(b'R', [event[0] for event in events])
        if ended == 0:
            self.assertIn(b'G', [event[0] for event in events])
            self.assertEqual(frames(s.run('printf STILL_MANUAL')), [])

    def test_blank_prompt_and_sequential_simple_commands_have_exact_lifecycles(self):
        s = self.shell(); s.install()
        blank = frames(s.run(''))
        self.assertTrue(all(event[0] == b'R' and event[1] == b'0' for event in blank), blank)
        previous = 0
        for command, code in [('true', 0), ('false', 1), ('pwd', 0), ('printf SIMPLE', 0)]:
            events = self.assert_lifecycle(s.run(command), command, code)
            self.assertEqual(int(events[0][1]), previous + 1 if HAS_PS0 else 0)
            previous += 1

    def test_history_change_does_not_reuse_previous_command(self):
        s = self.shell(); s.install()
        self.assertEqual([f[0] for f in frames(s.run('set +o history'))], [b'B', b'E', b'G'] if HAS_PS0 else [b'G', b'G'])
        self.assertEqual(frames(s.run('echo PRIVATE_AFTER_GAP')), [])

    def test_nested_shell_is_one_unfinished_outer_command_until_return(self):
        s = self.shell(); s.install()
        s.p.sendline((shlex.quote(BASH) + ' --noprofile --norc -i').encode())
        s.p.expect(rb'bash-[0-9.]+[$#] ')
        self.assertEqual([f[0] for f in frames(s.p.before)], [b'B'] if HAS_PS0 else [])
        s.p.sendline(b'echo INNER')
        s.p.expect(rb'bash-[0-9.]+[$#] ')
        self.assertEqual(frames(s.p.before), [])
        s.p.sendline(b'exit')
        s.p.expect_exact(PROMPT)
        self.assertEqual([f[0] for f in frames(s.p.before)], [b'E', b'R'] if HAS_PS0 else [b'G', b'R'])

    def test_exit_and_exec_do_not_fabricate_completion(self):
        for command in ['exit 7', 'exec /bin/true']:
            with self.subTest(command=command):
                s = self.shell("trap 'printf PRESERVED_EXIT_TRAP' EXIT")
                s.install()
                s.p.sendline(command.encode())
                s.p.expect(pexpect.EOF)
                self.assertEqual([f[0] for f in frames(s.p.before)], [b'B'] if HAS_PS0 else [])
                if command.startswith('exit'):
                    self.assertIn(b'PRESERVED_EXIT_TRAP', s.p.before)



class ControlledStartupContract(unittest.TestCase):
    def launch(self, rc='', account_shell=BASH):
        home = tempfile.TemporaryDirectory(prefix='xyz-startup-')
        self.addCleanup(home.cleanup)
        path = pathlib.Path(home.name)
        setup = "HISTFILE=; HISTCONTROL=ignoreboth; HISTIGNORE=; HISTSIZE=1000; shopt -s cmdhist promptvars interactive_comments; PS1='XYZ_PTY> '; PS2='CONT> '\n"
        (path / '.bashrc').write_text(setup + rc + '\n')
        (path / '.bash_profile').write_text('printf LOGIN_PROFILE_SHOULD_NOT_RUN\n')
        before = (path / '.bashrc').read_bytes()
        process = pexpect.spawn('/bin/sh', ['-c', STARTUP],
            env=dict(os.environ, HOME=home.name, SHELL=account_shell),
            encoding=None, timeout=5, echo=False)
        self.addCleanup(process.close, force=True)
        return process, path, before

    def test_ephemeral_rc_survives_exec_preserves_startup_aliases_and_hooks(self):
        p, path, before = self.launch("alias zz='printf STARTUP_ALIAS'; ff(){ return 23; }; PROMPT_COMMAND='printf STARTUP_HOOK'; printf RC_FINISHED")
        p.expect_exact(PROMPT)
        initial = p.before
        self.assertEqual([f[0] for f in frames(initial)], [b'H', b'R'])
        self.assertLess(initial.index(b'RC_FINISHED'), initial.index(b';H;'))
        self.assertLess(initial.index(b'STARTUP_HOOK'), initial.index(b';R;'))
        self.assertNotIn(b'LOGIN_PROFILE_SHOULD_NOT_RUN', initial)
        p.sendline(b'zz; ff # xyz-action:first')
        p.expect_exact(PROMPT)
        result = p.before
        self.assertIn(b'STARTUP_ALIAS', result)
        self.assertEqual([f[0] for f in frames(result)], [b'B', b'E', b'R'] if HAS_PS0 else [b'G', b'R'])
        if HAS_PS0:
            self.assertEqual(frames(result)[1][3], b'23')
        self.assertEqual((path / '.bashrc').read_bytes(), before)
        self.assertEqual(sorted(f.name for f in path.iterdir()), ['.bash_profile', '.bashrc'])

    def test_bash_history_default_after_rc_is_observed_not_assigned(self):
        for setting in ['', 'unset HISTSIZE']:
            with self.subTest(setting=setting):
                p, path, _ = self.launch()
                # This separate launch exercises truly minimal startup settings.
                p.close(force=True)
                (path / '.bashrc').write_text("HISTFILE=; PS1='XYZ_PTY> '; " + setting + "\n")
                p = pexpect.spawn('/bin/sh', ['-c', STARTUP],
                    env=dict(os.environ, HOME=str(path), SHELL=BASH),
                    encoding=None, timeout=5, echo=False)
                self.addCleanup(p.close, force=True)
                p.expect_exact(PROMPT)
                self.assertEqual([f[0] for f in frames(p.before)], [b'H', b'R'])
                p.sendline(b"printf 'HISTORY_LIMIT:%s\\n' \"$HISTSIZE\"")
                p.expect_exact(PROMPT)
                self.assertIn(b'HISTORY_LIMIT:500', p.before)
                self.assertEqual([f[0] for f in frames(p.before)], [b'B', b'E', b'R'] if HAS_PS0 else [b'G', b'R'])

    def test_absent_rc_uses_normal_bash_defaults(self):
        p, path, _ = self.launch()
        p.close(force=True)
        (path / '.bashrc').unlink()
        p = pexpect.spawn('/bin/sh', ['-c', STARTUP],
            env=dict(os.environ, HOME=str(path), SHELL=BASH, HISTFILE=''),
            encoding=None, timeout=5, echo=False)
        self.addCleanup(p.close, force=True)
        p.expect(FRAME)
        self.assertTrue(p.match.group(1).startswith(b'H;'))
        self.assertNotIn(b'LOGIN_PROFILE_SHOULD_NOT_RUN', p.before)
        p.expect(FRAME)
        self.assertTrue(p.match.group(1).startswith(b'R;0;'))

    def test_explicit_history_limits_are_never_overridden(self):
        for value in ['0', '', 'invalid']:
            with self.subTest(value=value):
                p, _, _ = self.launch('HISTSIZE=' + shlex.quote(value))
                p.expect_exact(PROMPT)
                self.assertEqual(frames(p.before), [[b'U', b'unsupported_options']])
                p.sendline(b"printf 'HISTORY_LIMIT:<%s>\\n' \"$HISTSIZE\"")
                p.expect_exact(PROMPT)
                self.assertIn(('HISTORY_LIMIT:<' + value + '>').encode(), p.before)
                self.assertEqual(frames(p.before), [])

    def test_startup_password_read_gets_no_automatic_input(self):
        p, _, _ = self.launch('read -r -s -p STARTUP_PASSWORD answer; [[ $answer == SYNTHETIC_SECRET ]] && printf STARTUP_ACCEPTED')
        p.expect_exact(b'STARTUP_PASSWORD')
        self.assertEqual(frames(p.before), [])
        self.assertEqual(p.expect([pexpect.TIMEOUT, pexpect.EOF], timeout=0.25), 0)
        self.assertEqual(frames(p.before), [])
        p.sendline(b'SYNTHETIC_SECRET')  # Explicit human fixture input only.
        p.expect_exact(PROMPT)
        self.assertIn(b'STARTUP_ACCEPTED', p.before)
        self.assertNotIn(b'SYNTHETIC_SECRET', p.before)
        self.assertEqual([f[0] for f in frames(p.before)], [b'H', b'R'])

    def test_startup_foreground_program_is_not_injected_and_disconnect_is_safe(self):
        helper = 'import sys; print("STARTUP_PROGRAM", flush=True); value=sys.stdin.readline(); print("RECEIVED:"+value, flush=True)'
        p, _, _ = self.launch(shlex.quote(sys.executable) + ' -c ' + shlex.quote(helper))
        p.expect_exact(b'STARTUP_PROGRAM\r\n')
        self.assertEqual(frames(p.before), [])
        self.assertEqual(p.expect([pexpect.TIMEOUT, pexpect.EOF], timeout=0.25), 0)
        p.close(force=True)
        self.assertFalse(p.isalive())

    def test_unsupported_account_shell_remains_manual_on_same_pty(self):
        p, _, _ = self.launch(account_shell='/bin/sh')
        p.expect(FRAME)
        self.assertEqual(p.match.group(1), b'U;unsupported_bash')
        p.sendline(b"printf 'MANUAL_%s\\n' WORKS")
        p.expect_exact(b'MANUAL_WORKS\r\n')
        self.assertEqual(frames(p.before), [])

    def test_unsupported_startup_settings_preserve_manual_shell(self):
        p, _, _ = self.launch("trap 'true' DEBUG; alias zz='printf PRESERVED_ALIAS'")
        p.expect_exact(PROMPT)
        self.assertEqual(frames(p.before), [[b'U', b'unsupported_trap']])
        p.sendline(b'zz')
        p.expect_exact(PROMPT)
        self.assertIn(b'PRESERVED_ALIAS', p.before)
        self.assertEqual(frames(p.before), [])

ALL_FRAME = re.compile(rb'\x1b\]777;xyzlog;[23];' + NONCE.encode() + rb';([^\x07]*)\x07')


def action_frames(output):
    return [body.split(b';') for body in ALL_FRAME.findall(output)]


def envelope(action_id, sequence, command):
    return subprocess.check_output([
        'cargo', 'run', '-p', 'xyz-core', '--locked', '--quiet', '--example',
        'shell_log_bootstrap', '--', '--envelope', action_id, str(sequence), command,
    ], cwd=ROOT)


class ExplicitActionEnvelopeContract(unittest.TestCase):
    launch = ControlledStartupContract.launch

    def prepared(self, rc=''):
        process, _, _ = self.launch(rc)
        process.expect_exact(PROMPT)
        self.assertEqual([f[0] for f in action_frames(process.before)], [b'H', b'R'], process.before)
        return process

    def action(self, process, command, sequence, code=0):
        action_id = f'fixture-action-{sequence}'
        transport = envelope(action_id, sequence, command)
        self.assertNotIn(b'\n', transport.rstrip(b'\n'))
        process.sendline(transport.rstrip(b'\n'))
        process.expect_exact(PROMPT)
        output = process.before
        events = action_frames(output)
        self.assertEqual([f[0] for f in events], [b'A', b'Z', b'R'], output)
        begin, end, ready = events
        self.assertEqual(begin[1], str(sequence).encode())
        self.assertEqual(end[1], begin[1])
        self.assertEqual(ready[1], begin[1])
        self.assertEqual(begin[3:7], end[3:7])
        self.assertEqual(begin[3], b'fixture-session')
        self.assertEqual(begin[4], b'1')
        self.assertEqual(begin[5], action_id.encode())
        self.assertEqual(begin[6], hashlib.sha256(command.encode()).hexdigest().encode())
        self.assertEqual(base64.b64decode(begin[8]), command.encode())
        self.assertEqual(end[7], str(code).encode())
        # Exactly one original command, no B/E records of wrapper internals.
        self.assertNotIn(b'B', [f[0] for f in events])
        self.assertNotIn(b'E', [f[0] for f in events])
        return output, begin

    def test_ai_simple_status_and_persistent_cwd_export(self):
        p = self.prepared()
        with tempfile.TemporaryDirectory(prefix='xyz-ai-cwd-') as directory:
            self.action(p, 'false', 1, 1)
            self.action(p, 'true', 2)
            self.action(p, 'cd -- ' + shlex.quote(directory), 3)
            output, begin = self.action(p, 'pwd -P', 4)
            self.assertIn(directory.encode(), output)
            self.assertEqual(base64.b64decode(begin[7]), directory.encode())
            self.action(p, 'export XYZ_FIXTURE_STATE=preserved', 5)
            output, _ = self.action(p, 'printenv XYZ_FIXTURE_STATE', 6)
            self.assertIn(b'preserved', output)

    def test_ai_literals_are_never_evaluated_before_payload_eval(self):
        p = self.prepared()
        literals = ["single'quote", '$HOME', '`printf INJECTED`', r'a\b', '中文🙂', 'a|b;&&', '']
        command = "printf '<%s>\\n' " + ' '.join(shlex.quote(value) for value in literals)
        output, _ = self.action(p, command, 1)
        for value in literals:
            self.assertIn(('<' + value + '>').encode(), output)
        self.assertNotIn(b'<INJECTED>', output)

    def test_ai_pipeline_respects_existing_pipefail_and_lists_keep_one_boundary(self):
        for pipefail in [False, True]:
            with self.subTest(pipefail=pipefail):
                p = self.prepared('set -o pipefail' if pipefail else '')
                self.action(p, 'false | true', 1, 1 if pipefail else 0)
                output, _ = self.action(p, "printf '%s\\n' alpha beta | grep beta", 2)
                self.assertIn(b'beta', output)
                command = 'false && printf MUST_NOT_RUN || printf FALLBACK; printf LAST'
                output, _ = self.action(p, command, 3)
                self.assertNotIn(b'MUST_NOT_RUN', output)
                self.assertIn(b'FALLBACKLAST', output)
                p.close(force=True)

    def test_ai_static_redirects_modify_only_controlled_temporary_files(self):
        p = self.prepared()
        with tempfile.TemporaryDirectory(prefix='xyz-ai-redirection-') as directory:
            target = pathlib.Path(directory) / 'literal file.txt'
            path = shlex.quote(str(target))
            self.action(p, "printf '%s' first > " + path, 1)
            self.assertEqual(target.read_text(), 'first')
            self.action(p, "printf '%s' second >> " + path, 2)
            self.assertEqual(target.read_text(), 'firstsecond')
            output, _ = self.action(p, 'cat < ' + path, 3)
            self.assertIn(b'firstsecond', output)

    def test_ai_interrupt_records_130_once_and_waits_for_real_ready(self):
        p = self.prepared()
        helper = 'import signal,time; signal.signal(signal.SIGINT, signal.SIG_DFL); print("AI_INTERRUPT_READY", flush=True); time.sleep(30)'
        command = shlex.quote(sys.executable) + ' -c ' + shlex.quote(helper)
        p.sendline(envelope('fixture-interrupt', 1, command).rstrip(b'\n'))
        p.expect_exact(b'AI_INTERRUPT_READY\r\n')
        start = action_frames(p.before)
        self.assertEqual([f[0] for f in start], [b'A'])
        p.sendcontrol('c')
        p.expect_exact(PROMPT)
        end = action_frames(p.before)
        self.assertEqual([f[0] for f in end], [b'Z', b'R'])
        self.assertEqual(end[0][7], b'130')
        self.assertEqual(end[0][1], start[0][1])
        self.assertEqual(end[1][1], start[0][1])

    def test_manual_observation_gap_does_not_remove_explicit_ai_capability(self):
        for command in ["(pwd)", "cat <<'END'\nhello\nEND", "sleep 2 &"]:
            with self.subTest(command=command):
                p = self.prepared()
                p.sendline(command.encode())
                p.expect_exact(PROMPT)
                events = action_frames(p.before)
                self.assertEqual([f[0] for f in events],
                                 [b'B', b'E', b'R'] if HAS_PS0 else [b'G', b'R'], p.before)
                next_sequence = int(events[-1][1]) + 1
                output, _ = self.action(p, 'printf AI_AFTER_MANUAL_GAP', next_sequence)
                self.assertIn(b'AI_AFTER_MANUAL_GAP', output)

    def test_ai_preserves_parent_status_for_raw_manual_probe_and_blank_enter(self):
        p = self.prepared()
        p.sendline(b'')
        p.expect_exact(PROMPT)
        self.assertEqual([f[0] for f in action_frames(p.before)], [b'R'])
        sequence = 1
        for command, code in [('false', 1), ('true', 0)]:
            self.action(p, command, sequence, code)
            p.sendline(b'')
            p.expect_exact(PROMPT)
            self.assertEqual([f[0] for f in action_frames(p.before)], [b'R'])
            # Deliberately raw manual probe, outside the approved AI grammar.
            p.sendline(b'printf "PARENT_STATUS:%s" "$?"')
            p.expect_exact(PROMPT)
            self.assertIn(('PARENT_STATUS:' + str(code)).encode(), p.before)
            events = action_frames(p.before)
            self.assertEqual([f[0] for f in events],
                             [b'B', b'E', b'R'] if HAS_PS0 else [b'G', b'R'])
            sequence = int(events[-1][1]) + 1

    def test_maximum_original_payload_survives_real_pty_transport(self):
        p = self.prepared()
        prefix = "printf '%s' '"
        literal = 'x' * (8192 - len(prefix.encode()) - 1)
        command = prefix + literal + "'"
        self.assertEqual(len(command.encode()), 8192)
        output, _ = self.action(p, command, 1)
        self.assertIn(literal.encode(), output)

    def test_ai_parent_death_never_fabricates_end_or_ready(self):
        p = self.prepared()
        helper = 'import os,signal; os.kill(os.getppid(),signal.SIGKILL)'
        command = shlex.quote(sys.executable) + ' -c ' + shlex.quote(helper)
        p.sendline(envelope('fixture-parent-death', 1, command).rstrip(b'\n'))
        p.expect(pexpect.EOF)
        self.assertEqual([f[0] for f in action_frames(p.before)], [b'A'])

    def test_custom_job_control_handlers_are_not_normalized_as_defaults(self):
        helpers = ROOT / 'crates/xyz-core/src/shell_action_helpers.bash'
        for signal in ['TSTP', 'TTIN', 'TTOU']:
            with self.subTest(signal=signal):
                snapshot = "trap -- 'printf CUSTOM_HANDLER' SIG" + signal
                normalized = subprocess.check_output(
                    [BASH, '--noprofile', '--norc', '-c',
                     'source "$1"; __xyz_norm_traps "$2"', 'fixture', str(helpers), snapshot])
                self.assertEqual(normalized.rstrip(b'\n'), snapshot.encode())
                setup = ("trap 'printf CUSTOM_HANDLER' " + signal +
                         "; printf TRAPS_BEGIN; builtin trap -p; printf TRAPS_END")
                p, path, before = self.launch(setup)
                p.expect_exact(PROMPT)
                initial = p.before
                observed = initial.split(b'TRAPS_BEGIN', 1)[1].split(b'TRAPS_END', 1)[0]
                if HAS_PS0:
                    self.assertIn(b'CUSTOM_HANDLER', observed)
                    self.assertEqual(frames(initial), [[b'U', b'unsupported_trap']], initial)
                else:
                    # Native interactive Bash4.2 discards these job-control
                    # handlers before integration observes them. Verify that
                    # fact, rather than pretending an absent handler survives.
                    self.assertNotIn(b'CUSTOM_HANDLER', observed)
                    self.assertIn(("trap -- '' SIG" + signal).encode(), observed)
                    self.assertEqual([f[0] for f in frames(initial)], [b'H', b'R'], initial)
                self.assertEqual((path / '.bashrc').read_bytes(), before)
                p.sendline(b'printf MANUAL_REMAINS_AVAILABLE')
                p.expect_exact(PROMPT)
                self.assertIn(b'MANUAL_REMAINS_AVAILABLE', p.before)
                self.assertEqual([f[0] for f in action_frames(p.before)],
                                 [] if HAS_PS0 else [b'G', b'R'])

    def test_surviving_trap_mutation_after_ready_blocks_ai(self):
        p = self.prepared()
        p.sendline(b"trap 'printf CUSTOM_ERR_HANDLER' ERR; builtin trap -p ERR")
        p.expect_exact(PROMPT)
        self.assertIn(b'CUSTOM_ERR_HANDLER', p.before)
        events = action_frames(p.before)
        self.assertEqual([f[0] for f in events], [b'B', b'E', b'G'] if HAS_PS0 else [b'G', b'G'], p.before)
        self.assertEqual(events[-1][-1], b'hooks_changed')
        p.sendline(envelope('blocked-after-trap', 2, 'printf AI_MUST_NOT_RUN').rstrip(b'\n'))
        p.expect_exact(PROMPT)
        self.assertNotIn(b'AI_MUST_NOT_RUN', p.before)
        self.assertFalse(any(f[0] in [b'A', b'Z', b'R'] for f in action_frames(p.before)))

    def test_shadowed_control_primitives_are_rejected_without_invoking_them(self):
        for name in ['local', 'return']:
            for kind in ['alias', 'function']:
                with self.subTest(name=name, kind=kind):
                    setup = ("alias " + name + "='printf CONTROL_SHADOW_INVOKED'" if kind == 'alias'
                             else name + "(){ printf CONTROL_SHADOW_INVOKED; }")
                    p, _, _ = self.launch(setup)
                    p.expect_exact(PROMPT)
                    self.assertEqual(frames(p.before), [[b'U', b'control_conflict']], p.before)
                    self.assertNotIn(b'CONTROL_SHADOW_INVOKED', p.before)

if __name__ == '__main__':
    if len(BOOTSTRAP.encode()) >= 16384:
        raise SystemExit('bootstrap exceeds one PTY input frame')
    unittest.main(verbosity=2)
