SHELL := /bin/sh
DB_DRIVER ?= postgres
DB_FEATURES ?= postgres,mysql

.PHONY: help setup run worker scheduler migrate fmt lint test check check-drivers test-postgres test-mysql test-redis build docker-build docker-db docker-cache docker-up docker-down

help:
	@printf '%s\n' 'setup         Copy .env.example if .env is absent' 'run           Start API (default Cargo binary)' 'worker        Start durable queue worker' 'scheduler     Start UTC task scheduler' 'migrate       Apply embedded migrations for DB_DRIVER' 'check         Formatting, clippy, tests, driver matrix' 'test-postgres TEST_DATABASE_URL=... (disposable DB, CREATE DATABASE grant)' 'test-mysql    TEST_DATABASE_URL=... (disposable DB, CREATE DATABASE grant)' 'test-redis    TEST_REDIS_URL=... (disposable Redis cache)' 'docker-build  Build all binaries (DB_FEATURES=postgres,mysql)' 'docker-db     Start local DB (DB_DRIVER=postgres or mysql)' 'docker-cache  Start optional development Redis cache' 'docker-up     Start API/worker/scheduler after migration'

setup:
	@test -f .env || cp .env.example .env
	@printf '%s\n' 'Set JWT_SECRET using openssl rand -hex 32, and configure DATABASE_URL in .env.'

run:
	cargo run --locked --bin api
worker:
	cargo run --locked --bin worker
scheduler:
	cargo run --locked --bin scheduler
migrate:
	cargo run --locked --bin migrate
fmt:
	cargo fmt --all
lint:
	cargo clippy --locked --all-targets --all-features -- -D warnings
test:
	cargo test --locked --all-targets
	cargo test --locked --doc
check-drivers:
	cargo check --locked --all-targets --no-default-features --features postgres
	cargo check --locked --all-targets --no-default-features --features mysql
check:
	cargo fmt --all -- --check
	$(MAKE) lint test check-drivers
test-postgres:
	@test -n "$(TEST_DATABASE_URL)" || (printf '%s\n' 'Set TEST_DATABASE_URL to a disposable PostgreSQL database'; exit 1)
	cargo test --locked --test database_users --test database_api --test database_queue postgres_ -- --ignored --test-threads=1
test-mysql:
	@test -n "$(TEST_DATABASE_URL)" || (printf '%s\n' 'Set TEST_DATABASE_URL to a disposable MySQL database'; exit 1)
	cargo test --locked --test database_users --test database_api --test database_queue mysql_ -- --ignored --test-threads=1
test-redis:
	@test -n "$$TEST_REDIS_URL" || (printf '%s\n' 'Set TEST_REDIS_URL to a disposable Redis instance'; exit 1)
	cargo test --locked --test cache_redis -- --ignored --test-threads=1
build:
	cargo build --locked --release --no-default-features --features "$(DB_FEATURES)" --bins
docker-build:
	docker build --build-arg DB_FEATURES="$(DB_FEATURES)" -t rust-api-skeleton:local .
docker-db:
	docker compose --profile "$(DB_DRIVER)" up -d --wait "$(DB_DRIVER)"
docker-cache:
	docker compose --profile redis up -d --wait redis
docker-up:
	docker compose up -d api worker scheduler
docker-down:
	docker compose --profile postgres --profile mysql --profile redis --profile tools down
