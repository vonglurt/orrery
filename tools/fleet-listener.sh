#!/bin/sh
# The Lab's buttons need something on the machine to answer them: this, once,
# as root, from ~/orrery-recipes (Machines -> "Answer the Lab's buttons" stages
# it and types it). Ubuntu and Alpine alike.
#
# THE LISTENER IS sshd, NOT A NEW DAEMON. A new port would be a new thing to
# authenticate, and the fleet already decided how: a certificate from its CA.
# So this makes the account copal-fleet, whose every login is sent to one
# forced command (fleet-exec.sh), and lets a human's certificate log in as the
# user for Terminal and Exchange. It adds:
#
#   the account copal-fleet, no password, no tty, no forwarding;
#   /usr/bin/copal-fleet-exec and three helpers in /usr/local/bin;
#   the fleet's CA and this machine's host certificate, both checked first;
#   one marked block in sshd_config, proved by sshd -t or taken back;
#   the rule that lets the account power off, reboot and read facts, no more;
#   the last message at every login.
#
# It changes no existing login: the user's own key goes on working, and sshd is
# reloaded rather than restarted, so a session open now stays open.
# --undo takes all of it away.
set -eu

# Raised when a machine must run this again: the Fleet screen offers Update.
VERSION=6
U="${USER_NAME:-user}"
ACCT=copal-fleet
CFG=/etc/ssh/sshd_config
BEGIN="# BEGIN orrery fleet-listener"
END="# END orrery fleet-listener"
HERE=$(cd "$(dirname "$0")" && pwd)
CA=/etc/ssh/copal-fleet-ca.pub
HOSTKEY=/etc/ssh/ssh_host_ed25519_key
HOSTCERT=/etc/ssh/ssh_host_ed25519_key-cert.pub
EXEC=/usr/bin/copal-fleet-exec

[ "$(id -u)" = 0 ] || { echo "run as root (doas or sudo)" >&2; exit 1; }
if command -v rc-service >/dev/null 2>&1; then ALPINE=1; else ALPINE=0; fi

reload_sshd() {
    if [ "$ALPINE" = 1 ]; then rc-service sshd reload >/dev/null 2>&1 || true
    else systemctl reload ssh >/dev/null 2>&1 || systemctl reload sshd >/dev/null 2>&1 || true; fi
}

without_block() { sed "/^$BEGIN\$/,/^$END\$/d" "$CFG"; }

if [ "${1:-}" = --undo ]; then
    without_block > "$CFG.fleet.new"
    sed -i "s/^\(AllowUsers .*\) $ACCT\$/\1/" "$CFG.fleet.new"
    cp "$CFG" "$CFG.fleet.bak"
    cat "$CFG.fleet.new" > "$CFG"; rm -f "$CFG.fleet.new"
    if ! sshd -t; then cat "$CFG.fleet.bak" > "$CFG"; echo "sshd rejected the undo; nothing changed" >&2; exit 1; fi
    reload_sshd
    rm -f "$HOSTCERT" "$CA" "$EXEC" /usr/local/bin/copal-fleet-facts /usr/local/bin/copal-fleet-power /usr/local/bin/fleet-chat /usr/local/bin/fleet-shot
    rm -f /etc/ssh/principals/"$ACCT" /etc/ssh/principals/"$U"
    rm -f /etc/doas.d/zz-fleet-listener.conf /etc/sudoers.d/fleet-listener
    rm -f /etc/update-motd.d/02-copal-message /etc/profile.d/copal-message.sh
    rm -f /etc/copal-fleet/login-user /etc/copal-fleet/version
    if [ "$ALPINE" = 1 ]; then deluser "$ACCT" 2>/dev/null || true; else userdel "$ACCT" 2>/dev/null || true; fi
    echo "the listener is gone; messages are kept in /var/lib/copal-fleet"
    exit 0
fi

