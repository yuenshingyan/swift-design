use super::*;

/// The href of the HTML or PDF export: the whole print, or one
/// zero-based sheet through the export route's `?sheet=N` query.
pub(super) fn export_href(print_id: &str, extension: &str, only: Option<usize>) -> String {
    match only {
        Some(index) => format!("/prints/{print_id}/export{extension}?sheet={}", index + 1),
        None => format!("/prints/{print_id}/export{extension}"),
    }
}

/// The href of the PNG export: one sheet's image when scoped, the zip
/// of every sheet otherwise.
pub(super) fn png_export_href(print_id: &str, only: Option<usize>) -> String {
    match only {
        Some(index) => format!("/prints/{print_id}/sheets/{}.png", index + 1),
        None => format!("/prints/{print_id}/export.zip"),
    }
}

/// The download name of a scoped export file, matching the zip entry.
pub(super) fn sheet_download_name(print_id: &str, index: usize, extension: &str) -> String {
    format!("{print_id}-sheet-{}.{extension}", index + 1)
}

/// The print toolbar's export menus: `All` downloads the whole
/// print in one format, and with more than one sheet a second menu
/// downloads the sheet on screen. Every action is one shot, with no
/// mode to forget.
#[component]
pub(super) fn PrintExportGroup(
    print_id: String,
    selected: usize,
    sheet_count: usize,
    can_export_with_chrome: bool,
) -> Element {
    let mut is_all_menu_open = use_signal(|| false);
    let mut is_unit_menu_open = use_signal(|| false);
    let number = selected + 1;
    rsx! {
        div { class: "unit-export",
            button {
                title: "Download the whole print",
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
                        href: export_href(&print_id, "", None),
                        onclick: move |_| is_all_menu_open.set(false),
                        "Download as HTML"
                    }
                    if can_export_with_chrome {
                        a {
                            href: export_href(&print_id, ".pdf", None),
                            onclick: move |_| is_all_menu_open.set(false),
                            "Download as PDF"
                        }
                        a {
                            href: png_export_href(&print_id, None),
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
        if sheet_count > 1 {
            div { class: "unit-export",
                button {
                    title: "Download only the sheet on screen",
                    onclick: move |_| is_unit_menu_open.set(!is_unit_menu_open()),
                    span { dangerous_inner_html: icons::DOWNLOAD }
                    "Sheet {number}"
                }
                if is_unit_menu_open() {
                    div {
                        class: "menu-backdrop",
                        onclick: move |_| is_unit_menu_open.set(false),
                    }
                    div { class: "toolbar-menu unit-export-menu",
                        a {
                            href: export_href(&print_id, "", Some(selected)),
                            onclick: move |_| is_unit_menu_open.set(false),
                            "Download as HTML"
                        }
                        if can_export_with_chrome {
                            a {
                                href: export_href(&print_id, ".pdf", Some(selected)),
                                onclick: move |_| is_unit_menu_open.set(false),
                                "Download as PDF"
                            }
                            a {
                                href: png_export_href(&print_id, Some(selected)),
                                download: sheet_download_name(&print_id, selected, "png"),
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

/// The stage class for a sheet of `viewport`: a narrow canvas is
/// limited by height, a wide one fills the width. No size gets the
/// phone bezel.
pub(super) fn preview_stage_class(viewport: design_model::Viewport) -> &'static str {
    if is_narrow_canvas(viewport) {
        return "preview-stage narrow";
    }
    "preview-stage"
}

/// Removes sheet `index`, unless it is the last sheet: a print keeps
/// at least one sheet.
pub(super) fn remove_sheet(print: &mut Print, index: usize) {
    if print.sheets.len() > 1 && index < print.sheets.len() {
        print.sheets.remove(index);
    }
}

/// The size option text: the name and the px canvas, portrait. The
/// orientation select rotates it.
pub(super) fn size_option_label(size: PrintSize) -> String {
    let viewport = size.viewport();
    format!(
        "{} · {} × {}",
        size.label(),
        viewport.width,
        viewport.height
    )
}

/// Short label for a thumbnail: position, then the first heading text,
/// else the first words of the sheet, else `Sheet N`.
pub(super) fn sheet_label(index: usize, sheet: &Sheet) -> String {
    fragment_label("Sheet", index, &sheet.html)
}

/// The planned title of outline entry `index`, as the thumbnail tooltip:
/// `5. Title`, or `5. Sheet 5` when the outline has no entry there.
pub(super) fn outline_title(print: &Print, index: usize) -> String {
    match print.outline.get(index) {
        Some(title) if !title.trim().is_empty() => format!("{}. {}", index + 1, title.trim()),
        _ => format!("{}. Sheet {}", index + 1, index + 1),
    }
}

/// A sample sheet for tests: a heading and a paragraph.
#[cfg(test)]
pub(super) fn default_sheet() -> Sheet {
    Sheet {
        html: "<div class='body'><h2>New sheet</h2><p>Text</p></div>".to_owned(),
        css: Some(
            ".body { padding: 72px; height: 100%; display: flex; flex-direction: column; gap: 16px; } h2 { font-size: 48px; }"
                .to_owned(),
        ),
        notes: None,
    }
}

/// Number of set leaf fields in the print. Matches the server's
/// provenance paths: absent optional fields are not counted.
pub(super) fn field_count(print: &Print) -> usize {
    // Print title, size, orientation, theme name, four colors, three
    // fonts.
    let mut count = 11;
    for sheet in &print.sheets {
        count += 1 + usize::from(sheet.css.is_some()) + usize::from(sheet.notes.is_some());
    }
    count
}

#[cfg(test)]
mod tests {
    use design_model::{FontSet, Orientation, Palette, Print, PrintSize, Sheet, Theme};

    use super::{
        PrintPreviewMode, default_sheet, export_href, field_count, outline_title, png_export_href,
        preview_stage_class, remove_sheet, sheet_download_name, sheet_label, size_option_label,
    };

    #[test]
    fn a_scoped_export_names_the_sheet_in_href_and_filename() {
        assert_eq!(export_href("launch", "", None), "/prints/launch/export");
        assert_eq!(
            export_href("launch", ".pdf", Some(1)),
            "/prints/launch/export.pdf?sheet=2"
        );
        assert_eq!(png_export_href("launch", None), "/prints/launch/export.zip");
        assert_eq!(
            png_export_href("launch", Some(1)),
            "/prints/launch/sheets/2.png"
        );
        assert_eq!(
            sheet_download_name("launch", 1, "png"),
            "launch-sheet-2.png"
        );
    }

    #[test]
    fn a_print_opens_in_read_mode_and_edit_loads_the_editing_script() {
        assert_eq!(PrintPreviewMode::ALL[0], PrintPreviewMode::Read);
        assert_eq!(
            PrintPreviewMode::ALL.map(PrintPreviewMode::label),
            ["Read", "Edit"]
        );
        assert_eq!(PrintPreviewMode::Read.render_query(), "");
        assert_eq!(PrintPreviewMode::Edit.render_query(), "&editable=true");
        assert!(PrintPreviewMode::Read.title().contains("reader"));
        assert!(PrintPreviewMode::Edit.title().contains("select"));
    }

    fn print() -> Print {
        Print {
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
            size: PrintSize::A4,
            orientation: Orientation::Portrait,
            sheets: vec![
                Sheet {
                    html: "<h1>Swift Design</h1>".to_owned(),
                    css: None,
                    notes: Some("Stock: 300 gsm matte.".to_owned()),
                },
                default_sheet(),
            ],
            outline: Vec::new(),
        }
    }

    #[test]
    fn sheet_labels_use_the_heading_then_the_text_then_a_number() {
        assert_eq!(sheet_label(0, &default_sheet()), "1. New sheet");
        let print = print();
        assert_eq!(sheet_label(0, &print.sheets[0]), "1. Swift Design");
        let empty = Sheet {
            html: "<div></div>".to_owned(),
            css: None,
            notes: None,
        };
        assert_eq!(sheet_label(1, &empty), "2. Sheet 2");
    }

    #[test]
    fn outline_titles_fall_back_to_the_sheet_number() {
        let mut planned = print();
        planned.outline = vec!["Front".to_owned(), "  ".to_owned()];
        assert_eq!(outline_title(&planned, 0), "1. Front");
        assert_eq!(outline_title(&planned, 1), "2. Sheet 2");
        assert_eq!(outline_title(&planned, 5), "6. Sheet 6");
    }

    #[test]
    fn the_default_sheet_validates_inside_a_print() {
        let print = print();
        assert_eq!(print.validate(), Vec::new());
        // 11 print and theme fields, html and notes on the first
        // sheet, html and css on the second.
        assert_eq!(field_count(&print), 11 + 2 + 2);
    }

    #[test]
    fn a_print_keeps_its_last_sheet() {
        let mut print = print();
        remove_sheet(&mut print, 5);
        assert_eq!(print.sheets.len(), 2);
        remove_sheet(&mut print, 0);
        assert_eq!(print.sheets.len(), 1);
        assert_eq!(print.sheets[0].html, default_sheet().html);
        remove_sheet(&mut print, 0);
        assert_eq!(print.sheets.len(), 1);
    }

    #[test]
    fn size_options_name_the_size_and_its_portrait_canvas() {
        assert_eq!(size_option_label(PrintSize::A5), "A5 · 559 × 794");
        assert_eq!(size_option_label(PrintSize::A4), "A4 · 794 × 1123");
        assert_eq!(size_option_label(PrintSize::A3), "A3 · 1123 × 1587");
        assert_eq!(size_option_label(PrintSize::Letter), "Letter · 816 × 1056");
        assert_eq!(
            size_option_label(PrintSize::Tabloid),
            "Tabloid · 1056 × 1632"
        );
    }

    #[test]
    fn every_sheet_is_limited_by_height() {
        // Even landscape Tabloid, the widest canvas, is narrower than
        // the 16:10 stage, so every print stage is narrow.
        for size in PrintSize::ALL {
            for orientation in Orientation::ALL {
                assert_eq!(
                    preview_stage_class(orientation.apply(size.viewport())),
                    "preview-stage narrow",
                    "{} {}",
                    size.label(),
                    orientation.label()
                );
            }
        }
    }
}
