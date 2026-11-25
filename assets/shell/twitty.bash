# Twitty shell integration for bash (4.4 or newer).
#
# Marks every prompt, command and its output with OSC 133 (A prompt, B command, C output,
# D;status end) and reports the working directory with OSC 7, so Twitty can draw command
# blocks. Your own startup files, prompt and PROMPT_COMMAND keep working.
#
# Twitty starts bash with `--rcfile` pointing here; this file then reads the startup files
# bash would have read itself. You can also source it at the end of your own ~/.bashrc.

if [[ -n "${__TWITTY_BASH_RCFILE-}" ]]; then
    builtin unset __TWITTY_BASH_RCFILE
    if [[ -n "${__TWITTY_BASH_LOGIN-}" ]]; then
        # A login shell: what bash reads for one (`bash -l`).
        builtin unset __TWITTY_BASH_LOGIN
        if [[ -r /etc/profile ]]; then
            builtin source /etc/profile
        fi
        for __twitty_file in ~/.bash_profile ~/.bash_login ~/.profile; do
            if [[ -r "$__twitty_file" ]]; then
                builtin source "$__twitty_file"
                break
            fi
        done
        builtin unset __twitty_file
    elif [[ -r ~/.bashrc ]]; then
        builtin source ~/.bashrc
    fi
fi

# Twitty's `claude` wrapper (config `claude_hooks`) goes first on PATH again, after the
# startup files above may have prepended their own directories.
if [[ -n "${TWITTY_CLAUDE_DIR-}" ]]; then
    __twitty_path=":${PATH-}:"
    while [[ "$__twitty_path" == *":$TWITTY_CLAUDE_DIR:"* ]]; do
        __twitty_path=${__twitty_path//":$TWITTY_CLAUDE_DIR:"/:}
    done
    __twitty_path=${__twitty_path#:}
    __twitty_path=${__twitty_path%:}
    PATH="$TWITTY_CLAUDE_DIR${__twitty_path:+:$__twitty_path}"
    builtin unset __twitty_path
fi

# Whether something the user loaded already marks prompts with OSC 133 (another terminal's
# integration, VS Code's, a hand-written PS1): then nothing is added, so no mark is doubled.
__twitty_marked_elsewhere() {
    [[ -n "${ITERM_SHELL_INTEGRATION_INSTALLED-}${VSCODE_SHELL_INTEGRATION-}" ]] && return 0
    [[ "${PS1-}${PS0-}" == *'133;'* ]] && return 0
    [[ "$(builtin declare -p PROMPT_COMMAND 2>/dev/null)" == *'133;'* ]] && return 0
    # Any function that prints a mark (written so this one does not match itself).
    local mark=']13''3;'
    [[ "$(builtin declare -f)" == *"$mark"* ]]
}

if [[ $- == *i* && -z "${__twitty_si_loaded-}" ]] && __twitty_marked_elsewhere; then
    builtin unset TWITTY_SHELL_INTEGRATION
elif [[ $- == *i* && -z "${__twitty_si_loaded-}" ]] \
    && ((BASH_VERSINFO[0] > 4 || (BASH_VERSINFO[0] == 4 && BASH_VERSINFO[1] >= 4))); then
    __twitty_si_loaded=1
    export TWITTY_SHELL_INTEGRATION=1

    # Prompt marks, as bash prompt escapes (\[ \] tell readline they take no room).
    __twitty_a='\[\e]133;A\a\]'
    __twitty_b='\[\e]133;B\a\]'
    __twitty_c='\e]133;C\a'

    # OSC 7: file://host/path, the path percent-encoded byte by byte.
    __twitty_osc7() {
        local LC_ALL=C path=$PWD out= c n i
        if [[ $path == *[!A-Za-z0-9/._~-]* ]]; then
            for ((i = 0; i < ${#path}; i++)); do
                c=${path:i:1}
                case $c in
                [A-Za-z0-9/._~-]) out+=$c ;;
                *)
                    builtin printf -v n '%d' "'$c"
                    builtin printf -v c '%%%02X' $((n & 255))
                    out+=$c
                    ;;
                esac
            done
        else
            out=$path
        fi
        builtin printf '\e]7;file://%s%s\a' "${HOSTNAME-}" "$out"
    }

    # First in PROMPT_COMMAND: the command ended (D with its status). Keeps $? for the rest.
    __twitty_precmd() {
        local status=$?
        builtin printf '\e]133;D;%s\a' "$status"
        return "$status"
    }

    # Last in PROMPT_COMMAND, after anything that rewrites PS1 (starship, git prompts): wrap the
    # prompt in A ... B again and report the directory.
    __twitty_prompt() {
        local status=$?
        __twitty_osc7
        if [[ "$PS1" != "$__twitty_a"*"$__twitty_b" ]]; then
            local prompt=${PS1//"$__twitty_a"/}
            prompt=${prompt//"$__twitty_b"/}
            PS1=$__twitty_a$prompt$__twitty_b
        fi
        # PS0 is printed after a command line is read, before it runs: the output starts there.
        if [[ "${PS0-}" != *"$__twitty_c" ]]; then
            PS0=${PS0-}$__twitty_c
        fi
        return "$status"
    }

    if [[ "$(builtin declare -p PROMPT_COMMAND 2>/dev/null)" == "declare -a"* ]]; then
        PROMPT_COMMAND=(__twitty_precmd "${PROMPT_COMMAND[@]}" __twitty_prompt)
    else
        # Newlines, not `;`: a PROMPT_COMMAND that ends in `;` stays valid.
        PROMPT_COMMAND=__twitty_precmd$'\n'${PROMPT_COMMAND:+$PROMPT_COMMAND$'\n'}__twitty_prompt
    fi
fi
builtin unset -f __twitty_marked_elsewhere
