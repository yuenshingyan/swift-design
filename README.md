# Swift Design

A design harness. You describe what you need, the agent asks a few questions in the chat, writes candidates, and edits them from the chat: a software demo, a slide deck, a paged document, a social post, a print piece, or an email.

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

## Six artifact kinds

Describe what you need and press **Create**. The app then asks which kind to
build, in a modal over the home page. The kind is fixed for the session; start a
new session to build another kind.

| Kind | What it is | JSON | Canvas | Saved under |
|---|---|---|---|---|
| `demo` | A software demo: a landing page, app screens, a flow | a design with `viewport` and `screens` | the viewport, 1440×900 by default; 390×844 for a phone; 1024×768 for a tablet | `/designs/{id}` |
| `deck` | A slide presentation | a deck with `slides` and no `viewport` | 1920×1080 | `/decks/{id}` |
| `document` | A paged document: a report, a memo, a proposal, a letter, a guide | a document with `paper` and `pages` | the paper: A4 (794×1123) by default, or Letter (816×1056) | `/documents/{id}` |
| `social` | A social post or a carousel for Instagram, LinkedIn, X, or Facebook | a social with `format` and `frames` | the format: portrait (1080×1350) by default, square (1080×1080), story (1080×1920), or landscape (1200×630) | `/socials/{id}` |
| `print` | A print piece: a poster, a flyer, a menu, a program, a certificate, a sign | a print with `size`, `orientation`, and `sheets` | the size: A4 (794×1123) by default, A5 (559×794), A3 (1123×1587), Letter (816×1056), or Tabloid (1056×1632); landscape swaps width and height | `/prints/{id}` |
| `mailing` | An email or an email sequence: a newsletter, an announcement, a promotion, a welcome email, a digest, an invitation | a mailing with `format` and `emails` | the format, 600 px wide: standard (600×1200) by default, short (600×800), or long (600×1800) | `/mailings/{id}` |

Every kind shares the theme, the HTML and CSS rules, the workflow, the templates, and the uploads. Decks add a presenter view, an audience window that follows it, and a PPTX export. The deck JSON, routes, presenter, and PPTX come from Swift Deck, which is now part of this project. Documents add a PDF export, one sheet per page, and a DOCX export: a flowing Word file built from the page HTML, with the theme's fonts and colors as its styles. Socials add a PDF export, one sheet per frame (the file a LinkedIn carousel takes), and a zip of one PNG per frame (the files an Instagram carousel takes). Prints add a PDF export, one PDF page per sheet (the file a print shop takes), and a zip of one PNG per sheet. Mailings add a PDF export, one PDF page per email, a zip of one PNG per email, and a zip of one email-client HTML file per email.

## Core principles

- Ask only choices that change the result, with 2 to 4 short options each.
- Ask at most three questions per turn, and no more once five are answered.
- Never require an answer: every question has `Use your best judgment`, and the whole set can be skipped.
- The app asks its own closed questions itself, from fixed lists: how the colors read for every kind; the canvas, how much to build, the product kind, the screen state, the fidelity (finished or wireframe), and the number of variations for a demo; the audience, the tone, the scenario, the length, the slide density, how much it leans on data, the candidates, and the variety for a deck; the audience, the tone, the kind of document, the paper, the page density, how much it leans on data, the length in pages, the candidates, and the variety for a document; the audience, the tone, how much it leans on data, the platform, the format, what the post is for, the length in frames, the candidates, and the variety for a social; the audience, the tone, how much it leans on data, the kind of print piece, the paper size, the orientation, the length in sheets, the candidates, and the variety for a print; the audience, the tone, how much it leans on data, the kind of email, the email format, the length in emails, the candidates, and the variety for a mailing. Their wording and options are the same in every session. The questions the request already answers come pre-selected, marked `suggested`, and one press accepts them.
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

The run reads what it can of each file: an image is shown to a model that
sees images, a PDF is sent as a file, and a text file is inlined. A Word
document, a PowerPoint deck, or an Excel workbook is inlined as its text:
one line per paragraph, one block per slide, one tab-separated line per row.

