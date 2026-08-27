//! The presenter view: what the speaker sees.
//!
//! `GET /decks/{id}/present` is a page with the current slide, the next
//! slide, the slide's notes, a timer, and a slide counter. It is the
//! source of truth for the slide position: every change is published on
//! a BroadcastChannel and in localStorage under one key per deck. The
//! audience window, `GET /decks/{id}/render?audience=true`, follows that
//! key. Both pages are same-origin, so no server round trip is needed.

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use design_model::{Deck, Slide};
use serde::Deserialize;

use crate::api_error;
use crate::decks::{DeckStore, is_valid_deck_id};
use crate::render::{escape_html, script_hash};

/// Moves the deck, publishes the position, runs the timer, and swaps the
/// next-slide frame and the notes. Every per-deck value comes from
/// `data-*` attributes on `<body>`, so the script text is constant and
/// its CSP hash never depends on user data.
const PRESENTER_SCRIPT: &str = r##"(() => {
  const body = document.body;
  const deckId = body.dataset.deckId;
  const slideCount = Number(body.dataset.slideCount) || 1;
  const channelName = body.dataset.channel;
  const audienceUrl = body.dataset.audienceUrl;
  const slideUrl = body.dataset.slideUrl;
  let current = Math.min(Math.max(Number(body.dataset.startSlide) || 0, 0), slideCount - 1);
  const counter = document.getElementById('counter');
  const timer = document.getElementById('timer');
  const nextFrame = document.getElementById('next-slide');
  const endNote = document.getElementById('end');
  const notes = Array.from(document.querySelectorAll('[data-notes-slide]'));
  const channel = window.BroadcastChannel ? new BroadcastChannel(channelName) : null;
  function publish() {
    const message = { type: 'swift-design-presenter', slide: current, sent_at: Date.now() };
    try { localStorage.setItem(channelName, JSON.stringify(message)); } catch (error) {}
    if (channel) { channel.postMessage(message); }
  }
  function show() {
    counter.textContent = String(current + 1);
    notes.forEach((note) => { note.hidden = Number(note.dataset.notesSlide) !== current; });
    const hasNext = current + 1 < slideCount;
    nextFrame.hidden = !hasNext;
    endNote.hidden = hasNext;
    if (hasNext) {
      const wanted = slideUrl + (current + 2);
      if (nextFrame.getAttribute('src') !== wanted) { nextFrame.setAttribute('src', wanted); }
    }
    publish();
  }
  function goTo(index) {
    const target = Math.min(Math.max(index, 0), slideCount - 1);
    if (target === current) { publish(); return; }
    current = target;
    show();
  }
  if (channel) {
    channel.addEventListener('message', (event) => {
      if (event.data && event.data.type === 'swift-design-audience-hello') { publish(); }
    });
  }
  const startedAt = { at: Date.now() };
  function pad(value) { return String(value).padStart(2, '0'); }
  function tick() {
    const seconds = Math.floor((Date.now() - startedAt.at) / 1000);
    const hours = Math.floor(seconds / 3600);
    const minutes = Math.floor((seconds % 3600) / 60);
    const rest = seconds % 60;
    timer.textContent = hours > 0 ? hours + ':' + pad(minutes) + ':' + pad(rest) : pad(minutes) + ':' + pad(rest);
  }
  setInterval(tick, 250);
  document.getElementById('reset-timer').addEventListener('click', () => { startedAt.at = Date.now(); tick(); });
  document.getElementById('previous').addEventListener('click', () => goTo(current - 1));
  document.getElementById('next').addEventListener('click', () => goTo(current + 1));
  document.getElementById('open-audience').addEventListener('click', () => {
    const opened = window.open(audienceUrl, 'swift-design-audience-' + deckId);
    if (opened) { opened.focus(); }
  });
  document.addEventListener('keydown', (event) => {
    const target = event.target;
    if (target && target.tagName === 'BUTTON' && event.key === ' ') { return; }
    const isNext = event.key === 'ArrowRight' || event.key === 'PageDown' || event.key === ' ';
    const isPrevious = event.key === 'ArrowLeft' || event.key === 'PageUp';
    if (isNext) { event.preventDefault(); goTo(current + 1); }
    else if (isPrevious) { event.preventDefault(); goTo(current - 1); }
    else if (event.key === 'Home') { event.preventDefault(); goTo(0); }
    else if (event.key === 'End') { event.preventDefault(); goTo(slideCount - 1); }
  });
  tick();
  show();
})();
"##;

