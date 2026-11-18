#!/usr/bin/env bash
# Regenerates src/net/roots/mozilla.pem (and mozilla.rs from it): the root certificates Mozilla
# trusts for websites, from the CCADB (Common CA Database) report, which
# is published under CDLA-Permissive-2.0
# (LICENSES/CDLA-Permissive-2.0-ccadb.txt). The same source as the
# webpki-roots crate. tvpipe uses them only for its HTTPS clients (the
# OpenTunnel service, docs/tunnels.md). Run it now and then (CAs are
# added and removed a few times a year) and commit the result.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
out="$root/src/net/roots/mozilla.pem"
url="https://ccadb.my.salesforce-sites.com/mozilla/IncludedRootsPEMTxt?TrustBitsInclude=Websites"
tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT
curl -fsSL --proto '=https' -o "$tmp" "$url"
n="$(grep -c 'BEGIN CERTIFICATE' "$tmp")"
if [ "$n" -lt 100 ]; then
  echo "only $n certificates; refusing to update" >&2
  exit 1
fi
{
  echo "# Mozilla's included root certificates with the Websites trust bit."
  echo "# Source: $url"
  echo "# Fetched: $(date -u +%Y-%m-%d), $n certificates."
  echo "# License: CDLA-Permissive-2.0 (LICENSES/CDLA-Permissive-2.0-ccadb.txt)."
  echo "# Regenerate with scripts/update-roots.sh."
  tr -d '\r' < "$tmp"
} > "$out"
echo "wrote $out ($n certificates)"
# The binary carries only the trust anchors (subject, key, constraints).
(cd "$root" && cargo test --quiet regenerate_roots -- --ignored >/dev/null)
echo "wrote ${out%.pem}.rs"
