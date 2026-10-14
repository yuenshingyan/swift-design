use super::components::{candidate_choices, is_judgment_choice, scenario_choices, variety_choices};
use super::*;

pub(crate) fn fixed_choices(choices: &[(&'static str, &'static str)]) -> Vec<(String, String)> {
    let mut options = vec![(String::new(), "The agent decides".to_owned())];
    options.extend(
        choices
            .iter()
            .map(|(value, label)| ((*value).to_owned(), (*label).to_owned())),
    );
    options
}

/// Most label characters a card may hold before it takes the full row.
const NARROW_CARD_CHARACTERS: usize = 90;

/// Most chips a card may hold before it takes the full row.
const NARROW_CARD_CHIPS: usize = 5;

/// True when a card's chips need the full row. A card with few short
/// chips shares a row with its neighbours, so the question grid stays
/// short.
pub(crate) fn is_wide_card(choices: &[(String, String)]) -> bool {
    let chips = choices
        .iter()
        .filter(|(value, _)| !is_judgment_choice(value))
        .count();
    let characters: usize = choices
        .iter()
        .filter(|(value, _)| !is_judgment_choice(value))
        .map(|(_, label)| label.chars().count())
        .sum();
    chips > NARROW_CARD_CHIPS || characters > NARROW_CARD_CHARACTERS
}

/// One app-owned question: the field it writes, its wording, and its
/// fixed choices.
pub(crate) struct Axis {
    /// Which `SessionOptions` field the pick lands in.
    pub(crate) key: &'static str,
    /// The question as the user reads it.
    pub(crate) label: &'static str,
    /// The fixed choices, before the judgment chip is added.
    pub(crate) choices: &'static [(&'static str, &'static str)],
}

