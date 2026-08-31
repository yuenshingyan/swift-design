# Swift Design — Project Conventions

## What Swift Design Is

Swift Design is a harness that guides LLM agents to build HTML artifacts of four kinds:

- **Software demos** ("designs"): landing pages, app screens, and similar layouts on a device viewport. A design is a theme, a viewport, and `screens` with HTML and CSS.
- **Decks**: slide presentations on a fixed 1920 by 1080 px canvas. A deck is a theme and `slides` with HTML and CSS. Decks keep the Swift Deck JSON shape and routes, with a presenter view and HTML, PDF, and PPTX exports.
- **Documents**: paged documents (reports, memos, proposals, letters, guides) on A4 (794 by 1123 px) or Letter (816 by 1056 px) paper. A document is a theme, a `paper`, and `pages` with HTML and CSS. It has no transition and no controls. Exports: HTML, PDF (one sheet per page), and DOCX (`docx.rs`: a flowing Word file built from the page HTML, no Chrome needed).
- **Socials**: social posts and carousels for Instagram, LinkedIn, X, and Facebook on a fixed canvas: square (1080 by 1080 px), portrait (1080 by 1350 px, the default), story (1080 by 1920 px), or landscape (1200 by 630 px). A social is a theme, a `format`, and `frames` with HTML and CSS. One frame is a single post; two or more are a carousel. It has no transition and no controls. Exports: HTML, PDF (one sheet per frame, the LinkedIn carousel file), and a zip of one PNG per frame (`export.zip`, the Instagram carousel files). No new export module: the PNGs come from the frame screenshot path and the zip from the `zip` crate `docx.rs` already uses.

The workflow is the same for every kind, copied from Swift Deck:

- The user picks the kind and sends a request. A run starts at once.
- Every run is one planner turn (`planner.rs`): the model reads the request, the answers so far, and the conversation, and replies with questions, a decision to write, a decision to edit the open candidate, or plain text. Questions are choices with 2 to 4 short options, never required, at most three per turn, none after five answers. The user can skip them all.
- The first turn of a session always ends in the question card. The app asks its own fixed list before anything is written, whatever the planner decided; `app_question_set` carries the card when the planner adds no question of its own. The planner only adds to that card.
- The app asks its own closed questions itself, on the same card, from fixed lists in `design-model` (`run_questions.rs`, `deck_questions.rs`): the color mode for every kind; the canvas, the scope, the product kind, the screen state, the fidelity, and the variations for a demo; the audience, the tone, and the evidence style for a deck, a document, and a social (`SPEECH_AXES`); the scenario, the length, the slide density, the candidates, and the variety for a deck; the document kind, the paper, the page density, the length in pages, the candidates, and the variety for a document; the platform, the format, the post goal, the length in frames, the candidates, and the variety for a social. These recur in every session, so hardcoding them keeps the wording and the options identical between runs. The agent asks only what the request itself raises. The setup planner turn also returns `suggestions`: an app axis the request answers by itself (`product_kind` from "a TODO app", `color_mode` from "dark"), as option key and fixed value. `RunOptions::suggest` fills the blank axes and lists them in `options.suggested`; the card shows them picked with a `suggested` tag, and a pick by the user drops the key.
- Agents build artifacts by writing JSON files that conform to the served schemas. There is no brief: `request.rs` renders the request, the app's choices, and the answers as the prompt input.
- After candidates exist, a chat message is a turn: with candidates pinned (`@` in the session chat lists them and `All candidates`; the message carries their ids in `pinned`), or a candidate open or chosen, the planner edits each of them in turn; otherwise it writes new candidates.
- **Generation runs on the user's own accounts, never on Swift Design's.** Two paths: an external agent CLI the user already runs (Claude Code, pi), or the built-in provider loop (`model_client.rs`, `generation.rs`, `deck_generation.rs`) that calls any LLM provider with the user's own keys. Swift Design supplies schemas, prompts, validation, the state machine, and the editors.

## Languages & Stack

- Rust everywhere. Edition 2024.
- Server: axum on tokio. Serves the studio UI, design and deck files, sessions, and uploads.
- Studio UI: Dioxus, compiled to WASM.
- Design, deck, and question definitions: JSON. serde structs are the source of truth. schemars generates the JSON Schemas that guide agents.
- Logging: tracing.