id "$U" >/dev/null 2>&1 || { echo "no user $U here" >&2; exit 1; }
for f in fleet-exec.sh fleet-ca.pub fleet-host-cert.pub; do
    [ -s "$HERE/$f" ] || { echo "$f is not beside this script: stage the recipe from Machines" >&2; exit 1; }
done
# A Match block claims every line after it, so a block added below one would be
# read as that Match's settings. Refused rather than guessed at.
if without_block | grep -qiE '^[[:space:]]*Match[[:space:]]'; then
    echo "$CFG has a Match block of its own; add the fleet's block by hand. Nothing changed." >&2
    exit 1
fi
# An exec this script did not write is Copal's own, and it is not ours to replace.
if [ -e "$EXEC" ] && ! grep -q 'orrery fleet-exec' "$EXEC"; then
    echo "$EXEC exists and is not this one: this machine is already in a fleet. Nothing changed." >&2
    exit 1
fi

# The certificate is proved here before sshd is told of it: the wrong file would
# present a key this machine does not hold, and every client would refuse it.
info=$(ssh-keygen -L -f "$HERE/fleet-host-cert.pub") || { echo "fleet-host-cert.pub is not a certificate" >&2; exit 1; }
case "$info" in *"host certificate"*) ;; *) echo "that is not a HOST certificate" >&2; exit 1 ;; esac
certkey=$(printf '%s\n' "$info" | sed -n 's/.*Public key: [^ ]* \(SHA256:[^ ]*\).*/\1/p' | head -1)
mykey=$(ssh-keygen -l -f "$HOSTKEY" | awk '{print $2}')
[ -n "$certkey" ] && [ "$certkey" = "$mykey" ] || { echo "that certificate is for another machine's key" >&2; exit 1; }
signer=$(printf '%s\n' "$info" | sed -n 's/.*Signing CA: [^ ]* \(SHA256:[^ ]*\).*/\1/p' | head -1)
ca=$(ssh-keygen -l -f "$HERE/fleet-ca.pub" | awk '{print $2}')
[ -n "$signer" ] && [ "$signer" = "$ca" ] || { echo "that certificate is signed by another authority" >&2; exit 1; }

# The account. '*' and not locked: sshd turns a locked account away before it
# reads a certificate, and '*' matches no password anyone can type.
if ! id "$ACCT" >/dev/null 2>&1; then
    if [ "$ALPINE" = 1 ]; then adduser -D -h /var/lib/copal-fleet -s /bin/sh "$ACCT"
    else useradd -r -m -d /var/lib/copal-fleet -s /bin/sh "$ACCT"; fi
fi
echo "$ACCT:*" | chpasswd -e
mkdir -p /var/lib/copal-fleet /etc/copal-fleet /etc/ssh/principals
chown "$ACCT" /var/lib/copal-fleet
chmod 755 /var/lib/copal-fleet
echo "$U" > /etc/copal-fleet/login-user
echo "$VERSION" > /etc/copal-fleet/version
: >> /var/log/copal-fleet.log
chown "$ACCT" /var/log/copal-fleet.log

install -m 0755 "$HERE/fleet-exec.sh" "$EXEC"
install -m 0644 "$HERE/fleet-ca.pub" "$CA"
install -m 0644 "$HERE/fleet-host-cert.pub" "$HOSTCERT"
# Automation lands in the forced command; only a human's certificate gets a shell.
echo fleet-operator > "/etc/ssh/principals/$ACCT"
echo fleet-human > "/etc/ssh/principals/$U"
chmod 644 "/etc/ssh/principals/$ACCT" "/etc/ssh/principals/$U"

cat > /usr/local/bin/copal-fleet-power <<'POWER'
#!/bin/sh
# orrery fleet-listener: off or reboot, and nothing else. Run as root.
case "${1:-}" in
    off) if command -v systemctl >/dev/null 2>&1; then exec systemctl poweroff; else exec /sbin/poweroff; fi ;;
    reboot) if command -v systemctl >/dev/null 2>&1; then exec systemctl reboot; else exec /sbin/reboot; fi ;;
    *) echo "off or reboot" >&2; exit 2 ;;
