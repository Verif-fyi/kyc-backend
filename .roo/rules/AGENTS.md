# AGENTS.md

## Purpose
Tokenization/user-storage backend with two HTTP surfaces:
- BFF: `/bff/*`
- Staff: `/staff/*`

## Revamp Status (2026-02-27)
- Legacy KYC SQL tables (`kyc_*`, `phone_deposit`) are replaced by a generic persisted state-machine store:
  - `sm_instance`, `sm_event`, `sm_step_attempt`
- Two KYC processes are implemented as state machines:
  - `KYC_PHONE_OTP`
  - `KYC_FIRST_DEPOSIT` (staff confirms payment then approves; worker calls CUSS `registerCustomer` then `approveAndDeposit`)
- Staff OpenAPI is exposing state-machine observability and controls.

`app/bins/backend` starts the server; `app/crates/backend-server` is a library crate.

## Core Architecture
- Runtime is native `axum`, using `Router::nest` to mount each API surface under a configurable base path.
- Layering is strict: `controller -> repository` (explicit service layer removed).
- Controllers: `app/crates/backend-server/src/api/mod.rs` (and submodules)
- API modules: `api/bff.rs`, `api/staff.rs`
- Repository: `app/crates/backend-repository/src/pg/mod.rs` (and submodules)
  - State machines: `app/crates/backend-repository/src/pg/state_machine.rs`

## Crate Roles (under `app/crates/`)
- `backend-core`: config + shared `Error`/`Result` + ID generation (`backend_core::id::*`)
- `backend-auth`: axum middleware layers for authentication and authorization (HMAC only).
- `backend-server`: router/controllers/state/retry worker
- `backend-repository`: Diesel-async repository layer.
- `backend-model`: Diesel models (`Queryable`, `Selectable`, `Insertable`, `Identifiable`) + `o2o` DTO mapping. Contains `schema.rs`.
- `backend-migrate`: migration runner and database factory (Postgres only)
- `gen_oas_*`: generated OA3 models/interfaces (under `app/gen/`, never edit manually)

## Hard Rules
1. Never hand-edit `app/gen/*`.
2. OpenAPI changes happen in `openapi/*` then regenerate.
3. Use Diesel DSL for database operations; avoid raw SQL strings where possible.
4. Use `diesel-async` for all database interactions in the repository layer.
5. Map Diesel errors to `backend_core::Error` using `Into::into`.
6. Use `backend_core::Error` only; avoid scattered custom error mapping.
7. Keep server config source in `backend-core::Config` only.
8. Keep `backend-server` as library; app binary wires and starts it.
9. `backend-core::Config` supports environment variable expansion in YAML files using `${VAR}` or `${VAR:-default}` syntax.
10. Use `TEXT` instead of `VARCHAR` for all string columns in migrations.
11. Use Argon2 for hashing sensitive data that needs verification (e.g., SMS OTPs).
12. Always sort JWK keys alphabetically before serializing to JSON for signature payloads to ensure deterministic string representations across different platforms (Frontend, Keycloak, Backend).

## IDs (Mandatory)
Always use prefix + CUID from `backend_core::id::*`:
- `usr_*` users
- `session_*` sessions
- `flow_*` flow instances
- `step_*` flow steps
- `upload_*` uploads
- `evidence_*` evidence

Never use UUID for backend IDs.

## Auth
- Auth logic is implemented as `axum` middleware layers in `backend-auth`.
- Each API surface (KC, BFF, Staff) has its own middleware layer applied at the router level in `backend-server`.
- `BackendApi` and `AppState` hold `Arc<OidcState>` and `Arc<SignatureState>` for runtime verification.

### Testing Coverage (Mandatory)
- Global error/exception mapping tests live in `app/crates/backend-core/tests/error_response.rs`.
- JWT middleware tests (BFF + Staff bearer auth) live in `app/crates/backend-auth/tests/jwt_auth_exclude_paths.rs`.
- **Unit Tests**: `backend-server` has comprehensive unit tests for `state`, `api::{bff, staff}`, and `worker` using `test_utils` mocks.