## Project Layout

Cargo workspace with three crates:

```
crates/
  design-model/ # serde + schemars types for designs, decks, questions, and the workflow. No IO.
  server/       # axum binary: API, sessions, engines, static assets, uploads, validation.
  ui/           # Dioxus studio (WASM).
```

- `server` and `ui` depend on `design-model`. `design-model` depends on no workspace crate.
- Declare shared dependency versions in `[workspace.dependencies]`.
- Organize modules by feature (`sessions.rs`, `planner.rs`, `render.rs`), not by layer.

## Four Pipelines

Designs, decks, documents, and socials are separate pipelines that share one workflow.

- Separate per kind: the model types (`design.rs`, `screen.rs`; `deck.rs`, `slide.rs`; `document.rs`, `page.rs`; `social.rs`, `frame.rs`), the stores (`designs.rs`, `decks.rs`, `documents.rs`, `socials.rs`), the routes (`/designs/*`, `/decks/*`, `/documents/*`, `/socials/*`), the render entry points (`render.rs`, `deck_render.rs`, `document_render.rs`, `social_render.rs`), the patches (`patch.rs`, `deck_patch.rs`, `document_patch.rs`, `social_patch.rs`), the polish prompts (`polish.rs`, `deck_polish.rs`, `document_polish.rs`, `social_polish.rs`), the generation prompts (`generation.rs`, `deck_generation.rs`, `document_generation.rs`, `social_generation.rs`), and the editors (`editor.rs`, `deck_editor.rs`, `document_editor.rs`, `social_editor.rs`). Deck-only modules: `presenter.rs` and `pptx.rs`. Document-only module: `docx.rs`.
- A document is the deck pipeline with the paper as the canvas: `Document::viewport()` comes from `paper`, the render passes it to the shared `stylesheet` and `print_stylesheet`, and the units are pages (`[page N]`, `?page=N`, `pages[N]`). A document has no transition, no presenter, no audience script, and no measurement script.
- A social is the document pipeline with the format as the canvas: `Social::viewport()` comes from `format` (`Format::ALL` in `social.rs`, the same shape as `Paper`), and the units are frames (`[frame N]`, `?frame=N`, `frames[N]`, `id="frame-N"`). The word `frame` is also the shared DOM name of a unit's outer wrapper (`data-swift-design-frame`, `NAVIGATION_SCRIPT`); the social render calls that wrapper `frame-shell` so the two do not collide. The polish loop drops `static_control` like `deck_polish`, and the rules tell the model that the first frame is the hook and the caption goes in `notes`.
- Shared only where the code is identical modulo names: `history.rs`, `provenance.rs`, `events.rs`, `api_error.rs`, `files.rs`, `screen_css.rs`, the Chrome runner in `screenshots.rs`, the font and upload inlining in `export.rs`, the page scripts in `render.rs`, the fix-round loop, the polish loop, and attachments in `generation.rs`, `model_client.rs`, `sessions.rs`, `planner.rs`, `request.rs`.
- A session has one `artifact_kind` (`demo`, `deck`, `document`, or `social`), set at creation. The engine, the chooser, and the studio read that one value. Do not infer the kind from ids or file contents.
- `render::FIT_SCRIPT` runs on every rendered page, print included. A root whose content needs more than the canvas grows to the largest box that fits and scales back through `--swift-design-fit`, so nothing is ever cut off. The audit reports the shrink as `overfull`; the PPTX measurement divides it out.
- A deck page, a document page, and a social page use the same DOM vocabulary as a design page (`main.design`, `data-swift-design-*`), so the layout, navigation, editing, and audit scripts serve all four. Only the audience-follow script and the PPTX measurement script are deck-only.
- `PATCH_FORMAT` and the polish wording are duplicated on purpose: the model sees one vocabulary per kind (screens, slides, or pages).
- A demo carries no script, no form, and no `<button>`. Interaction is static: a link to `#screen-N` opens screen N (the frame ids are `screen-N`, counted from 1, and `NAVIGATION_SCRIPT` handles the click), `<details>` opens a menu, and `<input type='checkbox'>` or `radio` with `:checked` holds a toggle, a tab, or a modal. `markup.rs` rejects every other input type. The audit reports a box that looks like a control and does nothing as `static_control`; the polish loop feeds it back like a layout finding, and `deck_polish` drops it, since a slide is not clicked. The design editor has two modes (`PreviewMode`): Play, the default, loads no editing script, so a click acts; Edit loads it, so a click selects. The deck editor has the same pair as `DeckPreviewMode`, the document editor as `DocumentPreviewMode`, and the social editor as `SocialPreviewMode`: Read, the default, and Edit. A single-screen page posts `swift-design-navigate` with `target` when a screen link is clicked.
- An edit turn is focused or systemic (`edit_focus.rs`). A change whose references name screens, slides, pages, or frames sends only those units, each with its index, and the Chrome findings for them; a change that names none sends the whole artifact and every finding. The audit runs before every edit, at every effort. After the edit, `fix_edited_deck`, `fix_edited_document`, `fix_edited_social`, and `fix_edited_design` measure the touched units again and ask the model to fix what Chrome finds, for up to `polish_round_limit(effort)` rounds; the best version measured is saved.

