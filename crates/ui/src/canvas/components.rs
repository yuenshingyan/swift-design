use super::*;

/// The candidate canvas for a session.
#[component]
pub(crate) fn CandidateCanvas(
    session_id: String,
    cards: Vec<CanvasCard>,
    run_designs: HashMap<String, u8>,
    /// The ids whose Finish was pressed and not yet served.
    #[props(default)]
    queued: HashSet<String>,
    revision: u64,
    chosen: Option<String>,
    /// What the session builds. A placeholder card takes the shape of
    /// the kind before any card exists.
    kind: ArtifactKind,
    /// The canvas a placeholder takes when no tab is open yet.
    blank_viewport: Viewport,
    on_open: EventHandler<(ArtifactKind, String)>,
    on_continue: EventHandler<String>,
    /// A Fork press: copy this candidate under the next free number.
    /// Called once per ticked card.
    on_fork: EventHandler<String>,
    on_error: EventHandler<String>,
) -> Element {
    let mut shown = use_signal(HashMap::<String, usize>::new);
    let mut open_tab = use_signal(usize::default);
    // The ticked cards, and whether the next Delete is the confirming
    // one. A delete cannot be undone, so it takes two clicks.
    let mut selected = use_signal(HashSet::<String>::new);
    let mut is_confirming_delete = use_signal(|| false);
    // Placeholder ids the run reports that are not on disk yet.
    let placeholders: Vec<String> = run_designs
        .keys()
        .filter(|id| !cards.iter().any(|card| &card.id == *id))
        .cloned()
        .collect();
    // A run writes one design per canvas. Showing every canvas at once
    // would put a phone card next to a desktop card at different
    // scales, so each canvas gets a tab.
    let tabs = canvas_tabs(&cards);
    let tab = open_tab().min(tabs.len().saturating_sub(1));
    let tab_viewport = tabs.get(tab).copied().unwrap_or(blank_viewport);
    let shown_cards: Vec<CanvasCard> = cards
        .iter()
        .filter(|card| tabs.len() <= 1 || card.viewport == tab_viewport)
        .cloned()
        .collect();
    // Every card of the tab shares one height, picked so they fit one
    // row: the placeholders count too, since they take the same shape.
    let on_tab = shown_cards.len() + placeholders.len();
    let kinds: HashMap<String, ArtifactKind> = cards
        .iter()
        .map(|card| (card.id.clone(), card.kind))
        .collect();
    let selected_count = selected().len();
    // The ticked cards Finish acts on: previews no run is writing.
    let finishable: HashSet<String> = cards
        .iter()
        .filter(|card| {
            card.is_preview() && !run_designs.contains_key(&card.id) && !queued.contains(&card.id)
        })
        .map(|card| card.id.clone())
        .collect();
    let finish_count = selected()
        .iter()
        .filter(|id| finishable.contains(*id))
        .count();
    let delete_selected = use_callback(move |_: ()| {
        let ids: Vec<String> = selected().iter().cloned().collect();
        let kinds = kinds.clone();
        spawn(async move {
            for id in ids {
                let deleted = match kinds.get(&id).copied().unwrap_or(kind) {
                    ArtifactKind::Demo => api::delete_design(&id).await,
                    ArtifactKind::Deck => api::delete_deck(&id).await,
                    ArtifactKind::Document => api::delete_document(&id).await,
                    ArtifactKind::Social => api::delete_social(&id).await,
                    ArtifactKind::Print => api::delete_print(&id).await,
                    ArtifactKind::Mailing => api::delete_mailing(&id).await,
                    ArtifactKind::Campaign => api::delete_campaign(&id).await,
                    ArtifactKind::Artwork => api::delete_artwork(&id).await,
                };
                if let Err(message) = deleted {
                    on_error.call(message);
                }
            }
            selected.write().clear();
            is_confirming_delete.set(false);
        });
    });
    rsx! {
        // The bar is always here. Showing it only with a selection moved
        // the whole canvas down on the first tick.
        div { class: "selection-bar",
            span { class: "selection-count",
                if selected_count > 0 {
                    "{selected_count} selected"
                } else {
                    "Select cards with the checkboxes"
                }
            }
            button {
                class: "selection-finish",
                disabled: finish_count == 0,
                title: "Write the rest of each ticked preview from its outline",
                onclick: move |_| {
                    let mut ids: Vec<String> = selected()
                        .iter()
                        .filter(|id| finishable.contains(*id))
                        .cloned()
                        .collect();
                    ids.sort();
                    for id in ids {
                        on_continue.call(id);
                    }
                    selected.write().clear();
                    is_confirming_delete.set(false);
                },
                span { dangerous_inner_html: icons::PLAY }
                if finish_count > 0 {
                    "Finish {finish_count}"
                } else {
                    "Finish"
                }
            }
            button {
                class: "selection-fork",
                disabled: selected_count == 0,
                title: "Copy each ticked candidate as a new one",
                onclick: move |_| {
                    let mut ids: Vec<String> = selected().iter().cloned().collect();
                    ids.sort();
                    for id in ids {
                        on_fork.call(id);
                    }
                    selected.write().clear();
                    is_confirming_delete.set(false);
                },
                if selected_count > 0 {
                    "Fork {selected_count}"
                } else {
                    "Fork"
                }
            }
            button {
                class: if is_confirming_delete() { "selection-delete confirm" } else { "selection-delete" },
                disabled: selected_count == 0,
                onclick: move |_| {
                    if is_confirming_delete() {
                        delete_selected.call(());
                    } else {
                        is_confirming_delete.set(true);
                    }
                },
                if is_confirming_delete() {
                    "Delete {selected_count}? This cannot be undone"
                } else if selected_count > 0 {
                    "Delete {selected_count}"
                } else {
                    "Delete"
                }
            }
            button {
                class: "selection-clear",
                disabled: selected_count == 0,
                onclick: move |_| {
                    selected.write().clear();
                    is_confirming_delete.set(false);
                },
                "Clear"
            }
        }
        if !tabs.is_empty() {
            div { class: "canvas-tabs", role: "tablist",
                for (index, viewport) in tabs.iter().enumerate() {
                    button {
                        key: "{index}",
                        role: "tab",
                        class: if index == tab { "canvas-tab open" } else { "canvas-tab" },
                        title: "{canvas_size(*viewport)}",
                        onclick: move |_| open_tab.set(index),
                        span { class: "tab-name", "{canvas_name(*viewport)}" }
                        span { class: "tab-count", "{cards_on_canvas(&cards, *viewport)}" }
                    }
                }
            }
        }
        div { class: "canvas-grid",
            for card in shown_cards {
                {
                    let id = card.id.clone();
                    let current = shown.read().get(&id).copied().unwrap_or(1);
                    let progress = run_designs.get(&id).copied();
                    let is_queued = queued.contains(&id);
                    let is_chosen = chosen.as_deref() == Some(id.as_str());
                    let is_selected = selected().contains(&id);
                    rsx! {
                        CandidateCard {
                            key: "{id}",
                            card,
                            on_tab,
                            current,
                            progress,
                            is_queued,
                            is_chosen,
                            is_selected,
                            revision,
                            on_open,
                            on_select: move |id: String| {
                                toggle_selection(&mut selected.write(), id);
                                is_confirming_delete.set(false);
                            },
                            on_page: move |(id, next): (String, usize)| {
                                shown.write().insert(id, next);
                            },
                        }
                    }
                }
            }
            for id in placeholders {
                PlaceholderCard {
                    key: "{id}",
                    percent: run_designs.get(&id).copied(),
                    viewport: tab_viewport,
                    is_bezelled: kind == ArtifactKind::Demo && is_portrait_canvas(tab_viewport),
                    on_tab,
                    id,
                }
            }
        }
    }
}

