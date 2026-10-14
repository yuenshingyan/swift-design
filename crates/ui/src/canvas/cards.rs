use super::*;

const CARD_STAGE_HEIGHT_REM: f64 = 13.5;
/// The tallest stage, for a tab with one or two cards.
const STAGE_HEIGHT_CAP_REM: f64 = 30.0;
/// The row the cards of one tab share, in rem: the canvas column of a
/// laptop window. The stage height is the tallest that fits every card
/// of the tab in that row.
const ROW_WIDTH_REM: f64 = 84.0;
/// The gap between two cards in a row, in rem. Matches `.canvas-grid`.
const CARD_GAP_REM: f64 = 1.0;
/// The room the bezel padding and the stage padding take above and
/// below a phone frame, in rem.
const BEZEL_INSET_REM: f64 = 1.5;

/// One card on the canvas: a design, a deck, a document, or a social
/// candidate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CanvasCard {
    /// The design, deck, document, or social id.
    pub id: String,
    /// The artifact's own title, as the model named it. The footer
    /// shows it after the candidate number.
    pub title: String,
    /// Which store the card comes from.
    pub kind: ArtifactKind,
    /// How many screens, slides, pages, or frames are written.
    pub count: usize,
    /// How many titles the outline plans. 0 without an outline.
    pub outline_count: usize,
    /// The CSS aspect-ratio of the preview frame.
    pub ratio: String,
    /// The canvas the artifact was written for. A demo run writes one
    /// design per canvas, and the tabs group by this.
    pub viewport: Viewport,
}

impl CanvasCard {
    /// True when the artifact is a preview that waits for more units.
    pub fn is_preview(&self) -> bool {
        self.outline_count > self.count
    }

    /// How many planned units are not written yet.
    pub fn remaining_count(&self) -> usize {
        self.outline_count.saturating_sub(self.count)
    }

    /// The render URL of unit `current` (1-based) at `revision`.
    pub fn preview_url(&self, revision: u64, current: usize) -> String {
        match self.kind {
            ArtifactKind::Demo => {
                format!("/designs/{}/render?v={revision}&screen={current}", self.id)
            }
            ArtifactKind::Deck => {
                format!("/decks/{}/render?v={revision}&slide={current}", self.id)
            }
            ArtifactKind::Document => {
                format!("/documents/{}/render?v={revision}&page={current}", self.id)
            }
            ArtifactKind::Social => {
                format!("/socials/{}/render?v={revision}&frame={current}", self.id)
            }
            ArtifactKind::Print => {
                format!("/prints/{}/render?v={revision}&sheet={current}", self.id)
            }
            ArtifactKind::Mailing => {
                format!("/mailings/{}/render?v={revision}&email={current}", self.id)
            }
            ArtifactKind::Campaign => {
                format!("/campaigns/{}/render?v={revision}&ad={current}", self.id)
            }
            ArtifactKind::Artwork => {
                format!("/artworks/{}/render?v={revision}&cover={current}", self.id)
            }
        }
    }

    /// True when the preview sits in a phone bezel: a demo on a
    /// portrait canvas. A document page and a portrait social frame
    /// are portrait too, but neither is a phone.
    pub fn is_bezelled(&self) -> bool {
        self.kind == ArtifactKind::Demo && is_portrait_canvas(self.viewport)
    }
}

/// The design cards that belong to `session_id`, chosen design first.
pub(crate) fn cards_from_designs(
    designs: &[api::DesignSummary],
    session_id: &str,
    chosen: Option<&str>,
) -> Vec<CanvasCard> {
    let mut mine: Vec<CanvasCard> = designs
        .iter()
        .filter(|summary| artifact_project(&summary.id) == session_id)
        .map(|summary| CanvasCard {
            id: summary.id.clone(),
            title: summary.title.clone(),
            kind: ArtifactKind::Demo,
            count: summary.screen_count,
            outline_count: summary.outline_count,
            ratio: summary.aspect_ratio(),
            viewport: summary.viewport,
        })
        .collect();
    mine.sort_by_key(|card| Some(card.id.as_str()) != chosen);
    mine
}

/// The deck cards that belong to `session_id`, chosen deck first.
pub(crate) fn cards_from_decks(
    decks: &[api::DeckSummary],
    session_id: &str,
    chosen: Option<&str>,
) -> Vec<CanvasCard> {
    let mut mine: Vec<CanvasCard> = decks
        .iter()
        .filter(|summary| artifact_project(&summary.id) == session_id)
        .map(|summary| CanvasCard {
            id: summary.id.clone(),
            title: summary.title.clone(),
            kind: ArtifactKind::Deck,
            count: summary.slide_count,
            outline_count: summary.outline_count,
            ratio: summary.aspect_ratio(),
            viewport: DECK_VIEWPORT,
        })
        .collect();
    mine.sort_by_key(|card| Some(card.id.as_str()) != chosen);
    mine
}