A link in the request or in a chat message is captured: the server opens
each `http(s)://` address (up to three per message) in Chrome and saves a
screenshot and the page text into the session's files, as
`capture-{host}.png` and `capture-{host}.txt`. The run reads them like any
upload, so "make it look like https://example.com" works. Addresses on this
machine and on private networks are refused. Without Chrome, links are left
as text.

A file belongs to one session. Files attached on the landing page wait in a
draft scope, and creating the session takes them. A run reads only its own
session's files, so a file attached to one project never reaches another
project's prompt. Deleting a session deletes its files. Two sessions may hold
the same name: the second is stored as `name-2.ext`.

## Templates

A template is a look kept for later: a theme plus a few example screens,
saved from a candidate with **Save as template** in the editor. The
template button on the landing page opens the picker. Pick one or more, and
every candidate of the new session is written in that look.

The picker also makes a template with no candidate behind it. Type a name
and a website, or attach brand files (a logo, a screenshot, a style guide)
to the composer, and press **Extract**: the model reads the material and
writes a theme (palette and fonts) plus a short style note. The template
shows as a swatch in the picker, and the note goes into every prompt that
uses it. A website extraction needs Chrome on the server machine.

Mark a template as **default** on its card (★): every new session starts
with it picked. Several templates can be default.

## Candidates

Every candidate is a card with a live preview. Cards on one tab share a height, so a desktop card is wide and a phone card is a tall bezel. Click a card to open it in the editor. Arrows at the edges of the preview, or `←` and `→` on a focused card, step through the screens or slides; the pill in the corner shows `n/m`. A preview candidate (the first screens plus an outline) shows a `planned` pill with the count still to write; tick it and press `Finish` in the bar to write the rest. Pressing it on a second card while the first is running joins that run. The chosen card is marked `Chosen`.

Type `@` in the chat to pin a candidate, or `All candidates`, for the next message: the change is applied to each pinned candidate. Without a pin the message edits the chosen candidate, or writes new ones.

Click a card's footer to select it, or ⌘-click anywhere on the card; a selected card shows a teal border and a teal name. Every action on candidates works the same way: tick cards, then press a button in the bar over the canvas. `Delete` takes two clicks: one arms the button, the second deletes. A deleted candidate that was the chosen one leaves the session with no choice.

Three more ways to work a candidate:

- **Fork.** Tick a card and press `Fork` in the bar over the canvas: the card is copied under the next free number, so a change can be tried on the copy while the original stays. Several ticked cards fork together.
- **Merge.** Pin two or more cards with `@` in the chat and say which parts to take from each, for example "the hero from candidate 1 and the pricing table from candidate 3". The planner writes one new candidate from those parts. The candidates of a merge must share a canvas.
- **Redo a screen or a slide.** In the editor, the circular arrow on a thumbnail writes that screen or slide anew: the model sees its name and notes, not its old markup. One click arms the button, the second sends. The old version stays in the history.

Every new candidate takes the next free number: a later run adds candidates after the ones the session has instead of overwriting them.

In `reviewing`, the chat is the edit input: type what should change and press **Send**. With a candidate chosen, the change is applied to it; otherwise new candidates are written.

## Architecture

The harness may use a local CLI agent or a remote API, but workflow state, answers, artifacts, and user decisions remain under application control.

One run does one turn: plan, then ask, write, edit, or reply. The built-in engine runs the planner, the concept planner, the candidate writers, the fix-round loop, and the polish loop.

Nothing lets a screen spill off the canvas. Every page measures the content and, when it needs more room than the canvas gives, grows the box and scales the whole screen back. The screen comes out smaller but whole, in the studio, the PDF, and the PPTX alike. The layout audit still reports it as `overfull` with the percentage, so the polish loop cuts the content instead of leaving it small.

Two loops tighten a candidate. The **fix-round loop** feeds validation errors back until the JSON is valid. The **polish loop** renders the candidate in Chrome, measures it (contrast, line length, overflow, overlap), screenshots every screen, and sends the findings and the images back for a patch. It repeats until the page measures clean, or a round fixes nothing, or the effort's ceiling runs out: 1 round on `low`, 3 on `medium`, 5 on `high`. The version that measured best is the one kept, so a round that makes the page worse is discarded. The run log says which of the three ended it.

