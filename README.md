# Brief-First Design Harness

A conversational design-agent harness that turns ambiguous requests into approved design briefs before generating artifacts.

## Why

Most design-generation tools optimize for:

```text
prompt → artifact
```

This harness optimizes for:

```text
request → clarify → brief → approve → generate → critique → revise
```

The purpose is not to delay output. It is to avoid poor alignment caused by guessing about audience, goals, content, brand direction, or platform.

## Core principles

- Ask only high-impact questions.
- Ask no more than three questions at once.
- Preserve answers, assumptions, and open questions separately.
- Treat the approved brief as the source of truth.
- Do not generate before approval unless the user explicitly selects **Generate with assumptions**.
- Make design decisions inspectable and revisable.

## Workflow

| State | Purpose |
|---|---|
| `intake` | Receive the initial request |
| `clarifying` | Ask focused design questions |
| `brief_ready` | Present the assembled brief |
| `awaiting_approval` | User approves, edits, or accepts assumptions |
| `generating` | Agent creates the artifact |
| `reviewing` | User critiques and requests revisions |
| `error` | Show a recoverable failure |

## Design brief

A brief records audience, user need, target artifact/platform, primary action, required content/functionality, information architecture, visual direction, brand assets, accessibility and technical constraints, assumptions, open questions, and revision history.

## Architecture

The harness may use a local CLI agent or a remote API, but workflow state, briefs, artifacts, and user decisions remain under application control.

There are two agent modes:

1. **Briefing mode** — asks, summarizes, and updates the brief; it cannot write artifacts.
2. **Generation mode** — creates the artifact from an approved brief.

See `AGENT.MD` for implementation rules.

## Run it

```sh
# Build the WASM studio once (needs: cargo install dioxus-cli).
cd crates/ui && dx build --release && cd ../..

# Run the server on http://127.0.0.1:3000.
cargo run -p server
```

Open `http://127.0.0.1:3000`, pick a model in the studio settings, and
describe a design. The agent runs on your own model account.

## Checks

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -p server --bin generate_schema && git diff --exit-code schemas/
```

## Environment

| Variable | Default | Purpose |
|---|---|---|
| `SWIFT_DESIGN_ADDRESS` | `127.0.0.1:3000` | Bind address |
| `SWIFT_DESIGN_SESSIONS_DIR` | `data/sessions` | Sessions, briefs, answers, runs |
| `SWIFT_DESIGN_DESIGNS_DIR` | `designs` | Design JSON files |
| `SWIFT_DESIGN_UPLOADS_DIR` | `uploads` | Source materials |
| `SWIFT_DESIGN_TEMPLATES_DIR` | `templates` | Saved style templates |
| `SWIFT_DESIGN_HISTORY_DIR` | `history` | Save snapshots |
| `SWIFT_DESIGN_SETTINGS_PATH` | `data/settings.json` | Provider, model, credential |
| `SWIFT_DESIGN_UI_DIR` | `target/dx/ui/release/web/public` | Built WASM bundle |
| `SWIFT_DESIGN_AGENT_COMMAND` | unset | External agent CLI; overrides the built-in engine |
| `SWIFT_DESIGN_CHROME` | unset | Chrome path for screenshots and PDF export |
| `SWIFT_DESIGN_PROVIDER` / `_MODEL` / `_PROVIDER_URL` / `_PROVIDER_API_KEY` | `google` | Built-in engine defaults |

## Layout

```
crates/
  design-model/  # serde + schemars types: design, brief, question, workflow. No IO.
  server/        # axum: sessions, engines, validation, render, static hosting.
  ui/            # Dioxus studio (WASM).
```

The workflow state machine, the question protocol, and the brief live in
`design-model`, so the server and the studio share one definition. See
`CLAUDE.md` for project conventions.
