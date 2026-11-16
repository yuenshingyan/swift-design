use super::*;

/// The href of the HTML or PDF export: the whole mailing, or one
/// zero-based email through the export route's `?email=N` query.
pub(super) fn export_href(mailing_id: &str, extension: &str, only: Option<usize>) -> String {
    match only {
        Some(index) => format!(
            "/mailings/{mailing_id}/export{extension}?email={}",
            index + 1
        ),
        None => format!("/mailings/{mailing_id}/export{extension}"),
    }
}

/// The href of the PNG export: one email's image when scoped, the zip
/// of every email otherwise.
pub(super) fn png_export_href(mailing_id: &str, only: Option<usize>) -> String {
    match only {
        Some(index) => format!("/mailings/{mailing_id}/emails/{}.png", index + 1),
        None => format!("/mailings/{mailing_id}/export.zip"),
    }
}

/// The href of the email-client HTML export: one email's file when
/// scoped, the zip of every email otherwise.
pub(super) fn email_export_href(mailing_id: &str, only: Option<usize>) -> String {
    match only {
        Some(index) => format!("/mailings/{mailing_id}/emails/{}.html", index + 1),
        None => format!("/mailings/{mailing_id}/export.email.zip"),
    }
}

/// The download name of a scoped export file, matching the zip entry.
pub(super) fn email_download_name(mailing_id: &str, index: usize, extension: &str) -> String {
    format!("{mailing_id}-email-{}.{extension}", index + 1)
}