/// The presenter page stylesheet: a dark tool window with the current
/// slide large on the left, the next slide and the notes on the right.
/// Light teal is the accent: the editor's teal does not survive on dark.
const PRESENTER_STYLE: &str = "html, body { margin: 0; height: 100%; background: #14171B; color: #F2F4F6;\n\
  font: 15px/1.4 Inter, system-ui, sans-serif; }\n\
body { display: flex; flex-direction: column; }\n\
*:focus-visible { outline: 2px solid #7FBFB4; outline-offset: 2px; }\n\
.bar { display: flex; align-items: center; gap: 14px; padding: 14px 22px; border-bottom: 1px solid #262B31; }\n\
.bar .title { font-size: 14px; font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }\n\
.bar .spacer { flex: 1; }\n\
.bar .divider { width: 1px; height: 22px; background: #262B31; flex: none; }\n\
.counter, .timer { font-family: 'JetBrains Mono', ui-monospace, monospace; font-variant-numeric: tabular-nums;\n\
  font-size: 20px; white-space: nowrap; }\n\
.counter .of { font-size: 13px; color: #7A828B; }\n\
.timer { min-width: 5ch; color: #7FBFB4; }\n\
button { font: inherit; font-size: 12px; color: #9AA2AA; background: #1B1F24; border: 1px solid #333940;\n\
  border-radius: 7px; padding: 6px 11px; cursor: pointer; white-space: nowrap;\n\
  transition: background-color 120ms ease, border-color 120ms ease; }\n\
button:hover { background: #22272D; border-color: #3E454D; color: #F2F4F6; }\n\
.nav { display: flex; border: 1px solid #333940; background: #1B1F24; border-radius: 7px; overflow: hidden; }\n\
.nav button { border: 0; border-radius: 0; padding: 7px 13px; font-size: 13px; color: #E6E9EC; }\n\
.nav button + button { border-left: 1px solid #333940; }\n\
.nav button:focus-visible { outline-offset: -2px; }\n\
button.primary { background: #7FBFB4; border-color: #7FBFB4; color: #0B0E11; font-weight: 600;\n\
  padding: 8px 14px; font-size: 12.5px; }\n\
button.primary:hover { background: #93CBC1; border-color: #93CBC1; color: #0B0E11; }\n\
.panes { flex: 1; display: grid; grid-template-columns: minmax(0, 1fr) 380px; gap: 18px;\n\
  padding: 18px 22px 22px; min-height: 0; }\n\
.pane h2 { margin: 0 0 9px; font-family: 'JetBrains Mono', ui-monospace, monospace; font-size: 11px;\n\
  font-weight: 500; letter-spacing: 0.11em; text-transform: uppercase; color: #7A828B; }\n\
.side { display: flex; flex-direction: column; gap: 18px; min-height: 0; }\n\
iframe { display: block; width: 100%; aspect-ratio: 16 / 9; border: 0; background: #000; pointer-events: none;\n\
  border-radius: 10px; box-shadow: 0 0 0 1px #262B31; }\n\
.pane.current iframe { box-shadow: 0 0 0 1px #262B31, 0 24px 50px -34px rgba(0,0,0,.9); }\n\
iframe[hidden], .end[hidden] { display: none; }\n\
.end { display: grid; place-items: center; aspect-ratio: 16 / 9; margin: 0; border: 1px dashed #333940;\n\
  border-radius: 10px; color: #7A828B; }\n\
.notes { flex: 1; min-height: 0; display: flex; flex-direction: column; }\n\
.notes-body { flex: 1; overflow: auto; background: #1B1F24; border: 1px solid #262B31; border-radius: 10px;\n\
  padding: 15px 16px; font-size: 15px; line-height: 1.65; color: #D6DBE0; }\n\
.notes-text { white-space: pre-wrap; margin: 0; }\n\
.empty { margin: 0; color: #7A828B; }\n";

