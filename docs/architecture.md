# Architecture

The runtime mounts only the church tracker API. Each feature has a model, repository, service, controller, and route module under `src/`:

- `church`: scoped church records.
- `member`: home-church membership and calculated balances.
- `project`: ownership, attachment, and project totals.
- `pledge`: versions, state events, and demand.
- `payment`: immutable payments, voiding, and allocation replay.
- `contribution_auth`: cookie sessions, CSRF, roles, and church scope.

`src/http/contribution.rs` contains shared problem responses and extractors. `src/repository/contribution.rs` contains cross-feature ledger reads, idempotency records, and audit writes. `src/service/contribution.rs` contains common validation and Kampala-date helpers. `crates/contributions-domain` is the pure accounting crate.

The single active migration creates `organizations`, `users`, `sessions`, `churches`, `members`, `projects`, `pledges`, `payments`, `allocations`, relation tables, idempotency records, and audit history. Tables have no `contribution_` prefix. The bootstrap migrations are not run against the new database. PostgreSQL data lives in a distinct Compose volume from the bootstrap application.

The repository still contains unused bootstrap source files for historical reference; they are not compiled or mounted by `src/main.rs`. `src/openapi.rs` generates the schema for currently mounted endpoints.
