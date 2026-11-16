use super::*;

/// The href of the HTML or PDF export: the whole artwork, or one
/// zero-based cover through the export route's `?cover=N` query.
pub(super) fn export_href(artwork_id: &str, extension: &str, only: Option<usize>) -> String {
    match only {
        Some(index) => format!(
            "/artworks/{artwork_id}/export{extension}?cover={}",
            index + 1
        ),
        None => format!("/artworks/{artwork_id}/export{extension}"),
    }
}

/// The href of the PNG export: one cover's image when scoped, the zip
/// of every cover otherwise.
pub(super) fn png_export_href(artwork_id: &str, only: Option<usize>) -> String {
    match only {
        Some(index) => format!("/artworks/{artwork_id}/covers/{}.png", index + 1),
        None => format!("/artworks/{artwork_id}/export.zip"),
    }
}

/// The download name of a scoped export file, matching the zip entry.
pub(super) fn cover_download_name(artwork_id: &str, index: usize, extension: &str) -> String {
    format!("{artwork_id}-cover-{}.{extension}", index + 1)
}

/// The artwork toolbar's export menus: `All` downloads the whole
/// artwork in one format, and with more than one cover a second menu
/// downloads the cover on screen. Every action is one shot, with no
/// mode to forget.
#[component]
pub(super) fn ArtworkExportGroup(
    artwork_id: String,
    selected: usize,
    cover_count: usize,
    can_export_with_chrome: bool,
) -> Element {
    let mut is_all_menu_open = use_signal(|| false);
    let mut is_unit_menu_open = use_signal(|| false);
    let number = selected + 1;
    rsx! {
        div { class: "unit-export",
            button {
                title: "Download the whole artwork",
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
                        href: export_href(&artwork_id, "", None),
                        onclick: move |_| is_all_menu_open.set(false),
                        "Download as HTML"
                    }
                    if can_export_with_chrome {
                        a {
                            href: export_href(&artwork_id, ".pdf", None),
                            onclick: move |_| is_all_menu_open.set(false),
                            "Download as PDF"
                        }
                        a {
                            href: png_export_href(&artwork_id, None),
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
        if cover_count > 1 {
            div { class: "unit-export",
                button {
                    title: "Download only the cover on screen",
                    onclick: move |_| is_unit_menu_open.set(!is_unit_menu_open()),
                    span { dangerous_inner_html: icons::DOWNLOAD }
                    "Cover {number}"
                }
                if is_unit_menu_open() {
                    div {
                        class: "menu-backdrop",
                        onclick: move |_| is_unit_menu_open.set(false),
                    }
                    div { class: "toolbar-menu unit-export-menu",
                        a {
                            href: export_href(&artwork_id, "", Some(selected)),
                            onclick: move |_| is_unit_menu_open.set(false),
                            "Download as HTML"
                        }
                        if can_export_with_chrome {
                            a {
                                href: export_href(&artwork_id, ".pdf", Some(selected)),
                                onclick: move |_| is_unit_menu_open.set(false),
                                "Download as PDF"
                            }
                            a {
                                href: png_export_href(&artwork_id, Some(selected)),
                                download: cover_download_name(&artwork_id, selected, "png"),
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

/// The stage class for a cover of `viewport`: a narrow canvas is
/// limited by height, a wide one fills the width. No size gets the
/// phone bezel.
pub(super) fn preview_stage_class(viewport: design_model::Viewport) -> &'static str {
    if is_narrow_canvas(viewport) {
        return "preview-stage narrow";
    }
    "preview-stage"
}

/// The strip tile width for a cover of `viewport`, in rem. The tile is
/// `STRIP_TILE_HEIGHT_REM` tall at the canvas ratio, but a wide unit
/// such as the 1500 by 500 header would grow past the strip, so the
/// width is capped and the tile shrinks below the shared height.
pub(super) fn strip_tile_width(viewport: design_model::Viewport) -> String {
    let ratio = f64::from(viewport.width) / f64::from(viewport.height);
    let height = STRIP_TILE_HEIGHT_REM.min(TILE_WIDTH_CAP_REM / ratio);
    frame_width_rem(viewport, height)
}

/// Widest a strip tile may be, in rem.
const TILE_WIDTH_CAP_REM: f64 = 18.0;

/// Removes cover `index`, unless it is the last cover: an artwork keeps
/// at least one cover.
pub(super) fn remove_cover(artwork: &mut Artwork, index: usize) {
    if artwork.covers.len() > 1 && index < artwork.covers.len() {
        artwork.covers.remove(index);
    }
}

/// The size option text: the name and the px canvas.
pub(super) fn size_option_label(size: CoverSize) -> String {
    let viewport = size.viewport();
    format!(
        "{} · {} × {}",
        size.label(),
        viewport.width,
        viewport.height
    )
}

/// Short label for a thumbnail: position, then the first heading text,
/// else the first words of the cover, else `Cover N`.
pub(super) fn cover_label(index: usize, cover: &Cover) -> String {
    fragment_label("Cover", index, &cover.html)
}

/// The planned title of outline entry `index`, as the thumbnail tooltip:
/// `5. Title`, or `5. Cover 5` when the outline has no entry there.
pub(super) fn outline_title(artwork: &Artwork, index: usize) -> String {
    match artwork.outline.get(index) {
        Some(title) if !title.trim().is_empty() => format!("{}. {}", index + 1, title.trim()),
        _ => format!("{}. Cover {}", index + 1, index + 1),
    }
}

/// A sample cover for tests: a heading and a paragraph.
#[cfg(test)]
pub(super) fn default_cover() -> Cover {
    Cover {
        html: "<div class='body'><h2>New cover</h2><p>Text</p></div>".to_owned(),
        css: Some(
            ".body { padding: 72px; height: 100%; display: flex; flex-direction: column; gap: 16px; } h2 { font-size: 48px; }"
                .to_owned(),
        ),
        notes: None,
    }
}

/// Number of set leaf fields in the artwork. Matches the server's
/// provenance paths: absent optional fields are not counted.
pub(super) fn field_count(artwork: &Artwork) -> usize {
    // Artwork title, size, theme name, four colors, three fonts.
    let mut count = 10;
    for cover in &artwork.covers {
        count += 1 + usize::from(cover.css.is_some()) + usize::from(cover.notes.is_some());
    }
    count
}

#[cfg(test)]
mod tests {
    use design_model::{Artwork, Cover, CoverSize, FontSet, Palette, Theme};

    use super::{
        ArtworkPreviewMode, cover_download_name, cover_label, default_cover, export_href,
        field_count, outline_title, png_export_href, preview_stage_class, remove_cover,
        size_option_label, strip_tile_width,
    };

    #[test]
    fn a_scoped_export_names_the_cover_in_href_and_filename() {
        assert_eq!(export_href("launch", "", None), "/artworks/launch/export");
        assert_eq!(
            export_href("launch", ".pdf", Some(1)),
            "/artworks/launch/export.pdf?cover=2"
        );
        assert_eq!(
            png_export_href("launch", None),
            "/artworks/launch/export.zip"
        );
        assert_eq!(
            png_export_href("launch", Some(1)),
            "/artworks/launch/covers/2.png"
        );
        assert_eq!(
            cover_download_name("launch", 1, "png"),
            "launch-cover-2.png"
        );
    }

    #[test]
    fn a_artwork_opens_in_read_mode_and_edit_loads_the_editing_script() {
        assert_eq!(ArtworkPreviewMode::ALL[0], ArtworkPreviewMode::Read);
        assert_eq!(
            ArtworkPreviewMode::ALL.map(ArtworkPreviewMode::label),
            ["Read", "Edit"]
        );
        assert_eq!(ArtworkPreviewMode::Read.render_query(), "");
        assert_eq!(ArtworkPreviewMode::Edit.render_query(), "&editable=true");
        assert!(ArtworkPreviewMode::Read.title().contains("reader"));
        assert!(ArtworkPreviewMode::Edit.title().contains("select"));
    }

    fn artwork() -> Artwork {
        Artwork {
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
            size: CoverSize::Thumbnail,
            covers: vec![
                Cover {
                    html: "<h1>Swift Design</h1>".to_owned(),
                    css: None,
                    notes: Some("Title: Swift Design tour.".to_owned()),
                },
                default_cover(),
            ],
            outline: Vec::new(),
        }
    }

    #[test]
    fn cover_labels_use_the_heading_then_the_text_then_a_number() {
        assert_eq!(cover_label(0, &default_cover()), "1. New cover");
        let artwork = artwork();
        assert_eq!(cover_label(0, &artwork.covers[0]), "1. Swift Design");
        let empty = Cover {
            html: "<div></div>".to_owned(),
            css: None,
            notes: None,
        };
        assert_eq!(cover_label(1, &empty), "2. Cover 2");
    }

    #[test]
    fn outline_titles_fall_back_to_the_cover_number() {
        let mut planned = artwork();
        planned.outline = vec!["Front".to_owned(), "  ".to_owned()];
        assert_eq!(outline_title(&planned, 0), "1. Front");
        assert_eq!(outline_title(&planned, 1), "2. Cover 2");
        assert_eq!(outline_title(&planned, 5), "6. Cover 6");
    }

    #[test]
    fn the_default_cover_validates_inside_a_artwork() {
        let artwork = artwork();
        assert_eq!(artwork.validate(), Vec::new());
        // 10 artwork and theme fields, html and notes on the first
        // cover, html and css on the second.
        assert_eq!(field_count(&artwork), 10 + 2 + 2);
    }

    #[test]
    fn a_artwork_keeps_its_last_cover() {
        let mut artwork = artwork();
        remove_cover(&mut artwork, 5);
        assert_eq!(artwork.covers.len(), 2);
        remove_cover(&mut artwork, 0);
        assert_eq!(artwork.covers.len(), 1);
        assert_eq!(artwork.covers[0].html, default_cover().html);
        remove_cover(&mut artwork, 0);
        assert_eq!(artwork.covers.len(), 1);
    }

    #[test]
    fn size_options_name_the_size_and_its_canvas() {
        assert_eq!(
            size_option_label(CoverSize::Thumbnail),
            "Thumbnail · 1280 × 720"
        );
        assert_eq!(
            size_option_label(CoverSize::Banner),
            "Channel banner · 2560 × 1440"
        );
        assert_eq!(
            size_option_label(CoverSize::Book),
            "Book cover · 1600 × 2560"
        );
    }

    #[test]
    fn tall_units_are_limited_by_height_and_wide_units_by_width() {
        assert_eq!(
            preview_stage_class(CoverSize::Album.viewport()),
            "preview-stage narrow"
        );
        assert_eq!(
            preview_stage_class(CoverSize::Book.viewport()),
            "preview-stage narrow"
        );
        assert_eq!(
            preview_stage_class(CoverSize::Thumbnail.viewport()),
            "preview-stage"
        );
        assert_eq!(
            preview_stage_class(CoverSize::Banner.viewport()),
            "preview-stage"
        );
        assert_eq!(
            preview_stage_class(CoverSize::Header.viewport()),
            "preview-stage"
        );
    }

    #[test]
    fn wide_units_get_a_capped_strip_tile() {
        // A thumbnail keeps the shared tile height.
        assert_eq!(strip_tile_width(CoverSize::Thumbnail.viewport()), "9.78");
        // The 3:1 header is the widest unit and stays under the cap.
        assert_eq!(strip_tile_width(CoverSize::Header.viewport()), "16.50");
        assert_eq!(strip_tile_width(CoverSize::Book.viewport()), "3.44");
    }
}
