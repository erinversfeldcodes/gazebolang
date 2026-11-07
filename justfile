set dotenv-load := true

# Show available recipes
default:
    @just --list

# Start local Postgres + Garage S3 (image storage)
db-up:
    docker compose up -d postgres garage

# Stop local services
db-down:
    docker compose down

# One-time Garage bootstrap: cluster layout, image bucket, and a fixed
# local-dev access key. Run once after `just db-up` on a fresh volume;
# safe to re-run (steps that are already done are skipped/ignored).
storage-init:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "waiting for garage to accept commands..."
    until docker compose exec -T garage /garage node id -q >/dev/null 2>&1; do sleep 1; done
    node_id=$(docker compose exec -T garage /garage node id -q | cut -d'@' -f1)
    if docker compose exec -T garage /garage layout show 2>/dev/null | grep -q "Current cluster layout version: 0"; then
        docker compose exec -T garage /garage layout assign -z dc1 -c 1G "$node_id"
        docker compose exec -T garage /garage layout apply --version 1
    fi
    docker compose exec -T garage /garage key import gazebo-local-dev-key gazebo-local-dev-secret-access-key-0000000000000000 --yes -n gazebo || true
    docker compose exec -T garage /garage bucket create gazebo-cards || true
    docker compose exec -T garage /garage bucket allow --read --write gazebo-cards --key gazebo-local-dev-key
    docker compose exec -T garage /garage bucket website --allow gazebo-cards
    echo "garage ready: bucket=gazebo-cards key=gazebo-local-dev-key"

# One-time/idempotent: clone (or update, if already cloned) the private
# fixtures repo crates/importer's golden regression test needs — real
# photographed dictionary pages, kept out of this public repo (see that
# repo's own README). Lands as a sibling directory, not tracked here.
fixtures-pull:
    #!/usr/bin/env bash
    set -euo pipefail
    dir="../ulungelelwano-lwe-gazebo-fixtures"
    if [ -d "$dir/.git" ]; then
        git -C "$dir" pull --ff-only origin main
    else
        git clone git@github.com:erinversfeldcodes/gazebolang-fixtures.git "$dir"
    fi
    echo "fixtures ready at $dir"

# Run pending migrations against the local database
db-migrate:
    sqlx migrate run --source migrations

# Run the app with hot reload (server + client)
dev:
    cargo leptos watch

# Format, lint, and test — the same gates CI runs
ci: fmt-check lint test

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all -- --check

# `web`'s ssr and hydrate halves are mutually exclusive features that each
# only compile for one target (native vs wasm32), so each needs its own pass.
lint:
    cargo clippy --workspace --all-targets --features web/ssr -- -W clippy::pedantic -D warnings
    cargo clippy -p web --no-default-features --features hydrate --target wasm32-unknown-unknown --lib -- -W clippy::pedantic -D warnings

test:
    cargo nextest run --workspace --features web/ssr

# Production-profile build (used by the Dockerfile)
build-release:
    cargo leptos build --release --precompress
