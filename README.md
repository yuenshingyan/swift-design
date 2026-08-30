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

Describe what you need and press **Create**. The app then asks which kind to
build, in a modal over the home page. The kind is fixed for the session; start a
new session to build the other kind.

| Kind | What it is | JSON | Canvas | Saved under |
|---|---|---|---|---|
| `demo` | A software demo: a landing page, app screens, a flow | a design with `viewport` and `screens` | the viewport, 1440×900 by default; 390×844 for a phone; 1024×768 for a tablet | `/designs/{id}` |
| `deck` | A slide presentation | a deck with `slides` and no `viewport` | 1920×1080 | `/decks/{id}` |

Both kinds share the theme, the HTML and CSS rules, the workflow, the templates, and the uploads. Decks add a presenter view, an audience window that follows it, and a PPTX export. The deck JSON, routes, presenter, and PPTX come from Swift Deck, which is now part of this project.

## Core principles

- Ask only choices that change the result, with 2 to 4 short options each.
- Ask at most three questions per turn, and no more once five are answered.
- Never require an answer: every question has `Use your best judgment`, and the whole set can be skipped.
- The app asks its own closed questions itself, from fixed lists: how the colors read for both kinds; the canvas, how much to build, the product kind, the screen state, and the number of variations for a demo; the audience, the tone, the scenario, the length, the slide density, how much it leans on data, the candidates, and the variety for a deck. Their wording and options are the same in every session. The questions the request already answers come pre-selected, marked `suggested`, and one press accepts them.
- The agent asks 0 to 3 more, only what the request raises and the fixed list does not cover, such as which features a demo must show. Asking nothing is a normal turn.
- After the candidates exist, the chat edits: a message with a candidate open changes that candidate, a message without one writes new candidates.

## Workflow

| State | Purpose |
|---|---|
| `intake` | The request arrived; the first turn is running |
| `clarifying` | The agent asked questions and waits for answers |
| `generating` | The agent writes or edits candidates |
| `reviewing` | Candidates exist; the chat asks for changes |
| `stopped` | You stopped a run, or it was cut short; resume keeps the data |
| `error` | A run failed; retry keeps the data |

Stopping is not a failure. A run you stop, or one cut short by the server
going away, leaves the session `stopped` with its data intact and no error
message; **Resume** returns it to the state the run halted in. Only a real
failure reaches `error`. Both leave through the same `Recovered` event.

Every turn is one run of the planner, copied from Swift Deck: it reads the request, the answers, and the conversation, and replies with questions, a decision to write, a decision to edit the open candidate, or plain text.

The first turn is not the planner's to skip. The app owns a fixed list of
questions per kind, and asks it before anything is written, so a session always
opens with the setup card. The planner adds its own questions to that card, or
adds none. **Skip the questions and generate** writes straight away.

Source files reach a run three ways: the `+` button, a paste into the page, or
a drop onto the page. A paste or a drop can carry a folder. The app walks the
folder, keeps the path in each stored name, and skips hidden entries,
`node_modules`, `target`, `dist`, files above 45 MB, and everything past 200
files. It reports what it skipped.

A file belongs to one session. Files attached on the landing page wait in a
draft scope, and creating the session takes them. A run reads only its own
session's files, so a file attached to one project never reaches another
project's prompt. Deleting a session deletes its files. Two sessions may hold
the same name: the second is stored as `name-2.ext`.

## Candidates

Every candidate is a card with a live preview. Cards on one tab share a height, so a desktop card is wide and a phone card is a tall bezel. Click a card to open it in the editor. Arrows at the edges of the preview, or `←` and `→` on a focused card, step through the screens or slides; the pill in the corner shows `n/m`. A preview candidate (the first screens plus an outline) carries a `Finish` button that writes the rest; pressing it on a second card while the first is running joins that run. The chosen card is marked `Chosen`.

Type `@` in the chat to pin a candidate, or `All candidates`, for the next message: the change is applied to each pinned candidate. Without a pin the message edits the chosen candidate, or writes new ones.

The box at the top left of a card selects it. With cards selected, a bar over the canvas deletes them: one click arms the button, the second deletes. A deleted candidate that was the chosen one leaves the session with no choice.

Three more ways to work a candidate:

- **Fork.** The `Fork` button on a card copies it under the next free number, so a change can be tried on the copy while the original stays.
- **Merge.** Tick two or more cards and press `Merge`: the cards are pinned in the chat. Say which parts to take from each, for example "the hero from candidate 1 and the pricing table from candidate 3". The planner writes one new candidate from those parts. The candidates of a merge must share a canvas.
- **Redo a screen or a slide.** In the editor, the circular arrow on a thumbnail writes that screen or slide anew: the model sees its name and notes, not its old markup. One click arms the button, the second sends. The old version stays in the history.

Every new candidate takes the next free number: a later run adds candidates after the ones the session has instead of overwriting them.

In `reviewing`, the chat is the edit input: type what should change and press **Send**. With a candidate chosen, the change is applied to it; otherwise new candidates are written.

## Architecture

The harness may use a local CLI agent or a remote API, but workflow state, answers, artifacts, and user decisions remain under application control.

One run does one turn: plan, then ask, write, edit, or reply. The built-in engine runs the planner, the concept planner, the candidate writers, the fix-round loop, and the polish loop.

Nothing lets a screen spill off the canvas. Every page measures the content and, when it needs more room than the canvas gives, grows the box and scales the whole screen back. The screen comes out smaller but whole, in the studio, the PDF, and the PPTX alike. The layout audit still reports it as `overfull` with the percentage, so the polish loop cuts the content instead of leaving it small.

Two loops tighten a candidate. The **fix-round loop** feeds validation errors back until the JSON is valid. The **polish loop** renders the candidate in Chrome, measures it (contrast, line length, overflow, overlap), screenshots every screen, and sends the findings and the images back for a patch. It repeats until the page measures clean, or a round fixes nothing, or the effort's ceiling runs out: 1 round on `low`, 3 on `medium`, 5 on `high`. The version that measured best is the one kept, so a round that makes the page worse is discarded. The run log says which of the three ended it.

Designs and decks are two pipelines behind one workflow: separate types, stores, routes, renderers, prompts, and editors, with the shared helpers (history, provenance, CSS scoping, fonts, Chrome, the fix-round loop, the model client) used by both. See `CLAUDE.md` for the rules.

## Run it

```sh
# Build the WASM studio once (needs: cargo install dioxus-cli).
cd crates/ui && dx build --release && cd ../..

# Run the server on http://127.0.0.1:3000.
cargo run -p server
```

Open `http://127.0.0.1:3000`, pick a model in the studio settings, and describe
what you need. Pressing **Create** asks whether to build a software demo or a
deck. The agent runs on your own model account.

The design editor has two modes on a tab pair: **Play** (the default) and **Edit**. In
Edit a click selects a node. In Play a click acts as it would for a user:
a link to `#screen-3` opens screen 3, a `<details>` menu opens, and a
checkbox or radio toggle flips. A demo carries no script. Flows are links
between screens, and widgets are CSS states.

A demo exports as one HTML file. It has no PDF export: a print loses
the flows and the widgets.

For a deck, the editor adds **Present** (the presenter view with notes, a
timer, and an audience window that follows it), **PDF**, and **PPTX**
next to the HTML export. PDF, PPTX, and screenshots need Chrome or
Chromium on the server machine.

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