#### Auth and Error Scenarios
Required scenarios to keep covered in tests:
- `backend_core::Error` metadata mapping and `IntoResponse` payload/status behavior.
- Bearer middleware bypass cases (`enabled = false`, blank base path, path outside protected base path).
- Bearer middleware enforcement cases (missing token, non-bearer scheme, invalid token, valid token).

Suggested verification commands:
- `cargo test -p backend-core --features axum --test error_response`
- `cargo test -p backend-auth --test jwt_auth_exclude_paths`
- `cargo test -p backend-server` (runs all unit tests with mocks)

## Caching
- In-process cache uses `lru`.
- Redis is available in compose for distributed/shared cache use.

## Migrations & Database Factory
Migrations are located in `app/crates/backend-migrate/migrations`.
Naming convention: `YYYYMMDDHHMMSS_description.sql`.

Database indices and schema constraints must be defined within these migration files to ensure consistency across environments.

**Development Workflow Note**:
Migrations are compile-time checked and embedded using `diesel_migrations::embed_migrations!`. When adding a new `.sql` migration file, you **MUST** touch a Rust file in the `backend-migrate` crate (e.g., `touch app/crates/backend-migrate/src/migrate.rs`) to force Cargo to recompile the crate and include the new migration in the binary.

The `backend-migrate` crate provides a `DbFactory` for constructing database pools and running migrations:
- `DbFactory::postgres(url)`: Creates a factory for Postgres.
- `connect_postgres_and_migrate(url)`: Helper to connect and run migrations in one step.

## Repository Layer (Diesel-Async)
The repository layer uses `diesel-async` for type-safe database access.
- **Traits**: Define domain-specific operations in traits (e.g., `UserRepo`).
- **Implementation**: Implement traits using Diesel DSL and `diesel-async`.
- **Pool**: Use `deadpool_diesel::Pool<diesel_async::AsyncPgConnection>`.
- **Error Handling**: Map Diesel errors to `backend_core::Error`.

Example:
```rust
impl UserRepo for UserRepository {
    async fn get_user(&self, user_id_val: &str) -> RepoResult<Option<db::UserRow>> {
        use backend_model::schema::app_user::dsl::*;
        let mut conn = self.get_conn().await?;

        app_user
            .filter(id.eq(user_id_val))
            .first::<db::UserRow>(&mut conn)
            .await
            .optional()
            .map_err(Into::into)
    }
}
```

The repository implementation is split into domain-specific modules under `src/pg/`:
- `device.rs`: Device binding and lookup.
- `state_machine.rs`: Generic state machine persistence (`sm_*` tables).
- `user.rs`: User management and search.

## SMS Provider Architecture
- **Trait**: `SmsProvider` (in `app/crates/backend-server/src/sms_provider.rs`) defines the contract for sending SMS.
- **Implementations**:
  - `ConsoleSmsProvider`: Logs SMS content to stdout (dev/test only).
  - `SnsSmsProvider`: Sends SMS via AWS SNS (production).
- **Configuration**: The provider is selected at runtime based on the `sms.provider` config key (`console` or `sns`).

## Abstractions & Mocking
To support unit testing without external dependencies, `backend-server` uses trait-based abstractions in `AppState`:
- **Storage**: `MinioStorage` trait (in `app/crates/backend-server/src/file_storage.rs`) abstracts S3-compatible object storage (MinIO/S3).
- **Queues**: `NotificationQueue` and `StateMachineQueue` traits abstract Redis-backed job enqueueing.
- **Repositories**: `StateMachineRepo`, `UserRepo`, and `DeviceRepo` are used via `Arc<dyn Trait>`.

Mocks for these traits are provided in `app/crates/backend-server/src/test_utils/mod.rs` using `mockall`.

## Validation Checklist
Before finalizing:
1. `cargo check --workspace`
2. No runtime use of `swagger` or generated `server::Service`
3. No manual edits under `app/gen/*`
5. Auth and error tests pass:
   - `cargo test -p backend-core --features axum --test error_response`
   - `cargo test -p backend-auth --test jwt_auth_exclude_paths`