/// The app-owned questions for `kind`, in the order they are shown.
///
/// These recur in every session, so the app asks them from a fixed list
/// and the agent never invents options for them. The agent may still
/// add questions this list does not cover.
pub(crate) fn axes_for(kind: ArtifactKind) -> Vec<Axis> {
    match kind {
        ArtifactKind::Demo => vec![
            Axis {
                key: "color_mode",
                label: "How should the colors read?",
                choices: &COLOR_MODES,
            },
            Axis {
                key: "scope",
                label: "How much should I build?",
                choices: &DEMO_SCOPES,
            },
            Axis {
                key: "product_kind",
                label: "What kind of product is it?",
                choices: &PRODUCT_KINDS,
            },
            Axis {
                key: "data_state",
                label: "What data should the screens show?",
                choices: &DATA_STATES,
            },
            Axis {
                key: "fidelity",
                label: "How finished should it look?",
                choices: &FIDELITIES,
            },
        ],
        // The audience and the tone are a deck's questions: a deck
        // speaks to a room. A demo's request already says what the
        // product is. A deck draws its colors card beside the scenario
        // card, and its evidence card last, so `DeckQuestions` draws
        // both.
        ArtifactKind::Deck => vec![
            Axis {
                key: "audience",
                label: "Who is it for?",
                choices: &AUDIENCES,
            },
            Axis {
                key: "tone",
                label: "What tone should it have?",
                choices: &TONES,
            },
            Axis {
                key: "slide_density",
                label: "How much goes on a slide?",
                choices: &SLIDE_DENSITIES,
            },
        ],
        // A document speaks to a reader, so it asks the audience and
        // the tone like a deck. Its own axes name the kind of document,
        // the paper, and the density of a page. The colors, the
        // evidence, the length, the candidates, and the variety sit on
        // `DocumentQuestions`.
        ArtifactKind::Document => vec![
            Axis {
                key: "audience",
                label: "Who is it for?",
                choices: &AUDIENCES,
            },
            Axis {
                key: "tone",
                label: "What tone should it have?",
                choices: &TONES,
            },
            Axis {
                key: "document_kind",
                label: "What kind of document is it?",
                choices: &DOCUMENT_KINDS,
            },
            Axis {
                key: "paper",
                label: "What paper is it for?",
                choices: &PAPERS,
            },
            Axis {
                key: "page_density",
                label: "How much goes on a page?",
                choices: &SLIDE_DENSITIES,
            },
        ],
        // A social speaks to a feed, so it asks the audience and the
        // tone like a deck. Its own axes name the platform, the
        // format, and the goal of the post. The colors, the evidence,
        // the length, the candidates, and the variety sit on
        // `SocialQuestions`.
        ArtifactKind::Social => vec![
            Axis {
                key: "audience",
                label: "Who is it for?",
                choices: &AUDIENCES,
            },
            Axis {
                key: "tone",
                label: "What tone should it have?",
                choices: &TONES,
            },
            Axis {
                key: "platform",
                label: "What platform is it for?",
                choices: &PLATFORMS,
            },
            Axis {
                key: "format",
                label: "What format is it?",
                choices: &FORMATS,
            },
            Axis {
                key: "post_goal",
                label: "What is the post for?",
                choices: &POST_GOALS,
            },
        ],
        // A print speaks to a reader, so it asks the audience and the
        // tone like a document. Its own axes name the print kind, the
        // paper size, and the orientation. The colors, the evidence,
        // the length, the candidates, and the variety sit on
        // `PrintQuestions`.
        ArtifactKind::Print => vec![
            Axis {
                key: "audience",
                label: "Who is it for?",
                choices: &AUDIENCES,
            },
            Axis {
                key: "tone",
                label: "What tone should it have?",
                choices: &TONES,
            },
            Axis {
                key: "print_kind",
                label: "What kind of print piece is it?",
                choices: &PRINT_KINDS,
            },
            Axis {
                key: "print_size",
                label: "What paper size is it for?",
                choices: &PRINT_SIZES,
            },
            Axis {
                key: "orientation",
                label: "How is it turned?",
                choices: &ORIENTATIONS,
            },
        ],
        // A mailing speaks to a reader, so it asks the audience and
        // the tone like a document. Its own axes name the email kind
        // and the format. The colors, the evidence, the length, the
        // candidates, and the variety sit on `MailingQuestions`.
        ArtifactKind::Mailing => vec![
            Axis {
                key: "audience",
                label: "Who is it for?",
                choices: &AUDIENCES,
            },
            Axis {
                key: "tone",
                label: "What tone should it have?",
                choices: &TONES,
            },
            Axis {
                key: "email_kind",
                label: "What kind of email is it?",
                choices: &EMAIL_KINDS,
            },
            Axis {
                key: "email_format",
                label: "What canvas is it laid out on?",
                choices: &EMAIL_FORMATS,
            },
        ],
        // A campaign speaks to a reader too. Its own axes name the ad
        // kind and the size. The colors, the evidence, the length, the
        // candidates, and the variety sit on `CampaignQuestions`.
        ArtifactKind::Campaign => vec![
            Axis {
                key: "audience",
                label: "Who is it for?",
                choices: &AUDIENCES,
            },
            Axis {
                key: "tone",
                label: "What tone should it have?",
                choices: &TONES,
            },
            Axis {
                key: "ad_kind",
                label: "What kind of ad is it?",
                choices: &AD_KINDS,
            },
            Axis {
                key: "ad_size",
                label: "What size is it laid out on?",
                choices: &AD_SIZES,
            },
        ],
        // An artwork speaks to a viewer too. Its own axes name the
        // cover kind and the size. The colors, the evidence, the
        // length, the candidates, and the variety sit on
        // `ArtworkQuestions`.
        ArtifactKind::Artwork => vec![
            Axis {
                key: "audience",
                label: "Who is it for?",
                choices: &AUDIENCES,
            },
            Axis {
                key: "tone",
                label: "What tone should it have?",
                choices: &TONES,
            },
            Axis {
                key: "cover_kind",
                label: "What kind of cover is it?",
                choices: &COVER_KINDS,
            },
            Axis {
                key: "cover_size",
                label: "What size is it laid out on?",
                choices: &COVER_SIZES,
            },
        ],
    }
}

/// True when the planner filled `key` from the request and the user
/// has not picked it since.
pub(crate) fn is_suggested(options: &api::SessionOptions, key: &str) -> bool {
    options.suggested.iter().any(|known| known == key)
}

