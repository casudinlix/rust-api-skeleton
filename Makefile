SHELL := /bin/sh
.DEFAULT_GOAL := help

DB_DRIVER ?= postgres
DB_FEATURES ?= postgres,mysql
DEV ?= 0
SCHEDULER ?= 0
FORCE ?= 0
SHUTDOWN_TIMEOUT ?= 60

# Freeze command-line values before Make expands/exports them. Values such as
# $(shell ...) must reach the CLI as literal data, never executable Make syntax.
override NAME := $(value NAME)
override PACKAGE := $(value PACKAGE)
override FEATURES := $(value FEATURES)
override DEV := $(value DEV)
override SCHEDULER := $(value SCHEDULER)
override FORCE := $(value FORCE)
override SHUTDOWN_TIMEOUT := $(value SHUTDOWN_TIMEOUT)
override DB_DRIVER := $(value DB_DRIVER)
override DB_FEATURES := $(value DB_FEATURES)
override TEST_DATABASE_URL := $(value TEST_DATABASE_URL)
override TEST_REDIS_URL := $(value TEST_REDIS_URL)

unexport NAME PACKAGE FEATURES DEV SCHEDULER FORCE SHUTDOWN_TIMEOUT
unexport ARTISAN_NAME ARTISAN_PACKAGE ARTISAN_FEATURES
ifneq ($(value NAME),)
export ARTISAN_NAME := $(value NAME)
endif
ifneq ($(value PACKAGE),)
export ARTISAN_PACKAGE := $(value PACKAGE)
endif
ifneq ($(value FEATURES),)
export ARTISAN_FEATURES := $(value FEATURES)
endif
export ARTISAN_DEV := $(value DEV)
export ARTISAN_SCHEDULER := $(value SCHEDULER)
export ARTISAN_FORCE := $(value FORCE)
export ARTISAN_SHUTDOWN_TIMEOUT := $(value SHUTDOWN_TIMEOUT)
export TEST_DATABASE_URL TEST_REDIS_URL

# These selectors are only recipe inputs. Do not export the default DB_DRIVER:
# native applications must still be able to select MySQL from their .env file.
export MAKE_DB_DRIVER := $(value DB_DRIVER)
export MAKE_DB_FEATURES := $(value DB_FEATURES)

.PHONY: help install setup serve key require remove update \
        controller model request dto service repository job middleware migration \
        run worker relay scheduler migrate fmt lint test check check-drivers \
        test-postgres test-mysql test-redis test-redis-transport \
        test-queue-postgres test-queue-mysql test-scaffolds build docker-build docker-db \
        docker-cache docker-queue docker-up docker-down

help:
	@printf '%s\n' \
	  'install       Initialize .env/key and build locked dependencies (setup is an alias)' \
	  'serve         Run API + worker together (SCHEDULER=1 adds scheduler)' \
	  'controller    Generate JSON handler: NAME=CustomerController' \
	  'model         Generate model with UUIDv7 customer_id: NAME=Customer (singular entity)' \
	  'request       Generate validated request DTO: NAME=CreateCustomerRequest (alias: dto)' \
	  'service       Generate service: NAME=CustomerService' \
	  'repository    Generate named-PK adapters: NAME=CustomerRepository (singular entity)' \
	  'job           Generate job payload/handler: NAME=SendInvoiceJob' \
	  'middleware    Generate middleware: NAME=AuditMiddleware' \
	  'migration     Generate paired SQL migrations: NAME=create_customers_table' \
	  'key           Generate initial JWT key (FORCE=1 rotates an existing key)' \
	  'require       Add crate: PACKAGE=crate@version [FEATURES=one,two] [DEV=1]' \
	  'remove        Remove crate: PACKAGE=crate [DEV=1]' \
	  'update        Update Cargo.lock [PACKAGE=crate]' \
	  'run           Start API alone (default Cargo binary)' \
	  'worker        Start durable queue worker alone' \
	  'relay         Publish committed SQL outbox jobs to Redis (QUEUE_DRIVER=redis)' \
	  'scheduler     Start UTC task scheduler alone' \
	  'migrate       Apply embedded migrations for configured DB_DRIVER' \
	  'check         Formatting, clippy, tests, driver matrix' \
	  'test-postgres TEST_DATABASE_URL=... (disposable DB, CREATE DATABASE grant)' \
	  'test-mysql    TEST_DATABASE_URL=... (disposable DB, CREATE DATABASE grant)' \
	  'test-redis    TEST_REDIS_URL=... (disposable Redis cache)' \
	  'test-redis-transport TEST_REDIS_URL=... (disposable Redis queue)' \
	  'test-queue-postgres TEST_DATABASE_URL=... TEST_REDIS_URL=... (Redis + PostgreSQL)' \
	  'test-queue-mysql TEST_DATABASE_URL=... TEST_REDIS_URL=... (Redis + MySQL)' \
	  'test-scaffolds Generate all scaffolds in a temporary project and verify compilation' \
	  'build         Build release binaries (DB_FEATURES=postgres,mysql)' \
	  'docker-build  Build runtime image (DB_FEATURES=postgres,mysql)' \
	  'docker-db     Start local DB (DB_DRIVER=postgres or mysql)' \
	  'docker-cache  Start optional development Redis cache' \
	  'docker-queue  Start persistent development Redis queue (localhost:6380)' \
	  'docker-up     Start API/worker/scheduler after migration' \
	  'See docs/development.md for complete workflows and Artisan CLI usage.'

