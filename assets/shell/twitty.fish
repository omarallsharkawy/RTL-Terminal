# Twitty shell integration for fish.
#
# Marks every prompt, command and its output with OSC 133 (A prompt, B command, C output,
# D;status end), sends the command line with OSC 633 E and the working directory with OSC 7,
# so Twitty can draw command blocks. Your own config and prompt keep working.
#
# Twitty adds a directory to XDG_DATA_DIRS whose fish/vendor_conf.d holds this file, so fish
# loads it at startup; it puts XDG_DATA_DIRS back right away. You can also source it from
# your own config.fish.

if set -q __TWITTY_FISH_XDG_DATA_DIRS
    if test -n "$__TWITTY_FISH_XDG_DATA_DIRS"
        set -gx XDG_DATA_DIRS $__TWITTY_FISH_XDG_DATA_DIRS
    else
        set -e XDG_DATA_DIRS
    end
    set -e __TWITTY_FISH_XDG_DATA_DIRS
end

# Another integration that marks prompts (iTerm2's, VS Code's): nothing is added. fish 4
# marks prompts itself; Twitty ignores the repeated marks.
if status is-interactive; and not set -q __twitty_si_loaded; and not set -q ITERM_SHELL_INTEGRATION_INSTALLED; and not set -q VSCODE_SHELL_INTEGRATION
    set -g __twitty_si_loaded 1
    set -gx TWITTY_SHELL_INTEGRATION 1
    set -g __twitty_ran

    # OSC 633 E escaping: `\` as `\\`, `;` and control characters as `\xHH`.
    # Every value is passed quoted: `string` reads stdin when it gets no argument.
    function __twitty_escape
        test -n "$argv[1]"; or return
        set -l s (string replace -a -- '\\' '\\\\' "$argv[1]" | string collect)
        set s (string replace -a -- ';' '\\x3b' "$s" | string collect)
        set s (string replace -a -- \n '\\x0a' "$s" | string collect)
        set s (string replace -a -- \r '\\x0d' "$s" | string collect)
        set s (string replace -a -- \t '\\x09' "$s" | string collect)
        set s (string replace -a -- \e '\\x1b' "$s" | string collect)
        set s (string replace -a -- \a '\\x07' "$s" | string collect)
        string replace -ra -- '[\x00-\x1f\x7f]' '' "$s"
    end

    # Each prompt: the directory (OSC 7), and the prompt function wrapped in A ... B. It is
    # wrapped at the first prompt, once config.fish and the autoloaded prompt are in place,
    # and again whenever something defines a new fish_prompt.
    function __twitty_on_prompt --on-event fish_prompt
        printf '\e]7;file://%s%s\a' $hostname (string escape --style=url -- $PWD)
        if functions -q fish_prompt; and not string match -q '*__twitty_user_prompt*' -- (functions fish_prompt | string collect)
            functions -e __twitty_user_prompt
            functions -c fish_prompt __twitty_user_prompt
            function fish_prompt
                printf '\e]133;A\a'
                __twitty_user_prompt
                printf '\e]133;B\a'
            end
        end
    end

    # Before a command runs: its command line (E) and the start of its output (C).
    function __twitty_preexec --on-event fish_preexec
        set -g __twitty_ran 1
        printf '\e]633;E;%s\a\e]133;C\a' (__twitty_escape "$argv[1]")
    end

    # After it ran: the end of its output (D with its status).
    function __twitty_postexec --on-event fish_postexec
        set -l st $status
        if test -n "$__twitty_ran"
            printf '\e]133;D;%s\a' $st
            set -g __twitty_ran
        end
    end
end
