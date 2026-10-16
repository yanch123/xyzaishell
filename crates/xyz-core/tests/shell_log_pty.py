#!/usr/bin/env python3
"""Real CURRENT-Bash PTY contract tests (synthetic shells only).
Run: python3 crates/xyz-core/tests/shell_log_pty.py
Requires Rust/Cargo, Bash 5.1 or 5.2, and Python pexpect. No network, rc changes, or user history.
This fixture configures its own test shell; production preserves/rejects settings.
"""
import base64
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
NONCE = '0123456789abcdef0123456789abcdef'
SOURCE = (ROOT / 'crates/xyz-core/src/shell_log_bootstrap.bash').read_text().replace('@NONCE@', NONCE)
BOOTSTRAP = subprocess.check_output(
    ['cargo', 'run', '-p', 'xyz-core', '--locked', '--quiet', '--example', 'shell_log_bootstrap'], cwd=ROOT,
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
        self.p = pexpect.spawn('/bin/bash', ['--noprofile', '--norc', '-i'], encoding=None, timeout=5, echo=False)
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
        self.assertIn(b'PREEXISTING0', output)
        self.assertIn(b'PC:1', output)
        self.assertLess(output.index(b';E;'), output.index(b'PC:1'))
        self.assertLess(output.index(b'PC:1'), output.index(b';R;'))
        output = s.run('printf "STATUS:%s\\n" "$?"; test "$$" = "$saved_pid"; zz; ff')
        self.assert_lifecycle(output, 'printf "STATUS:%s\\n" "$?"; test "$$" = "$saved_pid"; zz; ff', 23)
        self.assertIn(b'STATUS:1', output)
        self.assertIn(b'ALIAS_OK', output)
        names = base64.b64decode(frames(output)[2][3]).splitlines()
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
        self.assertEqual([f[0] for f in frames(s.run(' hidden_manual_command'))][0], b'G')

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
        self.assertEqual([f[0] for f in started], [b'B'])
        self.assertEqual(base64.b64decode(started[0][4]).rstrip(b'\n'), command.encode())
        s.p.sendcontrol('c')
        s.p.expect_exact(PROMPT)
        events = frames(s.p.before)
        self.assertEqual([f[0] for f in events], [b'E', b'R'])
        self.assertEqual(events[0][3], b'130')
        self.assertEqual(events[0][1], started[0][1])
        self.assertEqual(events[1][1], started[0][1])

    def test_array_prompt_hooks_keep_order_and_original_values_and_restore(self):
        s = self.shell("PS0='ORIGINAL0'; PROMPT_COMMAND=('builtin printf \"ONE:%s\\n\" \"$?\"' 'builtin printf \"TWO:%s\\n\" \"$?\"'); before=$(declare -p PROMPT_COMMAND PS0); trap 'printf EXIT_OK' EXIT")
        s.install()
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
        for command, code in [('printf a | cat', 0), ('set -o pipefail; false | true', 1), ("printf '%s\\n' 'one\ntwo'", 0), ("cat <<'END'\nhello\nEND", 0), ('(cd /; pwd)', 0), ('sleep 2 &', 0)]:
            self.assert_lifecycle(s.run(command), command, code)
        events = self.assert_lifecycle(s.run('cd /tmp; pwd'), 'cd /tmp; pwd', 0)
        self.assertNotEqual(base64.b64decode(events[0][3]), b'/tmp')
        # Foreground completion is explicitly NOT background-job completion.
        self.assertIn(b'Running', s.run('jobs'))

    def test_interactive_input_not_recorded_and_ready_after_blocking_prompt_hook(self):
        s = self.shell("PROMPT_COMMAND='if [[ ${block_prompt-} == yes ]]; then read -r -p HOOK_WAIT value; block_prompt=no; fi'")
        s.install()
        s.p.sendline(b'block_prompt=yes; false')
        s.p.expect_exact(b'HOOK_WAIT')
        output = s.p.before
        self.assertEqual([f[0] for f in frames(output)], [b'B', b'E'])
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
        for setup in ["trap 'true' ERR", "trap 'read -r value' WINCH", "trap 'read -r value' ALRM", "PS1='$(printf dynamic)XYZ_PTY> '", "shopt -u promptvars", "shopt -u interactive_comments", "HISTIGNORE='*'"]:
            with self.subTest(setup=setup):
                s = self.shell("PS0='KEPT'; PROMPT_COMMAND='true'; " + setup + "; before=$(declare -p PS0 PROMPT_COMMAND); old_traps=$(trap -p ERR)")
                output = s.install()
                self.assertEqual([f[0] for f in frames(output)], [b'U'])
                output = s.run('test "$before" = "$(declare -p PS0 PROMPT_COMMAND)" && test "$old_traps" = "$(trap -p ERR)"; printf "UNCHANGED:%s\\n" "$?"')
                self.assertIn(b'UNCHANGED:0', output)
                self.assertEqual(frames(output), [])
                s.close()

    def test_changed_hooks_emit_gap_never_ready_and_cleanup_does_not_overwrite(self):
        s = self.shell(); s.install()
        output = s.run("PS0='USER_EDIT'")
        self.assertEqual([f[0] for f in frames(output)], [b'B', b'E', b'G'])
        self.assertNotIn(b';R;', s.run('printf STILL_MANUAL'))
        s.run('__xyz_uninstall')
        self.assertIn(b'USER_EDIT', s.run('declare -p PS0'))

    def test_runtime_async_trap_and_disabled_comments_stop_before_next_ready(self):
        for command in ["trap 'read -r value' WINCH", 'shopt -u interactive_comments']:
            with self.subTest(command=command):
                s = self.shell(); s.install()
                output = s.run(command)
                self.assertEqual([f[0] for f in frames(output)], [b'B', b'E', b'G'])
                self.assertNotIn(b';R;', output)
                s.close()

    def test_history_change_does_not_reuse_previous_command(self):
        s = self.shell(); s.install()
        self.assertEqual([f[0] for f in frames(s.run('set +o history'))], [b'B', b'E', b'G'])
        self.assertEqual(frames(s.run('echo PRIVATE_AFTER_GAP')), [])

    def test_nested_shell_is_one_unfinished_outer_command_until_return(self):
        s = self.shell(); s.install()
        s.p.sendline(b'bash --noprofile --norc -i')
        s.p.expect(rb'bash-[0-9.]+[$#] ')
        self.assertEqual([f[0] for f in frames(s.p.before)], [b'B'])
        s.p.sendline(b'echo INNER')
        s.p.expect(rb'bash-[0-9.]+[$#] ')
        self.assertEqual(frames(s.p.before), [])
        s.p.sendline(b'exit')
        s.p.expect_exact(PROMPT)
        self.assertEqual([f[0] for f in frames(s.p.before)], [b'E', b'R'])

    def test_exit_and_exec_do_not_fabricate_completion(self):
        for command in ['exit 7', 'exec /bin/true']:
            with self.subTest(command=command):
                s = self.shell("trap 'printf PRESERVED_EXIT_TRAP' EXIT")
                s.install()
                s.p.sendline(command.encode())
                s.p.expect(pexpect.EOF)
                self.assertEqual([f[0] for f in frames(s.p.before)], [b'B'])
                if command.startswith('exit'):
                    self.assertIn(b'PRESERVED_EXIT_TRAP', s.p.before)



class ControlledStartupContract(unittest.TestCase):
    def launch(self, rc='', account_shell='/bin/bash'):
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
        self.assertEqual([f[0] for f in frames(result)], [b'B', b'E', b'R'])
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
                    env=dict(os.environ, HOME=str(path), SHELL='/bin/bash'),
                    encoding=None, timeout=5, echo=False)
                self.addCleanup(p.close, force=True)
                p.expect_exact(PROMPT)
                self.assertEqual([f[0] for f in frames(p.before)], [b'H', b'R'])
                p.sendline(b"printf 'HISTORY_LIMIT:%s\\n' \"$HISTSIZE\"")
                p.expect_exact(PROMPT)
                self.assertIn(b'HISTORY_LIMIT:500', p.before)
                self.assertEqual([f[0] for f in frames(p.before)], [b'B', b'E', b'R'])

    def test_absent_rc_uses_normal_bash_defaults(self):
        p, path, _ = self.launch()
        p.close(force=True)
        (path / '.bashrc').unlink()
        p = pexpect.spawn('/bin/sh', ['-c', STARTUP],
            env=dict(os.environ, HOME=str(path), SHELL='/bin/bash', HISTFILE=''),
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

if __name__ == '__main__':
    if len(BOOTSTRAP.encode()) >= 16384:
        raise SystemExit('bootstrap exceeds one PTY input frame')
    unittest.main(verbosity=2)
