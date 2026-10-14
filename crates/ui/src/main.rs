//! Swift Design editor: the browser UI for tweaking designs.
//!
//! Compiled to WASM with `dx build --release` and served by the
//! `server` crate. Run in development with `dx serve` from this
//! crate's directory.

mod api;
mod artwork_editor;
mod campaign_editor;
mod canvas;
mod chat;
mod chat_controls;
mod deck_editor;
mod document_editor;
mod editor;
mod home;
mod icons;
mod mailing_editor;
mod print_editor;
mod prompt_history;
mod question_card;
mod revert;
mod route;
mod run_settings;
mod select;
mod session;
mod settings;
mod social_editor;
mod status;
mod uploads;

use dioxus::document;
use dioxus::prelude::*;

use crate::route::View;

/// Embedded stylesheet: the editor ships as one WASM bundle with no
/// separate asset pipeline yet. The palette and type follow the
/// product canvas: three paper surfaces, ink text, one teal accent that
/// means "working" or "selected", Inter for text, JetBrains Mono for
/// identifiers. Radii, shadows, and colours are tokens on `:root`.
const STYLESHEET: &str = include_str!("main/styles.css");

/// The Swift Design pinwheel, as one SVG path in a 178 by 179 box. The
/// brand mark and the tab icon both draw this path, so the two cannot
/// drift apart.
const LOGO_PATH: &str = "\
M95,99L95,129L94,130L94,136L93,137L93,144L92,145L92,147L91,149L92,149L116,125L116,120ZM89,101L\
87,99L85,99L84,98L82,100L81,100L81,101L79,102L79,103L77,104L77,105L75,106L75,107L73,108L73,109\
L71,110L71,111L69,112L69,113L67,114L67,115L65,116L65,117L63,118L63,119L61,120L61,121L59,122L59\
,123L57,124L57,125L55,126L55,127L53,128L53,129L51,130L51,131L49,132L49,133L47,134L47,135L45,13\
6L45,137L43,138L43,139L41,140L41,141L39,142L39,143L37,144L37,145L35,146L35,147L33,148L33,149L3\
1,150L31,151L29,152L29,153L27,154L27,155L25,156L25,157L23,158L23,159L21,160L21,161L19,162L19,1\
63L17,164L17,165L15,166L14,168L13,168L13,170L16,170L17,171L34,171L35,172L42,172L43,171L58,171L\
59,170L62,170L63,169L67,169L70,167L72,167L74,165L76,165L80,161L80,160L84,155L84,153L86,149L86,\
146L87,145L87,142L88,141L88,132L89,131ZM29,92L29,93L30,93L31,95L32,95L33,97L34,97L35,99L36,99L\
37,101L38,101L39,103L40,103L41,105L42,105L43,107L44,107L45,109L46,109L47,111L48,111L49,113L50,\
113L51,115L52,115L54,117L56,117L58,116L78,96L78,95L47,95L46,94L38,94L37,93L34,93L33,92ZM100,89\
L99,90L98,95L168,165L170,165L170,156L171,155L171,125L170,124L170,116L168,112L168,109L164,101L1\
61,98L160,98L158,96L152,93L150,93L149,92L146,92L145,91L142,91L141,90L134,90L133,89L123,89L122,\
88L107,88L106,89ZM87,86L85,89L86,91L89,93L91,91L92,91L92,88L90,86ZM99,82L99,83L131,83L132,84L1\
37,84L138,85L144,85L148,87L148,85L125,62L120,62L118,64L117,64L117,65L115,66L115,67L113,68L113,\
69L111,70L111,71L109,72L109,73L107,74L107,75L105,76L105,77L103,78L103,79L101,80L100,82ZM86,29L\
62,53L61,55L61,57L63,59L63,60L81,78L83,79L83,46L84,45L84,37L85,36ZM8,13L7,15L7,30L6,31L6,48L7,\
49L7,59L8,60L8,63L9,64L9,67L10,68L11,73L13,75L13,76L19,82L25,85L30,86L31,87L36,87L37,88L40,88L\
41,89L56,89L57,90L66,90L67,89L70,90L71,89L78,89L78,87L80,85L80,84L9,13ZM165,8L157,8L156,7L145,\
7L144,6L134,6L133,7L123,7L122,8L116,8L115,9L108,10L106,12L104,12L102,14L101,14L96,19L93,25L93,\
27L91,31L91,35L90,36L90,39L89,40L89,53L88,54L88,72L89,73L89,78L90,79L92,79L94,80L165,9Z";

/// Builds the pinwheel as one SVG document, `size` pixels square and
/// filled with `fill`.
fn logo_svg(size: u32, fill: &str) -> String {
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{size}\" height=\"{size}\" \
         viewBox=\"0 0 178 179\" aria-hidden=\"true\">\
         <path fill=\"{fill}\" d=\"{LOGO_PATH}\"></path></svg>"
    )
}

fn main() {
    dioxus::launch(App);
}

/// What the top bar shows beside the brand while a view has a live
/// run: the status sentence and the model that runs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TopbarContext {
    /// The status sentence, like `Working… 45%`.
    pub label: String,
    /// `provider/model`, when a model is chosen.
    pub model: Option<String>,
}

