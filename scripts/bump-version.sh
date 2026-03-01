#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage:
  scripts/bump-version.sh patch
  scripts/bump-version.sh minor
  scripts/bump-version.sh major
  scripts/bump-version.sh set <x.y.z>
USAGE
}

if [[ $# -lt 1 ]]; then
  usage
  exit 1
fi

mode="$1"
cargo_file="Cargo.toml"
current="$(awk -F'"' '/^version = / { print $2; exit }' "$cargo_file")"
if [[ -z "${current}" ]]; then
  echo "error: unable to locate version in ${cargo_file}" >&2
  exit 1
fi

IFS='.' read -r major minor patch <<<"${current}"
case "${mode}" in
  patch)
    patch="$((patch + 1))"
    next="${major}.${minor}.${patch}"
    ;;
  minor)
    minor="$((minor + 1))"
    patch=0
    next="${major}.${minor}.${patch}"
    ;;
  major)
    major="$((major + 1))"
    minor=0
    patch=0
    next="${major}.${minor}.${patch}"
    ;;
  set)
    if [[ $# -ne 2 ]]; then
      usage
      exit 1
    fi
    next="$2"
    if [[ ! "${next}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
      echo "error: version must be semver x.y.z" >&2
      exit 1
    fi
    ;;
  *)
    usage
    exit 1
    ;;
esac

perl -0pi -e "s/^version = \"[0-9]+\\.[0-9]+\\.[0-9]+\"/version = \"${next}\"/m" "${cargo_file}"
echo "updated ${cargo_file}: ${current} -> ${next}"
