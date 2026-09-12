#!/bin/sh
# Run orrery's TLS 1.3 client against OpenSSL's own server.
#
# The key schedule is the part of TLS a reader gets subtly wrong and then
# implements twice, so there is no fake server in src/tls.rs and this is the
# only proof that the client is right. It builds the fleet shape: an Ed25519
# CA, a node certificate naming museum-01, and a client certificate for the
# operator -- which is the mutual TLS that docs/lockdown.md says is what stops
# a standard RDP client.
#
#   docker run --rm --platform linux/arm64 -v "$PWD":/w -w /w \
#       -e CARGO_TARGET_DIR=/tmp/t rust:alpine sh /w/tools/tls-check.sh
#
# macOS ships LibreSSL, which has no Ed25519 at all, so this cannot run there.
set -e
apk add --no-cache musl-dev openssl >/dev/null 2>&1

D=/tmp/tlsfleet
rm -rf "$D"; mkdir -p "$D"; cd "$D"

openssl genpkey -algorithm ed25519 -out ca.key
openssl req -x509 -new -key ca.key -days 3650 -subj "/CN=copal fleet CA" -out ca.pem

openssl genpkey -algorithm ed25519 -out node.key
openssl req -new -key node.key -subj "/CN=museum-01" -out node.csr
printf 'subjectAltName=DNS:museum-01\nbasicConstraints=CA:FALSE\n' > node.ext
openssl x509 -req -in node.csr -CA ca.pem -CAkey ca.key -days 365 \
    -extfile node.ext -out node.pem

openssl genpkey -algorithm ed25519 -out operator.key
openssl req -new -key operator.key -subj "/CN=operator" -out operator.csr
printf 'subjectAltName=DNS:fleet-operator\nbasicConstraints=CA:FALSE\n' > op.ext
openssl x509 -req -in operator.csr -CA ca.pem -CAkey ca.key -days 365 \
    -extfile op.ext -out operator.pem

# Three servers: one that asks for nothing, one that demands a client
# certificate from the fleet CA, and one that will only do AES -- which this
# console must refuse rather than crawl to. `-rev` sends each line back
# reversed, which is the smallest proof the record layer works both ways.
openssl s_server -tls1_3 -cert node.pem -key node.key -accept 4434 -rev -naccept 20 \
    >/dev/null 2>&1 &
openssl s_server -tls1_3 -cert node.pem -key node.key -accept 4433 -rev -naccept 20 \
    -CAfile ca.pem -Verify 1 >/dev/null 2>&1 &
openssl s_server -tls1_3 -cert node.pem -key node.key -accept 4435 -rev -naccept 20 \
    -ciphersuites TLS_AES_256_GCM_SHA384 >/dev/null 2>&1 &
sleep 1

export ORRERY_TLS_PLAIN=127.0.0.1:4434
export ORRERY_TLS_MUTUAL=127.0.0.1:4433
export ORRERY_TLS_AES=127.0.0.1:4435
export ORRERY_TLS_CA="$D/ca.pem"
export ORRERY_TLS_CLIENT_CERT="$D/operator.pem"
export ORRERY_TLS_CLIENT_KEY="$D/operator.key"

cd /w
status=0
cargo test tls:: -- --test-threads=1 --nocapture || status=$?
cargo test x509:: || status=$?
kill %1 %2 %3 2>/dev/null || true
exit $status