Designs, decks, documents, socials, prints, and mailings are six pipelines behind one workflow: separate types, stores, routes, renderers, prompts, and editors, with the shared helpers (history, provenance, CSS scoping, fonts, Chrome, the fix-round loop, the model client) used by all. See `CLAUDE.md` for the rules.

## Run it

```sh
# Build the WASM studio once (needs: cargo install dioxus-cli).
cd crates/ui && dx build --release && cd ../..

# Run the server on http://127.0.0.1:3000.
cargo run -p server
```

Open `http://127.0.0.1:3000`, pick a model in the studio settings, and describe
what you need. Pressing **Create** asks whether to build a software demo, a
deck, a document, a social post, a print piece, or an email. The agent runs on your own model account.

The design editor has two modes on a tab pair: **Play** (the default) and **Edit**. In
Edit a click selects a node. In Play a click acts as it would for a user:
a link to `#screen-3` opens screen 3, a `<details>` menu opens, and a
checkbox or radio toggle flips. A demo carries no script. Flows are links
between screens, and widgets are CSS states. The deck editor, the
document editor, the social editor, the print editor, and the mailing
editor have the same pair as **Read** (the default) and **Edit**: Read
shows the slide, the page, the frame, the sheet, or the email as a
reader sees it, with no selection outlines. The document editor's
properties sheet also switches the paper between A4 and Letter; the
social editor's switches the format between square, portrait, story,
and landscape; the print editor's switches the size between A5, A4, A3,
Letter, and Tabloid, and the orientation between portrait and
landscape; the mailing editor's switches the format between short,
standard, and long.

In Edit, a click also puts a reference to the node in the chat, so "make
this bigger" names the exact element. To send several notes as one turn,
type a note and press **+ Comment** (or ⌘Enter): the note is kept with its
reference, the composer clears, and the next click starts the next note.
**Send** posts every kept comment as one message, one per line, and the
agent applies them in one edit.

A demo exports as one HTML file. It has no PDF export: a print loses
the flows and the widgets.

For a deck, the editor adds **Present** (the presenter view with notes, a
timer, and an audience window that follows it), **PDF**, and **PPTX**
next to the HTML export. PDF, PPTX, and screenshots need Chrome or
Chromium on the server machine.

For a document, the editor adds **PDF** (one sheet per page, through
Chrome) and **DOCX** next to the HTML export. The DOCX needs no Chrome: the
server walks the HTML of every page into headings, paragraphs, lists,
quotes, code, tables, and pictures, puts a page break between pages, and
writes the theme's fonts and colors as the Word styles. The page CSS is not
carried, because a Word file flows. Inline SVG and the notes are left out.

For a social, the editor adds **PDF** (one sheet per frame, through Chrome:
the file a LinkedIn carousel takes) and **PNG** (a zip with one PNG per
frame, through Chrome: the files an Instagram carousel takes) next to the
HTML export. The frame notes hold the caption to post with the frame.

For a print, the editor adds **PDF** (one PDF page per sheet, through
Chrome: the file a print shop takes) and **PNG** (a zip with one PNG per
sheet, through Chrome) next to the HTML export. The sheet notes hold print
instructions such as the paper stock or the bleed.

For a mailing, the editor adds **Copy** (one click puts the open email
on the clipboard as rich text: paste it straight into a Gmail or
Outlook compose window, or into an email service's HTML template box,
which takes the source), **Email** (a zip with one email-client
HTML file per email plus a subjects file; no Chrome needed), **PDF**
(one PDF page per email, through Chrome), and **PNG** (a zip with one
PNG per email, through Chrome) next to the HTML export. The Email files
carry inlined styles, a 600 px table shell, and Outlook ghost tables,
so they survive real email clients; the export has no fit script, so an
email flows taller than its canvas instead of shrinking. Uploaded
images are embedded as `data:` URIs, which Gmail blocks: rehost images
at a public URL for Gmail. The email notes hold the subject line and
the preheader text, as a `Subject:` line and a `Preheader:` line; the
export writes them into a subjects file and the hidden preview text.

## Agent routes

