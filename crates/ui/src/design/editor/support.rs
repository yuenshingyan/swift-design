use super::*;

/// The download URL for the current screen's PNG.
fn png_export_href(design_id: &str, index: usize) -> String {
    format!("/designs/{design_id}/screens/{}.png", index + 1)
}

/// The download name of a scoped screen image, matching the zip entry.
pub(super) fn screen_download_name(design_id: &str, index: usize) -> String {
    format!("{design_id}-screen-{}.png", index + 1)
}

/// A thumbnail's export-only selection, independent of navigation and chat pins.
#[component]
pub(super) fn ScreenExportCheckbox(
    screen: usize,
    label: String,
    mut selection: Signal<Vec<usize>>,
) -> Element {
    rsx! {
        input {
            class: "thumbnail-export-checkbox",
            r#type: "checkbox",
            "aria-label": "Select {label} for export",
            title: "Select {label} for export",
            checked: selection.read().contains(&screen),
            onclick: move |event| event.stop_propagation(),
            onchange: move |event: Event<FormData>| {
                let mut screens = selection.write();
                screens.retain(|&number| number != screen);
                if event.checked() {
                    screens.push(screen);
                    screens.sort_unstable();
                }
            },
        }
    }
}

/// Toggles a whole group for PNG export, including screens hidden by search.
#[component]
pub(super) fn GroupExportCheckbox(
    label: String,
    screens: Vec<usize>,
    mut selection: Signal<Vec<usize>>,
) -> Element {
    let count = screens.len();
    let selected = selection.read();
    let selected_count = screens
        .iter()
        .filter(|screen| selected.contains(screen))
        .count();
    let (state, mark) = if selected_count == 0 {
        ("false", "")
    } else if selected_count == count {
        ("true", "✓")
    } else {
        ("mixed", "−")
    };
    let unit = if count == 1 { "screen" } else { "screens" };
    let action = if state == "true" {
        "Deselect"
    } else {
        "Select"
    };
    let description = format!("{action} all {count} {label} {unit} for export");
    rsx! {
        button {
            class: "group-export-checkbox",
            r#type: "button",
            role: "checkbox",
            aria_checked: "{state}",
            aria_label: "Select all {count} {label} {unit} for export",
            title: "{description}",
            onclick: move |_| toggle_export_group(&mut selection.write(), &screens),
            span { aria_hidden: "true", "{mark}" }
        }
    }
}

fn toggle_export_group(selection: &mut Vec<usize>, screens: &[usize]) {
    let all_selected = screens.iter().all(|screen| selection.contains(screen));
    selection.retain(|screen| !screens.contains(screen));
    if !all_selected {
        selection.extend_from_slice(screens);
        selection.sort_unstable();
    }
}

/// An empty or out-of-range selection cannot start a selected export.
fn selected_export_query(screens: &[usize], count: usize) -> Option<String> {
    if screens.is_empty() || screens.iter().any(|&screen| screen == 0 || screen > count) {
        return None;
    }
    let query = screens
        .iter()
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join(",");
    Some(query)
}

#[derive(serde::Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum ExportUpdate {
    Progress { completed: usize, total: usize },
    Ready { href: String },
    Failed { message: String },
}

