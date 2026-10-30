#!/bin/sh
# fleet-listener.sh against a real sshd, in a container of each kind:
#
#   docker run --rm -v "$PWD/tools":/t:ro alpine:3.22 sh /t/fleet-listener-check.sh
#   docker run --rm -v "$PWD/tools":/t:ro ubuntu:24.04 sh /t/fleet-listener-check.sh
#
# WHAT THE MACHINES WOULD DO, in the order that matters: what must be refused is
# tried as hard as what must work, because a forced command that answers its
# verbs and also hands out a shell passes every test of the verbs.
set -u
fails=0
ok() { echo "ok    $1"; }
bad() { echo "FAIL  $1"; fails=$((fails + 1)); }
expect() { # name wanted got
    case "$3" in *"$2"*) ok "$1" ;; *) bad "$1: wanted '$2', got '$3'" ;; esac
}
refused() { # name got
    case "$2" in *refused*|*denied*|*"not allowed"*) ok "$1" ;; *) bad "$1: answered '$2'" ;; esac
}

if command -v apk >/dev/null; then
    apk add -q openrc openssh doas git shadow procps util-linux sed >/dev/null
    mkdir -p /etc/doas.d /run/openrc; touch /run/openrc/softlevel
    adduser -D -s /bin/sh user; echo 'user:*' | chpasswd -e
    echo 'AllowUsers user' >> /etc/ssh/sshd_config
else
    export DEBIAN_FRONTEND=noninteractive
    apt-get -qq update >/dev/null; apt-get -qq install -y openssh-server sudo git procps >/dev/null
    mkdir -p /run/sshd
    useradd -m -s /bin/sh user; echo 'user:*' | chpasswd -e
    mkdir -p /etc/update-motd.d
fi
ssh-keygen -A >/dev/null

# The fleet's side: a CA, the host's certificate, a human's, another CA's.
W=/tmp/fleet; mkdir -p "$W"; cd "$W" || exit 1
ssh-keygen -q -t ed25519 -N '' -C ca -f ca
ssh-keygen -q -t ed25519 -N '' -C ca -f otherca
ssh-keygen -q -t ed25519 -N '' -f op
ssh-keygen -q -s ca -I t -n fleet-operator,fleet-human -V +1h op.pub
ssh-keygen -q -t ed25519 -N '' -f wrong
ssh-keygen -q -s ca -I t -n somebody-else -V +1h wrong.pub
ssh-keygen -q -t ed25519 -N '' -f forged
ssh-keygen -q -s otherca -I t -n fleet-operator -V +1h forged.pub
cp /etc/ssh/ssh_host_ed25519_key.pub h.pub
ssh-keygen -q -s ca -I t -h -n node1,localhost -V +1h h.pub

R=/home/user/orrery-recipes; mkdir -p "$R"
cp /t/fleet-listener.sh /t/fleet-exec.sh "$R"/
cp ca.pub "$R/fleet-ca.pub"; cp h-cert.pub "$R/fleet-host-cert.pub"

# Refused before anything changes: a certificate for some other machine's key.
ssh-keygen -q -t ed25519 -N '' -f otherhost
ssh-keygen -q -s ca -I t -h -n x -V +1h otherhost.pub
cp otherhost-cert.pub "$R/fleet-host-cert.pub"
out=$(sh "$R/fleet-listener.sh" 2>&1); expect "another machine's certificate is refused" "another machine" "$out"
id copal-fleet >/dev/null 2>&1 && bad "the refusal left an account behind" || ok "the refusal changed nothing"
cp h-cert.pub "$R/fleet-host-cert.pub"

sh "$R/fleet-listener.sh" || bad "the install failed"
sh "$R/fleet-listener.sh" >/dev/null || bad "the second install failed"
n=$(grep -c 'BEGIN orrery fleet-listener' /etc/ssh/sshd_config); expect "run twice, one block" 1 "$n"
sshd -t && ok "sshd accepts the file" || bad "sshd rejects the file"
/usr/sbin/sshd -p 2222 -E /tmp/sshd.log