External agents read `GET /instructions` and the schemas at
`GET /schemas/{design,deck,document,social,print,mailing,question-set}`. A demo session
writes to `PUT /designs/{session}-candidate-N`; a deck session writes to
`PUT /decks/{session}-candidate-N`; a document session writes to
`PUT /documents/{session}-candidate-N`; a social session writes to
`PUT /socials/{session}-candidate-N`; a print session writes to
`PUT /prints/{session}-candidate-N`; a mailing session writes to
`PUT /mailings/{session}-candidate-N`. The run environment carries
`SWIFT_DESIGN_SESSION_ID`, `SWIFT_DESIGN_RUN_MODE`, and
`SWIFT_DESIGN_ARTIFACT_KIND`.

Turns: `POST /sessions/{id}/messages` sends a chat turn and starts a run;
`PUT /sessions/{id}/question-set` asks; `POST /sessions/{id}/answers` answers;
`POST /sessions/{id}/generate` opens the session for writing without more
questions; `POST /sessions/{id}/complete` ends a run.

Deck-only routes: `GET /decks/{id}/present`, `GET /decks/{id}/render?audience=true`,
`GET /decks/{id}/slides/{n}.png`, `GET /decks/{id}/export.pptx`.

Document-only routes: `GET /documents/{id}/pages/{n}.png`,
`GET /documents/{id}/export.pdf`, `GET /documents/{id}/export.docx`.

Social-only routes: `GET /socials/{id}/frames/{n}.png`,
`GET /socials/{id}/export.pdf`, `GET /socials/{id}/export.zip`.
Print-only routes: `GET /prints/{id}/sheets/{n}.png`,
`GET /prints/{id}/export.pdf`, `GET /prints/{id}/export.zip`.

Mailing-only routes: `GET /mailings/{id}/emails/{n}.png`,
`GET /mailings/{id}/emails/{n}.html`, `GET /mailings/{id}/export.pdf`,
`GET /mailings/{id}/export.zip`, `GET /mailings/{id}/export.email.zip`.

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
| `SWIFT_DESIGN_DOCUMENTS_DIR` | `documents` | Document JSON files |
| `SWIFT_DESIGN_DOCUMENT_HISTORY_DIR` | `document-history` | Document save snapshots |
| `SWIFT_DESIGN_SOCIALS_DIR` | `socials` | Social JSON files |
| `SWIFT_DESIGN_SOCIAL_HISTORY_DIR` | `social-history` | Social save snapshots |
| `SWIFT_DESIGN_PRINTS_DIR` | `prints` | Print JSON files |
| `SWIFT_DESIGN_PRINT_HISTORY_DIR` | `print-history` | Print save snapshots |
| `SWIFT_DESIGN_MAILINGS_DIR` | `mailings` | Mailing JSON files |
| `SWIFT_DESIGN_MAILING_HISTORY_DIR` | `mailing-history` | Mailing save snapshots |
| `SWIFT_DESIGN_SETTINGS_PATH` | `data/settings.json` | Provider, model, credential |
| `SWIFT_DESIGN_UI_DIR` | `target/dx/ui/release/web/public` | Built WASM bundle |
| `SWIFT_DESIGN_AGENT_COMMAND` | unset | External agent CLI; overrides the built-in engine |
| `SWIFT_DESIGN_CHROME` | unset | Chrome path for screenshots, PDF export, and PPTX export |
| `SWIFT_DESIGN_PROVIDER` / `_MODEL` / `_PROVIDER_URL` / `_PROVIDER_API_KEY` | `google` | Built-in engine defaults |

## Layout

```
crates/
  design-model/  # serde + schemars types: design, deck, document, social, print, mailing, question, workflow. No IO.
  server/        # axum: sessions, engines, validation, render, presenter, exports, static hosting.
  ui/            # Dioxus studio (WASM): session workspace, design, deck, document, social, print, and mailing editors.
fixtures/        # sample-design.json, sample-deck.json, sample-document.json, sample-social.json, sample-print.json, and sample-mailing.json
schemas/         # generated copies of the served JSON Schemas
```

The workflow state machine and the question protocol live in
`design-model`, so the server and the studio share one definition. See
`CLAUDE.md` for project conventions.
