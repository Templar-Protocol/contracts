#!/usr/bin/env bash
# Ensure a Postgres is available for the relayer's `#[sqlx::test]` fixtures and
# export DATABASE_URL. Idempotent and safe to source more than once. Respects a
# caller-provided DATABASE_URL — only when it is unset do we boot the local
# compose Postgres (waiting until it is healthy), so a machine with its own
# database, or without Docker, can opt out by setting DATABASE_URL itself.
#
# SOURCE this (it exports DATABASE_URL); do not execute it in a subshell.
_postgres_up_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [ -z "${DATABASE_URL:-}" ]; then
    docker compose \
        --file "${_postgres_up_root}/service/relayer/compose.dev.yaml" up postgres \
        --detach --wait
    export DATABASE_URL="postgres://relayeruser:password@localhost:5432/relayer"
fi
unset _postgres_up_root