/// The screen step for an arrow key on a focused card.
pub(crate) fn key_step(key: &Key) -> Option<i32> {
    match key {
        Key::ArrowLeft => Some(-1),
        Key::ArrowRight => Some(1),
        _ => None,
    }
}

/// Toggles one candidate without changing the other selected candidates.
pub(crate) fn toggle_selection(selected: &mut HashSet<String>, id: String) {
    if !selected.remove(&id) {
        selected.insert(id);
    }
}

/// One candidate card with a preview and a visible selection checkbox.
#[component]
fn CandidateCard(
    card: CanvasCard,
    /// How many cards the tab holds, placeholders included. Sets the
    /// shared height.
    on_tab: usize,
    current: usize,
    progress: Option<u8>,
    /// True while the run has this card's Finish in its queue but has
    /// not reported progress on it yet.
    #[props(default)]
    is_queued: bool,
    is_chosen: bool,
    is_selected: bool,
    revision: u64,
    on_open: EventHandler<(ArtifactKind, String)>,
    on_select: EventHandler<String>,
    on_page: EventHandler<(String, usize)>,
) -> Element {
    let id = card.id.clone();
    let kind = card.kind;
    let count = card.count.max(1);
    let current = current.clamp(1, count);
    let is_phone = card.is_bezelled();
    let frame = card_frame(card.viewport, on_tab, is_phone);
    let class = card_class(CardFlags {
        is_chosen,
        is_phone,
        is_placeholder: false,
        is_selected,
    });
    let ratio = card.ratio.clone();
    let preview = card.preview_url(revision, current);
    let remaining = card.remaining_count();
    // A preview waits for the rest of its outline. Finish in the bar
    // asks the app for it, and the run then shows its progress here.
    let is_finish_offered = card.is_preview() && progress.is_none();
    let is_finishing = is_finish_offered && is_queued;
    let has_pages = count > 1;
    // The edge arrows and the arrow keys step through the screens. The
    // arrows sit inside the card, and the card opens on click, so they
    // keep their clicks to themselves.
    let step = {
        let id = id.clone();
        use_callback(move |delta: i32| {
            on_page.call((id.clone(), stepped_screen(current, delta, count)));
        })
    };
    // A click opens the card. A command-click selects it, like a
    // finder row; the footer selects on a plain click.
    let open = {
        let id = id.clone();
        move |event: MouseEvent| {
            if event.modifiers().meta() || event.modifiers().ctrl() {
                on_select.call(id.clone());
            } else {
                on_open.call((kind, id.clone()));
            }
        }
    };
    let arrow = move |delta: i32| {
        move |event: MouseEvent| {
            event.stop_propagation();
            step(delta);
        }
    };
    rsx! {
        article {
            class: "{class}",
            style: "{frame.style()}",
            tabindex: "0",
            onclick: open,
            onkeydown: {
                let id = id.clone();
                move |event: KeyboardEvent| {
                    if let Some(delta) = key_step(&event.key()) {
                        event.prevent_default();
                        step(delta);
                    } else if event.key() == Key::Enter {
                        on_open.call((kind, id.clone()));
                    }
                }
            },
            div { class: "card-stage",
                label {
                    class: "card-select",
                    title: if is_selected { "Deselect candidate" } else { "Select for Finish, Fork, or Delete" },
                    onclick: move |event: MouseEvent| event.stop_propagation(),
                    onkeydown: move |event: KeyboardEvent| event.stop_propagation(),
                    input {
                        r#type: "checkbox",
                        checked: is_selected,
                        aria_label: "Select candidate {card.title}",
                        onchange: {
                            let id = id.clone();
                            move |_| on_select.call(id.clone())
                        },
                    }
                    span {
                        class: "card-select-mark",
                        aria_hidden: "true",
                        dangerous_inner_html: icons::CHECK,
                    }
                }
                // A phone sits in a bezel drawn by a wrapper, so the
                // iframe itself carries no rounded corners or shadows.
                if is_phone {
                    div { class: "bezel",
                        iframe { src: "{preview}", title: "{id}", tabindex: "-1" }
                    }
                } else {
                    iframe {
                        src: "{preview}",
                        style: "aspect-ratio: {ratio}",
                        title: "{id}",
                        tabindex: "-1",
                    }
                }
                if has_pages {
                    button {
                        class: "card-arrow left",
                        aria_label: "Previous screen",
                        tabindex: "-1",
                        disabled: current == 1,
                        onclick: arrow(-1),
                        "‹"
                    }
                    button {
                        class: "card-arrow right",
                        aria_label: "Next screen",
                        tabindex: "-1",
                        disabled: current == count,
                        onclick: arrow(1),
                        "›"
                    }
                }
                div { class: "card-pills",
                    if is_chosen {
                        span { class: "chosen-pill",
                            span { dangerous_inner_html: icons::CHECK }
                            "Chosen"
                        }
                    }
                    if progress.is_some() {
                        span { class: "card-pill",
                            span { class: "dot" }
                            "writing"
                        }
                    }
                    if is_finishing {
                        span { class: "card-pill",
                            span { class: "dot" }
                            "queued"
                        }
                    }
                    // A preview waits for the rest of its outline. The
                    // pill says how much, and Finish in the bar writes it.
                    if is_finish_offered && !is_finishing {
                        span { class: "card-pill planned", "{remaining} planned" }
                    }
                }
                if let Some(percent) = progress {
                    div { class: "card-progress",
                        div {
                            class: "card-progress-fill",
                            style: "width: {percent}%",
                        }
                    }
                }
                // A queued Finish has no percentage yet: the bar slides
                // until the run reports one.
                if is_finishing {
                    div { class: "card-progress indeterminate",
                        div { class: "card-progress-fill" }
                    }
                }
            }
            // Keep footer selection as a shortcut beside the visible toggle.
            div {
                class: "card-footer",
                title: if is_selected { "Selected · click to deselect" } else { "Click to select" },
                "aria-pressed": "{is_selected}",
                onclick: {
                    let id = id.clone();
                    move |event: MouseEvent| {
                        event.stop_propagation();
                        on_select.call(id.clone());
                    }
                },
                div { class: "card-name",
                    // The number is what the chat calls the card (`@2`),
                    // so it stays; the title tells the cards apart.
                    span { class: "card-number", title: "{candidate_label(&id)}",
                        "{candidate_number(&id)}"
                    }
                    span { class: "card-title", title: "{card.title}", "{card.title}" }
                }
                span { class: "card-count", "{current}/{count}" }
            }
        }
    }
}

/// A card for an artifact the run has reported but not saved yet. It
/// takes the shape of the open tab's canvas.
#[component]
fn PlaceholderCard(
    id: String,
    viewport: Viewport,
    /// True when the blank sits in a phone bezel.
    is_bezelled: bool,
    on_tab: usize,
    percent: Option<u8>,
) -> Element {
    let is_phone = is_bezelled;
    let class = card_class(CardFlags {
        is_chosen: false,
        is_phone,
        is_placeholder: true,
        is_selected: false,
    });
    let frame = card_frame(viewport, on_tab, is_phone);
    rsx! {
        article { class: "{class}", style: "{frame.style()}",
            div { class: "card-stage",
                if is_phone {
                    div { class: "bezel",
                        div { class: "card-blank" }
                    }
                } else {
                    div { class: "card-blank" }
                }
                span { class: "card-pill",
                    span { class: "dot" }
                    "writing"
                }
                if let Some(percent) = percent {
                    div { class: "card-progress",
                        div {
                            class: "card-progress-fill",
                            style: "width: {percent}%",
                        }
                    }
                }
            }
            div { class: "card-footer",
                div { class: "card-name",
                    span { class: "card-label", "{candidate_label(&id)}" }
                }
            }
        }
    }
}