/// The `/decks/{id}/present` route table.
pub fn routes() -> Router<crate::AppState> {
    Router::new().route("/decks/{id}/present", get(present_deck))
}

/// The BroadcastChannel and localStorage key the presenter of `deck_id`
/// publishes on. One key per deck, so two decks never cross.
pub(crate) fn channel_name(deck_id: &str) -> String {
    format!("swift-design-presenter:{deck_id}")
}

/// Query of `GET /decks/{id}/present`.
#[derive(Debug, Deserialize)]
struct PresentQuery {
    /// Start on this one-based slide. Defaults to the first.
    #[serde(default)]
    slide: Option<usize>,
}

/// Serves the presenter page for a stored deck.
async fn present_deck(
    State(store): State<DeckStore>,
    Path(id): Path<String>,
    Query(query): Query<PresentQuery>,
) -> Response {
    if !is_valid_deck_id(&id) {
        return api_error::invalid_deck_id(&id);
    }
    let deck = match store.load(&id).await {
        Ok(Some(deck)) => deck,
        Ok(None) => return api_error::deck_not_found(&id),
        Err(error) => return api_error::internal_error(&error),
    };
    let errors = deck.validate();
    if !errors.is_empty() {
        return api_error::deck_validation_failed(&errors);
    }
    let start = match starting_slide(query.slide, deck.slides.len()) {
        Ok(start) => start,
        Err(number) => {
            return api_error::error_response(
                StatusCode::NOT_FOUND,
                &format!(
                    "deck `{id}` has no slide {number}: use 1 to {}",
                    deck.slides.len()
                ),
                Vec::new(),
            );
        }
    };
    Html(presenter_page(&id, &deck, start)).into_response()
}

/// Turns the optional one-based `slide` query into a zero-based index.
/// `Err` carries the number that was out of range.
fn starting_slide(number: Option<usize>, slide_count: usize) -> Result<usize, usize> {
    match number {
        None => Ok(0),
        Some(number) if number >= 1 && number <= slide_count => Ok(number - 1),
        Some(number) => Err(number),
    }
}

/// The per-deck values the presenter page is built from.
struct PresenterContext<'a> {
    /// The deck id, for the URLs and the channel.
    deck_id: &'a str,
    /// The deck itself.
    deck: &'a Deck,
    /// The zero-based slide the page starts on.
    start_slide: usize,
}

impl PresenterContext<'_> {
    fn slide_count(&self) -> usize {
        self.deck.slides.len()
    }

    fn has_next(&self) -> bool {
        self.start_slide + 2 <= self.slide_count()
    }

    fn audience_url(&self) -> String {
        format!("/decks/{}/render?audience=true", self.deck_id)
    }

    fn slide_url(&self) -> String {
        format!("/decks/{}/render?slide=", self.deck_id)
    }
}

/// Builds the presenter page for `deck`, starting on `start_slide`
/// (zero-based).
fn presenter_page(deck_id: &str, deck: &Deck, start_slide: usize) -> String {
    let context = PresenterContext {
        deck_id,
        deck,
        start_slide,
    };
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; \
         script-src 'sha256-{hash}'; style-src 'unsafe-inline'; frame-src 'self'; \
         connect-src 'none'; object-src 'none'; form-action 'none'; base-uri 'none'\">\n\
         <title>{title} · presenter</title>\n<style>\n{style}</style>\n</head>\n\
         <body data-deck-id=\"{id}\" data-slide-count=\"{slide_count}\" data-start-slide=\"{start_slide}\" \
         data-channel=\"{channel}\" data-audience-url=\"{audience_url}\" data-slide-url=\"{slide_url}\">\n\
         {header}{panes}\
         <script>{script}</script>\n</body>\n</html>\n",
        hash = script_hash(PRESENTER_SCRIPT),
        title = escape_html(&deck.title),
        style = PRESENTER_STYLE,
        id = escape_html(deck_id),
        slide_count = context.slide_count(),
        channel = escape_html(&channel_name(deck_id)),
        audience_url = escape_html(&context.audience_url()),
        slide_url = escape_html(&context.slide_url()),
        header = presenter_header(&context),
        panes = presenter_panes(&context),
        script = PRESENTER_SCRIPT,
    )
}