/// Root component: top bar plus the current view. The view is kept in
/// the URL hash, so a reload lands on the same screen.
#[component]
fn App() -> Element {
    let mut view = use_signal(|| Option::<View>::None);
    let topbar_context = use_context_provider(|| Signal::new(Option::<TopbarContext>::None));
    // The arrow keys move the caret inside a draft; only on its first
    // or last line do they recall an earlier prompt.
    use_hook(|| {
        document::eval(prompt_history::ARROW_GUARD);
    });
    // Read the hash on load and on every hashchange.
    use_future(move || async move {
        let mut channel = document::eval(route::HASH_LISTENER);
        while let Ok(hash) = channel.recv::<String>().await {
            let next = route::route_from_hash(&hash);
            if view.peek().as_ref() != Some(&next) {
                view.set(Some(next));
            }
        }
    });
    // Write the hash the app chose.
    use_effect(move || {
        if let Some(current) = view() {
            let writer = document::eval(route::WRITE_HASH);
            let _ = writer.send(route::hash_for(&current));
        }
    });
    rsx! {
        style { dangerous_inner_html: STYLESHEET }
        header { class: "topbar",
            button { class: "brand", onclick: move |_| view.set(Some(View::Home)),
                span { dangerous_inner_html: logo_svg(20, "currentColor") }
                span { "Swift Design" }
            }
            if let Some(context) = topbar_context() {
                div { class: "topbar-context",
                    span { class: "status-dot" }
                    span { "{context.label}" }
                    if let Some(model) = context.model {
                        span { "{model}" }
                    }
                }
            }
        }
        match view() {
            None => rsx! {
                p { class: "lede", "Loading…" }
            },
            Some(View::Home) => rsx! {
                home::Home { on_open_session: move |id| view.set(Some(View::Session(id))) }
            },
            Some(View::Session(id)) => rsx! {
                session::SessionWorkspace {
                    session_id: id,
                    on_open_design: move |design_id| view.set(Some(View::Design(design_id))),
                    on_open_deck: move |deck_id| view.set(Some(View::Deck(deck_id))),
                    on_open_document: move |document_id| view.set(Some(View::Document(document_id))),
                    on_open_social: move |social_id| view.set(Some(View::Social(social_id))),
                    on_open_print: move |print_id| view.set(Some(View::Print(print_id))),
                    on_open_mailing: move |mailing_id| view.set(Some(View::Mailing(mailing_id))),
                    on_open_campaign: move |campaign_id| view.set(Some(View::Campaign(campaign_id))),
                    on_open_artwork: move |artwork_id| view.set(Some(View::Artwork(artwork_id))),
                    on_home: move |_| view.set(Some(View::Home)),
                }
            },
            Some(View::Design(id)) => {
                let session = settings::artifact_project(&id);
                rsx! {
                    editor::Editor {
                        design_id: id,
                        on_back: move |_| view.set(Some(View::Session(session.clone()))),
                    }
                }
            }
            Some(View::Deck(id)) => {
                let session = settings::artifact_project(&id);
                rsx! {
                    deck_editor::DeckEditor {
                        deck_id: id,
                        on_back: move |_| view.set(Some(View::Session(session.clone()))),
                    }
                }
            }
            Some(View::Document(id)) => {
                let session = settings::artifact_project(&id);
                rsx! {
                    document_editor::DocumentEditor {
                        document_id: id,
                        on_back: move |_| view.set(Some(View::Session(session.clone()))),
                    }
                }
            }
            Some(View::Social(id)) => {
                let session = settings::artifact_project(&id);
                rsx! {
                    social_editor::SocialEditor {
                        social_id: id,
                        on_back: move |_| view.set(Some(View::Session(session.clone()))),
                    }
                }
            }
            Some(View::Print(id)) => {
                let session = settings::artifact_project(&id);
                rsx! {
                    print_editor::PrintEditor {
                        print_id: id,
                        on_back: move |_| view.set(Some(View::Session(session.clone()))),
                    }
                }
            }
            Some(View::Mailing(id)) => {
                let session = settings::artifact_project(&id);
                rsx! {
                    mailing_editor::MailingEditor {
                        mailing_id: id,
                        on_back: move |_| view.set(Some(View::Session(session.clone()))),
                    }
                }
            }
            Some(View::Campaign(id)) => {
                let session = settings::artifact_project(&id);
                rsx! {
                    campaign_editor::CampaignEditor {
                        campaign_id: id,
                        on_back: move |_| view.set(Some(View::Session(session.clone()))),
                    }
                }
            }
            Some(View::Artwork(id)) => {
                let session = settings::artifact_project(&id);
                rsx! {
                    artwork_editor::ArtworkEditor {
                        artwork_id: id,
                        on_back: move |_| view.set(Some(View::Session(session.clone()))),
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_logo_holds_the_pinwheel_path_at_the_size_and_colour_asked_for() {
        let svg = logo_svg(32, "#0E6E63");
        assert!(svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"32\""));
        assert!(svg.contains("height=\"32\""));
        assert!(svg.contains("viewBox=\"0 0 178 179\""));
        assert!(svg.contains("fill=\"#0E6E63\""));
        assert!(svg.contains(LOGO_PATH));
        assert!(svg.ends_with("</svg>"));
    }

    #[test]
    fn the_tab_icon_draws_the_brand_path() {
        assert!(
            include_str!("../../../assets/favicon.svg").contains(LOGO_PATH),
            "assets/favicon.svg draws an old path: rebuild it and \
             assets/favicon.ico from LOGO_PATH"
        );
    }

    #[test]
    fn the_page_links_the_tab_icon_files() {
        let template = include_str!("../index.html");
        assert!(template.contains(r#"<link rel="icon" href="/favicon.ico" sizes="any">"#));
        assert!(template.contains(r#"type="image/svg+xml" href="/favicon.svg""#));
    }
}