printf '@cert-authority localhost %s\n' "$(cat ca.pub)" > kh
S="ssh -p 2222 -o BatchMode=yes -o ConnectTimeout=5 -o UserKnownHostsFile=$W/kh -o StrictHostKeyChecking=yes -o LogLevel=ERROR"
op() { $S -i op copal-fleet@localhost "$@" 2>&1; }

expect "the host presents its certificate" "$(hostname)" "$(op id)"
expect "uptime" "load average" "$(op uptime)"
f=$(op facts)
expect "facts name the system" "os=" "$f"
expect "facts count the processors" "cpus=" "$f"
expect "has finds a program" "yes" "$(op has git)"
expect "has says no" "no" "$(op has no-such-thing)"
expect "scene is recorded" "scene: show" "$(op scene apply show)"
expect "a message is kept" "kept for the next login" "$(op message Please stand back, the fleet restarts at 5.)"
expect "the file holds it" "Please stand back" "$(cat /var/lib/copal-fleet/last-message)"
expect "last-message reads it" "Please stand back" "$(op last-message)"
if [ -x /etc/update-motd.d/02-copal-message ]; then m=$(/etc/update-motd.d/02-copal-message)
else m=$(sh /etc/profile.d/copal-message.sh); fi
expect "the motd reads it" "Fleet message:" "$m"
expect "facts carry it" "Please stand back" "$(op facts)"

