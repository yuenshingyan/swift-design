use super::*;

/// Moves a unit while preserving navigation destinations and preview outline order.
pub(crate) fn move_screen<T: LinkedUnit>(
    screens: &mut Vec<T>,
    outline: &mut Vec<String>,
    from: usize,
    to: usize,
) {
    reorder_units(screens, outline, from, to);
}

/// Short label for one screen or slide of html: position, then the first
/// heading text, else the first words, else `{unit} N`. `unit` is
/// `Screen` or `Slide`.
pub(crate) fn fragment_label(unit: &str, index: usize, html: &str) -> String {
    let heading = first_heading(html).map(|text| strip_tags(&text));
    let text = match heading {
        Some(text) if !text.is_empty() => text,
        _ => strip_tags(html).chars().take(40).collect(),
    };
    if text.is_empty() {
        format!("{}. {unit} {}", index + 1, index + 1)
    } else {
        format!("{}. {text}", index + 1)
    }
}

/// The height in rem of every tile in the thumbnail strip.
pub(crate) const STRIP_TILE_HEIGHT_REM: f64 = 5.5;

/// The number tag and the planned title of outline entry `index`:
/// `("07", "Task detail")`, or `("07", "Screen 7")` when the outline has
/// no title there. `unit` is `Screen` or `Slide`.
pub(crate) fn outline_entry(outline: &[String], index: usize, unit: &str) -> (String, String) {
    let number = format!("{:02}", index + 1);
    let title = match outline.get(index).map(|title| title.trim()) {
        Some(title) if !title.is_empty() => title.to_owned(),
        _ => format!("{unit} {}", index + 1),
    };
    (number, title)
}

/// The counts in the strip heading: `6 written · 2 planned`, or
/// `6 written` when nothing is planned.
pub(crate) fn strip_summary(written: usize, planned: usize) -> String {
    if planned == 0 {
        return format!("{written} written");
    }
    format!("{written} written · {planned} planned")
}

/// Waits a moment on the page, so a burst of keystrokes saves once.
const AUTOSAVE_DELAY_SCRIPT: &str = "setTimeout(() => dioxus.send(0), 700);";

/// Saves shortly after the last change. Every call bumps `generation`;
/// only the call that is still the newest when the delay ends saves,
/// so typing saves once, after the typing stops. `reload_preview` is
/// for a change the preview must redraw, like the theme.
pub(crate) fn schedule_save(
    mut generation: Signal<u64>,
    save: Callback<bool>,
    reload_preview: bool,
) {
    let mine = generation() + 1;
    generation.set(mine);
    spawn(async move {
        let mut sleeper = document::eval(AUTOSAVE_DELAY_SCRIPT);
        let _ = sleeper.recv::<i32>().await;
        if generation() == mine {
            save.call(reload_preview);
        }
    });
}

/// The page reference the chat carries: the pinned pages, in order, or
/// `None` when nothing is pinned. The open page is never pinned by
/// itself: a change with no pins is about the whole artifact.
pub(crate) fn page_reference(unit: &str, pinned: &[usize]) -> Option<String> {
    if pinned.is_empty() {
        return None;
    }
    let mut pinned = pinned.to_vec();
    pinned.sort_unstable();
    Some(
        pinned
            .iter()
            .map(|index| format!("[{unit} {}]", index + 1))
            .collect::<Vec<String>>()
            .join(" "),
    )
}

/// Adds `index` to the pins, or removes it when it is pinned.
pub(crate) fn toggle_pin(pinned: &mut Vec<usize>, index: usize) {
    match pinned.iter().position(|pin| *pin == index) {
        Some(position) => {
            pinned.remove(position);
        }
        None => pinned.push(index),
    }
}

/// What decides a written tile's class list.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ThumbnailState {
    /// The tile is the screen or slide in the main preview.
    pub is_current: bool,
    /// The user drags the tile to a new position.
    pub is_dragging: bool,
    /// The canvas is portrait, so the tile is a narrow column.
    pub is_portrait: bool,
    /// The delete button waits for its confirm click.
    pub is_deleting: bool,
    /// The tile is pinned for the chat: the next message is about it.
    pub is_pinned: bool,
}

