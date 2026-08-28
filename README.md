# Swift Design

A design harness. You describe what you need, the agent asks a few questions in the chat, writes candidates, and edits them from the chat: a software demo or a slide deck.

## Why

Most design-generation tools optimize for:

```text
prompt → artifact
```

This harness adds one round of questions and keeps the conversation:

```text
request → questions in the chat → candidates → chat edits
```

The questions are short choices with `Use your best judgment` on each, and you can skip them all with **Skip the questions and generate**. There is no brief to approve.

## Two artifact kinds

Pick the kind on the home page. It is fixed for the session once generation starts.

| Kind | What it is | JSON | Canvas | Saved under |
|---|---|---|---|---|
| `demo` | A software demo: a landing page, app screens, a flow | a design with `viewport` and `screens` | the viewport, 1440×900 by default; 390×844 for a phone; 1024×768 for a tablet | `/designs/{id}` |
| `deck` | A slide presentation | a deck with `slides` and no `viewport` | 1920×1080 | `/decks/{id}` |

Both kinds share the theme, the HTML and CSS rules, the workflow, the templates, and the uploads. Decks add a presenter view, an audience window that follows it, and a PPTX export. The deck JSON, routes, presenter, and PPTX come from Swift Deck, which is now part of this project.

## Core principles

- Ask only choices that change the result, with 2 to 4 short options each.
- Ask at most three questions per turn, and no more once five are answered.
- Never require an answer: every question has `Use your best judgment`, and the whole set can be skipped.
- The app asks its own closed questions itself: the canvas and the number of variations for a demo; the scenario, the length, the candidates, and the variety for a deck.
- After the candidates exist, the chat edits: a message with a candidate open changes that candidate, a message without one writes new candidates.

## Workflow

| State | Purpose |
|---|---|
| `intake` | The request arrived; the first turn is running |
| `clarifying` | The agent asked questions and waits for answers |
| `generating` | The agent writes or edits candidates |
| `reviewing` | Candidates exist; the chat asks for changes |
| `error` | A run failed; retry keeps the data |

Every turn is one run of the planner, copied from Swift Deck: it reads the request, the answers, and the conversation, and replies with questions, a decision to write, a decision to edit the open candidate, or plain text.

## Candidates

Every candidate is a card with a live preview. Cards on one tab share a height, so a desktop card is wide and a phone card is a tall bezel. Click a card to open it in the editor. Arrows at the edges of the preview, or `←` and `→` on a focused card, step through the screens or slides; the pill in the corner shows `n/m`. A preview candidate (the first screens plus an outline) carries a `Finish` button that writes the rest. The chosen card is marked `Chosen`.

In `reviewing`, the chat is the edit input: type what should change and press **Send**. With a candidate chosen, the change is applied to it; otherwise new candidates are written.

## Architecture

The harness may use a local CLI agent or a remote API, but workflow state, answers, artifacts, and user decisions remain under application control.

One run does one turn: plan, then ask, write, edit, or reply. The built-in engine runs the planner, the concept planner, the candidate writers, the fix-round loop, and the polish pass.

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
`GET /schemas/{design,deck,question-set}`. A demo session writes to
`PUT /designs/{session}-candidate-N`; a deck session writes to
`PUT /decks/{session}-candidate-N`. The run environment carries
`SWIFT_DESIGN_SESSION_ID`, `SWIFT_DESIGN_RUN_MODE`, and
`SWIFT_DESIGN_ARTIFACT_KIND`.

Turns: `POST /sessions/{id}/messages` sends a chat turn and starts a run;
`PUT /sessions/{id}/question-set` asks; `POST /sessions/{id}/answers` answers;
`POST /sessions/{id}/generate` opens the session for writing without more
questions; `POST /sessions/{id}/complete` ends a run.

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
| `SWIFT_DESIGN_SESSIONS_DIR` | `data/sessions` | Sessions, question sets, answers, runs |
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
  design-model/  # serde + schemars types: design, deck, question, workflow. No IO.
  server/        # axum: sessions, engines, validation, render, presenter, exports, static hosting.
  ui/            # Dioxus studio (WASM): session workspace, design editor, deck editor.
fixtures/        # sample-design.json and sample-deck.json
schemas/         # generated copies of the served JSON Schemas
```

The workflow state machine and the question protocol live in
`design-model`, so the server and the studio share one definition. See
`CLAUDE.md` for project conventions.