## Workflow State

- A session has one persisted `WorkflowState`: `intake`, `clarifying`, `generating`, `reviewing`, or `error`.
- Every state change goes through `design_model::workflow::transition` and `SessionStore::apply`. Do not infer state from chat text, files, or UI flags.
- Events: `QuestionsAsked` (to clarifying), `GenerationStarted` (to generating, from intake, clarifying, or reviewing), `GenerationSucceeded` (to reviewing), `ContinueRequested`, `RunFailed`, `Recovered`.
- A run from `intake` always ends in `clarifying`: it opens the setup card and writes nothing.
- A continue request is allowed in `reviewing`, in `generating`, and in a halted state, where it is the resume. Pressing Finish on a second candidate joins the running turn: `continue_artifacts` wakes on every store change, reads the trailing continue requests whose artifact is still a preview, and starts the new ones alongside the running ones. `run_late_continues` catches a press that lands after the batch ends.
- A run dies with the process. `SessionStore::stop_orphaned_runs` runs at boot: a session in `generating` whose run has no end is stopped, so it takes messages again.
- A run may start in every state but `error`. A message, an answer set, and `POST /sessions/{id}/generate` each start one. A session already in `generating` at run start writes candidates without a planner turn (the user skipped the questions).
- Design, deck, document, and social writes answer 409 unless the session is `generating` (or `reviewing`, for user saves from the editor). An external agent posts `/sessions/{id}/generate` before it writes.
- The app's answers live on `Session.options`, not in a question set: the artifact kind, the variation count (1 to `CANDIDATE_LIMIT`), the axes in `SHARED_AXES`, `DEMO_AXES`, `SPEECH_AXES`, `DECK_AXES`, `DOCUMENT_AXES`, and `SOCIAL_AXES`, the canvases for a demo (1 to `PLATFORM_LIMIT`), for a deck the scenario (`DECK_SCENARIOS`), the slide count, and the variety, for a document the page count (1 to `PAGE_COUNT_LIMIT`) and the variety, and for a social the frame count (1 to `FRAME_COUNT_LIMIT`) and the variety. `RunOptions::axes` lists the picked axes for a kind, and `request_input` renders them; adding an axis means one entry in each table plus the `RunOptions` field and its two `match` arms (`axis`, `axis_slot_mut`), not a new branch. Every app question also takes an answer the user types: the chips are a shortcut, not the whole world. `option_problem` accepts a value from the fixed list or one that passes `is_custom_answer` (printable, at most `CUSTOM_ANSWER_LIMIT` characters), and `request_input` prints a typed answer as typed, because it has no label. The prompts tell the agent never to ask about them.
- A demo run writes one design per canvas per variation: `candidate_plans` in `generation.rs` names them, and the studio groups the cards under one tab per canvas. Every new candidate takes the next free number (`next_candidate_number` in `candidates.rs`): a later run, a fork, and a merge all number after the candidates the session has, so none overwrites an earlier one.
- Three candidate operations sit beside edit. A fork (`POST /designs/{id}/fork`, `POST /decks/{id}/fork`, `POST /documents/{id}/fork`, `POST /socials/{id}/fork`) copies a candidate under the next free number; it is refused with 409 while the session is `generating`. A merge is a planner verb (`merge` in the reply JSON, `GenerationTask::Merge`): two or more pinned candidates and an instruction write one new candidate through the fresh-candidate path (`generate_candidate` with `MergeInput`); design sources must share a `viewport`. A regenerate is a message with `action: "regenerate"` (`ChatMessage.is_regenerate`, `GenerationTask::Regenerate`): it skips the planner, and `edit_design`, `edit_deck`, `edit_document`, or `edit_social` runs with `EditOrder.is_fresh`, so the named units are shown without their html and css and the model writes them anew.
- Every question set is relaxed on write (`relax_question_set`): no question required, skip always allowed. An external agent's own flags are not trusted.

