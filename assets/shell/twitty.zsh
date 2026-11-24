# Twitty shell integration for zsh.
#
# Marks every prompt, command and its output with OSC 133 (A prompt, B command, C output,
# D;status end), sends the command line with OSC 633 E and the working directory with OSC 7,
# so Twitty can draw command blocks. Your own startup files, prompt and hooks keep working.
#
# Twitty points ZDOTDIR at a directory whose .zshenv, .zprofile, .zshrc and .zlogin each
# source this file with their own name: it then reads your file of that name from your own
# ZDOTDIR (or $HOME), and gives ZDOTDIR back after the last one. You can also source this
# file at the end of your own ~/.zshrc.

if [[ -n ${__TWITTY_ZSH_BOOTSTRAP-} && ${1-} == .z(shenv|profile|shrc|login) ]]; then
    if [[ $1 == .zshenv ]]; then
        typeset -g __twitty_zdotdir_ours=$ZDOTDIR
        typeset -g __twitty_zdotdir_user=${__TWITTY_ZSH_ZDOTDIR-}
        builtin unset __TWITTY_ZSH_ZDOTDIR
    fi
    # The user's ZDOTDIR while their file runs: files often refer to it. It came from the
    # environment, so it stays exported for the programs this shell starts.
    if [[ -n $__twitty_zdotdir_user ]]; then
        export ZDOTDIR=$__twitty_zdotdir_user
    else
        builtin unset ZDOTDIR
    fi
    if [[ -r ${ZDOTDIR:-$HOME}/$1 ]]; then
        builtin source -- "${ZDOTDIR:-$HOME}/$1"
    fi
    # A file may move ZDOTDIR (a .zshenv setting ZDOTDIR=~/.config/zsh is common).
    __twitty_zdotdir_user=${ZDOTDIR-}
    # The last startup file zsh reads keeps the user's ZDOTDIR; before that, zsh must come
    # back here for the next one.
    if [[ $1 == .zlogin ]] || [[ $1 == .zshrc && ! -o login ]] \
        || [[ $1 == .zshenv && ! -o interactive && ! -o login ]]; then
        builtin unset __TWITTY_ZSH_BOOTSTRAP __twitty_zdotdir_ours __twitty_zdotdir_user
    else
        ZDOTDIR=$__twitty_zdotdir_ours
    fi
    # Hooks go in after the user's .zshrc, so they run around the user's own.
    [[ $1 == .zshrc ]] || return 0
fi

# Whether something the user loaded already marks prompts with OSC 133 (another terminal's
# integration, VS Code's, a hand-written prompt): then nothing is added, so no mark is doubled.
__twitty_marked_elsewhere() {
    [[ -n ${ITERM_SHELL_INTEGRATION_INSTALLED-}${VSCODE_SHELL_INTEGRATION-} ]] && return 0
    [[ ${PS1-} == *'133;'* ]] && return 0
    # Any function that prints a mark (written so this one does not match itself).
    local mark=']13''3;'
    [[ ${(j::)functions} == *$mark* ]]
}

if [[ -o interactive ]] && (( ! ${+__twitty_si_loaded} )) && __twitty_marked_elsewhere; then
    builtin unset TWITTY_SHELL_INTEGRATION
elif [[ -o interactive ]] && (( ! ${+__twitty_si_loaded} )); then
    typeset -g __twitty_si_loaded=1
    export TWITTY_SHELL_INTEGRATION=1
    typeset -g __twitty_ran=

    # Prompt marks, as zsh prompt escapes (%{ %} take no room).
    typeset -g __twitty_a=$'%{\e]133;A\a%}'
    typeset -g __twitty_b=$'%{\e]133;B\a%}'

    # OSC 7: file://host/path, the path percent-encoded byte by byte.
    __twitty_osc7() {
        emulate -L zsh
        setopt no_multibyte
        local out= c i
        if [[ $PWD == *[^A-Za-z0-9/._~-]* ]]; then
            for (( i = 1; i <= ${#PWD}; i++ )); do
                c=${PWD[i]}
                if [[ $c == [A-Za-z0-9/._~-] ]]; then
                    out+=$c
                else
                    # The byte in two hex digits.
                    out+=%${(l:2::0:)$(( [##16] #c & 255 ))}
                fi
            done
        else
            out=$PWD
        fi
        builtin printf '\e]7;file://%s%s\a' "${HOST-}" "$out"
    }

    # OSC 633 E escaping: `\` as `\\`, `;` and control characters as `\xHH`.
    __twitty_escape() {
        emulate -L zsh
        local s=${1//\\/\\\\} out= c i
        s=${s//;/\\x3b}
        if [[ $s == *[[:cntrl:]]* ]]; then
            for (( i = 1; i <= ${#s}; i++ )); do
                c=${s[i]}
                if [[ $c == [[:cntrl:]] ]]; then
                    out+=\\x${(l:2::0:)$(( [##16] #c ))}
                else
                    out+=$c
                fi
            done
            s=$out
        fi
        builtin print -rn -- "$s"
    }

    # First precmd: the command ended (D with its status).
    __twitty_precmd() {
        local st=$?
        if [[ -n $__twitty_ran ]]; then
            builtin printf '\e]133;D;%s\a' "$st"
            __twitty_ran=
        fi
        return st
    }

    # Last precmd, after anything that rewrites the prompt (themes, git prompts): wrap PS1 in
    # A ... B again and report the directory.
    __twitty_prompt() {
        local st=$?
        __twitty_osc7
        if [[ $PS1 != "$__twitty_a"*"$__twitty_b" ]]; then
            local prompt=${PS1//"$__twitty_a"/}
            prompt=${prompt//"$__twitty_b"/}
            PS1=$__twitty_a$prompt$__twitty_b
        fi
        return st
    }

    # Before a command runs: its command line (E) and the start of its output (C).
    __twitty_preexec() {
        __twitty_ran=1
        # Stay the last precmd even when something registered after Twitty.
        precmd_functions=(${precmd_functions:#__twitty_prompt} __twitty_prompt)
        builtin printf '\e]633;E;%s\a\e]133;C\a' "$(__twitty_escape "$1")"
    }

    precmd_functions=(__twitty_precmd ${precmd_functions[@]} __twitty_prompt)
    preexec_functions+=(__twitty_preexec)
fi
builtin unfunction __twitty_marked_elsewhere