/// The app's own questions with their answers, for the chat record
/// of the setup card. A blank field, or a judgment pick, reads as
/// assumed: the agent decided it.
pub(crate) fn app_answers(
    kind: ArtifactKind,
    options: &api::SessionOptions,
) -> Vec<AnsweredQuestion> {
    let mut entries: Vec<AnsweredQuestion> = axes_for(kind)
        .iter()
        .map(|axis| {
            recorded(
                axis.label,
                axis_value(options, axis.key).as_deref(),
                &fixed_choices(axis.choices),
            )
        })
        .collect();
    match kind {
        ArtifactKind::Demo => {
            entries.push(recorded(
                "Variations",
                options.variations.map(|count| count.to_string()).as_deref(),
                &candidate_choices(),
            ));
            entries.push(canvas_record(&options.platforms));
        }
        ArtifactKind::Deck => {
            entries.push(recorded(
                "How should the colors read?",
                options.color_mode.as_deref(),
                &fixed_choices(&COLOR_MODES),
            ));
            entries.push(recorded(
                "What scenario is the deck for?",
                options.scenario.as_deref(),
                &scenario_choices(),
            ));
            entries.push(recorded(
                "How different should the candidates be?",
                Some(&options.variety),
                &variety_choices(),
            ));
            entries.push(recorded(
                "How long should the deck be?",
                options
                    .slide_count
                    .map(|count| count.to_string())
                    .as_deref(),
                &slide_count_options(),
            ));
            entries.push(recorded(
                "How many candidates should I write?",
                options.variations.map(|count| count.to_string()).as_deref(),
                &candidate_choices(),
            ));
            entries.push(recorded(
                "How much does it lean on data?",
                options.evidence_style.as_deref(),
                &fixed_choices(&EVIDENCE_STYLES),
            ));
        }
        ArtifactKind::Document => {
            entries.push(recorded(
                "How should the colors read?",
                options.color_mode.as_deref(),
                &fixed_choices(&COLOR_MODES),
            ));
            entries.push(recorded(
                "How different should the candidates be?",
                Some(&options.variety),
                &variety_choices(),
            ));
            entries.push(recorded(
                "How long should the document be?",
                options.page_count.map(|count| count.to_string()).as_deref(),
                &page_count_options(),
            ));
            entries.push(recorded(
                "How many candidates should I write?",
                options.variations.map(|count| count.to_string()).as_deref(),
                &candidate_choices(),
            ));
            entries.push(recorded(
                "How much does it lean on data?",
                options.evidence_style.as_deref(),
                &fixed_choices(&EVIDENCE_STYLES),
            ));
        }
        ArtifactKind::Social => {
            entries.push(recorded(
                "How should the colors read?",
                options.color_mode.as_deref(),
                &fixed_choices(&COLOR_MODES),
            ));
            entries.push(recorded(
                "How different should the candidates be?",
                Some(&options.variety),
                &variety_choices(),
            ));
            entries.push(recorded(
                "How many frames should it have?",
                options
                    .frame_count
                    .map(|count| count.to_string())
                    .as_deref(),
                &frame_count_options(),
            ));
            entries.push(recorded(
                "How many candidates should I write?",
                options.variations.map(|count| count.to_string()).as_deref(),
                &candidate_choices(),
            ));
            entries.push(recorded(
                "How much does it lean on data?",
                options.evidence_style.as_deref(),
                &fixed_choices(&EVIDENCE_STYLES),
            ));
        }
        ArtifactKind::Print => {
            entries.push(recorded(
                "How should the colors read?",
                options.color_mode.as_deref(),
                &fixed_choices(&COLOR_MODES),
            ));
            entries.push(recorded(
                "How different should the candidates be?",
                Some(&options.variety),
                &variety_choices(),
            ));
            entries.push(recorded(
                "How many sheets should it have?",
                options
                    .sheet_count
                    .map(|count| count.to_string())
                    .as_deref(),
                &sheet_count_options(),
            ));
            entries.push(recorded(
                "How many candidates should I write?",
                options.variations.map(|count| count.to_string()).as_deref(),
                &candidate_choices(),
            ));
            entries.push(recorded(
                "How much does it lean on data?",
                options.evidence_style.as_deref(),
                &fixed_choices(&EVIDENCE_STYLES),
            ));
        }
        ArtifactKind::Mailing => {
            entries.push(recorded(
                "How should the colors read?",
                options.color_mode.as_deref(),
                &fixed_choices(&COLOR_MODES),
            ));
            entries.push(recorded(
                "How different should the candidates be?",
                Some(&options.variety),
                &variety_choices(),
            ));
            entries.push(recorded(
                "How many emails are in the sequence?",
                options
                    .email_count
                    .map(|count| count.to_string())
                    .as_deref(),
                &email_count_options(),
            ));
            entries.push(recorded(
                "How many candidates should I write?",
                options.variations.map(|count| count.to_string()).as_deref(),
                &candidate_choices(),
            ));
            entries.push(recorded(
                "How much does it lean on data?",
                options.evidence_style.as_deref(),
                &fixed_choices(&EVIDENCE_STYLES),
            ));
        }
        ArtifactKind::Campaign => {
            entries.push(recorded(
                "How should the colors read?",
                options.color_mode.as_deref(),
                &fixed_choices(&COLOR_MODES),
            ));
            entries.push(recorded(
                "How different should the candidates be?",
                Some(&options.variety),
                &variety_choices(),
            ));
            entries.push(recorded(
                "How many ads are in the set?",
                options.ad_count.map(|count| count.to_string()).as_deref(),
                &ad_count_options(),
            ));
            entries.push(recorded(
                "How many candidates should I write?",
                options.variations.map(|count| count.to_string()).as_deref(),
                &candidate_choices(),
            ));
            entries.push(recorded(
                "How much does it lean on data?",
                options.evidence_style.as_deref(),
                &fixed_choices(&EVIDENCE_STYLES),
            ));
        }
        ArtifactKind::Artwork => {
            entries.push(recorded(
                "How should the colors read?",
                options.color_mode.as_deref(),
                &fixed_choices(&COLOR_MODES),
            ));
            entries.push(recorded(
                "How different should the candidates be?",
                Some(&options.variety),
                &variety_choices(),
            ));
            entries.push(recorded(
                "How many covers are in the set?",
                options
                    .cover_count
                    .map(|count| count.to_string())
                    .as_deref(),
                &cover_count_options(),
            ));
            entries.push(recorded(
                "How many candidates should I write?",
                options.variations.map(|count| count.to_string()).as_deref(),
                &candidate_choices(),
            ));
            entries.push(recorded(
                "How much does it lean on data?",
                options.evidence_style.as_deref(),
                &fixed_choices(&EVIDENCE_STYLES),
            ));
        }
    }
    entries
}

