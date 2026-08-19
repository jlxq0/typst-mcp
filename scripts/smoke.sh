#!/usr/bin/env bash
# Black-box release smoke test. It deliberately avoids printing credentials or bodies.

set -euo pipefail

: "${BASE:?set BASE to the server origin}"
: "${API_KEY:?set API_KEY to the primary REST key secret}"
: "${OTHER_API_KEY:?set OTHER_API_KEY to a differently labelled REST key secret}"
: "${MCP_BEARER:?set MCP_BEARER to a valid OIDC access token}"

BASE=${BASE%/}
EXPECTED_VERSION=${EXPECTED_VERSION:-}
HTTP_TIMEOUT=${HTTP_TIMEOUT:-45}
PROTOCOL=2026-07-28

for command in curl jq tar; do
  command -v "$command" >/dev/null 2>&1 || {
    echo "smoke: missing required command: $command" >&2
    exit 2
  }
done

work=$(mktemp -d "${TMPDIR:-/tmp}/typst-mcp-smoke.XXXXXX")
cleanup_work() {
  case "$(basename "$work")" in
    typst-mcp-smoke.*) find "$work" -depth -delete ;;
  esac
}
trap cleanup_work EXIT
umask 077
printf 'Authorization: Bearer %s\n' "$API_KEY" >"$work/api.header"
printf 'Authorization: Bearer %s\n' "$OTHER_API_KEY" >"$work/other.header"
printf 'Authorization: Bearer %s\n' "$MCP_BEARER" >"$work/mcp.header"

step_number=0
step() {
  step_number=$((step_number + 1))
  printf 'smoke %d/10: %s\n' "$step_number" "$1"
}

fail() {
  echo "smoke failed at step $step_number: $*" >&2
  exit 1
}

expect_status() {
  local actual=$1 expected=$2 context=$3
  [[ "$actual" == "$expected" ]] || fail "$context returned HTTP $actual, expected $expected"
}

extract_rpc() {
  local input=$1 output=$2
  if jq -e . "$input" >/dev/null 2>&1; then
    jq -c . "$input" >"$output"
  else
    sed -n 's/^data:[[:space:]]*//p' "$input" | tail -n 1 | jq -c . >"$output"
  fi
}

mcp_call() {
  local method=$1 params=$2 tool_name=${3:-}
  jq -cn \
    --arg method "$method" \
    --argjson params "$params" \
    --arg protocol "$PROTOCOL" \
    '{jsonrpc:"2.0",id:1,method:$method,params:($params + {"_meta":{
      "io.modelcontextprotocol/protocolVersion":$protocol,
      "io.modelcontextprotocol/clientCapabilities":{},
      "io.modelcontextprotocol/clientInfo":{name:"typst-mcp-smoke",version:"1"}
    }})}' >"$work/mcp-request.json"

  local curl_args=(
    -sS --connect-timeout 5 --max-time "$HTTP_TIMEOUT"
    -o "$work/mcp-body" -w '%{http_code}'
    --header "@$work/mcp.header"
    --header 'Accept: application/json, text/event-stream'
    --header 'Content-Type: application/json'
    --header "MCP-Protocol-Version: $PROTOCOL"
    --header "MCP-Method: $method"
  )
  if [[ -n "$tool_name" ]]; then
    curl_args+=(--header "MCP-Name: $tool_name")
  fi
  local status
  status=$(curl "${curl_args[@]}" --data-binary "@$work/mcp-request.json" "$BASE/mcp")
  expect_status "$status" 200 "MCP $method"
  extract_rpc "$work/mcp-body" "$work/mcp-response.json"
  jq -e '.error == null and .result != null' "$work/mcp-response.json" >/dev/null \
    || fail "MCP $method returned a JSON-RPC error"
  jq -c '.result' "$work/mcp-response.json" >"$work/mcp-result.json"
}

step "health and version"
status=$(curl -sS --connect-timeout 5 --max-time "$HTTP_TIMEOUT" \
  -o "$work/body" -w '%{http_code}' "$BASE/health")
expect_status "$status" 200 health
jq -e '.status == "healthy" and (.version | type == "string")' "$work/body" >/dev/null \
  || fail "health payload is not healthy"
if [[ -n "$EXPECTED_VERSION" ]]; then
  actual_version=$(jq -r '.version' "$work/body")
  [[ "$actual_version" == "$EXPECTED_VERSION" ]] \
    || fail "health version is $actual_version, expected $EXPECTED_VERSION"
fi

