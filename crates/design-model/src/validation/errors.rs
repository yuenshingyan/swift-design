/// A single problem found in a design, a deck, a document, a social,
/// a print, a mailing, a campaign, or an artwork.
///
/// Messages address the agent that wrote the artifact: they state what
/// is wrong and how to fix it.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ValidationError {
    /// The design `title` field is empty.
    #[error("design title is empty: set a non-empty `title`")]
    EmptyDesignTitle,
    /// The design has no screens.
    #[error("design has no screens: add at least one entry to `screens`")]
    NoScreens,
    /// The deck `title` field is empty.
    #[error("deck title is empty: set a non-empty `title`")]
    EmptyDeckTitle,
    /// The deck has no slides.
    #[error("deck has no slides: add at least one entry to `slides`")]
    NoSlides,
    /// The document `title` field is empty.
    #[error("document title is empty: set a non-empty `title`")]
    EmptyDocumentTitle,
    /// The document has no pages.
    #[error("document has no pages: add at least one entry to `pages`")]
    NoPages,
    /// The social `title` field is empty.
    #[error("social title is empty: set a non-empty `title`")]
    EmptySocialTitle,
    /// The social has no frames.
    #[error("social has no frames: add at least one entry to `frames`")]
    NoFrames,
    /// The print `title` field is empty.
    #[error("print title is empty: set a non-empty `title`")]
    EmptyPrintTitle,
    /// The print has no sheets.
    #[error("print has no sheets: add at least one entry to `sheets`")]
    NoSheets,
    /// The print has more sheets than the limit.
    #[error("print has {count} sheets: use at most {limit}")]
    TooManySheets {
        /// The rejected sheet count.
        count: usize,
        /// The allowed maximum.
        limit: usize,
    },
    /// The mailing `title` field is empty.
    #[error("mailing title is empty: set a non-empty `title`")]
    EmptyMailingTitle,
    /// The mailing has no emails.
    #[error("mailing has no emails: add at least one entry to `emails`")]
    NoEmails,
    /// The mailing has more emails than the limit.
    #[error("mailing has {count} emails: use at most {limit}")]
    TooManyEmails {
        /// The rejected email count.
        count: usize,
        /// The allowed maximum.
        limit: usize,
    },
    /// The campaign `title` field is empty.
    #[error("campaign title is empty: set a non-empty `title`")]
    EmptyCampaignTitle,
    /// The campaign has no ads.
    #[error("campaign has no ads: add at least one entry to `ads`")]
    NoAds,
    /// The campaign has more ads than the limit.
    #[error("campaign has {count} ads: use at most {limit}")]
    TooManyAds {
        /// The rejected ad count.
        count: usize,
        /// The allowed maximum.
        limit: usize,
    },
    /// The artwork `title` field is empty.
    #[error("artwork title is empty: set a non-empty `title`")]
    EmptyArtworkTitle,
    /// The artwork has no covers.
    #[error("artwork has no covers: add at least one entry to `covers`")]
    NoCovers,
    /// The artwork has more covers than the limit.
    #[error("artwork has {count} covers: use at most {limit}")]
    TooManyCovers {
        /// The rejected cover count.
        count: usize,
        /// The allowed maximum.
        limit: usize,
    },
    /// A theme color is not a `#rrggbb` hex string.
    #[error("theme.colors.{field} has value `{value}`: use the form #rrggbb")]
    InvalidThemeColor {
        /// Which palette field is wrong.
        field: &'static str,
        /// The rejected value.
        value: String,
    },
    /// A screen's `html` is blank.
    #[error("screens[{index}].html is empty: write the screen as an HTML fragment")]
    EmptyScreen {
        /// Zero-based screen index.
        index: usize,
    },
    /// A slide's `html` is blank.
    #[error("slides[{index}].html is empty: write the slide as an HTML fragment")]
    EmptySlide {
        /// Zero-based slide index.
        index: usize,
    },
    /// A page's `html` is blank.
    #[error("pages[{index}].html is empty: write the page as an HTML fragment")]
    EmptyPage {
        /// Zero-based page index.
        index: usize,
    },
    /// A frame's `html` is blank.
    #[error("frames[{index}].html is empty: write the frame as an HTML fragment")]
    EmptyFrame {
        /// Zero-based frame index.
        index: usize,
    },
    /// A sheet's `html` is blank.
    #[error("sheets[{index}].html is empty: write the sheet as an HTML fragment")]
    EmptySheet {
        /// Zero-based sheet index.
        index: usize,
    },
    /// An email's `html` is blank.
    #[error("emails[{index}].html is empty: write the email as an HTML fragment")]
    EmptyEmail {
        /// Zero-based email index.
        index: usize,
    },
    /// An ad's `html` is blank.
    #[error("ads[{index}].html is empty: write the ad as an HTML fragment")]
    EmptyAd {
        /// Zero-based ad index.
        index: usize,
    },
    /// A cover's `html` is blank.
    #[error("covers[{index}].html is empty: write the cover as an HTML fragment")]
    EmptyCover {
        /// Zero-based cover index.
        index: usize,
    },
    /// A screen's or slide's `html` or `css` is longer than the limit.
    #[error("{path} has {length} characters: keep it under {limit}")]
    TooLarge {
        /// Field path like `screens[2].html` or `slides[2].html`.
        path: String,
        /// The actual length in characters.
        length: usize,
        /// The allowed maximum.
        limit: usize,
    },
    /// A forbidden or malformed construct in a screen's or slide's `html`.
    #[error("{path}: {rule}")]
    InvalidHtml {
        /// Field path like `screens[2].html` or `slides[2].html`.
        path: String,
        /// What is wrong and how to fix it.
        rule: String,
    },
    /// The transition lasts longer than `MAX_TRANSITION_MS`.
    #[error("transition.duration_ms is {duration_ms}: use 0 to {limit}")]
    TransitionTooLong {
        /// The rejected duration in milliseconds.
        duration_ms: u32,
        /// The allowed maximum in milliseconds.
        limit: u32,
    },
    /// A forbidden or malformed construct in a screen's or slide's `css`.
    #[error("{path}: {rule}")]
    InvalidCss {
        /// Field path like `screens[2].css` or `slides[2].css`.
        path: String,
        /// What is wrong and how to fix it.
        rule: String,
    },
    /// A viewport side is outside the allowed range.
    #[error("viewport is {width} by {height}: use {min} to {max} px for each side")]
    InvalidViewport {
        /// The rejected width in px.
        width: u32,
        /// The rejected height in px.
        height: u32,
        /// The shortest allowed side in px.
        min: u32,
        /// The longest allowed side in px.
        max: u32,
    },
    /// The app window has zero width or height.
    #[error("app_window is {width} by {height}: set positive width and height")]
    InvalidAppWindowDimensions {
        /// The rejected width in px.
        width: u32,
        /// The rejected height in px.
        height: u32,
    },
    /// The app window does not fit inside the viewport.
    #[error(
        "app_window at {left},{top} size {width} by {height} does not fit viewport {viewport_width} by {viewport_height}: keep the window inside the viewport"
    )]
    AppWindowOutOfBounds {
        /// Distance from the viewport left edge in px.
        left: u32,
        /// Distance from the viewport top edge in px.
        top: u32,
        /// Window width in px.
        width: u32,
        /// Window height in px.
        height: u32,
        /// Viewport width in px.
        viewport_width: u32,
        /// Viewport height in px.
        viewport_height: u32,
    },
    /// A screen has no top-level app window marker.
    #[error(
        "screens[{index}].html has no top-level data-swift-design-window element: mark exactly one top-level non-void element"
    )]
    MissingAppWindowMarker {
        /// Zero-based screen index.
        index: usize,
    },
    /// A screen has more than one top-level app window marker.
    #[error(
        "screens[{index}].html has {count} top-level data-swift-design-window elements: keep exactly one"
    )]
    DuplicateAppWindowMarker {
        /// Zero-based screen index.
        index: usize,
        /// Number of top-level markers found.
        count: usize,
    },
    /// A screen has an app window marker below the top level.
    #[error(
        "screens[{index}].html has {count} nested data-swift-design-window elements: move the marker to one top-level non-void element"
    )]
    NestedAppWindowMarker {
        /// Zero-based screen index.
        index: usize,
        /// Number of nested markers found.
        count: usize,
    },
}
