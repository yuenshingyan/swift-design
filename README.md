# Swift Design

A brief-first design harness. It turns an ambiguous request into an approved design brief, then guides an LLM agent to build the artifact: a software demo or a slide deck.

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

## Two artifact kinds

Pick the kind on the home page. It is fixed for the session once generation starts.

| Kind | What it is | JSON | Canvas | Saved under |
|---|---|---|---|---|
| `demo` | A software demo: a landing page, app screens, a flow | a design with `viewport` and `screens` | the viewport, 1440×900 by default; 390×844 for a phone; 1024×768 for a tablet | `/designs/{id}` |
| `deck` | A slide presentation | a deck with `slides` and no `viewport` | 1920×1080 | `/decks/{id}` |

Both kinds share the theme, the HTML and CSS rules, the brief, the workflow, the templates, and the uploads. Decks add a presenter view, an audience window that follows it, and a PPTX export. The deck JSON, routes, presenter, and PPTX come from Swift Deck, which is now part of this project.

## Core principles

- Ask only high-impact questions.
- Ask no more than three questions at once.
- Preserve answers, assumptions, and open questions separately.
- Treat the approved brief as the source of truth.
- Do not generate before approval unless the user explicitly selects **Skip the questions and generate** or **Decide automatically**.
- Make design decisions inspectable and revisable.

## Workflow

| State | Purpose |
|---|---|
| `intake` | Receive the initial request |
| `clarifying` | Ask focused questions |
| `brief_ready` | Present the assembled brief |
| `awaiting_approval` | User approves, edits, or accepts assumptions |
| `generating` | Agent creates the design or the deck |
| `reviewing` | User critiques and requests revisions |
| `error` | Show a recoverable failure |

## Design brief

A brief records the artifact kind, audience, user need, target artifact/platform, primary action, required content/functionality, information architecture, visual direction, brand assets, accessibility and technical constraints, assumptions, open questions, the answered questions, and revision history. Every edit, critique, and automatic decision makes a new revision. The brief panel lists them under `Show the full brief`: click a row to read that revision, and `Restore this revision` writes it back as a new revision.

The panel shows the answers, the assumptions, and the open questions first. The full field list is one click away.

The app asks for the settings with a closed set of answers, so the agent does not spend a question on them: the artifact kind, the number of variations (1 to 5), the canvases for a demo, and the number of slides for a deck. The canvas picker sits next to the questions, where the rest of the requirements are answered.

Pick more than one canvas and the run writes one design per canvas per variation — desktop and phone with two variations is four candidates, grouped on the canvas under one tab per device.

## Architecture

The harness may use a local CLI agent or a remote API, but workflow state, briefs, artifacts, and user decisions remain under application control.

There are two agent modes:

1. **Briefing mode** — asks, summarizes, and updates the brief; it cannot write artifacts.
2. **Generation mode** — creates the design or the deck from an approved brief.

Designs and decks are two pipelines behind one workflow: separate types, stores, routes, renderers, prompts, and editors, with the shared helpers (history, provenance, CSS scoping, fonts, Chrome, the fix-round loop, the model client) used by both. See `CLAUDE.md` for the rules.

## Run it

```sh
# Build the WASM studio once (needs: cargo install dioxus-cli).
cd crates/ui && dx build --release && cd ../..

# Run the server on http://127.0.0.1:3000.
cargo run -p server
```

Open `http://127.0.0.1:3000`, pick a model in the studio settings, choose
**Software demo** or **Deck**, and describe what you need. The agent runs on
your own model account.

For a deck, the editor adds **Present** (the presenter view with notes, a
timer, and an audience window that follows it) and **PPTX** next to the
HTML and PDF exports. PDF, PPTX, and screenshots need Chrome or Chromium
on the server machine.

## Agent routes

External agents read `GET /instructions` and the schemas at
`GET /schemas/{design,deck,brief,question-set}`. A demo session writes to
`PUT /designs/{session}-candidate-N`; a deck session writes to
`PUT /decks/{session}-candidate-N`. The run environment carries
`SWIFT_DESIGN_SESSION_ID`, `SWIFT_DESIGN_RUN_MODE`, and
`SWIFT_DESIGN_ARTIFACT_KIND`.

Deck-only routes: `GET /decks/{id}/present`, `GET /decks/{id}/render?audience=true`,
`GET /decks/{id}/slides/{n}.png`, `GET /decks/{id}/export.pptx`.

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
| `SWIFT_DESIGN_DECKS_DIR` | `decks` | Deck JSON files |
| `SWIFT_DESIGN_UPLOADS_DIR` | `uploads` | Source materials |
| `SWIFT_DESIGN_TEMPLATES_DIR` | `templates` | Saved style templates |
| `SWIFT_DESIGN_HISTORY_DIR` | `history` | Design save snapshots |
| `SWIFT_DESIGN_DECK_HISTORY_DIR` | `deck-history` | Deck save snapshots |
| `SWIFT_DESIGN_SETTINGS_PATH` | `data/settings.json` | Provider, model, credential |
| `SWIFT_DESIGN_UI_DIR` | `target/dx/ui/release/web/public` | Built WASM bundle |
| `SWIFT_DESIGN_AGENT_COMMAND` | unset | External agent CLI; overrides the built-in engine |
| `SWIFT_DESIGN_CHROME` | unset | Chrome path for screenshots, PDF export, and PPTX export |
| `SWIFT_DESIGN_PROVIDER` / `_MODEL` / `_PROVIDER_URL` / `_PROVIDER_API_KEY` | `google` | Built-in engine defaults |

## Layout

```
crates/
  design-model/  # serde + schemars types: design, deck, brief, question, workflow. No IO.
  server/        # axum: sessions, engines, validation, render, presenter, exports, static hosting.
  ui/            # Dioxus studio (WASM): session workspace, design editor, deck editor.
fixtures/        # sample-design.json and sample-deck.json
schemas/         # generated copies of the served JSON Schemas
```

The workflow state machine, the question protocol, and the brief live in
`design-model`, so the server and the studio share one definition. See
`CLAUDE.md` for project conventions.