/// One recorded answer: the label of a preset, a typed answer as typed,
/// or an assumed row when nothing was picked.
fn recorded(question: &str, value: Option<&str>, choices: &[(String, String)]) -> AnsweredQuestion {
    let answer = value
        .filter(|value| !is_judgment_choice(value))
        .map(|value| {
            choices
                .iter()
                .find(|(known, _)| known == value)
                .map(|(_, label)| label.clone())
                .unwrap_or_else(|| value.to_owned())
        });
    AnsweredQuestion {
        question: question.to_owned(),
        is_assumed: answer.is_none(),
        answer: answer.unwrap_or_default(),
    }
}

/// The canvas row: the picked canvases by name, or assumed when none.
fn canvas_record(platforms: &[String]) -> AnsweredQuestion {
    let picked = picked_platforms(platforms);
    let names: Vec<&str> = platform_choices()
        .into_iter()
        .filter(|choice| picked.iter().any(|value| value == choice.value))
        .map(|choice| choice.label)
        .collect();
    AnsweredQuestion {
        question: "Canvas".to_owned(),
        is_assumed: names.is_empty(),
        answer: names.join(", "),
    }
}

/// The value stored for `key`, if any.
pub(crate) fn axis_value(options: &api::SessionOptions, key: &str) -> Option<String> {
    match key {
        "audience" => options.audience.clone(),
        "tone" => options.tone.clone(),
        "color_mode" => options.color_mode.clone(),
        "scope" => options.scope.clone(),
        "product_kind" => options.product_kind.clone(),
        "data_state" => options.data_state.clone(),
        "fidelity" => options.fidelity.clone(),
        "slide_density" => options.slide_density.clone(),
        "evidence_style" => options.evidence_style.clone(),
        "document_kind" => options.document_kind.clone(),
        "paper" => options.paper.clone(),
        "page_density" => options.page_density.clone(),
        "platform" => options.platform.clone(),
        "format" => options.format.clone(),
        "post_goal" => options.post_goal.clone(),
        "print_kind" => options.print_kind.clone(),
        "print_size" => options.print_size.clone(),
        "orientation" => options.orientation.clone(),
        "email_kind" => options.email_kind.clone(),
        "email_format" => options.email_format.clone(),
        "ad_kind" => options.ad_kind.clone(),
        "ad_size" => options.ad_size.clone(),
        "cover_kind" => options.cover_kind.clone(),
        "cover_size" => options.cover_size.clone(),
        _ => None,
    }
}