/// The class list of a written tile: `thumbnail` plus one word per flag.
pub(crate) fn thumbnail_class(state: ThumbnailState) -> String {
    let mut class = String::from("thumbnail");
    if state.is_current {
        class.push_str(" current");
    }
    if state.is_dragging {
        class.push_str(" dragging");
    }
    if state.is_portrait {
        class.push_str(" portrait");
    }
    if state.is_deleting {
        class.push_str(" deleting");
    }
    if state.is_pinned {
        class.push_str(" pinned");
    }
    class
}

/// The screen a navigate message opens: `target` when the message names
/// one that exists, else one step from `current`, kept inside the
/// design.
pub(crate) fn navigation_target(
    current: usize,
    count: usize,
    step: i32,
    target: Option<usize>,
) -> usize {
    if let Some(target) = target {
        return if target < count { target } else { current };
    }
    if step > 0 {
        (current + 1).min(count.saturating_sub(1))
    } else {
        current.saturating_sub(1)
    }
}

/// The class of the stage around the main preview. A narrow canvas is
/// limited by height, and a portrait canvas also gets the bezel.
pub(crate) fn preview_stage_class(viewport: Viewport) -> &'static str {
    if is_portrait_canvas(viewport) {
        return "preview-stage narrow phone";
    }
    if is_narrow_canvas(viewport) {
        return "preview-stage narrow";
    }
    "preview-stage"
}

/// The inner HTML of the first `<h1>`, `<h2>`, or `<h3>` in `html`.
pub(crate) fn first_heading(html: &str) -> Option<String> {
    let lowered = html.to_ascii_lowercase();
    let mut best: Option<(usize, String)> = None;
    for level in ["h1", "h2", "h3"] {
        let open = format!("<{level}");
        let Some(start) = lowered.find(&open) else {
            continue;
        };
        let Some(content_start) = lowered[start..].find('>').map(|offset| start + offset + 1)
        else {
            continue;
        };
        let close = format!("</{level}>");
        let Some(end) = lowered[content_start..]
            .find(&close)
            .map(|offset| content_start + offset)
        else {
            continue;
        };
        let candidate = html[content_start..end].to_owned();
        if best.as_ref().is_none_or(|(position, _)| start < *position) {
            best = Some((start, candidate));
        }
    }
    best.map(|(_, text)| text)
}