/// The document cards that belong to `session_id`, chosen document
/// first.
pub(crate) fn cards_from_documents(
    documents: &[api::DocumentSummary],
    session_id: &str,
    chosen: Option<&str>,
) -> Vec<CanvasCard> {
    let mut mine: Vec<CanvasCard> = documents
        .iter()
        .filter(|summary| artifact_project(&summary.id) == session_id)
        .map(|summary| CanvasCard {
            id: summary.id.clone(),
            title: summary.title.clone(),
            kind: ArtifactKind::Document,
            count: summary.page_count,
            outline_count: summary.outline_count,
            ratio: summary.aspect_ratio(),
            viewport: summary.viewport(),
        })
        .collect();
    mine.sort_by_key(|card| Some(card.id.as_str()) != chosen);
    mine
}

/// The social cards that belong to `session_id`, chosen social first.
pub(crate) fn cards_from_socials(
    socials: &[api::SocialSummary],
    session_id: &str,
    chosen: Option<&str>,
) -> Vec<CanvasCard> {
    let mut mine: Vec<CanvasCard> = socials
        .iter()
        .filter(|summary| artifact_project(&summary.id) == session_id)
        .map(|summary| CanvasCard {
            id: summary.id.clone(),
            title: summary.title.clone(),
            kind: ArtifactKind::Social,
            count: summary.frame_count,
            outline_count: summary.outline_count,
            ratio: summary.aspect_ratio(),
            viewport: summary.viewport(),
        })
        .collect();
    mine.sort_by_key(|card| Some(card.id.as_str()) != chosen);
    mine
}

/// The print cards that belong to `session_id`, chosen print first.
pub(crate) fn cards_from_prints(
    prints: &[api::PrintSummary],
    session_id: &str,
    chosen: Option<&str>,
) -> Vec<CanvasCard> {
    let mut mine: Vec<CanvasCard> = prints
        .iter()
        .filter(|summary| artifact_project(&summary.id) == session_id)
        .map(|summary| CanvasCard {
            id: summary.id.clone(),
            title: summary.title.clone(),
            kind: ArtifactKind::Print,
            count: summary.sheet_count,
            outline_count: summary.outline_count,
            ratio: summary.aspect_ratio(),
            viewport: summary.viewport(),
        })
        .collect();
    mine.sort_by_key(|card| Some(card.id.as_str()) != chosen);
    mine
}

/// The mailing cards that belong to `session_id`, chosen mailing
/// first.
pub(crate) fn cards_from_mailings(
    mailings: &[api::MailingSummary],
    session_id: &str,
    chosen: Option<&str>,
) -> Vec<CanvasCard> {
    let mut mine: Vec<CanvasCard> = mailings
        .iter()
        .filter(|summary| artifact_project(&summary.id) == session_id)
        .map(|summary| CanvasCard {
            id: summary.id.clone(),
            title: summary.title.clone(),
            kind: ArtifactKind::Mailing,
            count: summary.email_count,
            outline_count: summary.outline_count,
            ratio: summary.aspect_ratio(),
            viewport: summary.viewport(),
        })
        .collect();
    mine.sort_by_key(|card| Some(card.id.as_str()) != chosen);
    mine
}

/// The campaign cards that belong to `session_id`, chosen campaign
/// first.
pub(crate) fn cards_from_campaigns(
    campaigns: &[api::CampaignSummary],
    session_id: &str,
    chosen: Option<&str>,
) -> Vec<CanvasCard> {
    let mut mine: Vec<CanvasCard> = campaigns
        .iter()
        .filter(|summary| artifact_project(&summary.id) == session_id)
        .map(|summary| CanvasCard {
            id: summary.id.clone(),
            title: summary.title.clone(),
            kind: ArtifactKind::Campaign,
            count: summary.ad_count,
            outline_count: summary.outline_count,
            ratio: summary.aspect_ratio(),
            viewport: summary.viewport(),
        })
        .collect();
    mine.sort_by_key(|card| Some(card.id.as_str()) != chosen);
    mine
}

