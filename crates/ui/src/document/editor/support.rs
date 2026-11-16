use super::*;

/// The href of the PNG export: one page's image when scoped, the zip
/// of every page otherwise.
pub(super) fn png_export_href(document_id: &str, only: Option<usize>) -> String {
    match only {
        Some(index) => format!("/documents/{document_id}/pages/{}.png", index + 1),
        None => format!("/documents/{document_id}/export.zip"),
    }
}

/// The download name of a scoped page image, matching the zip entry.
pub(super) fn page_download_name(document_id: &str, index: usize) -> String {
    format!("{document_id}-page-{}.png", index + 1)
}

/// The label of the menu that downloads the page on display.
pub(super) fn page_export_label(index: usize) -> String {
    format!("Page {}", index + 1)
}

/// The document toolbar's export menus: `All` downloads the whole
/// document, and `Page N` downloads the page on display.
#[component]
pub(super) fn DocumentExportGroup(
    document_id: String,
    selected: usize,
    can_export_with_chrome: bool,
) -> Element {
    let mut is_all_menu_open = use_signal(|| false);
    let mut is_unit_menu_open = use_signal(|| false);
    let unit_label = page_export_label(selected);
    rsx! {
        div { class: "unit-export",
            button {
                title: "Download the whole document",
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
                        href: "/documents/{document_id}/export",
                        onclick: move |_| is_all_menu_open.set(false),
                        "Download as HTML"
                    }
                    a {
                        href: "/documents/{document_id}/export.docx",
                        onclick: move |_| is_all_menu_open.set(false),
                        "Download as DOCX"
                    }
                    if can_export_with_chrome {
                        a {
                            href: "/documents/{document_id}/export.pdf",
                            onclick: move |_| is_all_menu_open.set(false),
                            "Download as PDF"
                        }
                        a {
                            href: png_export_href(&document_id, None),
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
        div { class: "unit-export",
            button {
                title: "Download only the page on display",
                onclick: move |_| is_unit_menu_open.set(!is_unit_menu_open()),
                span { dangerous_inner_html: icons::DOWNLOAD }
                "{unit_label}"
            }
            if is_unit_menu_open() {
                div {
                    class: "menu-backdrop",
                    onclick: move |_| is_unit_menu_open.set(false),
                }
                div { class: "toolbar-menu unit-export-menu",
                    if can_export_with_chrome {
                        a {
                            href: png_export_href(&document_id, Some(selected)),
                            download: page_download_name(&document_id, selected),
                            onclick: move |_| is_unit_menu_open.set(false),
                            "Download as PNG"
                        }
                    } else {
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

/// Removes page `index`, unless it is the last page: a document keeps
/// at least one page.
pub(super) fn remove_page(document: &mut Document, index: usize) {
    if document.pages.len() > 1 && index < document.pages.len() {
        document.pages.remove(index);
    }
}

/// The paper option text: the name and the px canvas.
pub(super) fn paper_option_label(paper: Paper) -> String {
    let viewport = paper.viewport();
    format!(
        "{} · {} × {}",
        paper.label(),
        viewport.width,
        viewport.height
    )
}

/// Short label for a thumbnail: position, then the first heading text,
/// else the first words of the page, else `Page N`.
pub(super) fn page_label(index: usize, page: &Page) -> String {
    fragment_label("Page", index, &page.html)
}

/// The planned title of outline entry `index`, as the thumbnail tooltip:
/// `5. Title`, or `5. Page 5` when the outline has no entry there.
pub(super) fn outline_title(document: &Document, index: usize) -> String {
    match document.outline.get(index) {
        Some(title) if !title.trim().is_empty() => format!("{}. {}", index + 1, title.trim()),
        _ => format!("{}. Page {}", index + 1, index + 1),
    }
}

/// A sample page for tests: a heading and a paragraph.
#[cfg(test)]
pub(super) fn default_page() -> Page {
    Page {
        html: "<div class='body'><h2>New page</h2><p>Text</p></div>".to_owned(),
        css: Some(
            ".body { padding: 72px; height: 100%; display: flex; flex-direction: column; gap: 16px; } h2 { font-size: 28px; }"
                .to_owned(),
        ),
        notes: None,
    }
}

/// Number of set leaf fields in the document. Matches the server's
/// provenance paths: absent optional fields are not counted.
pub(super) fn field_count(document: &Document) -> usize {
    // Document title, paper, theme name, four colors, three fonts.
    let mut count = 10;
    for page in &document.pages {
        count += 1 + usize::from(page.css.is_some()) + usize::from(page.notes.is_some());
    }
    count
}

#[cfg(test)]
mod tests {
    use design_model::{Document, FontSet, Page, Palette, Paper, Theme};

    use super::{
        DocumentPreviewMode, default_page, field_count, outline_title, page_download_name,
        page_export_label, page_label, paper_option_label, png_export_href,
    };

    #[test]
    fn a_document_opens_in_read_mode_and_edit_loads_the_editing_script() {
        assert_eq!(DocumentPreviewMode::ALL[0], DocumentPreviewMode::Read);
        assert_eq!(
            DocumentPreviewMode::ALL.map(DocumentPreviewMode::label),
            ["Read", "Edit"]
        );
        assert_eq!(DocumentPreviewMode::Read.render_query(), "");
        assert_eq!(DocumentPreviewMode::Edit.render_query(), "&editable=true");
    }

    fn document() -> Document {
        Document {
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
            paper: Paper::A4,
            pages: vec![
                Page {
                    html: "<h1>Swift Design</h1>".to_owned(),
                    css: None,
                    notes: Some("Open".to_owned()),
                },
                default_page(),
            ],
            outline: Vec::new(),
        }
    }

    #[test]
    fn page_labels_use_the_heading_then_the_text_then_a_number() {
        assert_eq!(page_label(0, &default_page()), "1. New page");
        let document = document();
        assert_eq!(page_label(0, &document.pages[0]), "1. Swift Design");
        let empty = Page {
            html: "<div></div>".to_owned(),
            css: None,
            notes: None,
        };
        assert_eq!(page_label(1, &empty), "2. Page 2");
    }

    #[test]
    fn outline_titles_fall_back_to_the_page_number() {
        let mut planned = document();
        planned.outline = vec!["Intro".to_owned(), "  ".to_owned()];
        assert_eq!(outline_title(&planned, 0), "1. Intro");
        assert_eq!(outline_title(&planned, 1), "2. Page 2");
        assert_eq!(outline_title(&planned, 5), "6. Page 6");
    }

    #[test]
    fn the_default_page_validates_inside_a_document() {
        let document = document();
        assert_eq!(document.validate(), Vec::new());
        // 10 document and theme fields, html and notes on the first
        // page, html and css on the second.
        assert_eq!(field_count(&document), 10 + 2 + 2);
    }

    #[test]
    fn paper_options_name_the_paper_and_its_canvas() {
        assert_eq!(paper_option_label(Paper::A4), "A4 · 794 × 1123");
        assert_eq!(paper_option_label(Paper::Letter), "Letter · 816 × 1056");
    }

    #[test]
    fn png_exports_name_the_page_in_href_and_filename() {
        assert_eq!(
            png_export_href("launch", None),
            "/documents/launch/export.zip"
        );
        assert_eq!(
            png_export_href("launch", Some(1)),
            "/documents/launch/pages/2.png"
        );
        assert_eq!(page_download_name("launch", 1), "launch-page-2.png");
        assert_eq!(page_export_label(1), "Page 2");
    }
}