- A session id is a random version 4 UUID (`new_session_id`), not a name. The name is `title`, and the times are `created_at` and `updated_at`; the listing carries all three. An id built from the request text made the same request produce the same id, so two sessions with the same wording collided. A caller may still pass its own `id` to `POST /sessions`.
- Deleting a session deletes everything it owns: its designs, decks, documents, and socials with their candidates, sidecars, and history (`DesignStore::delete_session`, `DeckStore::delete_session`, `DocumentStore::delete_session`, `SocialStore::delete_session`), and its uploads. A session id comes from the request text, so the same request makes the same id; without the sweep the next session would open on the deleted one's candidates. Deleting one artifact deletes its history too.

- `attachment_parts` in `generation.rs` decides how an upload reaches the model by its content type: an image as an `image_url` part, a PDF as a `file` part, `text/*` and JSON inlined, and DOCX, PPTX, and XLSX inlined as the text `office.rs` reads out of their XML parts (no XML crate: a string walk over `<w:t>`, `<a:t>`, and `<t>`). The type comes from the extension (`content_type_of` in `uploads.rs`), and source extensions map to `text/plain`; a file of unknown type that is valid UTF-8 without a NUL byte is inlined too (`text_of`). Only a binary file is named only. Inlined text is cut per file at `ATTACHMENT_TEXT_LIMIT_BYTES` and per request at `ATTACHMENT_TEXT_TOTAL_LIMIT_BYTES`; the files past the budget are named, and the model is told to ask for what it needs.
- A template (`templates.rs`) is the only cross-session style carrier. `POST /templates/extract` (`brand.rs`) makes one from a website capture or from uploads: the model answers with a `Theme` and a style note, saved with no screens. `render_template` shows such a template as a swatch (`swatch_screen`), and `template_note` puts the note into the candidate prompt. `PUT /templates/{id}/default` sets `is_default`; the landing page picks the defaults once per visit.
- A link in a request or a message is captured by `capture.rs` before the run starts: `urls_in` reads up to `CAPTURE_LIMIT` addresses, `capture_problem` refuses local and private hosts, and Chrome (`screenshot_url`, `dump_url` in `screenshots.rs`) gives a PNG and a DOM, saved as `capture-{host}.png` and `capture-{host}.txt` in the session's uploads. Without Chrome the message posts with no capture. The capture runs inside the route handler, so the run that follows reads the files.
- The editor chat keeps comments (`comments` in `chat.rs`): `+ Comment` (⌘Enter) stores the reference plus the note, and Send posts them as one message, one per line (`message_with_comments`). `referenced_indexes` reads every `[screen N` in it, so the edit is focused on all the named units.
- An upload belongs to one scope: the session id, or `_draft` for a file attached before the session exists. `uploads/owners.json` records it, the path stays flat (`/uploads/{name}`) because stored designs and the export inliner name files that way, and `UploadStore::adopt` hands the draft files to a new session. `load_attachments` reads one session's files only.

## Studio