# Text on stdin, and a pretend desktop: a process of the right name holding the
# environment a real session has, and stand-ins that record what they were handed.
# THE CHECK IS THAT THE POPUP OUTLIVES THE LOGIN AND DOES NOT HOLD IT OPEN.
opin() { printf '%s' "$1" | $S -i op copal-fleet@localhost "$2" 2>&1; }
expect "save writes the file" "fleet-inbox/notes.txt" "$(opin 'line one
line $(two) `three` "four"' 'save notes.txt')"
expect "save kept the text exactly" 'line $(two) `three` "four"' "$(cat /home/user/fleet-inbox/notes.txt)"
refused "save over a file" "$(opin x 'save notes.txt')"
refused "save with a path" "$(opin x 'save ../../.profile')"
refused "save a dotfile" "$(opin x 'save .profile')"
refused "save nothing" "$(opin '' 'save empty.txt')"
head -c 1048577 /dev/zero | tr '\0' x > big
refused "save over a mebibyte" "$($S -i op copal-fleet@localhost 'save big.txt' < big 2>&1)"
ls -a /home/user | grep -q '.fleet-in' && bad "a refused save left its temporary" || ok "no temporaries left"
expect "clip with no desktop still lands in the file" "bytes in ~/clipboard.txt" "$(opin 'hello
headless' clip)"
expect "the file holds it all" "headless" "$(cat /home/user/clipboard.txt)"
expect "clip replaces the file" "bytes in ~/clipboard.txt" "$(opin second clip)"
expect "with the new text" "second" "$(cat /home/user/clipboard.txt)"
expect "a message with no desktop still lands" "nobody here now" "$(op message Nobody home)"
cat > /usr/local/bin/xterm <<STUB
#!/bin/sh
echo "\$@" > /tmp/popup.args
sleep 4
echo survived > /tmp/popup.survived
STUB
chmod 755 /usr/local/bin/xterm
cat > /usr/local/bin/xclip <<'STUB'
#!/bin/sh
cat "$4" > /tmp/clip.got
STUB
chmod 755 /usr/local/bin/xclip
# GNU sed waiting on a fifo: one real program that stays up with its environment
# readable. Busybox's under another name is an applet nobody has heard of, and
# ssh-agent makes its own /proc entry private.
cp /usr/bin/sed /tmp/gnome-shell 2>/dev/null || cp /bin/sed /tmp/gnome-shell; chmod 755 /tmp/gnome-shell
mkfifo /tmp/desk.fifo; chmod 666 /tmp/desk.fifo
su user -s /bin/sh -c 'DISPLAY=:7 XDG_RUNTIME_DIR=/tmp/xdg-test /tmp/gnome-shell -n p /tmp/desk.fifo >/dev/null 2>&1 &' >/dev/null 2>&1
mkdir -p /tmp/xdg-test; chown user /tmp/xdg-test
sleep 1
t0=$(date +%s)
expect "a message opens a chat window" "a chat window" "$(op message Please stand back)"
t1=$(date +%s)
[ $((t1 - t0)) -lt 3 ] && ok "the button does not wait for the window's OK" || bad "the message held the login $((t1 - t0)) s"
sleep 1; expect "the window runs the chat" "fleet-chat" "$(cat /tmp/popup.args 2>/dev/null)"
expect "with a font" "-fa Monospace" "$(cat /tmp/popup.args 2>/dev/null)"
expect "with a scroll bar" "-sb" "$(cat /tmp/popup.args 2>/dev/null)"
sleep 4; expect "the window outlived the login" "survived" "$(cat /tmp/popup.survived 2>/dev/null)"
expect "facts name the desktop" "desktop=gnome-shell" "$(op facts)"
cat > /usr/local/bin/import <<'STUB'
#!/bin/sh
printf '\211PNG\r\n\032\nfake'
STUB
chmod 755 /usr/local/bin/import
expect "shot brings the desktop's picture" "PNG" "$(op shot | head -c 8)"
expect "and it is in the log" "SHOT" "$(tail -n 3 /var/log/copal-fleet.log)"
expect "it says hello first, in the room" "Hello, you are being observed." "$(cat /home/user/.fleet-room.log)"
expect "and tells the others" "Hello, you are being observed." "$(cat /home/user/.fleet-room.out)"
rm -f /tmp/popup.args; t0=$(wc -l < /home/user/.fleet-room.log)
op shot >/dev/null; sleep 1
expect "and the chat window opens" "fleet-chat" "$(cat /tmp/popup.args 2>/dev/null)"
n0=$(wc -l < /home/user/.fleet-room.log)
shot=$(su user -s /bin/sh -c /usr/local/bin/fleet-shot 2>&1); expect "fleet-shot saves a png" ".png" "$shot"
[ "$(wc -l < /home/user/.fleet-room.log)" = "$n0" ] && ok "your own fleet-shot says nothing: nobody is watching" || bad "fleet-shot announced itself"
expect "clip reaches the clipboard" "on the gnome-shell clipboard" "$(opin 'two
lines' clip)"
expect "the clipboard got it all" "lines" "$(cat /tmp/clip.got 2>/dev/null)"
pkill -x gnome-shell 2>/dev/null

