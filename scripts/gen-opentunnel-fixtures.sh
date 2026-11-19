#!/usr/bin/env bash
# Regenerates crates/tvpipe/src/net/testdata/opentunnel/: a test CA and a tunnel
# certificate for tvtest.opentunnel.test and *.tvtest.opentunnel.test
# (P-256, valid until 2126), for the fake relay tests in net::opentunnel.
# Requires openssl (LibreSSL or OpenSSL).
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
dir="$root/crates/tvpipe/src/net/testdata/opentunnel"
mkdir -p "$dir"
cd "$dir"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
openssl ecparam -name prime256v1 -genkey -noout -out "$tmp/ca.key"
openssl req -x509 -new -key "$tmp/ca.key" -sha256 -days 36500 \
  -subj "/CN=tvpipe test CA" -out ca.pem \
  -extensions v3_ca -config <(printf '[req]\ndistinguished_name=dn\n[dn]\n[v3_ca]\nbasicConstraints=critical,CA:true\nkeyUsage=critical,keyCertSign,cRLSign\nsubjectKeyIdentifier=hash\n')
openssl ecparam -name prime256v1 -genkey -noout -out "$tmp/leaf.key"
openssl pkcs8 -topk8 -nocrypt -in "$tmp/leaf.key" -out private-key.pem
openssl req -new -key "$tmp/leaf.key" -subj "/CN=tvtest.opentunnel.test" -out "$tmp/leaf.csr"
printf 'basicConstraints=CA:false\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\nsubjectAltName=DNS:tvtest.opentunnel.test,DNS:*.tvtest.opentunnel.test\n' > "$tmp/ext"
openssl x509 -req -in "$tmp/leaf.csr" -CA ca.pem -CAkey "$tmp/ca.key" -CAcreateserial \
  -CAserial "$tmp/ca.srl" -sha256 -days 36500 -extfile "$tmp/ext" -out certificate.pem
cp ca.pem chain.pem
echo "wrote $dir"
