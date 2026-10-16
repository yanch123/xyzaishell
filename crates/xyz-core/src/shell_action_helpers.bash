  __xyz_controls_ok() {
   builtin local name kind
   for name in local return builtin command eval printf trap history shopt declare compgen unset pwd base64 read type if then else fi '[['; do
    kind=$(builtin type -t "$name")
    [[ $kind != alias && $kind != function ]] || builtin return 1
   done
  }
  __xyz_norm_traps() {
   builtin local line
   if [[ ${BASH_VERSINFO[0]} != 4 ]]; then builtin printf '%s' "$1"; builtin return; fi
   while IFS= builtin read -r line; do
    case $line in "trap -- '' SIGTSTP"|"trap -- '' SIGTTIN"|"trap -- '' SIGTTOU"|'') ;; *) builtin printf '%s\n' "$line" ;; esac
   done <<< "$1"
  }
  __xyz_is_envelope() {
   builtin local h
   h=$(HISTTIMEFORMAT= builtin history 1; builtin printf x)
   h=${h%x}; h=${h%$'\n'}
   if [[ $h =~ ^[\ ]*[0-9]+[\ *][\ ] ]]; then h=${h:${#BASH_REMATCH[0]}}; else builtin return 1; fi
   [[ $h == 'if [[ ${__xyz_nonce-} == '* ]]
  }
  __xyz_manual_step() { if __xyz_is_envelope; then builtin printf 0; else builtin printf 1; fi; }
  __xyz_action_begin() {
   builtin local now cwd cwd64 actual64 LC_ALL=C
   if [[ $1 != "$__xyz_nonce" || $__xyz_failed != 0 || $__xyz_ai_active != 0 || $5 != "$((__xyz_seq+1))" ]] || ! __xyz_intact "$8"; then __xyz_gap; builtin return 1; fi
   if (( ${#__xyz_payload} == 0 || ${#__xyz_payload} > 8192 )); then __xyz_gap; builtin return 1; fi
   actual64=$(__xyz_b64 "$__xyz_payload") || { __xyz_gap; builtin return 1; }
   [[ $actual64 == "$7" ]] || { __xyz_gap; builtin return 1; }
   cwd=$(builtin pwd -P; builtin printf x); cwd=${cwd%x}; cwd=${cwd%$'\n'}
   if [[ $cwd != /* ]] || (( ${#cwd} > 4096 )); then __xyz_gap; builtin return 1; fi
   cwd64=$(__xyz_b64 "$cwd") || { __xyz_gap; builtin return 1; }
   __xyz_action_session=$2; __xyz_action_generation=$3; __xyz_action_id=$4; __xyz_seq=$5; __xyz_action_digest=$6
   __xyz_ai_active=1; __xyz_at_prompt=0
   builtin printf -v now '%(%s)T' -1
   builtin printf '\033]777;xyzlog;3;%s;A;%s;%s;%s;%s;%s;%s;%s;%s\007' "$__xyz_nonce" "$__xyz_seq" "$now" "$2" "$3" "$4" "$6" "$cwd64" "$actual64"
  }
  __xyz_action_end() {
   builtin local code=$1 now
   if (( __xyz_ai_active != 1 || __xyz_failed != 0 )); then __xyz_gap; builtin return "$code"; fi
   builtin printf -v now '%(%s)T' -1
   builtin printf '\033]777;xyzlog;3;%s;Z;%s;%s;%s;%s;%s;%s;%s\007' "$__xyz_nonce" "$__xyz_seq" "$now" "$__xyz_action_session" "$__xyz_action_generation" "$__xyz_action_id" "$__xyz_action_digest" "$code"
   __xyz_ai_active=0; __xyz_ended=$__xyz_seq
   __xyz_last_history=$(HISTTIMEFORMAT= builtin history 1; builtin printf x)
   builtin return "$code"
  }
