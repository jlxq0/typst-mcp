#!/usr/bin/env bash
# Prepare a disposable RSA-signed OIDC token plus static discovery/JWKS files.
# The private key lives only in a mode-0700 temporary directory and is removed on exit.

set -euo pipefail

if [[ $# -ne 5 ]]; then
  echo "usage: $0 OUTPUT_DIR TOKEN_FILE ISSUER AUDIENCE TENANT_ID" >&2
  exit 2
fi

output_dir=$1
token_file=$2
issuer=${3%/}
audience=$4
tenant_id=$5

for command in jq openssl xxd; do
  command -v "$command" >/dev/null 2>&1 || {
    echo "missing required command: $command" >&2
    exit 2
  }
done

private_dir=$(mktemp -d "${TMPDIR:-/tmp}/typst-mcp-smoke-oidc.XXXXXX")
cleanup_private() {
  case "$(basename "$private_dir")" in
    typst-mcp-smoke-oidc.*) find "$private_dir" -depth -delete ;;
  esac
}
trap cleanup_private EXIT
umask 077

key_file="$private_dir/signing.pem"
signing_input="$private_dir/signing-input"
signature_file="$private_dir/signature"
kid=typst-mcp-smoke

openssl genpkey -quiet -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out "$key_file"
modulus_hex=$(openssl rsa -in "$key_file" -noout -modulus | sed 's/^Modulus=//')

base64url() {
  openssl base64 -A | tr '+/' '-_' | tr -d '='
}

modulus=$(printf '%s' "$modulus_hex" | xxd -r -p | base64url)
now=$(date +%s)
expires=$((now + 3600))

header=$(jq -cn --arg kid "$kid" '{alg:"RS256",typ:"JWT",kid:$kid}')
payload=$(jq -cn \
  --arg sub "typst-mcp-smoke" \
  --arg iss "$issuer" \
  --arg aud "$audience" \
  --arg tid "$tenant_id" \
  --argjson iat "$((now - 60))" \
  --argjson nbf "$((now - 60))" \
  --argjson exp "$expires" \
  '{sub:$sub,iss:$iss,aud:$aud,tid:$tid,scp:"render",iat:$iat,nbf:$nbf,exp:$exp}')

encoded_header=$(printf '%s' "$header" | base64url)
encoded_payload=$(printf '%s' "$payload" | base64url)
printf '%s.%s' "$encoded_header" "$encoded_payload" >"$signing_input"
openssl dgst -sha256 -sign "$key_file" -out "$signature_file" "$signing_input"
signature=$(base64url <"$signature_file")

if [[ -e "$output_dir" ]]; then
  echo "refusing to replace existing OIDC output path: $output_dir" >&2
  exit 2
fi
mkdir -p "$output_dir/.well-known"
printf '%s.%s\n' "$(cat "$signing_input")" "$signature" >"$token_file"
chmod 600 "$token_file"

jq -cn \
  --arg issuer "$issuer" \
  --arg jwks_uri "$issuer/keys" \
  '{issuer:$issuer,jwks_uri:$jwks_uri}' \
  >"$output_dir/.well-known/openid-configuration"
jq -cn \
  --arg kid "$kid" \
  --arg n "$modulus" \
  '{keys:[{kty:"RSA",kid:$kid,use:"sig",alg:"RS256",n:$n,e:"AQAB"}]}' \
  >"$output_dir/keys"

echo "prepared disposable OIDC discovery, JWKS, and token"
