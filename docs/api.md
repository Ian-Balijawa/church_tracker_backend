# Implemented API

Base path: `/api/v1`. JSON fields use camelCase; IDs are UUIDs and money is whole UGX. The machine-readable schema is served at `/api/v1/openapi.json`, with Swagger UI at `/swagger-ui/`. It documents the implemented surface, not every endpoint proposed in the full contract.

## Authentication

1. `GET /auth/csrf` sets an anonymous HttpOnly cookie and returns `data.csrfToken`.
2. `POST /auth/login` accepts `{phone,password}` with `X-CSRF-Token` and an allowed `Origin`. It sets a session cookie and returns the user, session, and a rotated CSRF token.
3. Send the session cookie on reads. Send the current `X-CSRF-Token` and an allowed `Origin` on writes. `GET /auth/session` returns the current session and CSRF token.
4. `POST /auth/logout` revokes the session.

Also implemented: `GET/DELETE /auth/sessions`, `POST /auth/unlock`, `PUT/DELETE /auth/pin`, `PUT /auth/password`, and organization-admin-only `POST /admin-users`.

## Resources

| Resource | Implemented endpoints |
| --- | --- |
| Churches | `GET/POST /churches`, `GET/PATCH /churches/{id}` |
| Members | `GET/POST /members`, `GET /members/{id}` |
| Projects | `GET/POST /projects`, `GET /projects/{id}`, `PUT /projects/{id}/members/{memberId}` |
| Pledges | `GET/POST /pledges`, `GET /pledges/{id}`, `GET /pledges/{id}/events`, `POST /pledges/{id}/pause`, `/resume`, `/cancel`, `/versions` |
| Payments | `GET/POST /payments`, `GET /payments/{id}`, `POST /payments/{id}/void`, `POST /payments/preview` |

Business writes require an `Idempotency-Key` UUID. Edits and state transitions require `If-Match` with the current strong `ETag`. Create requests supply a client-generated resource UUID. Payment creation and voiding recalculate allocations in a transaction. `GET /health` returns `{"status":"ok"}`; `GET /ready` probes PostgreSQL and Redis.

Errors use `application/problem+json`; financial and personal responses use `Cache-Control: private, no-store`.

## Current limits

The [full proposed contract](../API-CONTRACT.md-backend) includes additional filters, relationship changes, soft deletion, statements, reports, exports, sync, notifications, push, preferences, and password recovery. Those routes are not present in this server's OpenAPI document yet. The old bootstrap bearer-auth, task, file, scheduler, and configuration routes are no longer mounted.