/// The artwork cards that belong to `session_id`, chosen artwork
/// first.
pub(crate) fn cards_from_artworks(
    artworks: &[api::ArtworkSummary],
    session_id: &str,
    chosen: Option<&str>,
) -> Vec<CanvasCard> {
    let mut mine: Vec<CanvasCard> = artworks
        .iter()
        .filter(|summary| artifact_project(&summary.id) == session_id)
        .map(|summary| CanvasCard {
            id: summary.id.clone(),
            title: summary.title.clone(),
            kind: ArtifactKind::Artwork,
            count: summary.cover_count,
            outline_count: summary.outline_count,
            ratio: summary.aspect_ratio(),
            viewport: summary.viewport(),
        })
        .collect();
    mine.sort_by_key(|card| Some(card.id.as_str()) != chosen);
    mine
}

/// The card name from its id: `Candidate 2` from `talk-candidate-2`, or
/// `Candidate` when the id has no number.
pub(crate) fn candidate_label(id: &str) -> String {
    let number = id
        .rsplit("-candidate-")
        .next()
        .filter(|tail| tail.chars().all(|character| character.is_ascii_digit()))
        .unwrap_or("");
    if number.is_empty() {
        return "Candidate".to_owned();
    }
    format!("Candidate {number}")
}

/// The candidate number from its id: `2` from `talk-candidate-2`, or
/// empty when the id has no number.
pub(crate) fn candidate_number(id: &str) -> &str {
    id.rsplit("-candidate-")
        .next()
        .filter(|tail| tail.chars().all(|character| character.is_ascii_digit()))
        .unwrap_or("")
}

/// The name of one canvas, for a tab: `Desktop`, `Tablet`, `Phone`,
/// `Deck`, `A4`, `Letter`, a social format such as `Square`, a print
/// size such as `A3` or `A4 landscape`, an email format such as
/// `Email` or `Long email`, an ad size such as `Leaderboard`, or a
/// cover size such as `Thumbnail`.
pub(crate) fn canvas_name(viewport: Viewport) -> &'static str {
    if viewport == A4_VIEWPORT {
        return "A4";
    }
    if viewport == LETTER_VIEWPORT {
        return "Letter";
    }
    if let Some(format) = Format::ALL
        .into_iter()
        .find(|format| format.viewport() == viewport)
    {
        return format.label();
    }
    // Print A4 and Letter portrait matched the document papers above,
    // so only the other sizes and the landscape rotations land here.
    // `canvas_name` returns a static str, so the rotated names are
    // literal arms.
    if let Some(size) = PrintSize::ALL
        .into_iter()
        .find(|size| size.viewport() == viewport)
    {
        return size.label();
    }
    if let Some(size) = PrintSize::ALL
        .into_iter()
        .find(|size| Orientation::Landscape.apply(size.viewport()) == viewport)
    {
        return match size {
            PrintSize::A5 => "A5 landscape",
            PrintSize::A4 => "A4 landscape",
            PrintSize::A3 => "A3 landscape",
            PrintSize::Letter => "Letter landscape",
            PrintSize::Tabloid => "Tabloid landscape",
        };
    }
    // Every email format is 600 px wide; no other canvas is.
    if let Some(format) = EmailFormat::ALL
        .into_iter()
        .find(|format| format.viewport() == viewport)
    {
        return match format {
            EmailFormat::Short => "Short email",
            EmailFormat::Standard => "Email",
            EmailFormat::Long => "Long email",
        };
    }
    // The IAB ad units share no size with any canvas above.
    if let Some(size) = AdSize::ALL
        .into_iter()
        .find(|size| size.viewport() == viewport)
    {
        return size.label();
    }
    // The cover-art units share no size with any canvas above either.
    if let Some(size) = CoverSize::ALL
        .into_iter()
        .find(|size| size.viewport() == viewport)
    {
        return size.label();
    }
    match (viewport.width, viewport.height) {
        (390, 844) => "Phone",
        (1024, 768) => "Tablet",
        (1920, 1080) => "Deck",
        _ => "Desktop",
    }
}

/// The canvas size, for a tab tooltip: `1440 × 900`.
pub(crate) fn canvas_size(viewport: Viewport) -> String {
    format!("{} × {}", viewport.width, viewport.height)
}

/// True for a canvas narrower than 16:10, like a phone or a tablet. The
/// main editor preview limits such a canvas by height, not by width.
pub(crate) fn is_narrow_canvas(viewport: Viewport) -> bool {
    viewport.width * 10 < viewport.height * 16
}

/// True for a canvas taller than wide, like a phone. Only a portrait
/// canvas gets the device bezel: a tablet is narrow but not portrait.
pub(crate) fn is_portrait_canvas(viewport: Viewport) -> bool {
    viewport.width < viewport.height
}

/// The width in rem of a frame `height_rem` tall that keeps the ratio
/// of `viewport`, with two decimals: `21.60`.
pub(crate) fn frame_width_rem(viewport: Viewport, height_rem: f64) -> String {
    let width = height_rem * f64::from(viewport.width) / f64::from(viewport.height);
    format!("{width:.2}")
}

