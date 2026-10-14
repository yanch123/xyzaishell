# Ephemeral, opt-in recording shell. No startup files or remote history writes.
unset HISTFILE
HISTCONTROL=; HISTIGNORE=; HISTSIZE=10000
set -o history
shopt -s cmdhist lithist promptvars
builtin history -c
__xyz_seq=0
__xyz_nonce='@NONCE@'
__xyz_empty=
PS1='[xyzaishell log] \u@\h:\w\$ '
PS2='> '
__xyz_b64() { builtin printf '%s' "$1" | command base64 | command tr -d '\n'; }
__xyz_begin() {
 local LC_ALL=C h cmd cwd now
 builtin printf -v now '%(%s)T' -1
 if ! [[ -o history ]] || [[ -n $HISTCONTROL || -n $HISTIGNORE || $HISTSIZE != 10000 ]] || ! builtin shopt -q cmdhist lithist; then
   builtin printf 'G;%s;history_settings' "$__xyz_seq"; return
 fi
 # Sentinel preserves trailing newlines, including heredoc terminators.
 h=$(HISTTIMEFORMAT= builtin history 1; builtin printf x)
 h=${h%x}; h=${h%$'\n'}
 if [[ $h =~ ^[\ ]*[0-9]+[\ *][\ ] ]]; then cmd=${h:${#BASH_REMATCH[0]}}; else builtin printf 'G;%s;history_missing' "$__xyz_seq"; return; fi
 cwd=$(builtin pwd -P; builtin printf x)
 cwd=${cwd%x}; cwd=${cwd%$'\n'}
 if [[ -z $cwd || $cwd != /* ]]; then builtin printf 'G;%s;cwd_unavailable' "$__xyz_seq"; return; fi
 if (( ${#cmd} > 16384 || ${#cwd} > 4096 )); then builtin printf 'G;%s;oversize' "$__xyz_seq"; return; fi
 builtin printf 'B;%s;%s;%s;%s' "$__xyz_seq" "$now" "$(__xyz_b64 "$cwd")" "$(__xyz_b64 "$cmd")"
}
__xyz_end() {
 local code=$? now
 builtin printf -v now '%(%s)T' -1
 if (( __xyz_seq > 0 )); then builtin printf '\033]777;xyzlog;1;%s;E;%s;%s;%s\007' "$__xyz_nonce" "$__xyz_seq" "$now" "$code"; fi
}
# Increment in the parent expansion, not inside the command substitution.
PS0='${__xyz_empty:$((__xyz_seq+=1)):0}\e]777;xyzlog;1;@NONCE@;$(__xyz_begin)\a'
PROMPT_COMMAND=__xyz_end
# This EXIT trap belongs only to this disposable child, never to the parent shell.
__xyz_closed() { builtin printf '\033]777;xyzlog;1;%s;X\007' "$__xyz_nonce"; }
builtin trap '__xyz_closed' EXIT
# Do not accidentally instrument a nested shell through inherited export flags.
builtin export -n PS0 PROMPT_COMMAND __xyz_nonce __xyz_seq __xyz_empty
builtin printf '\033]777;xyzlog;1;%s;H;%s\007' "$__xyz_nonce" "$BASH_VERSION"
