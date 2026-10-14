#!/usr/bin/env python3
"""Optional real-PTY proof: python crates/xyz-core/tests/shell_log_pty.py
Requires Cargo/Rust, Python pexpect, Bash >=4.4, POSIX sh, base64 and tr. Uses only synthetic
inputs, no network, no persistent shell startup files, and no user's history.
"""
import pexpect,re,base64,json,pathlib,subprocess
repo = pathlib.Path(__file__).resolve().parents[3]
command = subprocess.check_output(
    ['cargo', 'run', '-p', 'xyz-core', '--locked', '--quiet', '--example', 'shell_log_bootstrap'],
    cwd=repo,
).decode('utf-8')
assert len(command.encode())<16384

p=pexpect.spawn('/bin/sh',['-i'],encoding=None,timeout=5)
p.sendline(b"PS1='PARENT> '");p.expect_exact(b'PARENT> ');p.send(command.encode());p.expect(rb'\x1b\]777;xyzlog;1;[0-9a-f]+;H;[^\x07]+\x07');p.expect(rb'\[xyzaishell log\].*?[$#] ')
results=[]
def run(c):
 p.sendline(c);p.expect(rb'\[xyzaishell log\].*?[$#] ');return p.before
# These regexes only drive this test fixture, never production capture.

def frames(out):
 result=[]
 for frame in re.findall(rb'\x1b\]777;xyzlog;1;[^\x07]*\x07',out):
  f=frame[2:-1].split(b';')
  if f[4]==b'B': f[7:]=[base64.b64decode(x) for x in f[7:]]
  result.append(f)
 return result
out=run(b'pwd'); start=frames(out)[0][7]
for c in [b'printf a | cat', b'cd /tmp; pwd', b'false', b'(cd /; pwd)', b"printf '%s\\n' 'one\ntwo'", b"cat <<'END'\nhello\nEND"]:
 f=frames(run(c)); assert f[0][8].rstrip(b'\n')==c; results.append([c.decode(),f[0][7].decode(),f[-1][-1].decode()])
assert results[1][1]==start.decode()
assert results[2][2]=='1'
p.sendline(b'read -s -p "SECRET> " secret');p.expect_exact(b'SECRET> '); a=p.before;p.sendline(b'SYNTHETIC_PASSWORD_NEVER_LOG');p.expect(rb'\[xyzaishell log\].*?[$#] ');a+=p.before
assert b'SYNTHETIC_PASSWORD_NEVER_LOG' not in a
assert len(frames(a))==2
run(b'set +o history')
assert frames(run(b'echo OFF'))[0][4]==b'G'
assert frames(run(b'set -o history'))[0][4]==b'G'
assert frames(run(b'echo RESTORED'))[0][8]==b'echo RESTORED'
# Raw invalid UTF-8 is encoded losslessly by the shell; Rust rejects it as Gap.
f=frames(run(b"printf '%s' '\xff'")); assert b'\xff' in f[0][8]
# Control bytes are protocol base64 data, never raw framing bytes.
f=frames(run(b"printf '\\033]evil\\007'"));assert f[0][8]==b"printf '\\033]evil\\007'"
# Uninstrumented nested shell is one outer command, no inner records.
p.sendline(b'bash --noprofile --norc -i');p.expect(rb'bash-[0-9.]+[$#] ');nested_begin=p.before
p.sendline(b'echo INNER');p.expect(rb'bash-[0-9.]+[$#] ');assert not frames(p.before)
p.sendline(b'exit');p.expect(rb'\[xyzaishell log\].*?[$#] ');assert len(frames(nested_begin+p.before))==2
p.sendline(b'exit');p.expect_exact(b'PARENT> ');assert [f[4] for f in frames(p.before)]==[b'B',b'X']
p.sendline(b'pwd');p.expect_exact(b'PARENT> ');assert start in p.before
p.close(force=True)
print(json.dumps(results,indent=2)); print('PASS: generated bootstrap via POSIX sh, complete shell texts, pre-cd cwd, status, read secret, history gaps, invalid UTF-8, controls, nested shell, exit restores parent cwd')
