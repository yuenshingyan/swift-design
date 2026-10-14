use super::*;

/// The href of the HTML or PDF export: the whole social, or one
/// zero-based frame through the export route's `?frame=N` query.
pub(super) fn export_href(social_id: &str, extension: &str, only: Option<usize>) -> String {
    match only {
        Some(index) => format!("/socials/{social_id}/export{extension}?frame={}", index + 1),
        None => format!("/socials/{social_id}/export{extension}"),
    }
}

/// The href of the PNG export: one frame's image when scoped, the zip
/// of every frame otherwise.
pub(super) fn png_export_href(social_id: &str, only: Option<usize>) -> String {
    match only {
        Some(index) => format!("/socials/{social_id}/frames/{}.png", index + 1),
        None => format!("/socials/{social_id}/export.zip"),
    }
}

/// The download name of a scoped export file, matching the zip entry.
pub(super) fn frame_download_name(social_id: &str, index: usize, extension: &str) -> String {
    format!("{social_id}-frame-{}.{extension}", index + 1)
}

/// The social toolbar's export menus: `All` downloads the whole
/// social in one format, and with more than one frame a second menu
/// downloads the frame on screen. Every action is one shot, with no
/// mode to forget.
#[component]
pub(super) fn SocialExportGroup(
    social_id: String,
    selected: usize,
    frame_count: usize,
    can_export_with_chrome: bool,
) -> Element {
    let mut is_all_menu_open = use_signal(|| false);
    let mut is_unit_menu_open = use_signal(|| false);
    let number = selected + 1;
    rsx! {
        div { class: "unit-export",
            button {
                title: "Download the whole social",
                onclick: move |_| is_all_menu_open.set(!is_all_menu_open()),
                span { dangerous_inner_html: icons::DOWNLOAD }
                "All"
            }
            if is_all_menu_open() {
                div {
                    class: "menu-backdrop",
                    onclick: move |_| is_all_menu_open.set(false),
                }
                div { class: "toolbar-menu unit-export-menu",
                    a {
                        href: export_href(&social_id, "", None),
                        onclick: move |_| is_all_menu_open.set(false),
                        "Download as HTML"
                    }
                    if can_export_with_chrome {
                        a {
                            href: export_href(&social_id, ".pdf", None),
                            onclick: move |_| is_all_menu_open.set(false),
                            "Download as PDF"
                        }
                        a {
                            href: png_export_href(&social_id, None),
                            onclick: move |_| is_all_menu_open.set(false),
                            "Download as PNG zip"
                        }
                    } else {
                        a {
                            "aria-disabled": "true",
                            title: "Install Chrome or Chromium on the server machine, or set SWIFT_DESIGN_CHROME",
                            "Download as PDF"
                        }
                        a {
                            "aria-disabled": "true",
                            title: "Install Chrome or Chromium on the server machine, or set SWIFT_DESIGN_CHROME",
                            "Download as PNG zip"
                        }
                    }
                }
            }
        }
        if frame_count > 1 {
            div { class: "unit-export",
                button {
                    title: "Download only the frame on screen",
                    onclick: move |_| is_unit_menu_open.set(!is_unit_menu_open()),
                    span { dangerous_inner_html: icons::DOWNLOAD }
                    "Frame {number}"
                }
                if is_unit_menu_open() {
                    div {
                        class: "menu-backdrop",
                        onclick: move |_| is_unit_menu_open.set(false),
                    }
                    div { class: "toolbar-menu unit-export-menu",
                        a {
                            href: export_href(&social_id, "", Some(selected)),
                            onclick: move |_| is_unit_menu_open.set(false),
                            "Download as HTML"
                        }
                        if can_export_with_chrome {
                            a {
                                href: export_href(&social_id, ".pdf", Some(selected)),
                                onclick: move |_| is_unit_menu_open.set(false),
                                "Download as PDF"
                            }
                            a {
                                href: png_export_href(&social_id, Some(selected)),
                                download: frame_download_name(&social_id, selected, "png"),
                                onclick: move |_| is_unit_menu_open.set(false),
                                "Download as PNG"
                            }
                        } else {
                            a {
                                "aria-disabled": "true",
                                title: "Install Chrome or Chromium on the server machine, or set SWIFT_DESIGN_CHROME",
                                "Download as PDF"
                            }
                            a {
                                "aria-disabled": "true",
                                title: "Install Chrome or Chromium on the server machine, or set SWIFT_DESIGN_CHROME",
                                "Download as PNG"
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The stage class for a frame of `viewport`: a narrow canvas is
/// limited by height, a wide one fills the width. No format gets the
/// phone bezel.
pub(super) fn preview_stage_class(viewport: design_model::Viewport) -> &'static str {
    if is_narrow_canvas(viewport) {
        return "preview-stage narrow";
    }
    "preview-stage"
}

/// Removes frame `index`, unless it is the last frame: a social keeps
/// at least one frame.
pub(super) fn remove_frame(social: &mut Social, index: usize) {
    if social.frames.len() > 1 && index < social.frames.len() {
        social.frames.remove(index);
    }
}

/// The format option text: the name and the px canvas.
pub(super) fn format_option_label(format: Format) -> String {
    let viewport = format.viewport();
    format!(
        "{} · {} × {}",
        format.label(),
        viewport.width,
        viewport.height
    )
}

/// Short label for a thumbnail: position, then the first heading text,
/// else the first words of the frame, else `Frame N`.
pub(super) fn frame_label(index: usize, frame: &Frame) -> String {
    fragment_label("Frame", index, &frame.html)
}

/// The planned title of outline entry `index`, as the thumbnail tooltip:
/// `5. Title`, or `5. Frame 5` when the outline has no entry there.
pub(super) fn outline_title(social: &Social, index: usize) -> String {
    match social.outline.get(index) {
        Some(title) if !title.trim().is_empty() => format!("{}. {}", index + 1, title.trim()),
        _ => format!("{}. Frame {}", index + 1, index + 1),
    }
}

/// A sample frame for tests: a heading and a paragraph.
#[cfg(test)]
pub(super) fn default_frame() -> Frame {
    Frame {
        html: "<div class='body'><h2>New frame</h2><p>Text</p></div>".to_owned(),
        css: Some(
            ".body { padding: 72px; height: 100%; display: flex; flex-direction: column; gap: 16px; } h2 { font-size: 48px; }"
                .to_owned(),
        ),
        notes: None,
    }
}

/// Number of set leaf fields in the social. Matches the server's
/// provenance paths: absent optional fields are not counted.
pub(super) fn field_count(social: &Social) -> usize {
    // Social title, format, theme name, four colors, three fonts.
    let mut count = 10;
    for frame in &social.frames {
        count += 1 + usize::from(frame.css.is_some()) + usize::from(frame.notes.is_some());
    }
    count
}

#[cfg(test)]
mod tests {
    use design_model::{
        FontSet, Format, Frame, LANDSCAPE_VIEWPORT, PORTRAIT_VIEWPORT, Palette, SQUARE_VIEWPORT,
        STORY_VIEWPORT, Social, Theme,
    };

    use super::{
        SocialPreviewMode, default_frame, export_href, field_count, format_option_label,
        frame_download_name, frame_label, outline_title, png_export_href, preview_stage_class,
        remove_frame,
    };

    #[test]
    fn a_scoped_export_names_the_frame_in_href_and_filename() {
        assert_eq!(export_href("launch", "", None), "/socials/launch/export");
        assert_eq!(
            export_href("launch", ".pdf", Some(1)),
            "/socials/launch/export.pdf?frame=2"
        );
        assert_eq!(
            png_export_href("launch", None),
            "/socials/launch/export.zip"
        );
        assert_eq!(
            png_export_href("launch", Some(1)),
            "/socials/launch/frames/2.png"
        );
        assert_eq!(
            frame_download_name("launch", 1, "png"),
            "launch-frame-2.png"
        );
    }

    #[test]
    fn a_social_opens_in_read_mode_and_edit_loads_the_editing_script() {
        assert_eq!(SocialPreviewMode::ALL[0], SocialPreviewMode::Read);
        assert_eq!(
            SocialPreviewMode::ALL.map(SocialPreviewMode::label),
            ["Read", "Edit"]
        );
        assert_eq!(SocialPreviewMode::Read.render_query(), "");
        assert_eq!(SocialPreviewMode::Edit.render_query(), "&editable=true");
        assert!(SocialPreviewMode::Read.title().contains("reader"));
        assert!(SocialPreviewMode::Edit.title().contains("select"));
    }

    fn social() -> Social {
        Social {
            title: "T".to_owned(),
            theme: Theme {
                name: "n".to_owned(),
                colors: Palette {
                    background: "#ffffff".to_owned(),
                    text: "#1a1d21".to_owned(),
                    accent: "#0e6e63".to_owned(),
                    muted: "#888888".to_owned(),
                },
                fonts: FontSet {
                    heading: "Inter".to_owned(),
                    body: "Inter".to_owned(),
                    mono: "Menlo".to_owned(),
                },
            },
            format: Format::Portrait,
            frames: vec![
                Frame {
                    html: "<h1>Swift Design</h1>".to_owned(),
                    css: None,
                    notes: Some("Caption: open".to_owned()),
                },
                default_frame(),
            ],
            outline: Vec::new(),
        }
    }

    #[test]
    fn frame_labels_use_the_heading_then_the_text_then_a_number() {
        assert_eq!(frame_label(0, &default_frame()), "1. New frame");
        let social = social();
        assert_eq!(frame_label(0, &social.frames[0]), "1. Swift Design");
        let empty = Frame {
            html: "<div></div>".to_owned(),
            css: None,
            notes: None,
        };
        assert_eq!(frame_label(1, &empty), "2. Frame 2");
    }

    #[test]
    fn outline_titles_fall_back_to_the_frame_number() {
        let mut planned = social();
        planned.outline = vec!["Hook".to_owned(), "  ".to_owned()];
        assert_eq!(outline_title(&planned, 0), "1. Hook");
        assert_eq!(outline_title(&planned, 1), "2. Frame 2");
        assert_eq!(outline_title(&planned, 5), "6. Frame 6");
    }

    #[test]
    fn the_default_frame_validates_inside_a_social() {
        let social = social();
        assert_eq!(social.validate(), Vec::new());
        // 10 social and theme fields, html and notes on the first
        // frame, html and css on the second.
        assert_eq!(field_count(&social), 10 + 2 + 2);
    }

    #[test]
    fn a_social_keeps_its_last_frame() {
        let mut social = social();
        remove_frame(&mut social, 5);
        assert_eq!(social.frames.len(), 2);
        remove_frame(&mut social, 0);
        assert_eq!(social.frames.len(), 1);
        assert_eq!(social.frames[0].html, default_frame().html);
        remove_frame(&mut social, 0);
        assert_eq!(social.frames.len(), 1);
    }

    #[test]
    fn format_options_name_the_format_and_its_canvas() {
        assert_eq!(format_option_label(Format::Square), "Square · 1080 × 1080");
        assert_eq!(
            format_option_label(Format::Portrait),
            "Portrait · 1080 × 1350"
        );
        assert_eq!(format_option_label(Format::Story), "Story · 1080 × 1920");
        assert_eq!(
            format_option_label(Format::Landscape),
            "Landscape · 1200 × 630"
        );
    }

    #[test]
    fn only_a_landscape_frame_fills_the_stage_width() {
        assert_eq!(preview_stage_class(SQUARE_VIEWPORT), "preview-stage narrow");
        assert_eq!(
            preview_stage_class(PORTRAIT_VIEWPORT),
            "preview-stage narrow"
        );
        assert_eq!(preview_stage_class(STORY_VIEWPORT), "preview-stage narrow");
        assert_eq!(preview_stage_class(LANDSCAPE_VIEWPORT), "preview-stage");
    }
}