esac
POWER
cat > /usr/local/bin/copal-fleet-facts <<'FACTS'
#!/bin/sh
# orrery fleet-listener: what the fleet selects a machine by, key=value, read
# as the user (treadcore is the user's checkout). Reads; changes nothing.
k() { printf '%s=%s\n' "$1" "$2"; }
PATH=$PATH:$HOME/.local/bin:$HOME/.cargo/bin
# The desktop this user is logged in to, if any, as the environment its programs
# need. READ FROM THE SESSION'S OWN PROCESS, because nothing else agrees: Copal
# keeps its runtime directory in /tmp and its bus somewhere random, Ubuntu in
# /run/user, and X may be :0 or :1. Assignments only; a value is never evaluated.
session() {
    SESSION=""
    for n in Hyprland gnome-shell sway kwin_wayland kwin_x11 xfce4-session mate-session; do
        p=$(pgrep -u "$(id -u)" -x "$n" 2>/dev/null | head -1)
        [ -n "$p" ] && { SESSION=$n; break; }
    done
    [ -n "$SESSION" ] || return 1
    for v in XDG_RUNTIME_DIR DBUS_SESSION_BUS_ADDRESS WAYLAND_DISPLAY DISPLAY XAUTHORITY HYPRLAND_INSTANCE_SIGNATURE; do
        val=$(cat "/proc/$p/environ" 2>/dev/null | tr '\0' '\n' | sed -n "s/^$v=//p" | head -1)
        [ -n "$val" ] && export "$v=$val"
    done
    # Hyprland makes its socket after it starts, so its own environment lacks it.
    if [ -z "${WAYLAND_DISPLAY:-}" ] && [ -n "${XDG_RUNTIME_DIR:-}" ]; then
        for w in "$XDG_RUNTIME_DIR"/wayland-*; do
            case "$w" in *.lock|*'*') ;; *) export WAYLAND_DISPLAY="${w##*/}"; break ;; esac
        done
    fi
    if [ -z "${HYPRLAND_INSTANCE_SIGNATURE:-}" ] && [ -d "${XDG_RUNTIME_DIR:-/nonexistent}/hypr" ]; then
        for h in "$XDG_RUNTIME_DIR"/hypr/*; do export HYPRLAND_INSTANCE_SIGNATURE="${h##*/}"; done
    fi
    if [ -z "${DISPLAY:-}" ]; then
        for x in /tmp/.X11-unix/X*; do
            case "$x" in *_|*'*') ;; *) export DISPLAY=":${x##*X}"; break ;; esac
        done
    fi
    return 0
}
ROOM="$HOME/.fleet-room.log"
OUT="$HOME/.fleet-room.out"
chat_open() { [ -s "$HOME/.fleet-chat.pid" ] && kill -0 "$(cat "$HOME/.fleet-chat.pid")" 2>/dev/null; }
# Say so to whoever is here, unless the chat is already open in front of them.
# Prints how it told them. The window is a terminal running fleet-chat, because a
# terminal is already input, paste and scrollback, and it is the same program a
# headless machine's person runs by hand.
announce() {
    t="$1"; how=""
    if chat_open; then echo "in the open chat"; return 0; fi
    if session; then
        # Hyprland tiles a new window, and a chat is not a place to work. The rule's
        # syntax changed at 0.53; the old form is refused by a new one, so try both.
        if [ "$SESSION" = Hyprland ]; then
            hyprctl keyword windowrule 'match:title ^(Fleet chat)$, float on' >/dev/null 2>&1
            hyprctl keyword windowrule 'match:title ^(Fleet chat)$, center on' >/dev/null 2>&1
            hyprctl keyword windowrulev2 'float,title:^(Fleet chat)$' >/dev/null 2>&1
        fi
        if [ "$SESSION" = gnome-shell ] && command -v gnome-terminal >/dev/null 2>&1; then
            away gnome-terminal --title="Fleet chat" -- /usr/local/bin/fleet-chat; how="a chat window"
        elif [ -n "${DISPLAY:-}" ] && command -v xterm >/dev/null 2>&1; then
            away xterm -fa Monospace -fs 11 -bg '#16181a' -fg '#e8e6df' -sb -sl 5000 -geometry 90x24 -title "Fleet chat" -e /usr/local/bin/fleet-chat; how="a chat window"
        elif command -v gnome-terminal >/dev/null 2>&1; then
            away gnome-terminal --title="Fleet chat" -- /usr/local/bin/fleet-chat; how="a chat window"
        elif command -v foot >/dev/null 2>&1; then
            away foot --title="Fleet chat" /usr/local/bin/fleet-chat; how="a chat window"
        fi
        if command -v notify-send >/dev/null 2>&1 && [ -n "${DBUS_SESSION_BUS_ADDRESS:-}" ]; then
            notify-send -u critical "Fleet message" "$t" >/dev/null 2>&1 && how="${how:+$how, }a notification"
        fi
    fi
    # The terminals this user is logged in on. From who, so a tmux pane running a
    # full-screen program is left undrawn-on. Each bounded: a terminal stopped with
    # ctrl-S would otherwise hold this write, and the button, for ever.
    n=0
    for tt in $(who 2>/dev/null | awk -v u="$(id -un)" '$1 == u {print $2}'); do
        [ -w "/dev/$tt" ] || continue
        printf '\r\n*** Fleet message: %s ***\r\n*** Answer with: fleet-chat ***\r\n' "$t" | timeout 2 tee "/dev/$tt" >/dev/null 2>&1 && n=$((n + 1))
    done
    [ "$n" = 0 ] || how="${how:+$how, }$n terminal(s)"
    printf '%s\n' "${how:-nobody here now}"
}
# Detached from this login entirely: a popup outlives the ssh that asked for it,
# and holds none of its descriptors, or the button would wait for someone to press OK.
away() { setsid "$@" </dev/null >/dev/null 2>&1 & }
# Stdin to a private file, a mebibyte at most. Prints its path.
take_stdin() {
    umask 077
    f=$(mktemp "$HOME/.fleet-in.XXXXXX") || exit 1
    head -c 1048577 > "$f"
    n=$(wc -c < "$f" | tr -d ' ')
    if [ "$n" = 0 ]; then rm -f "$f"; echo "refused: nothing arrived" >&2; exit 3; fi
    if [ "$n" -gt 1048576 ]; then rm -f "$f"; echo "refused: over 1 MiB" >&2; exit 3; fi
    printf '%s\n' "$f"
}
case "${1:-}" in
    has)
        case "${2:-}" in ''|*[!A-Za-z0-9._+-]*) exit 2 ;; esac
        if command -v "$2" >/dev/null 2>&1; then echo "yes program $(command -v "$2")"; exit 0; fi
        if command -v apk >/dev/null 2>&1 && apk info -e "$2" >/dev/null 2>&1; then echo "yes package $2"; exit 0; fi
        if command -v dpkg-query >/dev/null 2>&1 \
            && dpkg-query -W -f '${Status}' "$2" 2>/dev/null | grep -q 'ok installed'; then echo "yes package $2"; exit 0; fi
        echo no; exit 1 ;;
    notify)
        # The line itself reaches the room through the console; this is only telling
        # whoever is here to look.
        printf '%s\n' "$(announce "${2:-}")"
        exit 0 ;;
    observed)
        # Said to the room as well as the screen: whoever else is in it should know.
        t="Hello, you are being observed."
        printf 'orrery: %s\n' "$t" >> "$ROOM"
        printf '%s\n' "$t" >> "$OUT"
        announce "$t"
        exit 0 ;;
    chat-sync)
        # What was handed over last time is forgotten only once the console says it
        # arrived, so a dropped connection repeats a line rather than losing it.
        [ "${2:-}" = ack ] && rm -f "$OUT.sending"
        if [ -s "$OUT" ]; then
            mv "$OUT" "$OUT.$$" && cat "$OUT.$$" >> "$OUT.sending" && rm -f "$OUT.$$"
        fi
        head -c 1048576 | tr -d '\000-\011\013-\037\177' >> "$ROOM"
        [ -s "$OUT.sending" ] && cat "$OUT.sending"
        exit 0 ;;
    shot)
        # What is on this machine's screen, on stdout: a PNG from a desktop. A machine
        # with no desktop has no picture to take, so it gives the text of its terminal
        # sessions; tty1 itself is out of the user's reach and stays so.
        if session; then
            if [ -n "${WAYLAND_DISPLAY:-}" ] && [ "$SESSION" = Hyprland ] && command -v grim >/dev/null 2>&1; then
                exec grim -
            elif [ -n "${DISPLAY:-}" ] && command -v import >/dev/null 2>&1; then
                exec import -window root png:-
            elif [ -n "${DISPLAY:-}" ] && command -v scrot >/dev/null 2>&1; then
                exec scrot -o /dev/stdout
            fi
            echo "refused: the $SESSION desktop has no grim, import or scrot" >&2; exit 3
        fi
        if command -v tmux >/dev/null 2>&1 && tmux list-panes -a >/dev/null 2>&1; then
            tmux list-panes -a -F '#{session_name}:#{window_name}.#{pane_index}' | while read -r pn; do
                printf '=== %s, tmux %s, %s\n' "$(hostname)" "$pn" "$(date '+%Y-%m-%d %H:%M')"
                tmux capture-pane -p -t "$pn"
            done
            exit 0
        fi
        echo "refused: no desktop and no terminal session to read" >&2; exit 3 ;;
    clip)
        # ~/clipboard.txt ALWAYS: a headless machine has no other clipboard, and a
        # desktop's forgets at logout. It is the one file this verb replaces.
        f=$(take_stdin) || exit 3
        n=$(wc -c < "$f" | tr -d ' ')
        cp "$f" "$HOME/.clipboard.txt.new" && mv -f "$HOME/.clipboard.txt.new" "$HOME/clipboard.txt"
        said="$n bytes in ~/clipboard.txt"
        if session; then
            if [ -n "${WAYLAND_DISPLAY:-}" ] && command -v wl-copy >/dev/null 2>&1; then
                wl-copy < "$f" >/dev/null 2>&1 && said="$said and on the $SESSION clipboard"
            elif [ -n "${DISPLAY:-}" ] && command -v xclip >/dev/null 2>&1; then
                xclip -selection clipboard -i "$f" >/dev/null 2>&1 && said="$said and on the $SESSION clipboard"
            fi
        fi
        rm -f "$f"
        echo "$said"
        exit 0 ;;
    save)
        # The forced command has checked the name; so does this, a second door's lock.
        case "${2:-}" in ''|.*|*[!A-Za-z0-9._-]*) exit 2 ;; esac
        mkdir -p "$HOME/fleet-inbox"
        if [ -e "$HOME/fleet-inbox/$2" ]; then echo "refused: ~/fleet-inbox/$2 is already there" >&2; exit 3; fi
        f=$(take_stdin) || exit 3
        chmod 644 "$f"
        n=$(wc -c < "$f" | tr -d ' ')
        mv -n "$f" "$HOME/fleet-inbox/$2"
        echo "$n bytes in ~/fleet-inbox/$2"
        exit 0 ;;