install:
	cargo run --locked --bin artisan -- install
setup: install
serve:
	cargo run --locked --bin artisan -- serve
key:
	cargo run --locked --bin artisan -- key:generate
require:
	cargo run --locked --bin artisan -- require
remove:
	cargo run --locked --bin artisan -- remove
update:
	cargo run --locked --bin artisan -- update

controller model request service repository job middleware migration:
	cargo run --locked --bin artisan -- make:$@
dto:
	cargo run --locked --bin artisan -- make:request

run:
	cargo run --locked --bin api
worker:
	cargo run --locked --bin worker
relay:
	cargo run --locked --bin relay
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
	@test -n "$$TEST_DATABASE_URL" || (printf '%s\n' 'Set TEST_DATABASE_URL to a disposable PostgreSQL database'; exit 1)
	cargo test --locked --test database_users --test database_api --test database_queue --test database_outbox --test schema_alignment postgres_ -- --ignored --test-threads=1
test-mysql:
	@test -n "$$TEST_DATABASE_URL" || (printf '%s\n' 'Set TEST_DATABASE_URL to a disposable MySQL database'; exit 1)
	cargo test --locked --test database_users --test database_api --test database_queue --test database_outbox --test schema_alignment mysql_ -- --ignored --test-threads=1
test-redis:
	@test -n "$$TEST_REDIS_URL" || (printf '%s\n' 'Set TEST_REDIS_URL to a disposable Redis instance'; exit 1)
	cargo test --locked --test cache_redis -- --ignored --test-threads=1
test-redis-transport:
	@test -n "$$TEST_REDIS_URL" || (printf '%s\n' 'Set TEST_REDIS_URL to a disposable Redis queue instance'; exit 1)
	cargo test --locked --lib jobs::redis::tests:: -- --ignored --test-threads=1
test-queue-postgres:
	@test -n "$$TEST_DATABASE_URL" || (printf '%s\n' 'Set TEST_DATABASE_URL to a disposable PostgreSQL database'; exit 1)
	@test -n "$$TEST_REDIS_URL" || (printf '%s\n' 'Set TEST_REDIS_URL to a disposable Redis queue instance'; exit 1)
	cargo test --locked --test queue_redis postgres_ -- --ignored --test-threads=1
test-queue-mysql:
	@test -n "$$TEST_DATABASE_URL" || (printf '%s\n' 'Set TEST_DATABASE_URL to a disposable MySQL database'; exit 1)
	@test -n "$$TEST_REDIS_URL" || (printf '%s\n' 'Set TEST_REDIS_URL to a disposable Redis queue instance'; exit 1)
	cargo test --locked --test queue_redis mysql_ -- --ignored --test-threads=1
test-scaffolds:
	cargo test --locked --test artisan_scaffolds -- --ignored --test-threads=1
build:
	cargo build --locked --release --no-default-features --features "$$MAKE_DB_FEATURES" --bins
docker-build:
	docker build --build-arg "DB_FEATURES=$$MAKE_DB_FEATURES" -t rust-api-skeleton:local .
docker-db:
	docker compose --profile "$$MAKE_DB_DRIVER" up -d --wait "$$MAKE_DB_DRIVER"
docker-cache:
	docker compose --profile redis up -d --wait redis
docker-queue:
	docker compose --profile redis-queue up -d --wait redis-queue
docker-up:
	docker compose up -d api worker scheduler
docker-down:
	docker compose --profile postgres --profile mysql --profile redis --profile redis-queue --profile relay --profile tools down
