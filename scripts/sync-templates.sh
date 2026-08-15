#!/usr/bin/env bash
# Sync the vendored templates from their canonical home in OfficeMaster.
#
# Templates live in the OfficeMaster repo beside each brand's brief and its
# Word/PowerPoint masters. They are vendored here rather than cloned at image build so
# the Docker build needs no credentials, images stay hermetic, and drift shows up as a
# reviewable diff instead of two builds of the same tag quietly differing.
#
#   scripts/sync-templates.sh            # copy in, and record the upstream commit
#   scripts/sync-templates.sh --check    # fail if the vendored copy has drifted (CI)
#
# The check is a no-op when OfficeMaster is not checked out, so CI runners without it
# stay green rather than failing on something they cannot see.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SOURCE="${OFFICEMASTER_DIR:-$HOME/Code/thehansogroup/OfficeMaster}"
UPSTREAM="$REPO_ROOT/templates/UPSTREAM"

CHECK=false
[[ "${1:-}" == "--check" ]] && CHECK=true

if [[ ! -d "$SOURCE" ]]; then
  if $CHECK; then
    echo "sync-templates: $SOURCE not present; skipping drift check"
    exit 0
  fi
  echo "sync-templates: $SOURCE not found. Set OFFICEMASTER_DIR." >&2
  exit 1
fi

# brand -> the brand's Typst library within OfficeMaster.
#
# `hanso` currently lives in the standalone typst/ directory; the remaining brands will
# land under brands/<name>/typst/ as they are ported. Add a line per brand.
BRANDS=(
  "hanso:typst/hanso.typ"
)

fail=0
for entry in "${BRANDS[@]}"; do
  name="${entry%%:*}"
  rel="${entry#*:}"
  src="$SOURCE/$rel"
  dest="$REPO_ROOT/templates/$name/$(basename "$rel")"

  if [[ ! -f "$src" ]]; then
    echo "sync-templates: missing $src" >&2
    fail=1
    continue
  fi

  if $CHECK; then
    if ! diff -q "$src" "$dest" >/dev/null 2>&1; then
      echo "sync-templates: '$name' has drifted from $rel" >&2
      diff -u "$dest" "$src" | head -40 >&2 || true
      fail=1
    fi
  else
    cp "$src" "$dest"
    echo "sync-templates: updated $name"
  fi
done

if ! $CHECK && [[ $fail -eq 0 ]]; then
  commit="$(git -C "$SOURCE" rev-parse --short HEAD 2>/dev/null || echo unknown)"
  # `sed -i` needs an argument on BSD and must not have one on GNU; the .bak dance
  # works on both.
  sed -i.bak -E \
    -e "s/^commit = .*/commit = $commit/" \
    -e "s/^synced = .*/synced = $(date -u +%Y-%m-%d)/" \
    "$UPSTREAM"
  rm -f "$UPSTREAM.bak"
  echo "sync-templates: recorded upstream commit $commit"
fi

exit "$fail"