## Docker & Build System
- **Target**: `x86_64-unknown-linux-musl` or `aarch64-unknown-linux-musl`.
- **Base Image**: `rust:1-alpine` for building, `gcr.io/distroless/static-debian12:nonroot` for execution.
- **Static Linking**:
    - OpenSSL is statically linked via `openssl = { version = "0.10", features = ["vendored"] }`.
    - libpq is statically linked via `diesel = { version = "2.3", features = ["postgres", ..., "pq-src"] }`.
- **Build Command**: `just build` (uses Docker Compose).

## CI/CD (GitHub Actions)
- Main workflow: `.github/workflows/ci.yaml`
- Reusable actions:
  - `.github/actions/setup-rust/action.yaml`
  - `.github/actions/login-ghcr/action.yaml`
  - `.github/actions/check-cargo-change/action.yaml`
- Docker build context is repository root; Dockerfile at `deploy/docker/user-storage/Dockerfile`.

## Work flavors
Let's talk about all the rules we're having to work efficiently:
- To work here, you should take the habit of first checking the web if there's a newer version of a framework or tool, before using the "known" version
- When searching for code, use `grep` only in `{app,config,deploy,docs,openapi}` directories to avoid noise from `target/` or other ignored directories.

### Work Methodology
- **Analyze First**: Always read the existing implementation and related queries before starting a migration.
- **Incremental Migration**: Migrate one module at a time.
- **Type Safety**: Leverage Diesel DSL for type-safe queries. Avoid raw SQL.
- **Error Mapping**: Consistently map Diesel errors to `backend_core::Error` using `Into::into`.
- **Verification**: Run `cargo check --workspace` and relevant tests after every significant change.
- **Documentation**: Keep `AGENTS.md` updated with the current state of the project (e.g., which modules are migrated).
  - When you add new README/overview docs, include them at the workspace root so future contributors know where to look.

### Rust (Cargo workspace)

- Third-party dependency versions are declared **only** in the repo root `Cargo.toml` under `[workspace.dependencies]`.
- All crates and binaries under `backend/crates/*` and `backend/bins/*` must depend on third-party crates using `{ workspace = true }`.
- If a crate needs optional capabilities, add `features = [...]` on the `{ workspace = true }` dependency in that leaf `Cargo.toml`.
- Do not add `version = "..."` for third-party crates anywhere except the root `Cargo.toml`.

#### Code style

- Use stable Rust and keep code `rustfmt`-formatted.
- Prefer explicit, self-describing names; avoid single-letter identifiers except for well-understood indices.
- Keep modules small and cohesive: one primary concern per module.
- Use `tracing` for logging; avoid `println!` in production code.
- Surface errors with rich types (thiserror / anyhow patterns) rather than panicking; reserve `panic!` for truly unrecoverable situations.
- Keep async boundaries explicit and avoid blocking inside async tasks.
- Favor traits as the primary extension/abstraction mechanism:
    - Define behavior behind traits (with clear method contracts) rather than free-floating functions.
    - Prefer trait impls on small structs (or newtypes) over ad-hoc helper functions; use free functions only for pure, stateless utilities.
    - Add default methods on traits for common runners/wrappers instead of separate “helper” modules.
    - When extracting shared logic, start by defining the trait in the owning crate (e.g., peer/bootstrap, HTTP services, IPC services) and implement it per binary.

### Backends

All backends:

- use `clap` for CLI parameters and environment variable configuration.
- use `mimalloc` for allocation.

#### Roles and rules
- This backend is handling keycloak-storage, keycloak custom flow with device key
- Fineract is handling core-backing
- BFF is the client's backend
- Staff is the admin's frontend of this backend

## Implemented Features

### KYC Case/Submission Model
- **Architecture**: Uses a relational model with `kyc_case` (lifecycle) and `kyc_submission` (versioned data).
- **Data Storage**: Identity data (Name, DOB, etc.) is captured in each `kyc_submission` to maintain a historical snapshot.
- **Status Tracking**: `kyc_case` tracks the active submission and overall lifecycle. Tiers are no longer persisted but are calculated dynamically based on approved documents (Tier 1: Identity, Tier 2: Identity + Address).