- The server serves the built bundle from `target/dx/ui/release/web/public`. After any change under `crates/ui`, run `cd crates/ui && dx build --release`, then reload. A server change needs `cargo build -p server` and a restart.
- All studio CSS lives in `STYLESHEET` in `crates/ui/src/main.rs`. Verify a visual change with a screenshot before reporting it: a static mock built from the extracted stylesheet, or the live page on a spare port (`SWIFT_DESIGN_ADDRESS=127.0.0.1:3001`). Port 3000 belongs to the user's own server.
- Candidate cards fix the height and let the width follow the canvas ratio: `frame_width_rem` in `canvas.rs` emits `--frame-width`, and the strip tiles do the same with `--tile-width`. A demo on a portrait canvas (`CanvasCard::is_bezelled`: the kind is `Demo` and `is_portrait_canvas`, the phone) gets a bezel; a document page and a social frame are portrait too but they are paper and a picture, so they get none. A narrow canvas (`is_narrow_canvas`, the tablet too) only changes the main preview height. Do not mix the two. The tab of a document canvas is named by its paper (`A4`, `Letter`); the tab of a social canvas by its format (`Square`, `Portrait`, `Story`, `Landscape`).
- Cards page with the edge arrows and the arrow keys only. A swipe was tried and removed: a trackpad swipe reads as history navigation in Safari unless a real scroll container consumes it, and the sliding preview showed blank panes. Do not bring it back.
- The open question set renders in the workbench (`workbench-questions`), with the app's own cards in the same grid. It owns the workbench while it is open: the run settings and the candidates wait behind it. Once answered, the set becomes a Q&A record in the chat thread (`thread-answers`). The run state and the run error live in the chat column.
- `dx fmt` duplicates an `.await` that is split across lines. Bind every await on one line: `let sent = api::continue_artifact(&session_id, &id).await;`.

## Naming

- Rust defaults: `snake_case` functions, variables, modules, and files. `PascalCase` types. `SCREAMING_SNAKE_CASE` constants.
- Never abbreviate: `configuration` not `config`, `context` not `ctx`, `request` not `req`. Keep Rust-universal tokens (`id`, `impl`) and names a crate's API mandates.
- Boolean names start with `is_`, `has_`, `should_`, or `can_`.
- Dioxus components: `PascalCase` function names under `#[component]`.
- Vocabulary: a design has screens; a deck has slides; a document has pages; a social has frames. Use `artifact` only for code that serves every kind.

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
- Validate agent-written JSON (designs, decks, question sets) at the server boundary with serde and the model crate's validators. Report every validation error, not only the first — agents fix output from these messages.

## Error Handling

- `design-model` and other library code: typed error enums with thiserror.
- Binaries: `anyhow::Result` at the top level. Add context with `.context("...")`.
- Error messages state what failed, the input that caused it, and the fix when known.
- Never put credentials, authorization headers, prompt text, or filesystem paths into browser responses, session records, or run records. Use ids.

## Logging

- Use tracing with structured fields: `tracing::info!(session_id = %session_id, "questions asked")`.
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
- Agents receive everything from the running app, never from repo files: instructions at `GET /instructions`, the schemas at `GET /schemas/{design,deck,document,social,question-set}`, the session at `GET /sessions/{id}`. Do not add agent-facing markdown or prompt files to the repo.
- The instructions payload lives in `crates/server/src/instructions.rs`. It carries one rule list per kind (`DEMO_RULES`, `DECK_RULES`, `DOCUMENT_RULES`, `SOCIAL_RULES`) and one example per kind. Update it whenever agent-visible behavior changes.
- The app serves the schemas from the Rust types at runtime, so they cannot go stale. `schemas/` holds generated copies for review diffs only: regenerate and commit them whenever `design-model` types change (`cargo run -p server --bin generate_schema`). CI fails on a stale copy.
- Sample artifacts live in `fixtures/` (`sample-design.json`, `sample-deck.json`, `sample-document.json`, `sample-social.json`) for tests; the instructions payload embeds every example for agents.
- Write schema descriptions, prompts, and instruction strings in Simplified Technical English: short imperative sentences, one instruction per sentence, one term per concept.

## Server API

- Routes: plural nouns, kebab-case: `/sessions`, `/sessions/{id}/messages`, `/designs`, `/decks`, `/documents`, `/socials`, `/uploads`.
- Deck-only routes: `/decks/{id}/present`, `/decks/{id}/render?audience=true`, `/decks/{id}/slides/{n}.png`, `/decks/{id}/export.pptx`.
- Document-only routes: `/documents/{id}/pages/{n}.png`, `/documents/{id}/export.pdf`, `/documents/{id}/export.docx`.
- Social-only routes: `/socials/{id}/frames/{n}.png`, `/socials/{id}/export.pdf`, `/socials/{id}/export.zip`.
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