step "REST authentication challenge"
printf '%s' '{"source":"= Unauthorized"}' >"$work/request.json"
status=$(curl -sS --connect-timeout 5 --max-time "$HTTP_TIMEOUT" \
  -D "$work/headers" -o "$work/body" -w '%{http_code}' \
  --header 'Content-Type: application/json' --data-binary "@$work/request.json" \
  "$BASE/api/v1/render")
expect_status "$status" 401 "unauthenticated render"
awk 'BEGIN { IGNORECASE=1 } /^www-authenticate:[[:space:]]*Bearer/ { found=1 } END { exit !found }' \
  "$work/headers" || fail "401 response has no Bearer challenge"

step "RFC 9728 discovery aliases"
curl -fsS --connect-timeout 5 --max-time "$HTTP_TIMEOUT" \
  "$BASE/.well-known/oauth-protected-resource/mcp" >"$work/metadata-mcp.json"
curl -fsS --connect-timeout 5 --max-time "$HTTP_TIMEOUT" \
  "$BASE/.well-known/oauth-protected-resource" >"$work/metadata-root.json"
jq -S . "$work/metadata-mcp.json" >"$work/metadata-mcp.sorted"
jq -S . "$work/metadata-root.json" >"$work/metadata-root.sorted"
cmp -s "$work/metadata-mcp.sorted" "$work/metadata-root.sorted" \
  || fail "protected-resource metadata aliases differ"
jq -e --arg resource "$BASE/mcp" --arg server "$BASE" \
  '.resource == $resource and .authorization_servers == [$server]' \
  "$work/metadata-mcp.json" >/dev/null || fail "protected-resource metadata names the wrong origin"

step "MCP lists exactly eight tools"
mcp_call tools/list '{}'
expected_tools='["typst_assets","typst_compile","typst_fonts","typst_link","typst_render","typst_template_schema","typst_templates","typst_upload_template"]'
actual_tools=$(jq -c '[.tools[].name] | sort' "$work/mcp-result.json")
[[ "$actual_tools" == "$expected_tools" ]] || fail "MCP tool catalogue is not the exact eight-tool contract"

step "MCP renders Hanso with a visible preview"
render_arguments=$(jq -cn '{
  template:"hanso",
  data:{title:"Release Smoke",date:"2026-08-19",theme:"light"},
  body:"= Release smoke\n\nA representative branded document.",
  preview_pages:[1]
}')
mcp_call tools/call "$(jq -cn --argjson arguments "$render_arguments" \
  '{name:"typst_render",arguments:$arguments}')" typst_render
jq -e '.isError != true and ([.content[] | select(.type == "image")] | length) >= 1' \
  "$work/mcp-result.json" >/dev/null || fail "typst_render returned no preview image"
jq -r '.content[] | select(.type == "text") | .text' "$work/mcp-result.json" \
  | head -n 1 | jq -e '.job_id and .url and (.pages >= 1)' >/dev/null \
  || fail "typst_render returned no document envelope"

step "REST render and authenticated PDF download"
printf '%s' "$render_arguments" >"$work/request.json"
status=$(curl -sS --connect-timeout 5 --max-time "$HTTP_TIMEOUT" \
  -o "$work/body" -w '%{http_code}' --header "@$work/api.header" \
  --header 'Content-Type: application/json' --data-binary "@$work/request.json" \
  "$BASE/api/v1/render")
expect_status "$status" 200 "REST Hanso render"
jq -e '.job_id and .url and (.pages >= 1)' "$work/body" >/dev/null \
  || fail "REST render returned no document envelope"
job_id=$(jq -r '.job_id' "$work/body")
pdf_url=$(jq -r '.url' "$work/body")
status=$(curl -sS --connect-timeout 5 --max-time "$HTTP_TIMEOUT" \
  -o "$work/document.pdf" -w '%{http_code}' --header "@$work/api.header" "$pdf_url")
expect_status "$status" 200 "authenticated PDF download"
[[ $(LC_ALL=C head -c 5 "$work/document.pdf") == '%PDF-' ]] || fail "download is not a PDF"

step "signed link works and tampering is rejected"
jq -cn --arg job_id "$job_id" '{job_id:$job_id,ttl_seconds:900}' >"$work/request.json"
status=$(curl -sS --connect-timeout 5 --max-time "$HTTP_TIMEOUT" \
  -o "$work/body" -w '%{http_code}' --header "@$work/api.header" \
  --header 'Content-Type: application/json' --data-binary "@$work/request.json" \
  "$BASE/api/v1/links")
