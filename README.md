# Church expense and contribution tracker API

Rust/Axum backend for organization-scoped churches, members, projects, pledges, and payments. PostgreSQL stores the ledger; Redis provides login and unlock throttling. This is a separate project and database from the original bootstrap application.

## Local setup

```sh
cp .env.example .env
make up
make migrate
cargo run -- create-admin --organization "My Organization" --name "Administrator" --phone +256700000000 --password '<strong-local-password>'
make run
```

The sample credentials in `.env.example` are for local development. Change `POSTGRES_PASSWORD` and the matching password in `DATABASE_URL` for any other environment. `CORS_ORIGINS` must list the browser application's exact origin. The API listens on port 8000 by default.

`make up` starts the isolated Compose project `church-expense-tracker` with PostgreSQL on local port 15433 and Redis on 16380. Its database is `church_expense_tracker`, owned by `church_tracker_app`. The Compose project and volume names differ from the bootstrap project's, so the old volume and data are left alone. PostgreSQL applies `POSTGRES_DB`, `POSTGRES_USER`, and `POSTGRES_PASSWORD` only when its data volume is first initialized; changing them later requires a database credential migration or a new volume.

`make migrate` creates the tracker tables without the old `contribution_` prefix. Migrations do not import or modify bootstrap data. Run `make db-shell` to inspect the new database. `make down` stops services without deleting their volumes.

## API

The implemented API uses `/api/v1`, cookie sessions, `X-CSRF-Token` on writes, and `Idempotency-Key` on business mutations. See [API guide](docs/api.md) and [OpenAPI JSON](http://localhost:8000/api/v1/openapi.json). Swagger UI is at `/swagger-ui/`. The OpenAPI file describes implemented endpoints; [API-CONTRACT.md-backend](API-CONTRACT.md-backend) also proposes reports, sync, notifications, and other features that are still pending.

## Checks

```sh
make verify
```

This runs formatting, Clippy, and workspace tests. The pure accounting crate contains the contract's 10-day/15,000 UGX acceptance vector and lifecycle tests. See [architecture](docs/architecture.md) and [current gaps](docs/best-practices-audit.md).
