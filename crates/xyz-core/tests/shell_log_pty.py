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
import os
import time
import pexpect

ROOT = pathlib.Path(__file__).resolve().parents[3]
NONCE = '0123456789abcdef0123456789abcdef'
SOURCE = (ROOT / 'crates/xyz-core/src/shell_log_bootstrap.bash').read_text().replace('@NONCE@', NONCE)
BOOTSTRAP = subprocess.check_output(
    ['cargo', 'run', '-p', 'xyz-core', '--locked', '--quiet', '--example', 'shell_log_bootstrap'], cwd=ROOT,
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
        s.p.sendline(b'sleep 30 # xyz-action:interrupt')
        s.p.expect(rb';B;[0-9]+;')
        # PS0 emits Begin before the job has necessarily taken the foreground.
        # Synchronize with real PTY job control, never use output silence/sleep
        # as evidence that the application has started receiving terminal input.
        deadline = time.monotonic() + 5
        while os.tcgetpgrp(s.p.child_fd) == s.p.pid:
            self.assertLess(time.monotonic(), deadline, 'sleep did not become the foreground job')
            time.sleep(0.005)
        s.p.sendcontrol('c')
        s.p.expect_exact(PROMPT)
        events = frames(s.p.before)
        self.assertEqual([f[0] for f in events], [b'E', b'R'])
        self.assertEqual(events[0][3], b'130')

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


if __name__ == '__main__':
    if len(BOOTSTRAP.encode()) >= 16384:
        raise SystemExit('bootstrap exceeds one PTY input frame')
    unittest.main(verbosity=2)