/// The top bar: title, counter, timer, arrows, and the audience button.
fn presenter_header(context: &PresenterContext<'_>) -> String {
    format!(
        "<header class=\"bar\">\n\
         <span class=\"title\">{title}</span>\n\
         <span class=\"spacer\"></span>\n\
         <span class=\"counter\"><span id=\"counter\">{start_number}</span><span class=\"of\"> / {slide_count}</span></span>\n\
         <span class=\"divider\"></span>\n\
         <span id=\"timer\" class=\"timer\">00:00</span>\n\
         <button id=\"reset-timer\" type=\"button\">Reset</button>\n\
         <span class=\"divider\"></span>\n\
         <div class=\"nav\">\n\
         <button id=\"previous\" type=\"button\" title=\"Previous slide\">←</button>\n\
         <button id=\"next\" type=\"button\" title=\"Next slide\">→</button>\n\
         </div>\n\
         <button id=\"open-audience\" type=\"button\" class=\"primary\">Open audience window</button>\n\
         </header>\n",
        title = escape_html(&context.deck.title),
        start_number = context.start_slide + 1,
        slide_count = context.slide_count(),
    )
}

/// The two panes: the current slide, then the next slide and the notes.
fn presenter_panes(context: &PresenterContext<'_>) -> String {
    let has_next = context.has_next();
    format!(
        "<main class=\"panes\">\n\
         <section class=\"pane current\">\n<h2>Current</h2>\n\
         <iframe id=\"current\" title=\"Current slide\" src=\"{audience_url}\"></iframe>\n</section>\n\
         <aside class=\"side\">\n\
         <section class=\"pane\">\n<h2>Next</h2>\n\
         <iframe id=\"next-slide\" title=\"Next slide\" src=\"{slide_url}{next_number}\"{next_hidden}></iframe>\n\
         <p id=\"end\" class=\"end\"{end_hidden}>End of deck</p>\n</section>\n\
         <section class=\"pane notes\">\n<h2>Notes</h2>\n<div class=\"notes-body\">\n{notes}</div>\n</section>\n\
         </aside>\n</main>\n",
        audience_url = escape_html(&context.audience_url()),
        slide_url = escape_html(&context.slide_url()),
        next_number = if has_next {
            context.start_slide + 2
        } else {
            context.slide_count()
        },
        next_hidden = if has_next { "" } else { " hidden" },
        end_hidden = if has_next { " hidden" } else { "" },
        notes = render_notes(&context.deck.slides, context.start_slide),
    )
}

