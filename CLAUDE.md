# Swift Design — Project Conventions

## What Swift Design Is

Swift Design is a brief-first harness that guides LLM agents to build HTML artifacts of two kinds:

- **Software demos** ("designs"): landing pages, app screens, and similar layouts on a device viewport. A design is a theme, a viewport, and `screens` with HTML and CSS.
- **Decks**: slide presentations on a fixed 1920 by 1080 px canvas. A deck is a theme and `slides` with HTML and CSS. Decks keep the Swift Deck JSON shape and routes, with a presenter view and HTML, PDF, and PPTX exports.

The workflow is the same for both kinds:

- The user picks the kind and sends a request. The agent asks at most three material questions per turn, then writes a design brief.
- The user approves the brief, edits it, or generates with assumptions. Only then does the agent write a design or a deck.
- Agents build artifacts by writing JSON files that conform to the served schemas.
- The user critiques the result in the browser. Each critique creates a brief revision and a focused edit run.
- **Generation runs on the user's own accounts, never on Swift Design's.** Two paths: an external agent CLI the user already runs (Claude Code, pi), or the built-in provider loop (`model_client.rs`, `briefing.rs`, `generation.rs`, `deck_generation.rs`) that calls any LLM provider with the user's own keys. Swift Design supplies schemas, prompts, validation, the state machine, and the editors.

## Languages & Stack

- Rust everywhere. Edition 2024.
- Server: axum on tokio. Serves the studio UI, design and deck files, sessions, and uploads.
- Studio UI: Dioxus, compiled to WASM.
- Design, deck, brief, and question definitions: JSON. serde structs are the source of truth. schemars generates the JSON Schemas that guide agents.
- Logging: tracing.

## Project Layout

Cargo workspace with three crates:

```
crates/
  design-model/ # serde + schemars types for designs, decks, briefs, questions, and the workflow. No IO.
  server/       # axum binary: API, sessions, engines, static assets, uploads, validation.
  ui/           # Dioxus studio (WASM).
```

- `server` and `ui` depend on `design-model`. `design-model` depends on no workspace crate.
- Declare shared dependency versions in `[workspace.dependencies]`.
- Organize modules by feature (`sessions.rs`, `briefing.rs`, `render.rs`), not by layer.

## Two Pipelines

Designs and decks are separate pipelines that share one workflow.

- Separate per kind: the model types (`design.rs`, `screen.rs` and `deck.rs`, `slide.rs`), the stores (`designs.rs`, `decks.rs`), the routes (`/designs/*`, `/decks/*`), the render entry points (`render.rs`, `deck_render.rs`), the patches (`patch.rs`, `deck_patch.rs`), the polish prompts (`polish.rs`, `deck_polish.rs`), the generation prompts (`generation.rs`, `deck_generation.rs`), and the editors (`editor.rs`, `deck_editor.rs`). Deck-only modules: `presenter.rs` and `pptx.rs`.
- Shared only where the code is identical modulo names: `history.rs`, `provenance.rs`, `events.rs`, `api_error.rs`, `files.rs`, `screen_css.rs`, the Chrome runner in `screenshots.rs`, the font and upload inlining in `export.rs`, the page scripts in `render.rs`, the fix-round loop and attachments in `generation.rs`, `model_client.rs`, `sessions.rs`, `briefing.rs`.
- A session has one `artifact_kind` (`demo` or `deck`), set at creation. The brief carries the same value. The engine, the chooser, the critique route, and the studio read that one value. Do not infer the kind from ids or file contents.
- A deck page uses the same DOM vocabulary as a design page (`main.design`, `data-swift-design-*`), so the layout, navigation, editing, and audit scripts serve both. Only the audience-follow script and the PPTX measurement script are deck-only.
- `PATCH_FORMAT` and the polish wording are duplicated on purpose: the model sees one vocabulary per kind (screens or slides).

## Workflow State