# The room, as the console and a person at a headless machine have it.
sy() { printf '%s' "$1" | $S -i op copal-fleet@localhost "chat-sync $2" 2>&1; }
sy "11:00 calber: hello room
" '' >/dev/null
expect "the console's line reaches the room" "calber: hello room" "$(cat /home/user/.fleet-room.log)"
sy "bad$(printf '\033')[2Jline
" '' >/dev/null
case "$(cat /home/user/.fleet-room.log)" in *"$(printf '\033')"*) bad "a control character reached the room" ;; *) ok "control characters are taken out" ;; esac
su user -s /bin/sh -c 'printf "hello from the tty\n" | /usr/local/bin/fleet-chat >/dev/null 2>&1'
expect "the person's line shows as theirs" "you: hello from the tty" "$(cat /home/user/.fleet-room.log)"
expect "and goes to the console" "hello from the tty" "$(sy '' '')"
expect "unacknowledged, it is handed over again" "hello from the tty" "$(sy '' '')"
got=$(sy '' ack); [ -z "$got" ] && ok "acknowledged, it is forgotten" || bad "still handing over: $got"
refused "chat-sync with a word" "$(sy '' nonsense)"
# Shot: a desktop's picture came through above; here, a machine with none.
refused "shot with no desktop and no terminal" "$(op shot)"
expect "and it said hello even so" "being observed" "$(tail -n 2 /home/user/.fleet-room.log)"
ls /usr/local/bin/fleet-shot >/dev/null && ok "fleet-shot is installed" || bad "no fleet-shot"
refused "an unknown verb" "$(op rm -rf /)"
refused "a shell" "$(op sh -c id)"
refused "a verb after a semicolon" "$(op 'id; cat /etc/shadow')" 2>/dev/null
refused "a message with a quote" "$(op "message it's")"
refused "a message with a substitution" "$(op 'message $(id)')"
refused "a message with a backtick" "$(op 'message `id`')"
refused "a scene with a path" "$(op scene apply ../../etc/passwd)"
refused "has with a path" "$(op has /etc/passwd)"
refused "power without a word" "$(op power)"
refused "snapshot with nothing to restore" "$(op snapshot restore)"
long=$(printf 'x%.0s' $(seq 1 201)); refused "a long message" "$(op message "$long")"
tty=$($S -tt -i op copal-fleet@localhost id 2>&1); case "$tty" in *"PTY allocation request failed"*) ok "a tty is refused" ;; *) bad "tty: $tty" ;; esac
# A forward is only refused when something goes through it, so something does.
$S -i op -N -L 2223:localhost:2222 copal-fleet@localhost >/dev/null 2>&1 &
fw=$!; sleep 2
fwd=$(ssh-keyscan -T 3 -p 2223 localhost 2>/dev/null); kill "$fw" 2>/dev/null
case "$fwd" in *ssh-*) bad "forwarding carried a connection" ;; *) ok "forwarding carries nothing" ;; esac

# Who may come in at all.
out=$($S -i wrong copal-fleet@localhost id 2>&1); case "$out" in *denied*) ok "a certificate for another principal" ;; *) bad "wrong principal: $out" ;; esac
out=$($S -i forged copal-fleet@localhost id 2>&1); case "$out" in *denied*) ok "a certificate from another CA" ;; *) bad "forged: $out" ;; esac
expect "a human's certificate reaches the user's shell" "uid=" "$($S -i op user@localhost id 2>&1)"
out=$($S -i op root@localhost id 2>&1); case "$out" in *denied*) ok "and not root's" ;; *) bad "root: $out" ;; esac

# Power is the one verb that cannot be run here and live to report it, so the
# rule is read instead: it reaches the helper, and the helper nothing else.
if command -v doas >/dev/null; then
    su -s /bin/sh copal-fleet -c 'doas -n /usr/local/bin/copal-fleet-power nonsense' 2>&1 | grep -q 'off or reboot' \
        && ok "the rule reaches the power helper" || bad "the rule does not reach the helper"
    su -s /bin/sh copal-fleet -c 'doas -n /bin/cat /etc/shadow' >/dev/null 2>&1 && bad "the rule reaches cat" || ok "and nothing else"
else
    su -s /bin/sh copal-fleet -c 'sudo -n /usr/local/bin/copal-fleet-power nonsense' 2>&1 | grep -q 'off or reboot' \
        && ok "the rule reaches the power helper" || bad "the rule does not reach the helper"
    su -s /bin/sh copal-fleet -c 'sudo -n /bin/cat /etc/shadow' >/dev/null 2>&1 && bad "the rule reaches cat" || ok "and nothing else"
fi

# And away again: the old key's way in is untouched by leaving.
sh "$R/fleet-listener.sh" --undo >/dev/null || bad "the undo failed"
grep -q 'fleet-listener' /etc/ssh/sshd_config && bad "the undo left its block" || ok "the undo took its block"
sshd -t && ok "sshd accepts the file after" || bad "sshd rejects the file after"
id copal-fleet >/dev/null 2>&1 && bad "the undo left the account" || ok "the undo took the account"

echo; [ "$fails" = 0 ] && echo "ALL PASSED on $(. /etc/os-release; echo "$PRETTY_NAME")" || { echo "$fails FAILED"; exit 1; }
