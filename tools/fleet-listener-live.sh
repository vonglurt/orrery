#!/bin/sh
# orrery's own ssh client against a sshd configured by fleet-listener.sh and by
# nothing else: Terminal, Exchange and Send, with no real machine at risk.
#
#   sh tools/fleet-listener-live.sh alpine:3.22
#   sh tools/fleet-listener-live.sh ubuntu:24.04
#
# THE CLIENT IS HAND-WRITTEN AND SPEAKS ONLY ONE PROFILE, and these machines'
# sshd is stock: the listener deliberately does not narrow the daemon's ciphers,
# because other things log in to it. So the question is whether stock offers
# what the client insists on. Only a real handshake can answer that. The tests
# run on this Mac, the same binary the app is, through a published port.
set -eu
IMG="${1:-alpine:3.22}"
ROOT=$(cd "$(dirname "$0")/.." && pwd)
# Under home: colima shares that with its containers, and not /tmp.
mkdir -p "$HOME/.cache"; W=$(mktemp -d "$HOME/.cache/fleet-live.XXXXXX")
NAME=fleet-live-$$
trap 'docker rm -f "$NAME" >/dev/null 2>&1 || true; rm -rf "$W"' EXIT

cd "$W"
ssh-keygen -q -t ed25519 -N '' -C ca -f ca
ssh-keygen -q -t ed25519 -N '' -C op -f operator
ssh-keygen -q -s ca -I op -n fleet-operator,fleet-human -V -5m:+1h operator.pub
cp "$ROOT/tools/fleet-listener.sh" "$ROOT/tools/fleet-exec.sh" .

cat > inside.sh <<'IN'
set -e
if command -v apk >/dev/null; then
    apk add -q openrc openssh doas git shadow >/dev/null
    mkdir -p /etc/doas.d /run/openrc; touch /run/openrc/softlevel
    adduser -D -s /bin/sh user; echo 'user:*' | chpasswd -e
    echo 'AllowUsers user' >> /etc/ssh/sshd_config
else
    export DEBIAN_FRONTEND=noninteractive
    apt-get -qq update >/dev/null; apt-get -qq install -y openssh-server sudo git >/dev/null 2>&1
    mkdir -p /run/sshd
    useradd -m -s /bin/sh user; echo 'user:*' | chpasswd -e
fi
ssh-keygen -A >/dev/null
R=/home/user/orrery-recipes; mkdir -p "$R"
cp /f/fleet-listener.sh /f/fleet-exec.sh "$R"/; cp /f/ca.pub "$R/fleet-ca.pub"
cp /etc/ssh/ssh_host_ed25519_key.pub /tmp/h.pub
ssh-keygen -q -s /f/ca -I node -h -n museum-01 -V -5m:+1h /tmp/h.pub
cp /tmp/h-cert.pub "$R/fleet-host-cert.pub"
sh "$R/fleet-listener.sh"
exec /usr/sbin/sshd -D -e
IN
docker run -d --name "$NAME" -p 127.0.0.1:2299:22 -v "$W":/f:ro "$IMG" sh /f/inside.sh >/dev/null
i=0
until ssh-keyscan -T 2 -p 2299 127.0.0.1 2>/dev/null | grep -q ssh-ed25519; do
    i=$((i + 1)); [ "$i" -lt 90 ] || { docker logs "$NAME" | tail -20; echo "no sshd"; exit 1; }
    sleep 2
done

cd "$ROOT"
ORRERY_SSH_ADDR=127.0.0.1:2299 ORRERY_SSH_HOST=museum-01 ORRERY_SSH_USER=user \
ORRERY_SSH_KEY="$W/operator" ORRERY_SSH_CERT="$W/operator-cert.pub" ORRERY_SSH_CA="$W/ca.pub" \
    cargo test --release -- --test-threads=1 ssh::live sftp:: files:: 2>&1 | grep -E '^test |^test result'

