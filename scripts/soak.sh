#!/usr/bin/env bash
# Sequential distinct-filename soak. Measures the long-lived server/container, not workers.

set -euo pipefail

: "${BASE:?set BASE to the server origin}"
: "${API_KEY:?set API_KEY to the REST key secret}"

BASE=${BASE%/}
ITERATIONS=${ITERATIONS:-10000}
WARMUP=${WARMUP:-100}
SAMPLE_EVERY=${SAMPLE_EVERY:-100}
MAX_RSS_RATIO=${MAX_RSS_RATIO:-1.5}
HTTP_TIMEOUT=${HTTP_TIMEOUT:-45}

[[ "$ITERATIONS" =~ ^[0-9]+$ && "$ITERATIONS" -ge "$WARMUP" ]] \
  || { echo "soak: ITERATIONS must be an integer >= WARMUP" >&2; exit 2; }

for command in curl awk; do
  command -v "$command" >/dev/null 2>&1 || {
    echo "soak: missing required command: $command" >&2
    exit 2
  }
done

if [[ -z ${CONTAINER:-} && -z ${RSS_PID:-} ]]; then
  echo "soak: set CONTAINER for docker stats or RSS_PID for ps-based RSS measurement" >&2
  exit 2
fi

work=$(mktemp -d "${TMPDIR:-/tmp}/typst-mcp-soak.XXXXXX")
cleanup_work() {
  case "$(basename "$work")" in
    typst-mcp-soak.*) find "$work" -depth -delete ;;
  esac
}
trap cleanup_work EXIT
umask 077
printf 'Authorization: Bearer %s\n' "$API_KEY" >"$work/api.header"

to_bytes() {
  awk '
    function multiplier(unit) {
      if (unit == "KiB" || unit == "kB" || unit == "KB") return 1024
      if (unit == "MiB" || unit == "MB") return 1024 * 1024
      if (unit == "GiB" || unit == "GB") return 1024 * 1024 * 1024
      if (unit == "TiB" || unit == "TB") return 1024 * 1024 * 1024 * 1024
      return 1
    }
    {
      value=$1
      unit=value
      gsub(/[0-9.]/, "", unit)
      gsub(/[^0-9.]/, "", value)
      printf "%.0f\n", value * multiplier(unit)
    }
  '
}

rss_bytes() {
  if [[ -n ${CONTAINER:-} ]]; then
    docker stats --no-stream --format '{{.MemUsage}}' "$CONTAINER" \
      | awk -F/ '{ gsub(/^[[:space:]]+|[[:space:]]+$/, "", $1); print $1 }' \
      | to_bytes
  else
    local rss_kib
    rss_kib=$(ps -o rss= -p "$RSS_PID" | awk '{print $1}')
    [[ -n "$rss_kib" ]] || return 1
    echo $((rss_kib * 1024))
  fi
}

restart_count() {
  if [[ -n ${CONTAINER:-} ]]; then
    docker inspect --format '{{.RestartCount}}' "$CONTAINER"
  else
    echo 0
  fi
}

start_restarts=$(restart_count)
warm_rss=0
end_rss=0
peak_rss=0

for ((iteration = 1; iteration <= ITERATIONS; iteration++)); do
  printf -v filename 'soak-%06d.typ' "$iteration"
  printf -v payload \
    '{"files":[{"path":"%s","text":"= Distinct soak document %06d"}],"main":"%s","preview_pages":[]}' \
    "$filename" "$iteration" "$filename"
  status=$(curl -sS --connect-timeout 5 --max-time "$HTTP_TIMEOUT" \
    -o /dev/null -w '%{http_code}' --header "@$work/api.header" \
    --header 'Content-Type: application/json' --data-binary "$payload" \
    "$BASE/api/v1/compile?output=pdf")
  [[ "$status" == 200 ]] || {
    echo "soak: compile $iteration returned HTTP $status" >&2
    exit 1
  }

  if ((iteration == WARMUP || iteration == ITERATIONS || iteration % SAMPLE_EVERY == 0)); then
    current_rss=$(rss_bytes) || {
      echo "soak: could not measure RSS at iteration $iteration" >&2
      exit 1
    }
    ((current_rss > peak_rss)) && peak_rss=$current_rss
    ((iteration == WARMUP)) && warm_rss=$current_rss
    ((iteration == ITERATIONS)) && end_rss=$current_rss
    printf 'soak: %d/%d rss=%d bytes\n' "$iteration" "$ITERATIONS" "$current_rss"
  fi
done

end_restarts=$(restart_count)
[[ "$end_restarts" == "$start_restarts" ]] || {
  echo "soak: restart count changed from $start_restarts to $end_restarts" >&2
  exit 1
}

awk -v end="$end_rss" -v warm="$warm_rss" -v ratio="$MAX_RSS_RATIO" \
  'BEGIN { exit !(warm > 0 && end <= warm * ratio) }' || {
  echo "soak: ending RSS $end_rss exceeds ${MAX_RSS_RATIO}x warm RSS $warm_rss" >&2
  exit 1
}

echo "soak: $ITERATIONS distinct filenames passed; warm=$warm_rss end=$end_rss peak=$peak_rss restarts=$end_restarts"