/// Text without HTML tags, whitespace collapsed.
pub(crate) fn strip_tags(html: &str) -> String {
    let mut text = String::new();
    let mut is_in_tag = false;
    for character in html.chars() {
        match character {
            '<' => is_in_tag = true,
            '>' => {
                is_in_tag = false;
                text.push(' ');
            }
            other if !is_in_tag => text.push(other),
            _ => {}
        }
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `None` for blank input, so cleared fields leave the design JSON.
pub(crate) fn optional(value: String) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use design_model::TransitionEffect;

    use crate::api::UploadSummary;
    use crate::editor::history::history_label;
    use crate::editor::node::{image_upload_paths, node_reference, selection_paths};
    use crate::editor::preview::{
        NodeStyles, PreviewMode, SelectedNode, SelectionEntry, pin_reference, preview_panes,
        selection_reference,
    };
    use crate::editor::theme::{MONO_FONTS, TEXT_FONTS, font_options};
    use crate::editor::transition::effect_uses_motion;

    use super::{
        ThumbnailState, navigation_target, outline_entry, page_reference, preview_stage_class,
        strip_summary, thumbnail_class, toggle_pin,
    };

    #[test]
    fn preview_panes_keep_the_loaded_document_while_the_next_loads() {
        assert_eq!(
            preview_panes("/designs/a/render", "/designs/a/render"),
            vec!["/designs/a/render"]
        );
        assert_eq!(
            preview_panes("/designs/a/render", "/designs/a/render?editable=true"),
            vec!["/designs/a/render", "/designs/a/render?editable=true"]
        );
    }

    #[test]
    fn only_image_uploads_become_picker_entries() {
        let uploads = vec![
            UploadSummary {
                name: "hero.png".to_owned(),
                size_bytes: 10,
                content_type: "image/png".to_owned(),
                is_image: true,
            },
            UploadSummary {
                name: "notes.txt".to_owned(),
                size_bytes: 10,
                content_type: "text/plain".to_owned(),
                is_image: false,
            },
        ];
        assert_eq!(image_upload_paths(&uploads), vec!["/uploads/hero.png"]);
    }

    #[test]
    fn play_mode_asks_the_render_for_no_editing_script() {
        assert_eq!(PreviewMode::Edit.render_query(), "&editable=true");
        assert_eq!(PreviewMode::Play.render_query(), "");
        assert_eq!(PreviewMode::ALL.map(PreviewMode::label), ["Play", "Edit"]);
        assert_eq!(PreviewMode::ALL[0], PreviewMode::Play);
    }

    #[test]
    fn a_screen_link_opens_its_screen_and_a_step_moves_one() {
        assert_eq!(navigation_target(0, 4, 0, Some(2)), 2);
        assert_eq!(navigation_target(1, 4, 0, Some(9)), 1);
        assert_eq!(navigation_target(1, 4, 1, None), 2);
        assert_eq!(navigation_target(3, 4, 1, None), 3);
        assert_eq!(navigation_target(0, 4, -1, None), 0);
        assert_eq!(navigation_target(0, 0, 1, None), 0);
    }

    #[test]
    fn only_moving_effects_show_direction_and_duration() {
        assert!(!effect_uses_motion(None));
        assert!(!effect_uses_motion(Some(TransitionEffect::None)));
        assert!(effect_uses_motion(Some(TransitionEffect::Fade)));
        assert!(effect_uses_motion(Some(TransitionEffect::Push)));
        assert!(effect_uses_motion(Some(TransitionEffect::Cover)));
        assert!(effect_uses_motion(Some(TransitionEffect::Zoom)));
    }

    #[test]
    fn history_labels_read_as_utc_times() {
        assert_eq!(
            history_label("2026-08-25T10:14:02Z"),
            "2026-08-25 10:14:02 UTC"
        );
        assert_eq!(history_label("odd"), "odd");
    }

    #[test]
    fn outline_entries_split_the_number_from_the_title() {
        let outline = vec!["Intro".to_owned(), "  ".to_owned()];
        assert_eq!(
            outline_entry(&outline, 0, "Screen"),
            ("01".to_owned(), "Intro".to_owned())
        );
        assert_eq!(
            outline_entry(&outline, 1, "Screen"),
            ("02".to_owned(), "Screen 2".to_owned())
        );
        assert_eq!(
            outline_entry(&outline, 6, "Slide"),
            ("07".to_owned(), "Slide 7".to_owned())
        );
    }

    #[test]
    fn the_strip_summary_counts_written_and_planned() {
        assert_eq!(strip_summary(6, 2), "6 written · 2 planned");
        assert_eq!(strip_summary(6, 0), "6 written");
    }

    #[test]
    fn a_dragged_tile_is_current_only_when_selected() {
        let dragging = ThumbnailState {
            is_dragging: true,
            ..ThumbnailState::default()
        };
        assert_eq!(thumbnail_class(dragging), "thumbnail dragging");
        let everything = ThumbnailState {
            is_current: true,
            is_dragging: true,
            is_portrait: true,
            is_deleting: true,
            is_pinned: true,
        };
        assert_eq!(
            thumbnail_class(everything),
            "thumbnail current dragging portrait deleting pinned"
        );
    }

    #[test]
    fn pinned_pages_replace_the_open_page_in_the_reference() {
        assert_eq!(page_reference("slide", &[]), None);
        assert_eq!(
            page_reference("slide", &[6, 1]).as_deref(),
            Some("[slide 2] [slide 7]")
        );
        let mut pinned = vec![1];
        toggle_pin(&mut pinned, 6);
        toggle_pin(&mut pinned, 1);
        assert_eq!(pinned, vec![6]);
    }

    #[test]
    fn the_preview_stage_knows_narrow_and_portrait_canvases() {
        let phone = design_model::Viewport {
            width: 390,
            height: 844,
        };
        let tablet = design_model::Viewport {
            width: 1024,
            height: 768,
        };
        assert_eq!(preview_stage_class(phone), "preview-stage narrow phone");
        assert_eq!(preview_stage_class(tablet), "preview-stage narrow");
        assert_eq!(
            preview_stage_class(design_model::Viewport::default()),
            "preview-stage"
        );
    }

    #[test]
    fn a_selection_reference_names_every_node() {
        let entries = vec![
            SelectionEntry {
                path: "0/1".to_owned(),
                tag: "h2".to_owned(),
                classes: "title".to_owned(),
                text: "Hello".to_owned(),
            },
            SelectionEntry {
                path: "0/2".to_owned(),
                tag: "p".to_owned(),
                ..SelectionEntry::default()
            },
        ];
        assert_eq!(
            selection_reference("slide", 3, &entries),
            "[slide 4, nodes 0/1 <h2.title>; 0/2 <p>]"
        );
        assert_eq!(
            selection_reference("slide", 3, &entries[..1]),
            "[slide 4, node 0/1 <h2.title>: Hello]"
        );
        assert_eq!(
            selection_paths(&entries, "0/2"),
            vec!["0/2".to_owned(), "0/1".to_owned()]
        );
        assert_eq!(selection_paths(&[], "0/2"), vec!["0/2".to_owned()]);
    }

    #[test]
    fn a_pin_reference_names_the_selection_or_the_bare_unit() {
        let entries = vec![SelectionEntry {
            path: "0/1".to_owned(),
            tag: "h2".to_owned(),
            classes: "title".to_owned(),
            text: "Hello".to_owned(),
        }];
        assert_eq!(
            pin_reference("slide", 3, &entries),
            "[slide 4, node 0/1 <h2.title>: Hello]"
        );
        assert_eq!(pin_reference("slide", 3, &[]), "[slide 4]");
    }

    #[test]
    fn node_references_name_screen_path_tag_and_text() {
        let node = SelectedNode {
            path: "0/1".to_owned(),
            tag: "h2".to_owned(),
            classes: "title big".to_owned(),
            text: "What Swift Design does".to_owned(),
            styles: NodeStyles::default(),
        };
        assert_eq!(
            node_reference("screen", 2, &node),
            "[screen 3, node 0/1 <h2.title>: What Swift Design does]"
        );
        let image = SelectedNode {
            path: "2".to_owned(),
            tag: "img".to_owned(),
            classes: String::new(),
            text: String::new(),
            styles: NodeStyles::default(),
        };
        assert_eq!(
            node_reference("screen", 0, &image),
            "[screen 1, node 2 <img>]"
        );
        assert_eq!(
            node_reference("slide", 0, &image),
            "[slide 1, node 2 <img>]"
        );
    }

    #[test]
    fn font_options_keep_an_unlisted_family_first() {
        let options = font_options("Playfair Display", &TEXT_FONTS);
        assert_eq!(options.len(), TEXT_FONTS.len() + 4);
        assert_eq!(options[0], "Inter");
        assert!(options.iter().any(|family| family == "system-ui"));
        let custom = font_options("Comic Neue", &MONO_FONTS);
        assert_eq!(custom[0], "Comic Neue");
        assert_eq!(custom.len(), MONO_FONTS.len() + 5);
        // Case differences do not duplicate a listed family.
        assert_eq!(font_options("inter", &TEXT_FONTS)[0], "Inter");
        assert_eq!(font_options("", &MONO_FONTS)[0], "JetBrains Mono");
    }
}
