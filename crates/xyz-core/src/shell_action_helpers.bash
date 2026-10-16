  __xyz_controls_ok() {
   builtin local name kind
   for name in local return builtin command eval printf trap history shopt declare compgen unset pwd base64 stty sha256sum read type if then else fi '[['; do
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
   [[ $h == 'if __xyz_receive '* ]]
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
  __xyz_drain_receiver() {
   builtin local discarded count=0
   while (( count < 131072 )) && IFS= builtin read -r -t 0; do
    IFS= builtin read -r -s -t 0.01 -n 1 discarded || builtin return 1
    ((count+=1))
   done
   (( count < 131072 ))
  }
  __xyz_restore_tty() {
   builtin local restored=0
   if [[ -n ${__xyz_tty_saved-} ]]; then
    if command stty "$__xyz_tty_saved" < /dev/tty; then __xyz_tty_saved=; else restored=1; fi
   fi
   if [[ ${__xyz_receive_int_owned-0} == 1 ]]; then
    builtin trap - INT
    __xyz_receive_int_owned=0
   fi
   builtin return "$restored"
  }
  __xyz_receive() {
   builtin local frame record hex= bit byte i j value header rest actual code chunk deadline now attempts=0 LC_ALL=C
   if [[ $__xyz_failed != 0 || $__xyz_ai_active != 0 || $4 != "$((__xyz_seq+1))" ]] || ! __xyz_intact "$6"; then __xyz_gap; builtin return 1; fi
   __xyz_receiving=1
   __xyz_tty_saved=$(command stty -g < /dev/tty) || { __xyz_gap receiver_failed; builtin return 1; }
   [[ -n $__xyz_tty_saved ]] || { __xyz_gap receiver_failed; builtin return 1; }
   __xyz_receive_cancelled=0; __xyz_receive_int_owned=1
   builtin trap '__xyz_receive_cancelled=1; builtin return 130' INT
   if ! command stty -echo -icanon noflsh min 1 time 0 < /dev/tty; then __xyz_restore_tty; __xyz_gap receiver_failed; builtin return 1; fi
   builtin printf '\033]777;xyzlog;3;%s;M;%s;%s;%s;%s;%s\007' "$__xyz_nonce" "$4" "$1" "$2" "$3" "$5"
   if [[ ${BASH_VERSINFO[0]} == 4 ]]; then
    IFS= builtin read -r -s -t 15 -n 131072 frame
    code=$?
   else
    frame=; builtin printf -v deadline '%(%s)T' -1; ((deadline+=15))
    while ((attempts < 60)); do
     ((attempts+=1)); builtin printf -v now '%(%s)T' -1
     if ((now >= deadline)); then code=142; builtin break; fi
     chunk=
     IFS= builtin read -r -s -t 0.25 -n "$((131072-${#frame}))" chunk
     code=$?; frame+=$chunk
     if ((code != 142)); then builtin break; fi
    done
   fi
   __xyz_restore_tty || { __xyz_gap receiver_failed; builtin return 1; }
   if (( __xyz_receive_cancelled != 0 )); then __xyz_drain_receiver; code=130; fi
   if (( code != 0 || ${#frame} < 9 || ${#frame} >= 131072 )) || [[ ${frame:0:1} != '#' || $frame == *[!\ #]* ]]; then __xyz_gap receiver_failed; builtin return 1; fi
   frame=${frame:1}
   if (( ${#frame} % 8 != 0 )); then __xyz_gap receiver_failed; builtin return 1; fi
   j=${#frame}
   for ((i=0; i<j; i+=8)); do
    bit=${frame:i:8}; bit=${bit//#/1}; bit=${bit// /0}
    value=$((2#$bit))
    if (( value < 32 || value == 127 )); then __xyz_gap receiver_failed; builtin return 1; fi
    builtin printf -v byte '\\x%02x' "$value"
    hex+=$byte
   done
   builtin printf -v record '%b' "$hex"
   header="v1|$__xyz_nonce|$1|$2|$3|$4|$5|"
   [[ $record == "$header"* ]] || { __xyz_gap receiver_failed; builtin return 1; }
   rest=${record#"$header"}
   [[ $rest == *'|'* ]] || { __xyz_gap receiver_failed; builtin return 1; }
   header=${rest%%|*}; __xyz_payload=${rest#*|}
   if [[ $header != "${#__xyz_payload}" ]] || (( ${#__xyz_payload} == 0 || ${#__xyz_payload} > 8192 )); then __xyz_gap receiver_failed; builtin return 1; fi
   actual=$(builtin printf '%s' "$__xyz_payload" | command sha256sum) || { __xyz_gap receiver_failed; builtin return 1; }
   [[ ${actual%% *} == "$5" ]] || { __xyz_gap receiver_failed; builtin return 1; }
   __xyz_action_begin "$__xyz_nonce" "$1" "$2" "$3" "$4" "$5" "$(__xyz_b64 "$__xyz_payload")" "$6" || builtin return 1
   __xyz_receiving=0
   builtin printf '[AI] %s\n' "$__xyz_payload"
  }