/// The design toolbar's exports of all, current, or selected saved screens.
#[component]
pub(super) fn DesignExportGroup(
    design_id: String,
    selected: usize,
    screen_count: usize,
    mut export_screens: Signal<Vec<usize>>,
    can_export_with_chrome: bool,
) -> Element {
    let count = screen_count;
    let screens: Vec<_> = export_screens()
        .into_iter()
        .filter(|&screen| screen <= count)
        .collect();
    let export_query = if can_export_with_chrome {
        selected_export_query(&screens, count)
    } else {
        None
    };
    let selected_count = screens.len();
    let mut progress = use_signal(|| None::<(usize, usize)>);
    let mut download_href = use_signal(|| None::<String>);
    let mut export_error = use_signal(|| None::<String>);
    let start_export = use_callback({
        let url = format!("/designs/{design_id}/exports");
        move |query: Option<String>| {
            if progress.peek().is_some() {
                return;
            }
            let total = if query.is_some() {
                selected_count
            } else {
                count
            };
            progress.set(Some((0, total)));
            download_href.set(None);
            export_error.set(None);
            if query.is_some() {
                // Clear only at dispatch so later selections survive the export.
                export_screens.set(Vec::new());
            }
            let url = url.clone();
            spawn(async move {
                let mut channel = document::eval(include_str!("export_job.js"));
                if channel.send((url, query)).is_ok() {
                    while let Ok(update) = channel.recv::<ExportUpdate>().await {
                        match update {
                            ExportUpdate::Progress { completed, total } => {
                                progress.set(Some((completed, total)))
                            }
                            ExportUpdate::Ready { href } => {
                                progress.set(None);
                                download_href.set(Some(href));
                                return;
                            }
                            ExportUpdate::Failed { message } => {
                                progress.set(None);
                                export_error.set(Some(message));
                                return;
                            }
                        }
                    }
                }
                progress.set(None);
                export_error.set(Some(
                    "Could not start the PNG export. Check your connection and try again.".into(),
                ));
            });
        }
    });
    let mut is_all_menu_open = use_signal(|| false);
    let mut is_unit_menu_open = use_signal(|| false);
    let unit_label = screen_export_label(selected);
    rsx! {
        div { class: "unit-export",
            button {
                title: "Download the whole design",
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
                        href: "/designs/{design_id}/export",
                        onclick: move |_| is_all_menu_open.set(false),
                        "Download as HTML"
                    }
                    if can_export_with_chrome {
                        button {
                            disabled: progress().is_some() || count == 0,
                            onclick: move |_| {
                                is_all_menu_open.set(false);
                                start_export.call(None);
                            },
                            "Download as PNG zip"
                        }
                    } else {
                        a {
                            "aria-disabled": "true",
                            title: "Install Chrome or Chromium on the server machine, or set SWIFT_DESIGN_CHROME",
                            "Download as PNG zip"
                        }
                    }
                }
            }
        }
        div { class: "selected-screen-export",
            if let Some((completed, total)) = progress() {
                div { class: "export-progress",
                    span { role: "status", "Preparing ZIP: {completed} of {total}" }
                    progress {
                        aria_label: "PNG export progress",
                        max: "{total}",
                        value: "{completed}",
                    }
                }
            } else if let Some(query) = export_query {
                button {
                    class: "primary",
                    title: "Download only the screens checked on the thumbnails",
                    onclick: move |_| start_export.call(Some(query.clone())),
                    "Export selected ({selected_count})"
                }
            } else {
                button {
                    disabled: true,
                    title: if can_export_with_chrome {
                        "Hover over a thumbnail and check its top-left checkbox to select screens"
                    } else {
                        "PNG export requires Chrome or Chromium on the server"
                    },
                    "Export selected ({selected_count})"
                }
            }
            if let Some(href) = download_href() {
                span { class: "export-feedback", role: "status",
                    "ZIP ready. "
                    a { href, download: "{design_id}.zip", "Download ZIP" }
                }
            } else if let Some(message) = export_error() {
                span { class: "export-feedback error", role: "alert", "{message}" }
            }
            if selected_count > 0 {
                button {
                    disabled: progress().is_some(),
                    title: "Clear export selection without changing chat pins",
                    onclick: move |_| export_screens.set(Vec::new()),
                    "Clear"
                }
            }
        }
        div { class: "unit-export",
            button {
                title: "Download only the screen on display",
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
                            href: png_export_href(&design_id, selected),
                            download: screen_download_name(&design_id, selected),
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

/// Opens the full demo at a screen without editor panels or editing scripts.
pub(super) fn view_href(design_id: &str, index: usize) -> String {
    format!("/designs/{design_id}/render?start_screen={}", index + 1)
}

/// The label of the menu that downloads the screen on display.
pub(super) fn screen_export_label(index: usize) -> String {
    format!("Screen {}", index + 1)
}

/// Short label for a thumbnail: position, then the first heading text,
/// else the first words of the screen, else `Screen N`.
pub(super) fn screen_label(index: usize, screen: &Screen) -> String {
    fragment_label("Screen", index, &screen.html)
}

/// The planned title of outline entry `index`, as the thumbnail tooltip:
/// `5. Title`, or `5. Screen 5` when the outline has no entry there.
pub(super) fn outline_title(design: &Design, index: usize) -> String {
    match design.outline.get(index) {
        Some(title) if !title.trim().is_empty() => format!("{}. {}", index + 1, title.trim()),
        _ => format!("{}. Screen {}", index + 1, index + 1),
    }
}

/// A sample screen for tests: a heading and a paragraph.
#[cfg(test)]
pub(super) fn default_screen() -> Screen {
    Screen {
        name: String::new(),
        html: "<div class='body'><h2>New screen</h2><p>Text</p></div>".to_owned(),
        css: Some(
            ".body { padding: 90px; height: 100%; display: flex; flex-direction: column; gap: 30px; } h2 { font-size: 54px; }"
                .to_owned(),
        ),
        notes: None,
    }
}

/// Number of set leaf fields in the design. Matches the server's
/// provenance paths: absent optional fields are not counted.
pub(super) fn field_count(design: &Design) -> usize {
    // Design title, theme name, four colors, three fonts.
    let mut count = 9;
    for screen in &design.screens {
        count += 1 + usize::from(screen.css.is_some()) + usize::from(screen.notes.is_some());
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::design::api::DesignPreview;
    use crate::design::editor::{active_preview, preview_design};

    #[test]
    fn toggle_export_group_selects_all_then_clears_only_that_group() {
        let mut selection = vec![2, 4];
        toggle_export_group(&mut selection, &[1, 2, 3]);
        assert_eq!(selection, [1, 2, 3, 4]);
        toggle_export_group(&mut selection, &[1, 2, 3]);
        assert_eq!(selection, [4]);
        toggle_export_group(&mut selection, &[]);
        assert_eq!(selection, [4]);
    }

    #[test]
    fn selected_export_query_requires_nonempty_saved_screen_selection() {
        assert_eq!(
            selected_export_query(&[23, 24, 25], 25).as_deref(),
            Some("23,24,25")
        );
        assert_eq!(selected_export_query(&[], 25), None);
        assert_eq!(selected_export_query(&[0], 25), None);
        assert_eq!(selected_export_query(&[23, 24, 25], 24), None);
    }

    #[test]
    fn empty_thumbnail_selection_disables_export() {
        use dioxus::prelude::*;
        fn picker() -> Element {
            let export_screens = use_signal(Vec::<usize>::new);
            DesignExportGroup(super::DesignExportGroupProps {
                design_id: "demo".to_owned(),
                selected: 0,
                screen_count: 2,
                export_screens,
                can_export_with_chrome: true,
            })
        }
        let mut dom = VirtualDom::new(picker);
        let changes = dom.rebuild_to_vec();
        let rendered = format!(
            "{changes:?} {:?}",
            dom.get_scope(ScopeId::APP)
                .expect("root component scope should exist after rendering")
                .root_node()
        );
        assert!(rendered.contains("Export selected ("));
        assert!(rendered.contains("disabled"));
        assert!(!rendered.contains("\"primary\""));
    }

    #[test]
    fn thumbnail_selection_drives_toolbar_export_and_clear_control() {
        use dioxus::prelude::*;

        fn selected_thumbnails() -> Element {
            let export_screens = use_signal(|| vec![2, 4]);
            DesignExportGroup(super::DesignExportGroupProps {
                design_id: "demo".to_owned(),
                selected: 0,
                screen_count: 4,
                export_screens,
                can_export_with_chrome: true,
            })
        }
        let mut dom = VirtualDom::new(selected_thumbnails);
        let changes = dom.rebuild_to_vec();
        let rendered = format!(
            "{changes:?} {:?}",
            dom.get_scope(ScopeId::APP)
                .expect("root component scope should exist after rendering")
                .root_node()
        );
        assert!(rendered.contains("primary"));
        assert!(rendered.contains("Clear"));
        assert!(!rendered.contains("Screen numbers or ranges"));
    }

    #[test]
    fn thumbnail_checkbox_has_an_accessible_name_and_checked_state() {
        use dioxus::prelude::*;

        fn checkbox() -> Element {
            let selection = use_signal(|| vec![2]);
            super::ScreenExportCheckbox(super::ScreenExportCheckboxProps {
                screen: 2,
                label: "2. Settings".to_owned(),
                selection,
            })
        }
        let mut dom = VirtualDom::new(checkbox);
        let changes = dom.rebuild_to_vec();
        let rendered = format!(
            "{changes:?} {:?}",
            dom.get_scope(ScopeId::APP)
                .expect("root component scope should exist after rendering")
                .root_node()
        );
        assert!(rendered.contains("thumbnail-export-checkbox"));
        assert!(rendered.contains("Select 2. Settings for export"));
        assert!(rendered.contains("checked"));
    }

    #[test]
    fn outline_titles_fall_back_to_the_screen_number() {
        let mut planned = design();
        planned.outline = vec!["Intro".to_owned(), "  ".to_owned()];
        assert_eq!(outline_title(&planned, 0), "1. Intro");
        assert_eq!(outline_title(&planned, 1), "2. Screen 2");
        assert_eq!(outline_title(&planned, 5), "6. Screen 6");
    }

    #[test]
    fn png_exports_name_the_screen_in_href_and_filename() {
        assert_eq!(
            png_export_href("launch", 1),
            "/designs/launch/screens/2.png"
        );
        assert_eq!(screen_download_name("launch", 1), "launch-screen-2.png");
        assert_eq!(screen_export_label(1), "Screen 2");
    }

    #[test]
    fn live_draft_is_separate_and_dirty_local_changes_win() {
        let saved = design();
        let mut draft = saved.clone();
        draft.title = "Temporary".to_owned();
        draft.screens[0].html = "<p>Live</p>".to_owned();
        assert_eq!(preview_design(&saved, Some(&draft), false), &draft);
        assert_eq!(preview_design(&saved, Some(&draft), true), &saved);
        assert_eq!(saved.title, "T");
    }

    #[test]
    fn retained_checkpoints_cannot_replace_current_or_active_editor_content() {
        let mut preview = DesignPreview {
            design: design(),
            is_recovery: true,
        };
        assert!(active_preview(Some(preview.clone())).is_none());
        assert!(active_preview(None).is_none());
        preview.is_recovery = false;
        assert_eq!(active_preview(Some(preview.clone())), Some(preview.design));
    }

    #[test]
    fn clearing_or_accepting_a_draft_returns_to_saved_content() {
        let original = design();
        let mut accepted = original.clone();
        accepted.title = "Accepted".to_owned();
        assert_eq!(preview_design(&original, None, false), &original);
        assert_eq!(preview_design(&accepted, None, false), &accepted);
        assert_eq!(
            preview_design(&original, Some(&accepted), false),
            preview_design(&accepted, None, false),
        );
    }

    #[test]
    fn live_only_editor_renders_read_only_preview_without_exports() {
        use dioxus::prelude::*;

        fn live_editor() -> Element {
            super::super::LoadedEditor(super::super::LoadedEditorProps {
                design_id: "test-candidate-1".into(),
                initial: design(),
                initial_changes: Default::default(),
                initially_saved: false,
                on_back: EventHandler::new(|_| {}),
            })
        }

        let mut dom = VirtualDom::new(live_editor);
        let changes = dom.rebuild_to_vec();
        let mutations = format!(
            "{changes:?} {:?}",
            dom.get_scope(ScopeId::APP).expect("app").root_node()
        );
        assert!(mutations.contains("Live preview · read-only"));
        assert!(mutations.contains("preview=true"));
        assert!(mutations.contains("readonly"));
        assert!(mutations.contains("click to view"));
        assert!(!mutations.contains("drag to reorder"));
        assert!(!mutations.contains(" for export"));
        assert!(!mutations.contains("editable=true"));
        assert!(!mutations.contains("export.html"));
        assert!(!mutations.contains("Selected screens"));
        assert!(!mutations.contains("/history"));
    }

    #[test]
    fn current_snapshot_editor_offers_view_and_exports_without_draft_controls() {
        use dioxus::prelude::*;

        fn current_editor() -> Element {
            let mut snapshot = design();
            snapshot
                .screens
                .extend([default_screen(), default_screen()]);
            super::super::LoadedEditor(super::super::LoadedEditorProps {
                design_id: "test-candidate-1".into(),
                initial: snapshot,
                initial_changes: crate::design::api::ScreenChanges {
                    new: vec![3],
                    edited: vec![2],
                },
                initially_saved: true,
                on_back: EventHandler::new(|_| {}),
            })
        }

        let mut dom = VirtualDom::new(current_editor);
        let changes = dom.rebuild_to_vec();
        let mutations = format!("{changes:?}");
        assert!(mutations.contains("/designs/test-candidate-1/render?start_screen=1"));
        assert!(mutations.contains(" for export"));
        assert_eq!(
            mutations.matches("CreateTextNode { value: \"New\"").count(),
            1
        );
        assert_eq!(
            mutations
                .matches("CreateTextNode { value: \"Edited\"")
                .count(),
            1
        );
        assert!(!mutations.contains("preview=true"));
        for removed in [
            "Saved version",
            "Review draft",
            "Play review draft",
            "Repair draft",
        ] {
            assert!(
                !mutations.contains(removed),
                "unexpected control: {removed}"
            );
        }
    }

    fn design() -> design_model::Design {
        design_model::Design {
            app_window: None,
            transition: None,
            title: "T".to_owned(),
            theme: design_model::Theme {
                name: "n".to_owned(),
                colors: design_model::Palette {
                    background: "#000000".to_owned(),
                    text: "#ffffff".to_owned(),
                    accent: "#0e6e63".to_owned(),
                    muted: "#888888".to_owned(),
                },
                fonts: design_model::FontSet {
                    heading: "Inter".to_owned(),
                    body: "Inter".to_owned(),
                    mono: "Menlo".to_owned(),
                },
            },
            viewport: Default::default(),
            screens: vec![default_screen()],
            outline: Vec::new(),
        }
    }

    #[test]
    fn screen_labels_use_the_heading_then_the_text_then_a_number() {
        assert_eq!(screen_label(0, &default_screen()), "1. New screen");
        let plain = design_model::Screen {
            name: String::new(),
            html: "<p>Just <b>some</b> words here</p>".to_owned(),
            css: None,
            notes: None,
        };
        assert_eq!(screen_label(2, &plain), "3. Just some words here");
        let empty = design_model::Screen {
            name: String::new(),
            html: "<div></div>".to_owned(),
            css: None,
            notes: None,
        };
        assert_eq!(screen_label(1, &empty), "2. Screen 2");
        assert_eq!(
            first_heading("<p>x</p><h3>Third</h3><h2>Second</h2>"),
            Some("Third".to_owned())
        );
    }

    #[test]
    fn the_default_screen_validates_inside_a_design() {
        assert_eq!(design().validate(), Vec::new());
    }

    #[test]
    fn helpers_handle_blank_input_and_counts() {
        assert_eq!(optional("  ".to_owned()), None);
        assert_eq!(optional("x".to_owned()), Some("x".to_owned()));
        assert_eq!(strip_tags("<h1>Hello <em>world</em></h1>"), "Hello world");
        // 9 design and theme fields, then html and css on the one screen.
        assert_eq!(field_count(&design()), 11);
    }

    #[test]
    fn view_opens_the_full_demo_at_the_selected_screen_without_scoping_or_editing() {
        assert_eq!(
            view_href("demo-candidate-2", 2),
            "/designs/demo-candidate-2/render?start_screen=3"
        );
    }

    #[test]
    fn drag_reorder_keeps_button_destinations_and_outline_titles() {
        let mut screens = vec![default_screen(), default_screen(), default_screen()];
        for screen in &mut screens {
            screen.html = "<a href='#screen-2'>Open details</a>".to_owned();
        }
        let mut outline = vec![
            "Home".to_owned(),
            "Details".to_owned(),
            "Settings".to_owned(),
            "Next".to_owned(),
        ];
        move_screen(&mut screens, &mut outline, 1, 2);
        assert!(
            screens
                .iter()
                .all(|screen| screen.html.contains("href='#screen-3'"))
        );
        assert_eq!(outline, ["Home", "Settings", "Details", "Next"]);
        move_screen(&mut screens, &mut outline, 2, 0);
        assert!(
            screens
                .iter()
                .all(|screen| screen.html.contains("href='#screen-1'"))
        );
        assert_eq!(outline, ["Details", "Home", "Settings", "Next"]);
    }

    #[test]
    fn move_screen_shifts_the_others() {
        let mut screens = vec![default_screen(), default_screen(), default_screen()];
        for (index, screen) in screens.iter_mut().enumerate() {
            screen.notes = Some(index.to_string());
        }
        move_screen(&mut screens, &mut Vec::new(), 0, 2);
        let order: Vec<String> = screens
            .iter()
            .map(|screen| {
                screen
                    .notes
                    .clone()
                    .expect("optional test fixture should be present")
            })
            .collect();
        assert_eq!(order, ["1", "2", "0"]);
        move_screen(&mut screens, &mut Vec::new(), 5, 0);
        assert_eq!(screens.len(), 3);
    }
}