/// The stage height in rem for `count` cards of `viewport` on one tab:
/// the tallest that lets them share one row of `ROW_WIDTH_REM`, kept
/// between the floor and the cap. A portrait canvas is narrow, so it
/// stays tall for more cards than a desktop canvas does.
pub(crate) fn stage_height_rem(viewport: Viewport, count: usize) -> f64 {
    let count = count.max(1) as f64;
    let width_each = (ROW_WIDTH_REM - CARD_GAP_REM * (count - 1.0)) / count;
    let ratio = f64::from(viewport.width) / f64::from(viewport.height);
    // The floor keeps a card readable, but a very wide canvas floored
    // to it grows past the whole row: a 728 by 90 leaderboard at 13.5
    // rem is 109 rem wide. Drop the floor when even one floored card
    // cannot fit the row.
    let floor = if CARD_STAGE_HEIGHT_REM * ratio <= ROW_WIDTH_REM {
        CARD_STAGE_HEIGHT_REM
    } else {
        0.0
    };
    (width_each / ratio).clamp(floor, STAGE_HEIGHT_CAP_REM)
}

/// The sizes one card sets as CSS variables, in rem: the frame width at
/// the canvas ratio, the stage height, and the bezel height a phone
/// card uses.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CardFrame {
    /// The frame width: the bezel frame for a portrait canvas, else the
    /// full stage height at the canvas ratio.
    pub(crate) width: String,
    /// The stage height.
    pub(crate) stage_height: String,
    /// The bezel height, `BEZEL_INSET_REM` under the stage.
    pub(crate) bezel_height: String,
}

impl CardFrame {
    /// The `style` attribute of the card.
    pub(crate) fn style(&self) -> String {
        format!(
            "--frame-width: {}rem; --stage-height: {}rem; --bezel-height: {}rem",
            self.width, self.stage_height, self.bezel_height
        )
    }
}

/// The frame of a card on a tab that holds `count` cards of `viewport`.
/// A bezelled card follows the bezel, which sits inside the stage.
pub(crate) fn card_frame(viewport: Viewport, count: usize, is_bezelled: bool) -> CardFrame {
    let stage = stage_height_rem(viewport, count);
    let bezel = stage - BEZEL_INSET_REM;
    let height = match is_bezelled {
        true => bezel,
        false => stage,
    };
    CardFrame {
        width: frame_width_rem(viewport, height),
        stage_height: format!("{stage:.2}"),
        bezel_height: format!("{bezel:.2}"),
    }
}

/// How many cards were written for `viewport`, for the tab count.
pub(crate) fn cards_on_canvas(cards: &[CanvasCard], viewport: Viewport) -> usize {
    cards
        .iter()
        .filter(|card| card.viewport == viewport)
        .count()
}

/// The canvases the cards were written for, in first-seen order.
pub(crate) fn canvas_tabs(cards: &[CanvasCard]) -> Vec<Viewport> {
    let mut tabs: Vec<Viewport> = Vec::new();
    for card in cards {
        if !tabs.contains(&card.viewport) {
            tabs.push(card.viewport);
        }
    }
    tabs
}

/// What decides a card's class list.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct CardFlags {
    /// The user picked this card.
    pub is_chosen: bool,
    /// The canvas is portrait, so the frame sits in a bezel.
    pub is_phone: bool,
    /// The card stands in for an artifact the run has not saved yet.
    pub is_placeholder: bool,
    /// The user ticked this card for a bulk action.
    pub is_selected: bool,
}

/// The class list of a card: `canvas-card` plus one word per flag.
pub(crate) fn card_class(flags: CardFlags) -> String {
    let mut class = String::from("canvas-card");
    if flags.is_chosen {
        class.push_str(" chosen");
    }
    if flags.is_phone {
        class.push_str(" phone");
    }
    if flags.is_placeholder {
        class.push_str(" placeholder");
    }
    if flags.is_selected {
        class.push_str(" selected");
    }
    class
}

/// The ids whose Finish the running turn still has to serve: the
/// trailing continue requests, as the server reads them. The walk stops
/// at the first user message that is not a continue. With no run in
/// flight the list is empty, so a failed run gives the button back.
pub(crate) fn queued_finishes(messages: &[api::ChatMessage], is_running: bool) -> HashSet<String> {
    let mut ids = HashSet::new();
    if !is_running {
        return ids;
    }
    for message in messages.iter().rev() {
        if message.role != "user" || message.is_queued {
            continue;
        }
        if !message.is_continue {
            break;
        }
        if let Some(id) = &message.design {
            ids.insert(id.clone());
        }
    }
    ids
}