- A session has one persisted `WorkflowState`: `intake`, `clarifying`, `brief_ready`, `awaiting_approval`, `generating`, `reviewing`, or `error`.
- Every state change goes through `design_model::workflow::transition` and `SessionStore::apply`. Do not infer state from chat text, files, or UI flags.
- The approved brief revision is the only content input to generation. Confirmed facts, assumptions, and open questions stay in separate fields.
- Briefing mode never writes designs or decks. The server answers 409 to design and deck writes unless the session is `generating` (or `reviewing`, for user saves from the editor).
- The artifact kind may change through a user brief edit before generation. After generation it is fixed: the server answers 409.
- A question with a closed set of answers belongs to the app, not to the model: the artifact kind, the variation count (1 to `CANDIDATE_LIMIT`), the canvases for a demo (1 to `PLATFORM_LIMIT`), and the slide count for a deck. The studio asks with a control next to the questions; the prompts in `briefing.rs` and `instructions.rs` tell the agent never to ask about them.
- The app's answers live on `Session.options`, not in the brief. `write_brief_revision` mirrors them into every revision, so an agent that rewrites the brief cannot drop them. A control that wrote a brief revision instead would move the session to `awaiting_approval` mid-clarification.
- A demo run writes one design per canvas per variation: `candidate_plans` in `generation.rs` names them, and the studio groups the cards under one tab per canvas.
- The brief keeps the answered questions in `answered_questions`, as question text plus answer text. Never write an answer into `confirmed_facts` as `question: answer`: a fact is one short sentence.

## Naming

- Rust defaults: `snake_case` functions, variables, modules, and files. `PascalCase` types. `SCREAMING_SNAKE_CASE` constants.
- Never abbreviate: `configuration` not `config`, `context` not `ctx`, `request` not `req`. Keep Rust-universal tokens (`id`, `impl`) and names a crate's API mandates.
- Boolean names start with `is_`, `has_`, `should_`, or `can_`.
- Dioxus components: `PascalCase` function names under `#[component]`.
- Vocabulary: a design has screens; a deck has slides. Use `artifact` only for code that serves both.

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
- Every store writes through `files::write_atomically`: a temporary file, then a rename. `tokio::fs::write` truncates first, and the studio polls the stores while a run writes them, so a plain write lets a reader see an empty file.
- A background save must stop once the final save has landed. `LiveSaver` and `DeckLiveSaver` set `is_finished` under the write lock, so a partial draft spawned earlier cannot overwrite the finished artifact.
- Validate agent-written JSON (designs, decks, question sets, briefs) at the server boundary with serde and the model crate's validators. Report every validation error, not only the first — agents fix output from these messages.

## Error Handling

- `design-model` and other library code: typed error enums with thiserror.
- Binaries: `anyhow::Result` at the top level. Add context with `.context("...")`.
- Error messages state what failed, the input that caused it, and the fix when known.
- Never put credentials, authorization headers, prompt text, or filesystem paths into browser responses, session records, or run records. Use ids.

## Logging

- Use tracing with structured fields: `tracing::info!(session_id = %session_id, "brief drafted")`.
- Levels: `error` = broken, needs action. `warn` = degraded. `info` = state change. `debug` = development detail.
- Never log upload contents, prompts, or full design or deck bodies. Log identifiers and sizes.

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
- Shared test data: builder functions in a `test_support` module, plus sample designs and decks under `fixtures/`.

## Agent Harness Rules

- All LLM access goes through the provider registry in `crates/server/src/model_client.rs`: OpenAI-compatible chat endpoints keyed by the user's own environment variables, pi-style. Do not add provider-specific SDK crates; a provider is a name, a URL, and key variables.
- Swift Design never ships or requires its own API keys. Model calls happen only with the user's keys, and only when a run starts.
- Agents receive everything from the running app, never from repo files: instructions at `GET /instructions`, the schemas at `GET /schemas/{design,deck,brief,question-set}`, the session at `GET /sessions/{id}`. Do not add agent-facing markdown or prompt files to the repo.
- The instructions payload lives in `crates/server/src/instructions.rs`. It carries one rule list per kind (`DEMO_RULES`, `DECK_RULES`) and one example per kind. Update it whenever agent-visible behavior changes.
- The app serves the schemas from the Rust types at runtime, so they cannot go stale. `schemas/` holds generated copies for review diffs only: regenerate and commit them whenever `design-model` types change (`cargo run -p server --bin generate_schema`). CI fails on a stale copy.
- Sample artifacts live in `fixtures/` (`sample-design.json`, `sample-deck.json`) for tests; the instructions payload embeds both examples for agents.
- Write schema descriptions, prompts, and instruction strings in Simplified Technical English: short imperative sentences, one instruction per sentence, one term per concept.

## Server API

- Routes: plural nouns, kebab-case: `/sessions`, `/sessions/{id}/brief`, `/designs`, `/decks`, `/uploads`.
- Deck-only routes: `/decks/{id}/present`, `/decks/{id}/render?audience=true`, `/decks/{id}/slides/{n}.png`, `/decks/{id}/export.pptx`.
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