esac
. /etc/os-release
k name "$(hostname)"
k listener_version "$(cat /etc/copal-fleet/version 2>/dev/null)"
if session; then k desktop "$SESSION"; fi
k os "$ID"
k version "$VERSION_ID"
k kernel "$(uname -r)"
k arch "$(uname -m)"
c=$(getconf _NPROCESSORS_ONLN 2>/dev/null || nproc)
k cpus "$c"
k mem_gb "$(awk '/MemTotal/ {print int($2/1048576+.5)}' /proc/meminfo)"
k load "$(cut -d' ' -f1 /proc/loadavg)"
k uptime_s "$(cut -d. -f1 /proc/uptime)"
k users "$(who 2>/dev/null | wc -l | tr -d ' ')"
if command -v nvidia-smi >/dev/null 2>&1; then
    k gpu "$(nvidia-smi --query-gpu=name --format=csv,noheader 2>/dev/null | head -1)"
    k gpu_busy "$(nvidia-smi --query-gpu=utilization.gpu --format=csv,noheader,nounits 2>/dev/null | head -1)"
    k driver "$(nvidia-smi --query-gpu=driver_version --format=csv,noheader 2>/dev/null | head -1)"
fi
for r in treadcore crucible orrery copal-tm; do
    d=$HOME/code/$r
    [ -d "$d/.git" ] || continue
    k "$r" "$(git -C "$d" log -1 --format='%h %cs' 2>/dev/null)"