/// The mailing toolbar's export menus: `All` downloads the whole
/// mailing in one format, and with more than one email a second menu
/// downloads the email on screen. Every action is one shot, with no
/// mode to forget.
#[component]
pub(super) fn MailingExportGroup(
    mailing_id: String,
    selected: usize,
    email_count: usize,
    can_export_with_chrome: bool,
) -> Element {
    let mut is_all_menu_open = use_signal(|| false);
    let mut is_unit_menu_open = use_signal(|| false);
    let number = selected + 1;
    rsx! {
        div { class: "unit-export",
            button {
                title: "Download the whole mailing",
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
                        href: export_href(&mailing_id, "", None),
                        onclick: move |_| is_all_menu_open.set(false),
                        "Download as HTML"
                    }
                    a {
                        href: email_export_href(&mailing_id, None),
                        onclick: move |_| is_all_menu_open.set(false),
                        "Download as email HTML zip"
                    }
                    if can_export_with_chrome {
                        a {
                            href: export_href(&mailing_id, ".pdf", None),
                            onclick: move |_| is_all_menu_open.set(false),
                            "Download as PDF"
                        }
                        a {
                            href: png_export_href(&mailing_id, None),
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
        if email_count > 1 {
            div { class: "unit-export",
                button {
                    title: "Download only the email on screen",
                    onclick: move |_| is_unit_menu_open.set(!is_unit_menu_open()),
                    span { dangerous_inner_html: icons::DOWNLOAD }
                    "Email {number}"
                }
                if is_unit_menu_open() {
                    div {
                        class: "menu-backdrop",
                        onclick: move |_| is_unit_menu_open.set(false),
                    }
                    div { class: "toolbar-menu unit-export-menu",
                        a {
                            href: export_href(&mailing_id, "", Some(selected)),
                            onclick: move |_| is_unit_menu_open.set(false),
                            "Download as HTML"
                        }
                        a {
                            href: email_export_href(&mailing_id, Some(selected)),
                            download: email_download_name(&mailing_id, selected, "html"),
                            onclick: move |_| is_unit_menu_open.set(false),
                            "Download as email HTML"
                        }
                        if can_export_with_chrome {
                            a {
                                href: export_href(&mailing_id, ".pdf", Some(selected)),
                                onclick: move |_| is_unit_menu_open.set(false),
                                "Download as PDF"
                            }
                            a {
                                href: png_export_href(&mailing_id, Some(selected)),
                                download: email_download_name(&mailing_id, selected, "png"),
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

/// The stage class for an email of `viewport`: a narrow canvas is
/// limited by height, a wide one fills the width. No format gets the
/// phone bezel.
pub(super) fn preview_stage_class(viewport: design_model::Viewport) -> &'static str {
    if is_narrow_canvas(viewport) {
        return "preview-stage narrow";
    }
    "preview-stage"
}

/// Removes email `index`, unless it is the last email: a mailing keeps
/// at least one email.
pub(super) fn remove_email(mailing: &mut Mailing, index: usize) {
    if mailing.emails.len() > 1 && index < mailing.emails.len() {
        mailing.emails.remove(index);
    }
}

/// The format option text: the name and the px canvas.
pub(super) fn format_option_label(format: EmailFormat) -> String {
    let viewport = format.viewport();
    format!(
        "{} · {} × {}",
        format.label(),
        viewport.width,
        viewport.height
    )
}

/// Short label for a thumbnail: position, then the first heading text,
/// else the first words of the email, else `Email N`.
pub(super) fn email_label(index: usize, email: &Email) -> String {
    fragment_label("Email", index, &email.html)
}

/// The planned title of outline entry `index`, as the thumbnail tooltip:
/// `5. Title`, or `5. Email 5` when the outline has no entry there.
pub(super) fn outline_title(mailing: &Mailing, index: usize) -> String {
    match mailing.outline.get(index) {
        Some(title) if !title.trim().is_empty() => format!("{}. {}", index + 1, title.trim()),
        _ => format!("{}. Email {}", index + 1, index + 1),
    }
}

/// A sample email for tests: a heading and a paragraph.
#[cfg(test)]
pub(super) fn default_email() -> Email {
    Email {
        html: "<div class='body'><h2>New email</h2><p>Text</p></div>".to_owned(),
        css: Some(
            ".body { padding: 72px; height: 100%; display: flex; flex-direction: column; gap: 16px; } h2 { font-size: 48px; }"
                .to_owned(),
        ),
        notes: None,
    }
}

/// Number of set leaf fields in the mailing. Matches the server's
/// provenance paths: absent optional fields are not counted.
pub(super) fn field_count(mailing: &Mailing) -> usize {
    // Mailing title, format, theme name, four colors, three fonts.
    let mut count = 10;
    for email in &mailing.emails {
        count += 1 + usize::from(email.css.is_some()) + usize::from(email.notes.is_some());
    }
    count
}

#[cfg(test)]
mod tests {
    use design_model::{Email, EmailFormat, FontSet, Mailing, Palette, Theme};

    use super::{
        MailingPreviewMode, default_email, email_download_name, email_export_href, email_label,
        export_href, field_count, format_option_label, outline_title, png_export_href,
        preview_stage_class, remove_email,
    };

    #[test]
    fn a_scoped_export_names_the_email_in_href_and_filename() {
        assert_eq!(export_href("launch", "", None), "/mailings/launch/export");
        assert_eq!(
            export_href("launch", ".pdf", Some(1)),
            "/mailings/launch/export.pdf?email=2"
        );
        assert_eq!(
            png_export_href("launch", None),
            "/mailings/launch/export.zip"
        );
        assert_eq!(
            png_export_href("launch", Some(1)),
            "/mailings/launch/emails/2.png"
        );
        assert_eq!(
            email_export_href("launch", None),
            "/mailings/launch/export.email.zip"
        );
        assert_eq!(
            email_export_href("launch", Some(1)),
            "/mailings/launch/emails/2.html"
        );
        assert_eq!(
            email_download_name("launch", 1, "html"),
            "launch-email-2.html"
        );
    }

    #[test]
    fn a_mailing_opens_in_read_mode_and_edit_loads_the_editing_script() {
        assert_eq!(MailingPreviewMode::ALL[0], MailingPreviewMode::Read);
        assert_eq!(
            MailingPreviewMode::ALL.map(MailingPreviewMode::label),
            ["Read", "Edit"]
        );
        assert_eq!(MailingPreviewMode::Read.render_query(), "");
        assert_eq!(MailingPreviewMode::Edit.render_query(), "&editable=true");
        assert!(MailingPreviewMode::Read.title().contains("reader"));
        assert!(MailingPreviewMode::Edit.title().contains("select"));
    }

    fn mailing() -> Mailing {
        Mailing {
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
            format: EmailFormat::Standard,
            emails: vec![
                Email {
                    html: "<h1>Swift Design</h1>".to_owned(),
                    css: None,
                    notes: Some("Subject: Swift Design.".to_owned()),
                },
                default_email(),
            ],
            outline: Vec::new(),
        }
    }

    #[test]
    fn email_labels_use_the_heading_then_the_text_then_a_number() {
        assert_eq!(email_label(0, &default_email()), "1. New email");
        let mailing = mailing();
        assert_eq!(email_label(0, &mailing.emails[0]), "1. Swift Design");
        let empty = Email {
            html: "<div></div>".to_owned(),
            css: None,
            notes: None,
        };
        assert_eq!(email_label(1, &empty), "2. Email 2");
    }

    #[test]
    fn outline_titles_fall_back_to_the_email_number() {
        let mut planned = mailing();
        planned.outline = vec!["Front".to_owned(), "  ".to_owned()];
        assert_eq!(outline_title(&planned, 0), "1. Front");
        assert_eq!(outline_title(&planned, 1), "2. Email 2");
        assert_eq!(outline_title(&planned, 5), "6. Email 6");
    }

    #[test]
    fn the_default_email_validates_inside_a_mailing() {
        let mailing = mailing();
        assert_eq!(mailing.validate(), Vec::new());
        // 10 mailing and theme fields, html and notes on the first
        // email, html and css on the second.
        assert_eq!(field_count(&mailing), 10 + 2 + 2);
    }

    #[test]
    fn a_mailing_keeps_its_last_email() {
        let mut mailing = mailing();
        remove_email(&mut mailing, 5);
        assert_eq!(mailing.emails.len(), 2);
        remove_email(&mut mailing, 0);
        assert_eq!(mailing.emails.len(), 1);
        assert_eq!(mailing.emails[0].html, default_email().html);
        remove_email(&mut mailing, 0);
        assert_eq!(mailing.emails.len(), 1);
    }

    #[test]
    fn format_options_name_the_format_and_its_canvas() {
        assert_eq!(format_option_label(EmailFormat::Short), "Short · 600 × 800");
        assert_eq!(
            format_option_label(EmailFormat::Standard),
            "Standard · 600 × 1200"
        );
        assert_eq!(format_option_label(EmailFormat::Long), "Long · 600 × 1800");
    }

    #[test]
    fn every_email_is_limited_by_height() {
        // Every format is 600 px wide and taller than wide, so every
        // mailing stage is narrow.
        for format in EmailFormat::ALL {
            assert_eq!(
                preview_stage_class(format.viewport()),
                "preview-stage narrow",
                "{}",
                format.label()
            );
        }
    }
}
