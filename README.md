# Keybound Backend

This repository powers the user/device storage side of the Keycloak device-binding flow. The service exposes three HTTP surfaces:

- `/kc/*` handles the Keycloak-specific enrollment/lookup flow
- `/bff/*` is the customer-facing backend for web/mobile  
- `/staff/*` is the administrative interface

The runtime is Rust/`axum` with Diesel-async for the database layer and `diesel_migrations` for schema management. Every crate lives under `app/crates/` and depends on shared workspace dependencies defined at the workspace root.

## Repository layout

- `app/crates/backend-server`: Controllers, state, middleware wiring, and background worker logic
- `app/crates/backend-repository`: Diesel-async repository implementations plus integration tests
- `app/crates/backend-model`: Diesel schema, DTO mapping, and shared helpers such as `device_record_id`
- `app/crates/backend-migrate`: Migration runner and embedded migration artifacts (`app/crates/backend-migrate/migrations/**`)
- `app/bins/backend`: Binary that instantiates the server using the shared libraries
- `app/gen/`: Auto-generated OpenAPI clients/server scaffolding (do **not** edit manually)
- `config/`, `deploy/`, `docs/`, `openapi/`: configuration, deployment manifests, architecture docs, and OpenAPI sources

## Getting started

1. Copy `config/dev.yaml` (or your preferred environment) and set `DATABASE_URL`, `KEYCLOAK_ISSUER`, and any other required env vars
2. Run the database migrations: `just build` or programmatically via `backend-migrate`'s `DbFactory`
3. Start the server: `just run` (uses the `app/bins/backend` entry point) or call `cargo run -p backend-bins --bin backend -- --help` to see CLI options

## Testing

- Unit/integration tests: `cargo test --workspace`
- Repository integration tests that touch the database (e.g., `backend-repository/tests/device_repo.rs`) require `DATABASE_URL`; the test skips if the env var is unset. Use a local Postgres instance for full coverage
- Specialized tests: `cargo test -p backend-core --features axum --test error_response` and `cargo test -p backend-auth --test jwt_auth_exclude_paths`

## Documentation

- Refer back to `AGENTS.md` for up-to-date architectural constraints and workflows
- Refer to `AGENTS.md` section "Opencode AI Agents" for AI-powered development workflows
- Migrations live in `app/crates/backend-migrate/migrations` and follow the `YYYYMMDDHHMMSS_description.sql` naming scheme
- Keep `app/gen/*` files in sync only through automated OpenAPI generation (changes should start in `openapi/`)

## AI-Powered Development

This project includes **10 specialized AI agents** via the opencode CLI tool to assist with implementation. Agents work in coordinated phases to complete the flow SDK implementation.

### Installation

```bash
# Install opencode CLI (if not already installed)
curl -sSL https://install.opencode.ai | bash

# Verify installation
opencode --version

# Initialize project configuration
opencode init
```

### Quick Start

```bash
# 1. List all available agents and their capabilities
opencode agent list

# 2. Run daily project status check (recommended first command)
opencode run --agent agent-orchestrator daily-standup

# 3. Generate BFF OpenAPI code (Phase 1: Foundation)
opencode run --agent bff-generator generate-bff

# 4. Implement Phone OTP flow (Phase 2: Core Flows)
opencode run --agent flow-otp-master implement-otp-flow

# 5. Validate implementation
opencode run --agent flow-otp-master validate-flow phone_otp

# 6. Test flow
opencode run --agent test-engineer test-flow phone_otp
```

### Available Agents

| Agent | Type | LLM Model | Primary Purpose |
|-------|------|-----------|-----------------|
| **agent-orchestrator** | primary | gemini-2.5-pro | Master coordinator for all agents |
| **bff-generator** | subagent | gemini-3.1-flash-lite | Generate BFF OpenAPI code |
| **flow-architect** | subagent | kimi-k2-thinking | Design integration traits |
| **flow-otp-master** | primary | deepseek-v3p2 | Implement Phone OTP flow |
| **flow-email-wizard** | primary | cogito-671b-v2-p1 | Implement Email Magic flow |
| **flow-deposit-builder** | primary | kimi-k2-instruct-0905 | Implement First Deposit flow |
| **integration-specialist** | subagent | gemini-2.5-flash | Document verification flows |
| **test-engineer** | primary | qwen3-vl-30b-a3b-thinking | Write comprehensive tests |
| **project-closer** | subagent | minimax-m2p2 | Final polish and delivery |
| **flow-orchestrator** | primary | gemini-2.5-pro | Project coordination |

### Project Phases

**Phase 1: Foundation**
```bash
opencode run --agent bff-generator generate-bff
opencode run --agent flow-architect design-integration-traits
```

**Phase 2: Core Flows**
```bash
opencode run --agent flow-otp-master implement-otp-flow
opencode run --agent flow-email-wizard implement-email-flow
```

**Phase 3: Advanced Flows**
```bash
opencode run --agent flow-deposit-builder validate-flow first_deposit
opencode run --agent integration-specialist validate-flow id_document
```

**Phase 4: Testing**
```bash
opencode run --agent test-engineer test-flow phone_otp
```

**Phase 5: Delivery**
```bash
opencode run --agent project-closer final-lint
opencode run --agent project-closer update-documentation
```

### Agent Orchestration Commands

**Use agent-orchestrator for coordination:**
```bash
# Run all agents in optimal order based on dependencies
opencode run --agent agent-orchestrator run-all-agents

# Track progress across all agents
opencode run --agent agent-orchestrator track-progress

# Run specific project phase
opencode run --agent agent-orchestrator coordinate-phase foundation

# Resolve conflicts between agents
opencode run --agent agent-orchestrator resolve-conflicts flow-otp-master flow-email-wizard
```

### Full Documentation

For complete agent documentation, project phases, and configuration details, see:
- `.opencode/AGENTS-QUICK-REFERENCE.md` - Complete agent reference with commands
- `.opencode/PROJECT-EXECUTION.md` - Detailed project phases and tasks
- `.opencode/QUICKSTART.md` - Quick start guide with examples

### Troubleshooting

**Agent not loading:**
```bash
# Verify opencode is properly initialized
opencode agent list

# Check agent files exist
ls -la .opencode/agents/
```

**Commands not found:**
```bash
# Ensure commands are executable
chmod +x .opencode/commands/*

# Check command syntax
opencode run --help
```

## Notes

- Use `backend_core::id::*` helpers (`usr_*`, `session_*`, `flow_*`, etc.) for all generated IDs to stay consistent with the system's conventions