### Optimized Staff Submissions Query
- **Endpoint**: `/api/kyc/staff/submissions`
- **Performance**: Uses SQL-level filtering, sorting, and pagination (limit/offset) to handle large volumes of submissions efficiently.

### KYC Profile Patch (Optimistic Locking)
- **Endpoint**: `PATCH /api/registration/kyc/profile`
- **Description**: Allows partial updates to the KYC profile using JSON Patch (RFC 6902).
- **Concurrency Control**: Uses `If-Match` header with ETag (version number) to prevent lost updates.
- **Implementation**:
    - **Handler**: `app/crates/backend-server/src/api/bff.rs` handles the request, checks the version, applies the patch, and calls the repository.
    - **Repository**: `app/crates/backend-repository/src/pg/kyc.rs` executes the update using Diesel DSL.

### Phone Deposit Requests (BFF)
- **Endpoints**:
  - `POST /internal/deposits/phone`
  - `GET /internal/deposits/{depositId}`
- **Persistence**: Deposits are stored in `phone_deposit` with status, assigned contact, and expiry metadata.
- **Ownership**: API enforces JWT user ownership at create/get time.
- **Implementation**:
  - **Handler**: `app/crates/backend-server/src/api/bff.rs` (`Deposits` trait impl).
  - **Repository**: `app/crates/backend-repository/src/pg/kyc.rs` with Diesel DSL methods.
  - **Migration**: `app/crates/backend-migrate/migrations/20260227110000_phone_deposit`.

### Background Worker for SMS Retries
- **Description**: A background worker, powered by the `apalis` crate, handles the retrying of SMS messages.
- **Concurrency Control**: Uses Redis for distributed locking to ensure that only one worker instance processes the SMS queue at a time.
- **Implementation**:
    - **CLI**: The application can be started in `server`, `worker`, or `shared` mode via a CLI flag.
    - **Worker Logic**: The worker logic is located in `app/crates/backend-server/src/worker.rs`.
    - **Queueing**: SMS messages are enqueued into a Redis-backed queue for the worker to process.

### SMS Provider Trait & Argon2 Hashing
- **Description**: A pluggable SMS provider system supports both local development (console logging) and production (AWS SNS).
- **Security**: SMS OTPs are generated as 6-digit codes and hashed using Argon2 before storage. Verification compares the hash of the input against the stored hash.
- **Implementation**:
    - **Trait**: `SmsProvider` in `app/crates/backend-server/src/sms_provider.rs`.
    - **Hashing**: Uses the `argon2` crate for secure password hashing.

### OIDC Discovery & Keycloak Signature Verification
- **OIDC Discovery**: `backend-auth` supports automatic OIDC discovery. It fetches the discovery document from the configured `issuer` to obtain the `jwks_uri` and caches it. The `jwks_url` configuration field has been removed as it is now fully inferred from the `issuer`.
- **Signature Verification**: Keycloak requests are verified using a HMAC-SHA256 signature.
    - **Headers**: `x-kc-signature` and `x-kc-timestamp`.
    - **Canonical Payload**: `timestamp + "\n" + method + "\n" + path + "\n" + body`.
    - **Encoding**: The resulting HMAC digest is Base64URL encoded (no padding).
- **JWT Validation**: Tokens are validated against the JWKS obtained via OIDC discovery.
- **Integration**: `backend-server`'s `AppState` and `BackendApi` now hold `OidcState` and `SignatureState`. API handlers use these for robust JWT verification and signature checking.

### Backend-Server Unit Testing Expansion
- **Coverage**: Expanded coverage for `state`, `api::{bff, staff}`, and `worker` modules.
- **Mocks**: Uses `mockall` and `test_utils` to exercise handlers without hitting real AWS/SNS/Redis/HTTP dependencies.
- **Traits**: Introduced `MinioStorage`, `NotificationQueue`, and `StateMachineQueue` traits to enable clean dependency injection in `AppState`.