done
k scene "$(cat /var/lib/copal-fleet/scene 2>/dev/null)"
k last_message "$(cat /var/lib/copal-fleet/last-message 2>/dev/null)"
FACTS
cat > /usr/local/bin/fleet-chat <<'CHATPROG'
#!/bin/sh
# orrery fleet-listener: Fleet Chat, one room for the whole fleet, hosted by the
# console. Type, press Return. The terminal is the input, the paste and the scroll
# bar; Ctrl-C leaves. Lines reach the others while orrery is open, and wait until then.
LOG="$HOME/.fleet-room.log"
OUT="$HOME/.fleet-room.out"
touch "$LOG"
echo $$ > "$HOME/.fleet-chat.pid"
echo "Fleet Chat, from $(hostname). Type and press Return; Ctrl-C leaves."
tail -n 200 -f "$LOG" &
T=$!
trap 'kill "$T" 2>/dev/null; rm -f "$HOME/.fleet-chat.pid"; exit 0' INT TERM HUP EXIT
while IFS= read -r line; do
    # What was typed comes back through the log; the echo of it is taken away.
    [ -t 1 ] && printf '\033[1A\033[2K'
    clean=$(printf '%s' "$line" | tr -d '\000-\037\177')
    [ -n "$clean" ] || continue
    printf 'you: %s\n' "$clean" >> "$LOG"
    printf '%s\n' "$clean" >> "$OUT"