# And the Fleet screen's own tool, the same forced command, by the same words.
F="$W/fdir"; mkdir -p "$F"
cp "$W/operator" "$W/operator-cert.pub" "$W/operator.pub" "$F/"
echo 'node1 127.0.0.1' > "$F/nodes"
ssh-keyscan -p 2299 -t ed25519 127.0.0.1 2>/dev/null | cut -d' ' -f1- > "$W/kh"
export COPAL_FLEET_DIR="$F" COPAL_SSH_OPTS="-p 2299 -o UserKnownHostsFile=$W/kh -o IdentitiesOnly=yes"
O="python3 $ROOT/tools/orchestrate.py $W"
OS=$(echo "$IMG" | cut -d: -f1)
fails=0
expect() { case "$3" in *"$2"*) echo "ok    $1" ;; *) echo "FAIL  $1: wanted '$2', got '$3'"; fails=$((fails + 1)) ;; esac; }
chose() { $O select "$@"; }
expect "select by system" '"chosen": ["node1"]' "$(chose "os=$OS")"
other=ubuntu; [ "$OS" = ubuntu ] && other=alpine
expect "select by the other system: none" '"chosen": []' "$(chose "os=$other")"
expect "or" '"chosen": ["node1"]' "$(chose "os=$other" or "os=$OS")"
expect "select by version" '"chosen": ["node1"]' "$(chose 'version>=3')"
expect "select by a version above it: none" '"chosen": []' "$(chose 'version>=99')"
expect "select by a package it has" '"chosen": ["node1"]' "$(chose has:git)"
expect "select by a package it has not: none" '"chosen": []' "$(chose has:no-such-thing)"
expect "select by a fact" '"chosen": ["node1"]' "$(chose cpus)"
expect "a message goes" '"ok": true' "$($O send node1 message Please stand back)"
expect "it is read back" 'Please stand back' "$($O send node1 last-message)"
expect "a message joins the room" "Please stand back" "$($O chat)"
expect "the facts carry it" 'Please stand back' "$(ssh -n -o BatchMode=yes -i "$F/operator" $COPAL_SSH_OPTS copal-fleet@127.0.0.1 facts)"
expect "a verb outside the list stops here" 'no verb called' "$($O send node1 rm -rf / 2>&1)"
expect "a bad message stops there" '"ok": false' "$($O send node1 'message $(id)')"
expect "a line reaches the room" "it's the room" "$($O say -- "it's the room")"
sleep 1
expect "and the machine has it" "it's the room" "$(docker exec "$NAME" cat /home/user/.fleet-room.log)"
docker exec "$NAME" su user -c 'echo from the machine >> ~/.fleet-room.out'
expect "a machine's line comes back, under its own name" "node1: from the machine" "$($O chat)"
expect "and is not repeated" "1" "$($O chat | python3 -c "import json,sys; print(sum('from the machine' in l for l in json.load(sys.stdin)['lines']))")"
own=$(docker exec "$NAME" grep -c 'node1: from the machine' /home/user/.fleet-room.log || true)
expect "a machine is not sent its own line" "0" "$own"
expect "clip lands in the file" 'clipboard.txt' "$($O text node1 clip -- 'some text')"
expect "save names the file" 'fleet-inbox/a.txt' "$($O text node1 save a.txt -- 'some text')"
# The treadcore question needs a checkout, so the user gets one.
docker exec "$NAME" sh -c "su user -c 'mkdir -p ~/code/treadcore && cd ~/code/treadcore && git init -q && git -c user.name=t -c user.email=t@t commit -q --allow-empty -m one'"
sha=$(docker exec "$NAME" su user -c 'git -C ~/code/treadcore log -1 --format=%h')
expect "select by the commit" '"chosen": ["node1"]' "$(chose "treadcore=$sha")"
expect "select by another commit: none" '"chosen": []' "$(chose treadcore=0000000)"
expect "select by the date" '"chosen": ["node1"]' "$(chose 'treadcore>=2020-01-01')"
expect "select by a later date: none" '"chosen": []' "$(chose 'treadcore>=2099-01-01')"
[ "$fails" = 0 ] && echo "ORCHESTRATE PASSED on $IMG" || { echo "ORCHESTRATE $fails FAILED"; exit 1; }
