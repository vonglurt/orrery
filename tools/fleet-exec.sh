#!/bin/sh
# orrery fleet-exec: the forced command the Lab's buttons land in.
#
# THIS FILE IS THE SECURITY BOUNDARY, and it is copal-prep.sh's copal-fleet-exec
# with the Copal taken out: the same verbs where they mean the same thing, on
# Ubuntu and Alpine alike. sshd sends every login as copal-fleet here with no
# tty and no forwarding, and what arrives is A VERB LIST, never a command. A
# verb this file does not know is a refusal and a line in the log.
set -eu
set -f # "message *" must not become the names of the files in this directory.
LOG=/var/log/copal-fleet.log
D=/var/lib/copal-fleet
U=$(cat /etc/copal-fleet/login-user 2>/dev/null || echo user)

logline() { printf '%s %s %s\n' "$(date '+%Y-%m-%dT%H:%M:%S')" "${SSH_CLIENT%% *}" "$*" >> "$LOG" 2>/dev/null || true; }
refuse() { logline "REFUSED ${SSH_ORIGINAL_COMMAND:-<empty>}"; echo "refused: $*" >&2; exit 3; }
as_root() { if command -v doas >/dev/null 2>&1; then doas -n "$@"; else sudo -n "$@"; fi; }
as_user() { if command -v doas >/dev/null 2>&1; then doas -n -u "$U" "$@"; else sudo -n -u "$U" "$@"; fi; }

# shellcheck disable=SC2086 -- what arrives is a list of words, and that is the point.
set -- ${SSH_ORIGINAL_COMMAND:-}
[ $# -gt 0 ] || refuse "nothing to do"
# Not the room's polling, every few seconds: the log is for what someone did.
[ "$1" = chat-sync ] || logline "$*"

case "$1" in
    id) exec hostname ;;
    uptime) exec uptime ;;
    facts|state|status) as_user /usr/local/bin/copal-fleet-facts ;;
    has)
        # One word, an allow-list of its characters: the name of a program or
        # a package, read by the helper and never run.
        case "${2:-}" in ''|*[!A-Za-z0-9._+-]*) refuse "has takes the name of a package or a program" ;; esac
        as_user /usr/local/bin/copal-fleet-facts has "$2" ;;
    message)
        shift
        [ $# -gt 0 ] || refuse "message takes some text"
        _t="$*"
        [ ${#_t} -le 200 ] || refuse "a banner is 200 characters or fewer"
        # The console's is_banner, spelled again: the two are one rule.
        case "$_t" in *[!\ A-Za-z0-9.,!?:-]*) refuse "a banner is letters, digits and . , ! ? : -" ;; esac
        # THE MOTD READS THIS FILE, and it is written before anyone is told:
        # a machine nobody is at still has the message at the next login.
        printf '%s  %s\n' "$(date '+%Y-%m-%d %H:%M')" "$_t" > "$D/last-message.tmp"
        mv "$D/last-message.tmp" "$D/last-message"
        cat "$D/last-message" >> "$D/messages.log"
        logline "MESSAGE $_t"
        # Whoever is here now: a window on the desktop, a line on each terminal.
        # Neither is the message arriving, so neither can fail the verb.
        _how=$(as_user /usr/local/bin/copal-fleet-facts notify "$_t" 2>/dev/null) || true
        echo "kept for the next login; ${_how:-nobody told now}" ;;
    clip)
        # Text on stdin, for the desktop's clipboard. ANY TEXT, unlike a message:
        # it is held in a clipboard and never drawn or run, and it is read by the
        # helper from stdin, never from a word of this line.
        logline "CLIP"
        as_user /usr/local/bin/copal-fleet-facts clip ;;
    save)
        # Text on stdin, to a file in the user's ~/fleet-inbox. One name, no path,
        # and no overwriting: this verb can add a file and can change none.
        case "${2:-}" in
            ''|.*|*[!A-Za-z0-9._-]*) refuse "a file's name is letters, digits and . _ - and does not start with a dot" ;;
        esac
        [ ${#2} -le 64 ] || refuse "a file's name is 64 characters or fewer"
        logline "SAVE $2"
        as_user /usr/local/bin/copal-fleet-facts save "$2" ;;
    shot)
        # THE ONE VERB THAT LOOKS, AND IT SAYS SO FIRST. The hello is here, on the
        # machine, so no console can leave it out, and no picture is taken if it could
        # not be said. A moment for the window to open: then it is in the picture too.
        logline "SHOT"
        as_user /usr/local/bin/copal-fleet-facts observed >/dev/null 2>&1 || refuse "could not say so first"
        sleep 1
        as_user /usr/local/bin/copal-fleet-facts shot ;;
    chat-sync)
        # THE ROOM'S ONE DOOR. The console hosts Fleet Chat: stdin is what this machine
        # has not seen, stdout is what its person has typed. Control characters come
        # out in the helper, so a line cannot move a cursor on a terminal showing it.
        case "${2:-}" in ''|ack) ;; *) refuse "chat-sync takes ack" ;; esac
        as_user /usr/local/bin/copal-fleet-facts chat-sync "${2:-}" ;;
    last-message)
        cat "$D/last-message" 2>/dev/null || echo "no message yet" ;;
    scene)
        # THIS RECORDS A SCENE. IT DOES NOT APPLY ONE: a verb that took
        # instructions over the wire is what this file exists to forbid.
        [ "${2:-}" = apply ] || refuse "scene takes apply NAME"
        case "${3:-}" in ''|*[!a-z0-9-]*) refuse "a scene name is lowercase letters, digits and dashes" ;; esac
        printf '%s\n' "$3" > "$D/scene"
        logline "SCENE $3"
        echo "scene: $3" ;;
    power)
        case "${2:-}" in
            off) logline "POWEROFF"; as_root /usr/local/bin/copal-fleet-power off ;;
            reboot) logline "REBOOT"; as_root /usr/local/bin/copal-fleet-power reboot ;;
            *) refuse "power takes off or reboot" ;;
        esac ;;
    snapshot)
        # Absent rather than improvised. A restore replaces a machine's disk
        # state, and these machines hold weeks of training: nothing here takes
        # a snapshot, so there is nothing true this verb could restore.
        [ "${2:-}" = restore ] || refuse "snapshot takes restore"
        [ -x /usr/local/bin/copal-snapshot ] || refuse "no snapshots on this machine: nothing takes them yet"
        exec doas -n /usr/local/bin/copal-snapshot restore ;;
    remote)
        [ "${2:-}" = status ] || refuse "remote takes status"
        echo "not installed" ;;
    log)
        [ "${2:-}" = tail ] || refuse "log takes tail [N]"
        case "${3:-20}" in *[!0-9]*) refuse "log tail takes a number" ;; esac
        exec tail -n "${3:-20}" "$LOG" ;;
    *) refuse "no verb called '$1'" ;;
esac
