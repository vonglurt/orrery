#!/bin/sh
# Run orrery's SSH client against a real OpenSSH server.
#
# The tests in src/ssh.rs prove the client against `ssh::fake`, which is
# useful and is not the same thing: a hand-written server agrees with the
# hand-written client's reading of the RFC, including wherever that reading is
# wrong. THIS IS THE ROW IN docs/plan.md THAT MATTERS -- sshd is stricter than
# the specification in a dozen places, and the fleet's nodes run sshd.
#
#   docker run --rm --platform linux/arm64 -v "$PWD":/w -w /w \
#       -e CARGO_TARGET_DIR=/tmp/t rust:alpine sh /w/tools/ssh-check.sh
#
# It builds the same fleet shape copal-prep.sh builds: a CA, a host certificate
# for a node called museum-01, an operator certificate carrying the
# fleet-operator and fleet-human principals, a principals file, and the sshd
# lines profile.rs renders.
set -e
apk add --no-cache musl-dev openssh openssh-server >/dev/null 2>&1

D=/tmp/fleet
rm -rf "$D"; mkdir -p "$D" /etc/ssh/principals
cd "$D"

# The certificate authority, and the two certificates it issues.
ssh-keygen -q -t ed25519 -f "$D/fleet_ca" -N '' -C 'copal fleet CA'
rm -f /etc/ssh/ssh_host_ed25519_key /etc/ssh/ssh_host_ed25519_key.pub
ssh-keygen -q -t ed25519 -f /etc/ssh/ssh_host_ed25519_key -N ''
ssh-keygen -q -s "$D/fleet_ca" -I 'museum-01 host key' -h -n museum-01 \
    -V -5m:+60m /etc/ssh/ssh_host_ed25519_key.pub
ssh-keygen -q -t ed25519 -f "$D/operator" -N '' -C 'operator@museum'
ssh-keygen -q -s "$D/fleet_ca" -I operator -n fleet-operator,fleet-human \
    -V -5m:+60m "$D/operator.pub"

# The account the console logs into, and the principals it may present.
adduser -D -s /bin/sh copal 2>/dev/null || true
# A locked password field makes sshd refuse the account before it looks at a
# key, which reads as an authentication failure and is not one.
sed -i 's/^copal:!:/copal:*:/' /etc/shadow
printf 'fleet-operator\nfleet-human\n' > /etc/ssh/principals/copal

cat > /etc/ssh/sshd_config_orrery <<CFG
Port 2222
ListenAddress 127.0.0.1
HostKey /etc/ssh/ssh_host_ed25519_key
HostCertificate /etc/ssh/ssh_host_ed25519_key-cert.pub
TrustedUserCAKeys $D/fleet_ca.pub
AuthorizedPrincipalsFile /etc/ssh/principals/%u
AuthorizedKeysFile none
LogLevel VERBOSE
PidFile $D/sshd.pid
CFG

# The profile's own lines, rendered by the program under test rather than
# copied here -- if the whitelist and the server's configuration can disagree,
# this script is testing the wrong thing.
(cd /w && cargo run --quiet -- --profile-sshd) >> /etc/ssh/sshd_config_orrery

mkdir -p /var/empty
/usr/sbin/sshd -f /etc/ssh/sshd_config_orrery -E "$D/sshd.log"
sleep 1
if ! [ -f "$D/sshd.pid" ]; then echo "sshd did not start:"; cat "$D/sshd.log"; exit 1; fi

export ORRERY_SSH_ADDR=127.0.0.1:2222
export ORRERY_SSH_HOST=museum-01
export ORRERY_SSH_USER=copal
export ORRERY_SSH_KEY="$D/operator"
export ORRERY_SSH_CERT="$D/operator-cert.pub"
export ORRERY_SSH_CA="$D/fleet_ca.pub"

cd /w
status=0
cargo test ssh:: -- --nocapture --test-threads=1 || status=$?
echo "--- sshd said ---"
tail -30 "$D/sshd.log" || true
kill "$(cat "$D/sshd.pid")" 2>/dev/null || true
exit $status