done
CHATPROG
cat > /usr/local/bin/fleet-shot <<'SHOTPROG'
#!/bin/sh
# orrery fleet-listener: this machine's screen, to a file in ~/fleet-shots: a PNG
# from a desktop, the terminals' text from a machine with none.
d="$HOME/fleet-shots"; mkdir -p "$d"
f="$d/$(hostname)-$(date '+%Y%m%d-%H%M%S')"
/usr/local/bin/copal-fleet-facts shot > "$f.tmp" || { rm -f "$f.tmp"; exit 1; }
case "$(head -c 4 "$f.tmp" | od -An -c | tr -d ' ')" in
    *PNG) mv "$f.tmp" "$f.png"; echo "$f.png" ;;
    *) mv "$f.tmp" "$f.txt"; echo "$f.txt" ;;
esac
SHOTPROG
chmod 0755 /usr/local/bin/copal-fleet-power /usr/local/bin/copal-fleet-facts /usr/local/bin/fleet-chat /usr/local/bin/fleet-shot

# The one rule, proved by its own checker before it is put where it counts.
if [ -d /etc/doas.d ] && command -v doas >/dev/null 2>&1; then
    f=/etc/doas.d/zz-fleet-listener.conf
    { echo "# Written by orrery tools/fleet-listener.sh."
      echo "permit nopass $ACCT as root cmd /usr/local/bin/copal-fleet-power"
      echo "permit nopass nolog $ACCT as $U cmd /usr/local/bin/copal-fleet-facts"; } > "$f.tmp"
    chmod 0400 "$f.tmp"
    doas -C "$f.tmp" >/dev/null 2>&1 || { rm -f "$f.tmp"; echo "doas rejected the rule; run --undo" >&2; exit 1; }
    mv "$f.tmp" "$f"
