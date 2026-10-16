# Temporary CURRENT-shell integration. Never source this at an unknown foreground.
# Bash 4.2 uses explicit AI envelopes and prompt-only limited manual observation.
# No DEBUG trap is installed: arbitrary manual grammar retains native semantics.
if [[ ${BASH_VERSINFO[0]-} != 4 || ${BASH_VERSINFO[1]-} != 2 || $- != *i* ]]; then
 builtin printf '\033]777;xyzlog;2;@NONCE@;U;unsupported_bash\007'
elif [[ -n $(builtin compgen -A variable __xyz_) || -n $(builtin compgen -A function __xyz_) || -n $(builtin compgen -A alias __xyz_) ]]; then
 builtin printf '\033]777;xyzlog;2;@NONCE@;U;namespace_conflict\007'
else
# @ACTION_HELPERS@
 __xyz_install() {
  if ! __xyz_controls_ok; then builtin printf '\033]777;xyzlog;2;@NONCE@;U;control_conflict\007'; builtin return; fi
  builtin local pc_decl name value
  if [[ $- == *e* || $- == *T* || $- == *a* || $- == *r* ]] || builtin shopt -q extdebug || [[ -o posix ]] || ! [[ -o history ]] || ! builtin shopt -q cmdhist promptvars interactive_comments || [[ ${HISTSIZE-0} -le 0 || -n ${HISTIGNORE-} || ${histchars-'!^#'} != '!^#' ]]; then
   builtin printf '\033]777;xyzlog;2;@NONCE@;U;unsupported_options\007'; builtin return
  fi
  if [[ ${HISTSIZE+x} != x && $(builtin declare -p HISTSIZE 2>/dev/null) =~ ^declare\ -[^\ ]*r ]]; then
   builtin printf '\033]777;xyzlog;2;@NONCE@;U;unsupported_options\007'; builtin return
  fi
  # Only an EXIT trap is supported. An async trap can read input between ready
  # and dispatch; DEBUG/RETURN/ERR also alter status/hook semantics. None changed.
  if [[ $(__xyz_norm_traps "$1") != "$(__xyz_norm_traps "$2")" ]]; then
   builtin printf '\033]777;xyzlog;2;@NONCE@;U;unsupported_trap\007'; builtin return
  fi
  # Prompt expansion happens AFTER PROMPT_COMMAND. Permit only non-executing
  # parameter forms, including Ubuntu's debian_chroot and native \$ suffix.
  # Reject octal escapes: they could synthesize a dollar/backtick after decoding.
  value=${PS1-}
  if [[ $value == *'$('* || $value == *'$['* || $value == *'`'* || $value =~ \\[0-7]{3} ]]; then
   builtin printf '\033]777;xyzlog;2;@NONCE@;U;dynamic_prompt\007'; builtin return
  fi
  while [[ $value == *'$'* ]]; do
   value=${value#*\$}
   if [[ $value == '{'* ]]; then
    name=${value#\{}; name=${name%%\}*}
    if ! [[ $name =~ ^[a-zA-Z_][a-zA-Z_0-9]*$ || $name =~ ^[a-zA-Z_][a-zA-Z_0-9]*:[+-][^\{\}]*$ ]] || [[ $value != *'}'* ]]; then
     builtin printf '\033]777;xyzlog;2;@NONCE@;U;dynamic_prompt\007'; builtin return
    fi
    value=${value#*\}}
   fi
  done
  pc_decl=$(builtin declare -p PROMPT_COMMAND 2>/dev/null)
  if [[ $pc_decl =~ ^declare\ -[^\ ]*[rxanA] ]]; then
   builtin printf '\033]777;xyzlog;2;@NONCE@;U;unsupported_hook_attributes\007'; builtin return
  fi
  command -v base64 >/dev/null && command -v stty >/dev/null && command -v sha256sum >/dev/null || {
   builtin printf '\033]777;xyzlog;2;@NONCE@;U;missing_tools\007'; builtin return
  }
  __xyz_nonce='@NONCE@'
  __xyz_seq=0
  __xyz_ended=0
  __xyz_empty=
  __xyz_failed=0
  __xyz_ai_active=0; __xyz_receiving=0; __xyz_tty_saved=
  __xyz_at_prompt=0
  __xyz_original_pc_decl=$pc_decl
  __xyz_original_ps1=${PS1-}
  __xyz_original_traps=$(__xyz_norm_traps "$1")
  __xyz_original_histcontrol=${HISTCONTROL-}
  __xyz_original_histignore=${HISTIGNORE-}
  __xyz_original_histsize=${HISTSIZE-}
  __xyz_last_history=$(HISTTIMEFORMAT= builtin history 1; builtin printf x)
  __xyz_b64() { builtin local encoded; encoded=$(builtin printf '%s' "$1" | command base64) || builtin return; builtin printf '%s' "${encoded//$'\n'/}"; }
  __xyz_gap() {
   if [[ -z ${__xyz_tty_saved-} ]]; then __xyz_receiving=0; fi
   __xyz_failed=1
   builtin printf '\033]777;xyzlog;2;%s;G;%s;%s\007' "$__xyz_nonce" "$__xyz_seq" "${1:-hooks_changed}"
  }
  __xyz_manual_gap() {
   # Observation only: no command success is inferred. The real prompt hook
   # must still independently verify integrity and announce readiness.
   __xyz_ended=$__xyz_seq
   __xyz_last_history=$(HISTTIMEFORMAT= builtin history 1; builtin printf x)
   __xyz_at_prompt=0
   builtin printf '\033]777;xyzlog;2;%s;G;%s;%s\007' "$__xyz_nonce" "$__xyz_seq" "$1"
  }
  __xyz_end() {
   builtin local code=$? current_history
   if [[ -n ${__xyz_tty_saved-} || ${__xyz_receiving-0} != 0 ]]; then
    __xyz_restore_tty; __xyz_receiving=0; __xyz_gap receiver_failed; builtin return "$code"
   fi
   if (( __xyz_ai_active == 1 )); then __xyz_action_end "$code"; builtin return "$code"; fi
   if (( __xyz_at_prompt == 1 && __xyz_failed == 0 )); then
    current_history=$(HISTTIMEFORMAT= builtin history 1; builtin printf x)
    if [[ $current_history != "$__xyz_last_history" ]]; then __xyz_manual_gap legacy_unobserved_command; fi
   fi
   __xyz_at_prompt=0
   builtin return "$code"
  }
  __xyz_intact() {
   __xyz_controls_ok || builtin return 1
   [[ -z $(builtin compgen -A alias __xyz_) ]] || builtin return 1
   [[ $(builtin declare -f $__xyz_helper_names) == "$__xyz_owned_helpers" ]] || builtin return 1
   if [[ $(builtin declare -p PROMPT_COMMAND) != "$__xyz_owned_pc_decl" || ${PS1-} != "$__xyz_original_ps1" || $(__xyz_norm_traps "$1") != "$__xyz_original_traps" || ${HISTCONTROL-} != "$__xyz_original_histcontrol" || ${HISTIGNORE-} != "$__xyz_original_histignore" || ${HISTSIZE-} != "$__xyz_original_histsize" || ${histchars-'!^#'} != '!^#' || $- == *e* || $- == *T* || $- == *a* || $- == *r* ]] || builtin shopt -q extdebug || [[ -o posix ]] || ! [[ -o history ]] || ! builtin shopt -q cmdhist promptvars interactive_comments; then
    builtin return 1
   fi
   builtin return 0
  }
  __xyz_ready() {
   builtin local code=$? names names64 now LC_ALL=C
   if (( __xyz_failed != 0 )); then builtin return "$code"; fi
   if ! __xyz_intact "$1"; then __xyz_gap; builtin return "$code"; fi
   # Names only, never definitions/values (which may contain credentials).
   names=$(builtin compgen -A alias; builtin compgen -A function)
   if (( ${#names} > 8192 )); then __xyz_gap; builtin return "$code"; fi
   names64=$(__xyz_b64 "$names") || { __xyz_gap; builtin return "$code"; }
   builtin printf -v now '%(%s)T' -1
   builtin printf '\033]777;xyzlog;2;%s;R;%s;%s;%s\007' "$__xyz_nonce" "$__xyz_seq" "$now" "$names64"
   __xyz_at_prompt=1
   builtin return "$code"
  }
  __xyz_uninstall() {
   builtin local code=$? restore_pc=0 name
   [[ $(builtin declare -p PROMPT_COMMAND 2>/dev/null) == "$__xyz_owned_pc_decl" ]] && restore_pc=1
   if (( restore_pc )); then
    builtin unset PROMPT_COMMAND
    if [[ -n $__xyz_original_pc_decl ]]; then builtin eval "${__xyz_original_pc_decl/declare /declare -g }"; fi
   fi
   builtin printf '\033]777;xyzlog;2;%s;X\007' "$__xyz_nonce"
   __xyz_failed=1
   if (( restore_pc )); then
    for name in $(builtin compgen -A variable __xyz_); do builtin unset "$name"; done
    for name in $(builtin compgen -A function __xyz_); do builtin unset -f "$name"; done
   fi
   builtin return "$code"
  }
  PROMPT_COMMAND=$'__xyz_end\n'${PROMPT_COMMAND-}$'\n__xyz_ready "$(builtin trap -p)"'
  __xyz_helper_names='__xyz_controls_ok __xyz_norm_traps __xyz_is_envelope __xyz_manual_step __xyz_action_begin __xyz_action_end __xyz_restore_tty __xyz_receive __xyz_b64 __xyz_gap __xyz_manual_gap __xyz_end __xyz_intact __xyz_ready __xyz_uninstall'
  __xyz_owned_helpers=$(builtin declare -f $__xyz_helper_names)
  __xyz_owned_pc_decl=$(builtin declare -p PROMPT_COMMAND)
  builtin printf '\033]777;xyzlog;2;%s;H;%s\007' "$__xyz_nonce" "$BASH_VERSION"
 }
 __xyz_install "$(builtin trap -p)" "$(builtin trap -p EXIT)"
 builtin unset -f __xyz_install
fi
