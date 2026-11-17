.DEFAULT_GOAL := help
.PHONY: help up down db-shell migrate run dev check fmt test verify

help:
	@printf '%s\n' \
		'make up        Start the isolated church tracker PostgreSQL and Redis services' \
		'make down      Stop services without deleting data' \
		'make db-shell  Open psql in the church tracker database' \
		'make migrate   Apply church tracker migrations' \
		'make run       Start the API' \
		'make dev       Start services, migrate, and run the API' \
		'make verify    Format check, Clippy, and tests'

up:
	docker compose up -d --wait

down:
	docker compose down

db-shell:
	docker compose exec postgres sh -c 'psql -U "$$POSTGRES_USER" -d "$$POSTGRES_DB"'

migrate:
	cargo run -- migrate

run:
	cargo run

dev: up migrate
	cargo run

check:
	cargo check

fmt:
	cargo fmt

test:
	cargo test --workspace

verify:
	cargo fmt --check
	cargo clippy --all-targets -- -D warnings
	cargo test --workspace
