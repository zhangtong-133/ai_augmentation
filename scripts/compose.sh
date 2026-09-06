#!/usr/bin/env sh
set -eu

project_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
compose_file="$project_root/compose.yaml"

if docker compose version >/dev/null 2>&1; then
  exec docker compose --project-directory "$project_root" -f "$compose_file" "$@"
fi

if command -v docker-compose >/dev/null 2>&1; then
  exec docker-compose --project-directory "$project_root" -f "$compose_file" "$@"
fi

printf '%s\n' 'Docker Compose is not installed.' >&2
exit 1
