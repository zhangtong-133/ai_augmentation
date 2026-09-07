#!/usr/bin/env sh
set -eu

failures=0

required() {
  label="$1"
  command_name="$2"
  if command -v "$command_name" >/dev/null 2>&1; then
    printf '[ok]   %-16s %s\n' "$label" "$("$command_name" --version 2>/dev/null | head -n 1)"
  else
    printf '[fail] %-16s missing\n' "$label"
    failures=$((failures + 1))
  fi
}

optional() {
  label="$1"
  command_name="$2"
  if command -v "$command_name" >/dev/null 2>&1; then
    printf '[ok]   %-16s installed\n' "$label"
  else
    printf '[warn] %-16s missing (optional)\n' "$label"
  fi
}

required Rust rustc
required Cargo cargo
required Node node
required npm npm
required Docker docker
required Git git
optional pnpm pnpm
optional just just
optional protoc protoc

if docker info >/dev/null 2>&1; then
  printf '[ok]   %-16s reachable\n' 'Docker daemon'
else
  printf '[warn] %-16s inaccessible from this process; check sandbox restrictions, socket permissions, context, and service status\n' 'Docker daemon'
  printf '       Recheck docker info in a normal terminal or an approved unsandboxed check before diagnosing a host failure.\n'
fi

if docker compose version >/dev/null 2>&1; then
  printf '[ok]   %-16s v2\n' 'Compose'
elif command -v docker-compose >/dev/null 2>&1; then
  printf '[warn] %-16s legacy v1; supported temporarily\n' 'Compose'
else
  printf '[warn] %-16s missing\n' 'Compose'
fi

if [ "$failures" -ne 0 ]; then
  exit 1
fi
