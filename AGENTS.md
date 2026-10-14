# Project Coding Conventions

## Languages & Stack

- These coding conventions cover Rust only. HTML, CSS, and JavaScript style is out of scope.
- Use Rust 2024 in the existing Cargo workspace.
- Preserve the current stack: Axum and Tokio on the server; Dioxus 0.7 for the WASM UI; Serde and Schemars for serialization and schemas; `thiserror` and `anyhow` for errors; `tracing` for logging.
- See [README.md](README.md) for product behavior, architecture, environment variables, and build instructions.

## Naming Conventions

- Use `snake_case` for variables, fields, functions, methods, modules, and Rust filenames.
- Use `PascalCase` for types, traits, enum variants, and Dioxus component functions.
- Use `SCREAMING_SNAKE_CASE` for constants and statics.
- Allow common abbreviations when their meaning is clear.
- Use boolean prefixes such as `is_`, `has_`, `should_`, and `can_` when they clarify meaning.

## Formatting

- Use standard `rustfmt` defaults: four-space indentation, a 100-character target width, same-line opening braces, and formatter-managed commas and blank lines.
- Group imports in this order, with a blank line between groups: standard library, external crates, project imports. Sort alphabetically within each group.

```rust
use std::path::Path;

use axum::Json;
use serde::Serialize;

use design_model::Design;

use crate::time::rfc3339_now;
```

## Functions & Code Structure

- Prefer direct, top-level functions, guard clauses, and early returns.
- Keep nesting shallow. Avoid redundant helpers, forwarding-only modules, and wrapper chains.
- Do not introduce abstractions or decorators without a concrete need and explicit approval. Retain established framework macros and patterns.
- Use judgment for function length; there is no hard line limit.
- Limit function signatures to five parameters. When more are needed, group related inputs in a struct.
- Remove duplication directly rather than adding unnecessary layers.

## Types & Safety

- Represent optional values with `Option` and recoverable failures with `Result`.
- Avoid `unwrap()` and `expect()` outside tests; propagate or explicitly handle failures.
- Use mutable bindings when needed; immutability is not a blanket requirement.
- Validate external inputs at trust boundaries. Avoid redundant validation inside trusted code.

## Error Handling

- Use `thiserror`-based typed errors for failures callers need to distinguish. Use `anyhow` with context for other propagated failures.
- Identify failure categories with explicit error types or structured results, not traceback filenames, function names, or message parsing.
- Write precise, one-line error messages stating what failed and why; include a recovery action when useful.
- Example: `Cannot save design: the storage directory is read-only; choose a writable directory.`
- Keep notification and ticket delivery separate from business logic.

## Logging

- Use `tracing`, structured fields, and severity levels appropriate to the event.
- Redact credentials, tokens, and sensitive user data in log fields and messages.

## Comments & Documentation

- Document public APIs with Rust doc comments, consistent with the workspace's `missing_docs` lint.
- Use implementation comments to explain why, rather than restating obvious behavior.
- Include an issue link with every `TODO`, `FIXME`, or `HACK` marker.
- Maintain the top-level README; do not introduce per-crate READMEs.

## Testing

- Put unit tests beside the implementation in `#[cfg(test)]` modules. Put integration tests in each crate's `tests/` directory.
- Use descriptive `snake_case` test names, such as `invalid_transition_preserves_session_state`.
- Prefer unit tests and mocks. Add integration tests when unit tests cannot adequately verify the behavior.
- Add tests for new behavior and bug fixes. Cover normal cases, failures, and relevant boundaries.
- Maintain at least 90% test coverage across the whole workspace, not just changed code.
- Do not claim that the coverage target is met without measuring it; passing the test suite alone does not establish coverage.

### Existing project checks

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -p server --bin generate_schema && git diff --exit-code schemas/
```

- Keep generated schemas consistent with model changes.
- Report which checks ran and any checks that could not run.

## File & Project Structure

- Preserve the three-crate workspace:
  - `crates/design-model`: shared types, schemas, domain rules, and workflow state; no I/O.
  - `crates/server`: Axum routes, server-side I/O, persistence, engines, rendering, and exports.
  - `crates/ui`: Dioxus rendering, browser UI state, and interaction with the server.
- Preserve the artifact-specific pipelines and existing shared helpers described in the README. Do not collapse them into a new abstraction without approval.
- Keep the existing environment-variable and settings-file configuration approach.
- Ask before adding any new dependency.
- Pin exact registry dependency versions when adding or updating dependencies; Cargo's `"1.2.3"` is a range, while `"=1.2.3"` is an exact pin.
- Existing manifests contain ranges. Do not perform an unrelated bulk pinning migration; change them only within approved dependency work.

## Git Workflow

- Use Conventional Commits, such as `fix: preserve session state after cancellation`.
- Name branches `type/description`, such as `fix/export-timeout`.
- Keep PRs small and focused, under 300 changed lines.

## API Conventions

- Preserve existing route names and JSON formats unless a change is explicitly approved.
- Use plural resource nouns for new REST routes, matching `/sessions` and `/designs`.
- Match existing endpoint conventions for response payloads; do not impose a new response envelope.
- Use ISO 8601 strings for API and persisted date/time values. Existing RFC 3339 UTC timestamps satisfy this convention, for example `2026-01-15T14:30:00Z`.
- Use cursor-based pagination for new paginated list APIs.

## Framework Conventions

- Keep Dioxus rendering and UI state in components.
- Keep shared domain rules in `design-model` and server-side I/O in `server`.
- Preserve existing Dioxus component macros and `PascalCase` component names.
- Follow framework-default loading behavior.

## Security

- Keep credentials out of Git. Use the existing environment-variable and settings-file mechanisms; credential-bearing files must remain untracked.
- Validate external inputs at boundaries before trusted code uses them.
- Redact sensitive fields in logs, including credentials, tokens, and sensitive user data.
- Preserve the current authentication approach. Ask before introducing or changing authentication.
- Audit dependencies regularly. Ask before adding audit tooling or dependencies.

## Performance

- Use `async`/`await` and Tokio for server-side asynchronous work.
- Keep blocking work off async executor threads; use an appropriate blocking task or worker.
- Cache conservatively and define explicit invalidation whenever caching is introduced.
- Monitor WASM bundle size; there is no hard size budget.
- Follow framework-default loading behavior and use cursor-based pagination for new paginated list APIs.

## AI Collaboration

- Ask before proceeding when requirements are ambiguous.
- Ask before breaking changes, dependency additions, authentication changes, or new abstractions.
- Change only what was requested. Do not refactor unrelated or adjacent code without approval.
- Do not leave placeholders or unfinished implementations within the agreed scope.
- Include tests with new behavior and bug fixes, following the testing rules above.
- When reverting, restore the actual relevant Git version rather than approximating it with a rewrite; preserve unrelated user changes.
- Keep responses short and concrete. Avoid jargon, unsolicited examples, architecture proposals, and repeated alternatives.