expect_status "$status" 200 "signed-link creation"
signed_url=$(jq -r '.url' "$work/body")
status=$(curl -sS --connect-timeout 5 --max-time "$HTTP_TIMEOUT" \
  -o "$work/signed.pdf" -w '%{http_code}' "$signed_url")
expect_status "$status" 200 "anonymous signed download"
signature=$(printf '%s' "$signed_url" | sed -n 's/.*[?&]sig=\([^&]*\).*/\1/p')
expired_url="${signed_url%%\?*}?exp=1&sig=$signature"
status=$(curl -sS --connect-timeout 5 --max-time "$HTTP_TIMEOUT" \
  -o "$work/body" -w '%{http_code}' "$expired_url")
expect_status "$status" 410 "expired signed download"

step "ephemeral template isolation and render"
mkdir "$work/template"
cat >"$work/template/template.toml" <<'EOF'
name = "smoke-draft"
kind = "wrapper"
version = "1.0.0"
description = "Disposable release smoke template"
entrypoint = "draft.typ"
wrapper_fn = "smoke-doc"

[args.title]
arg = "title"
type = "str"
EOF
cat >"$work/template/draft.typ" <<'EOF'
#let smoke-doc(title: "Smoke", body) = [
  #set text(font: "Figtree")
  #heading(title)
  #body
]
EOF
(cd "$work/template" && tar -cf "$work/template.tar" template.toml draft.typ)
status=$(curl -sS --connect-timeout 5 --max-time "$HTTP_TIMEOUT" \
  -o "$work/body" -w '%{http_code}' --header "@$work/api.header" \
  --header 'Content-Type: application/x-tar' --data-binary "@$work/template.tar" \
  "$BASE/api/v1/templates")
expect_status "$status" 201 "template upload"
template_id=$(jq -r '.id' "$work/body")
[[ "$template_id" == tpl_* ]] || fail "template upload returned no tpl_ id"
status=$(curl -sS --connect-timeout 5 --max-time "$HTTP_TIMEOUT" \
  -o "$work/body" -w '%{http_code}' --header "@$work/other.header" \
  "$BASE/api/v1/templates/$template_id")
expect_status "$status" 404 "cross-tenant template lookup"
jq -cn --arg template "$template_id" \
  '{template:$template,data:{title:"Draft smoke"},body:"The draft rendered."}' \
  >"$work/request.json"
status=$(curl -sS --connect-timeout 5 --max-time "$HTTP_TIMEOUT" \
  -o "$work/body" -w '%{http_code}' --header "@$work/api.header" \
  --header 'Content-Type: application/json' --data-binary "@$work/request.json" \
  "$BASE/api/v1/render")
expect_status "$status" 200 "ephemeral template render"

step "broken source returns positioned diagnostics"
jq -cn --arg source $'= Heading\n#let broken = (' '{source:$source}' >"$work/request.json"
status=$(curl -sS --connect-timeout 5 --max-time "$HTTP_TIMEOUT" \
  -o "$work/body" -w '%{http_code}' --header "@$work/api.header" \
  --header 'Content-Type: application/json' --data-binary "@$work/request.json" \
  "$BASE/api/v1/render")
expect_status "$status" 422 "broken source"
jq -e '[.. | objects | select(has("line") and (.line | type == "number"))] | length > 0' \
  "$work/body" >/dev/null || fail "compile failure contains no positioned diagnostic"

step "finite CPU timeout and recovery"
runaway='#let acc = 0
#for i in range(20000) { for j in range(20000) { acc = acc + 1 } }
#acc'
jq -cn --arg source "$runaway" '{source:$source,preview_pages:[]}' >"$work/request.json"
status=$(curl -sS --connect-timeout 5 --max-time "$HTTP_TIMEOUT" \
  -o "$work/body" -w '%{http_code}' --header "@$work/api.header" \
  --header 'Content-Type: application/json' --data-binary "@$work/request.json" \
  "$BASE/api/v1/render?output=pdf")
expect_status "$status" 504 "finite CPU-bound compile"
jq -cn '{source:"= Still serving",preview_pages:[]}' >"$work/request.json"
status=$(curl -sS --connect-timeout 5 --max-time "$HTTP_TIMEOUT" \
  -o "$work/recovery.pdf" -w '%{http_code}' --header "@$work/api.header" \
  --header 'Content-Type: application/json' --data-binary "@$work/request.json" \
  "$BASE/api/v1/render?output=pdf")
expect_status "$status" 200 "post-timeout recovery compile"
[[ $(LC_ALL=C head -c 5 "$work/recovery.pdf") == '%PDF-' ]] \
  || fail "post-timeout recovery did not return a PDF"

echo "smoke: all 10 steps passed"
