#!/bin/sh
# Run orrery's RDP client against a real RDP server.
#
# docs/wire.md's R2 says nothing in this repository has ever spoken to
# hypr-rdp, and that is still true. This is the next best thing and it is not
# nothing: a server written by other people, speaking the MS-RDPBCGR a node's
# server will speak.
#
# WHY xrdp AND NOT FreeRDP'S SAMPLE SERVER, which was tried first and is the
# more obvious choice: FreeRDP's server cannot complete a TLS handshake with an
# Ed25519 certificate. It derives the "tls-server-end-point" channel bindings
# by hashing the certificate with the digest named in its signature algorithm,
# and Ed25519 names none -- so it logs `unable to retrieve bindings` and drops
# the connection before RDP begins. That is a real finding about the fleet's
# Ed25519-only decision and it is written up as R5 in docs/wire.md.
#
#   docker run --rm --platform linux/arm64 -v "$PWD":/w -w /w \
#       -e CARGO_TARGET_DIR=/tmp/t rust:alpine sh /w/tools/rdp-check.sh
set -e
apk add --no-cache musl-dev openssl xrdp >/dev/null 2>&1

D=/tmp/rdpfleet
rm -rf "$D"; mkdir -p "$D"; cd "$D"

# The same fleet shape as the SSH and TLS checks: one CA, one node.
openssl genpkey -algorithm ed25519 -out ca.key
openssl req -x509 -new -key ca.key -days 3650 -subj "/CN=copal fleet CA" -out ca.pem
openssl genpkey -algorithm ed25519 -out node.key
openssl req -new -key node.key -subj "/CN=museum-01" -out node.csr
printf 'subjectAltName=DNS:museum-01\nbasicConstraints=CA:FALSE\n' > node.ext
openssl x509 -req -in node.csr -CA ca.pem -CAkey ca.key -days 365 \
    -extfile node.ext -out node.pem
chmod 644 "$D/node.pem"; chmod 600 "$D/node.key"

# TLS ONLY, AND 1.3 ONLY -- the same narrowness profile.rs asks a node for.
# xrdp would otherwise negotiate its own RC4 security layer, which is the thing
# PROTOCOL_SSL exists to refuse.
sed -i "s|^security_layer=.*|security_layer=tls|; \
        s|^certificate=.*|certificate=$D/node.pem|; \
        s|^key_file=.*|key_file=$D/node.key|; \
        s|^ssl_protocols=.*|ssl_protocols=TLSv1.3|" /etc/xrdp/xrdp.ini
mkdir -p /var/run/xrdp

xrdp --nodaemon > "$D/xrdp.log" 2>&1 &
sleep 2

export ORRERY_RDP_ADDR=127.0.0.1:3389
export ORRERY_RDP_HOST=museum-01
export ORRERY_RDP_CA="$D/ca.pem"

cd /w
status=0
cargo test rdp:: -- --test-threads=1 --nocapture || status=$?
echo "--- xrdp said ---"
tail -20 "$D/xrdp.log" || true
kill %1 2>/dev/null || true
exit $status
