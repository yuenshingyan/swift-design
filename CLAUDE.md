# Swift Design — Project Conventions

## What Swift Design Is

Swift Design is a brief-first harness that guides LLM agents to design HTML artifacts ("designs"): landing pages, app screens, and similar layouts.

- The user sends a request. The agent asks at most three material questions per turn, then writes a design brief.
- The user approves the brief, edits it, or generates with assumptions. Only then does the agent write a design.
- Agents build designs by writing JSON files: a theme, a viewport, and screens with HTML and CSS.
- The user critiques the design in the browser. Each critique creates a brief revision and a focused edit run.
- **Generation runs on the user's own accounts, never on Swift Design's.** Two paths: an external agent CLI the user already runs (Claude Code, pi), or the built-in provider loop (`model_client.rs`, `briefing.rs`, `generation.rs`) that calls any LLM provider with the user's own keys. Swift Design supplies schemas, prompts, validation, the state machine, and the editor.

## Languages & Stack

- Rust everywhere. Edition 2024.
- Server: axum on tokio. Serves the studio UI, design files, sessions, and uploads.
- Studio UI: Dioxus, compiled to WASM.
- Design, brief, and question definitions: JSON. serde structs are the source of truth. schemars generates the JSON Schemas that guide agents.
- Logging: tracing.

## Project Layout

Cargo workspace with three crates:

```
crates/
  design-model/ # serde + schemars types for designs, briefs, questions, and the workflow. No IO.
  server/       # axum binary: API, sessions, engines, static assets, uploads, validation.
  ui/           # Dioxus studio (WASM).
```

- `server` and `ui` depend on `design-model`. `design-model` depends on no workspace crate.
- Declare shared dependency versions in `[workspace.dependencies]`.
- Organize modules by feature (`sessions.rs`, `briefing.rs`, `render.rs`), not by layer.

## Workflow State

- A session has one persisted `WorkflowState`: `intake`, `clarifying`, `brief_ready`, `awaiting_approval`, `generating`, `reviewing`, or `error`.
- Every state change goes through `design_model::workflow::transition` and `SessionStore::apply`. Do not infer state from chat text, files, or UI flags.
- The approved brief revision is the only content input to generation. Confirmed facts, assumptions, and open questions stay in separate fields.
- Briefing mode never writes designs. The server answers 409 to design writes unless the session is `generating` (or `reviewing`, for user saves from the editor).

## Naming

- Rust defaults: `snake_case` functions, variables, modules, and files. `PascalCase` types. `SCREAMING_SNAKE_CASE` constants.
- Never abbreviate: `configuration` not `config`, `context` not `ctx`, `request` not `req`. Keep Rust-universal tokens (`id`, `impl`) and names a crate's API mandates.
- Boolean names start with `is_`, `has_`, `should_`, or `can_`.
- Dioxus components: `PascalCase` function names under `#[component]`.

## Formatting & Linting

- rustfmt defaults. Run `cargo fmt` before every commit. Do not add rustfmt.toml overrides without discussion.
- Run `dx fmt` on files that contain `rsx!` blocks.
- `cargo clippy --workspace --all-targets -- -D warnings` must pass.
- Imports go at the top of the file in three groups: std, external crates, workspace crates. Blank line between groups.
- No `use` inside function bodies. Exception: `use super::*;` in `#[cfg(test)]` modules.

## Functions & Structure

- Write small, single-purpose functions. At ~40 lines, split the function.
- Maximum 4 parameters. Past that, pass a struct.
- Prefer early returns over nested conditionals.

## Types & Safety

- No `unwrap()` or `expect()` outside tests. Configure `[workspace.lints.clippy] unwrap_used = "warn"`.
- Library code returns `Result`. It does not panic on expected failures.
- Validate agent-written JSON (designs, question sets, briefs) at the server boundary with serde and the model crate's validators. Report every validation error, not only the first — agents fix output from these messages.

## Error Handling

- `design-model` and other library code: typed error enums with thiserror.
- Binaries: `anyhow::Result` at the top level. Add context with `.context("...")`.
- Error messages state what failed, the input that caused it, and the fix when known.
- Never put credentials, authorization headers, prompt text, or filesystem paths into browser responses, session records, or run records. Use ids.

## Logging

- Use tracing with structured fields: `tracing::info!(session_id = %session_id, "brief drafted")`.
- Levels: `error` = broken, needs action. `warn` = degraded. `info` = state change. `debug` = development detail.
- Never log upload contents, prompts, or full design bodies. Log identifiers and sizes.

## Comments & Documentation

- `///` doc comment on every public item. Configure `[workspace.lints.rust] missing_docs = "warn"`.
- First doc line: one sentence that states what the item does.
- Inline comments explain why, not what. Place `//` on its own line above the code.
- `TODO` must name an owner and a reason: `// TODO(hindy): blocked on layout schema`.

## Testing

- Every new function or module ships with unit tests.
- Unit tests: `#[cfg(test)] mod tests` in the same file.
- Integration tests: `tests/` directory in each crate.
- Every type in `design-model` gets a JSON round-trip test (JSON → struct → JSON).
- Test names are snake_case sentences: `fn rejects_a_fourth_question()`.
- Mock nothing internal. Fake real boundaries only: the filesystem with tempfile, the model provider with the fake HTTP server in `test_support.rs`.
- Shared test data: builder functions in a `test_support` module, plus sample designs under `fixtures/`.

## Agent Harness Rules

- All LLM access goes through the provider registry in `crates/server/src/model_client.rs`: OpenAI-compatible chat endpoints keyed by the user's own environment variables, pi-style. Do not add provider-specific SDK crates; a provider is a name, a URL, and key variables.
- Swift Design never ships or requires its own API keys. Model calls happen only with the user's keys, and only when a run starts.
- Agents receive everything from the running app, never from repo files: instructions at `GET /instructions`, the schemas at `GET /schemas/{design,brief,question-set}`, the session at `GET /sessions/{id}`. Do not add agent-facing markdown or prompt files to the repo.
- The instructions payload lives in `crates/server/src/instructions.rs`. Update it whenever agent-visible behavior changes.
- The app serves the schemas from the Rust types at runtime, so they cannot go stale. `schemas/` holds generated copies for review diffs only: regenerate and commit them whenever `design-model` types change (`cargo run -p server --bin generate_schema`). CI fails on a stale copy.
- Sample designs live in `fixtures/` for tests; the instructions payload embeds the example for agents.
- Write schema descriptions, prompts, and instruction strings in Simplified Technical English: short imperative sentences, one instruction per sentence, one term per concept.

## Server API

- Routes: plural nouns, kebab-case: `/sessions`, `/sessions/{id}/brief`, `/designs`, `/uploads`.
- JSON fields: `snake_case` (serde default). Do not rename to camelCase.
- Success responses return the payload directly. Errors return `{ "error": { "message": "...", "details": [...] } }`.
- Timestamps: RFC 3339 strings.

## Git Workflow

- Conventional Commits: `feat:`, `fix:`, `refactor:`, `docs:`, `test:`, `chore:`. Optional scope: `feat(design-model): add the workflow state machine`.
- Subject line: imperative mood, ≤72 characters, no trailing period.
- Branches: `type/short-description`, for example `feat/session-store`.

## AI Coding Rules

- Ambiguous request: ask before proceeding.
- New dependency: ask first, every time.
- Tests accompany all new code.
- Change only what the task requires. Ask before touching unrelated code.
- No placeholders. Finish the change, or ask about scope.