/// One hidden article per slide with its notes escaped; the article for
/// `shown` starts visible. Slides without notes say so.
fn render_notes(slides: &[Slide], shown: usize) -> String {
    let mut html = String::new();
    for (index, slide) in slides.iter().enumerate() {
        let hidden = if index == shown { "" } else { " hidden" };
        let body = match slide.notes.as_deref().map(str::trim) {
            Some(notes) if !notes.is_empty() => {
                format!("<p class=\"notes-text\">{}</p>", escape_html(notes))
            }
            _ => "<p class=\"empty\">No notes.</p>".to_owned(),
        };
        html.push_str(&format!(
            "<article data-notes-slide=\"{index}\"{hidden}>{body}</article>\n"
        ));
    }
    html
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use design_model::Deck;
    use sha2::Digest;

    use crate::export::base64_encode;
    use crate::presenter::{channel_name, presenter_page, render_notes, starting_slide};

    fn sample_deck() -> Deck {
        serde_json::from_str(include_str!("../../../fixtures/sample-deck.json")).unwrap()
    }

    #[test]
    fn channel_name_is_scoped_by_deck_id() {
        assert_eq!(channel_name("talk"), "swift-design-presenter:talk");
        assert_ne!(channel_name("talk"), channel_name("talk-2"));
    }

    #[test]
    fn starting_slide_defaults_to_the_first_slide() {
        assert_eq!(starting_slide(None, 3), Ok(0));
    }

    #[test]
    fn starting_slide_accepts_one_based_numbers_in_range() {
        assert_eq!(starting_slide(Some(1), 3), Ok(0));
        assert_eq!(starting_slide(Some(3), 3), Ok(2));
    }

    #[test]
    fn starting_slide_rejects_zero_and_past_the_end() {
        assert_eq!(starting_slide(Some(0), 3), Err(0));
        assert_eq!(starting_slide(Some(4), 3), Err(4));
    }

    #[test]
    fn presenter_page_escapes_the_notes() {
        let mut deck = sample_deck();
        deck.slides[0].notes = Some("<script>alert(1)</script> & more".to_owned());
        let html = presenter_page("talk", &deck, 0);
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt; &amp; more"));
        assert!(!html.contains("<script>alert(1)</script>"));
    }

    #[test]
    fn presenter_page_shows_the_counter_and_the_slide_count() {
        let deck = sample_deck();
        let html = presenter_page("talk", &deck, 1);
        assert!(html.contains(&format!("data-slide-count=\"{}\"", deck.slides.len())));
        assert!(html.contains("data-start-slide=\"1\""));
        assert!(html.contains("<span id=\"counter\">2</span>"));
        assert!(html.contains(&format!(
            "<span class=\"of\"> / {}</span>",
            deck.slides.len()
        )));
        assert!(html.contains("data-channel=\"swift-design-presenter:talk\""));
    }

    #[test]
    fn presenter_page_groups_the_arrows_and_labels_the_panes() {
        let html = presenter_page("talk", &sample_deck(), 0);
        assert!(html.contains("<div class=\"nav\">"));
        assert!(html.contains(
            "<button id=\"previous\" type=\"button\" title=\"Previous slide\">←</button>"
        ));
        assert!(
            html.contains("<button id=\"next\" type=\"button\" title=\"Next slide\">→</button>")
        );
        assert!(html.contains("<button id=\"reset-timer\" type=\"button\">Reset</button>"));
        for label in ["<h2>Current</h2>", "<h2>Next</h2>", "<h2>Notes</h2>"] {
            assert!(html.contains(label), "missing {label}");
        }
    }

    #[test]
    fn presenter_page_marks_slides_without_notes() {
        let mut deck = sample_deck();
        deck.slides[1].notes = None;
        let notes = render_notes(&deck.slides, 0);
        assert!(notes.contains("<article data-notes-slide=\"0\">"));
        assert!(
            notes.contains(
                "<article data-notes-slide=\"1\" hidden><p class=\"empty\">No notes.</p>"
            )
        );
    }

    #[test]
    fn the_presenter_csp_hash_matches_the_emitted_script() {
        let html = presenter_page("talk", &sample_deck(), 0);
        let start = html.find("<script>").unwrap() + "<script>".len();
        let end = html.find("</script>").unwrap();
        let hash = base64_encode(&sha2::Sha256::digest(&html.as_bytes()[start..end]));
        assert!(html.contains(&format!("script-src 'sha256-{hash}'")));
        assert!(html.contains("frame-src 'self'"));
        assert!(html.contains("'swift-design-presenter'"));
    }

    #[test]
    fn presenter_page_links_the_audience_window_and_the_next_slide() {
        let deck = sample_deck();
        let html = presenter_page("talk", &deck, 0);
        assert!(html.contains("src=\"/decks/talk/render?audience=true\""));
        assert!(html.contains("src=\"/decks/talk/render?slide=2\"></iframe>"));
        assert!(html.contains("<p id=\"end\" class=\"end\" hidden>"));
        let last = presenter_page("talk", &deck, deck.slides.len() - 1);
        assert!(last.contains("render?slide=3\" hidden></iframe>"));
        assert!(last.contains("<p id=\"end\" class=\"end\">End of deck</p>"));
    }
}