/// The options with `key` set to `value`. An empty value clears it,
/// which leaves that axis to the agent. A pick is the user's own, so
/// the axis stops being a suggestion.
pub(crate) fn with_axis(
    options: &api::SessionOptions,
    key: &str,
    value: String,
) -> api::SessionOptions {
    let mut next = options.clone();
    next.suggested.retain(|known| known != key);
    let picked = (!value.is_empty()).then_some(value);
    match key {
        "audience" => next.audience = picked,
        "tone" => next.tone = picked,
        "color_mode" => next.color_mode = picked,
        "scope" => next.scope = picked,
        "product_kind" => next.product_kind = picked,
        "data_state" => next.data_state = picked,
        "fidelity" => next.fidelity = picked,
        "slide_density" => next.slide_density = picked,
        "evidence_style" => next.evidence_style = picked,
        "document_kind" => next.document_kind = picked,
        "paper" => next.paper = picked,
        "page_density" => next.page_density = picked,
        "platform" => next.platform = picked,
        "format" => next.format = picked,
        "post_goal" => next.post_goal = picked,
        "print_kind" => next.print_kind = picked,
        "print_size" => next.print_size = picked,
        "orientation" => next.orientation = picked,
        "email_kind" => next.email_kind = picked,
        "email_format" => next.email_format = picked,
        "ad_kind" => next.ad_kind = picked,
        "ad_size" => next.ad_size = picked,
        "cover_kind" => next.cover_kind = picked,
        "cover_size" => next.cover_size = picked,
        _ => {}
    }
    next
}

/// The options after one pick on a setup card. `key` is an axis key,
/// or `scenario`, `slides`, `pages`, `frames`, `sheets`, `emails`,
/// `ads`, `covers`, `candidates`, or `variety`.
/// An empty value is the judgment choice: it clears the field, or sets
/// the server default where the field has no blank state.
pub(crate) fn with_pick(
    options: &api::SessionOptions,
    key: &str,
    value: &str,
) -> api::SessionOptions {
    let mut next = options.clone();
    match key {
        "scenario" => next.scenario = (!value.is_empty()).then(|| value.to_owned()),
        "slides" => next.slide_count = value.parse().ok(),
        "pages" => next.page_count = value.parse().ok(),
        "frames" => next.frame_count = value.parse().ok(),
        "sheets" => next.sheet_count = value.parse().ok(),
        "emails" => next.email_count = value.parse().ok(),
        "ads" => next.ad_count = value.parse().ok(),
        "covers" => next.cover_count = value.parse().ok(),
        "candidates" => next.variations = value.parse().ok(),
        "variety" => {
            next.variety = if value.is_empty() {
                "medium".to_owned()
            } else {
                value.to_owned()
            };
        }
        _ => return with_axis(options, key, value.to_owned()),
    }
    next
}

/// The options with every pick made on this page applied. A save is
/// built from the options the session holds plus all the picks, so a
/// second pick made before the session polls back keeps the first.
pub(crate) fn with_picks(
    options: &api::SessionOptions,
    picks: &HashMap<String, String>,
) -> api::SessionOptions {
    picks.iter().fold(options.clone(), |next, (key, value)| {
        with_pick(&next, key, value)
    })
}

/// True when the planner filled `key` and the user has not picked it
/// on this page either. A pick drops the tag at once, before the
/// session polls back without the suggestion.
pub(crate) fn is_still_suggested(
    options: &api::SessionOptions,
    picks: &HashMap<String, String>,
    key: &str,
) -> bool {
    is_suggested(options, key) && !picks.contains_key(key)
}

/// The picks made on this page, keyed by card.
type Picks = Signal<HashMap<String, String>>;

/// The callback that records a pick as (card key, value).
type Pick = Callback<(String, String)>;

/// The picks made on this page, and the callback that records one and
/// saves the options. A card reads its pick from the map, so it shows
/// the choice at once instead of waiting for the session to poll back,
/// and the judgment choice shows as itself even where the server keeps
/// a default in its place.
pub(crate) fn use_setup_picks(
    session_id: String,
    options: api::SessionOptions,
    on_error: EventHandler<String>,
) -> (Picks, Pick) {
    let mut picks = use_signal(HashMap::<String, String>::new);
    let pick = use_callback(move |(key, value): (String, String)| {
        picks.write().insert(key, value);
        let next = with_picks(&options, &picks());
        if next == options {
            return;
        }
        let id = session_id.clone();
        spawn(async move {
            if let Err(message) = api::save_session_options(&id, &next).await {
                on_error.call(message);
            }
        });
    });
    (picks, pick)
}