elif [ -d /etc/sudoers.d ] && command -v sudo >/dev/null 2>&1; then
    f=/etc/sudoers.d/fleet-listener
    { echo "# Written by orrery tools/fleet-listener.sh."
      echo "$ACCT ALL=(root) NOPASSWD: /usr/local/bin/copal-fleet-power"
      echo "$ACCT ALL=($U) NOPASSWD: /usr/local/bin/copal-fleet-facts"
      # The room polls every few seconds: not a line in the journal each time.
      echo "Defaults!/usr/local/bin/copal-fleet-facts !syslog, !pam_session"; } > "$f.tmp"
    chmod 0440 "$f.tmp"
    visudo -cf "$f.tmp" >/dev/null || { rm -f "$f.tmp"; echo "visudo rejected the rule; run --undo" >&2; exit 1; }
    mv "$f.tmp" "$f"
else
    echo "neither doas nor sudo here: Power and the facts will be refused" >&2
fi

# The motd: Ubuntu builds its own from update-motd.d, Alpine prints a fixed
# file, so there the login shell reads it.
if [ -d /etc/update-motd.d ]; then
    cat > /etc/update-motd.d/02-copal-message <<'MOTD'
#!/bin/sh
# orrery fleet-listener: the last message sent to the fleet.
[ -s /var/lib/copal-fleet/last-message ] && printf '\n  Fleet message: %s\n\n' "$(cat /var/lib/copal-fleet/last-message)"
exit 0
MOTD
    chmod 0755 /etc/update-motd.d/02-copal-message
else
    mkdir -p /etc/profile.d
    cat > /etc/profile.d/copal-message.sh <<'MOTD'
# orrery fleet-listener: the last message sent to the fleet.
[ -s /var/lib/copal-fleet/last-message ] && printf '\n  Fleet message: %s\n\n' "$(cat /var/lib/copal-fleet/last-message)"
MOTD
    chmod 0644 /etc/profile.d/copal-message.sh
fi

# sshd_config, last: everything it names now exists. Proved, or taken back.
cp "$CFG" "$CFG.fleet.bak"
{ without_block | sed "s/^\(AllowUsers .*\)\$/\1 $ACCT/; s/\( $ACCT\)\( $ACCT\)*\$/\1/"
  echo "$BEGIN"
  echo "# Written by orrery tools/fleet-listener.sh; --undo takes it away."
  echo "TrustedUserCAKeys $CA"
  echo "AuthorizedPrincipalsFile /etc/ssh/principals/%u"
  echo "HostCertificate $HOSTCERT"
  echo "Match User $ACCT"
  echo "    ForceCommand $EXEC"
  echo "    PermitTTY no"
  echo "    X11Forwarding no"
  echo "    AllowTcpForwarding no"
  echo "    AllowAgentForwarding no"
  echo "    PermitOpen none"
  echo "    PasswordAuthentication no"
  echo "$END"
} > "$CFG.fleet.new"
cat "$CFG.fleet.new" > "$CFG"; rm -f "$CFG.fleet.new"
if ! sshd -t; then
    cat "$CFG.fleet.bak" > "$CFG"
    echo "sshd rejected the block; sshd_config is as it was" >&2
    exit 1
fi
reload_sshd
# A chat window from an older version shows a room nothing writes to any more, and
# reads as open, so nothing would open a new one. It goes; the next message opens one.
pkill -f /usr/local/bin/fleet-chat 2>/dev/null || true
echo "the listener is in: $(hostname) answers the fleet's certificates"
