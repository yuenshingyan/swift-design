use std::collections::BTreeMap;
use std::rc::Rc;

use design_model::{AppWindow, Design, Screen, Theme, Viewport};
use dioxus::prelude::*;
use gloo_net::http::Request;
use serde::Serialize;

use crate::api::send_checked;
use crate::settings::artifact_project;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Region {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl Region {
    fn from_drag([x1, y1, x2, y2]: [f64; 4]) -> Option<Self> {
        if ![x1, y1, x2, y2]
            .iter()
            .all(|value| (0.0..=1.0).contains(value))
            || x1 == x2
            || y1 == y2
        {
            return None;
        }
        Some(Self {
            x: x1.min(x2),
            y: y1.min(y2),
            width: (x1 - x2).abs(),
            height: (y1 - y2).abs(),
        })
    }
}

#[derive(Clone, PartialEq)]
pub(super) struct ScreenAnnotation {
    screen: usize,
    snapshot: Rc<Design>,
    region: Region,
    comment: String,
}

impl ScreenAnnotation {
    fn matches(&self, snapshot: &Design) -> bool {
        self.snapshot.screens.get(self.screen) == snapshot.screens.get(self.screen)
            && self.snapshot.viewport == snapshot.viewport
            && self.snapshot.theme == snapshot.theme
            && self.snapshot.app_window == snapshot.app_window
    }
}

#[component]
pub(super) fn AnnotationSurface(
    screen: usize,
    snapshot: Rc<Design>,
    mut annotations: Signal<Vec<ScreenAnnotation>>,
) -> Element {
    let boxes: Vec<_> = annotations
        .read()
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.screen == screen && entry.matches(&snapshot))
        .map(|(index, entry)| (index, entry.region))
        .collect();
    let width = snapshot.viewport.width;
    let height = snapshot.viewport.height;
    rsx! {
        div { class: "annotation-layer",
            div { class: "annotation-canvas",
                style: "width:min(100cqw, calc(100cqh * {width} / {height})); aspect-ratio:{width} / {height}",
                svg {
                    class: "annotation-surface",
                    view_box: "0 0 1 1",
                    preserve_aspect_ratio: "none",
                    tabindex: "0",
                    "aria-label": "Drag to draw a box on the screen",
                    onmounted: move |_| {
                        let snapshot = snapshot.clone();
                        spawn(async move {
                            let mut channel = document::eval(include_str!("annotation.js"));
                            while let Ok(points) = channel.recv::<[f64; 4]>().await {
                                if let Some(region) = Region::from_drag(points) {
                                    annotations.write().push(ScreenAnnotation { screen, snapshot: snapshot.clone(), region, comment: String::new() });
                                }
                            }
                        });
                    },
                    for (index, region) in &boxes {
                        rect {
                            key: "{index}",
                            class: "annotation-box",
                            x: "{region.x}", y: "{region.y}", width: "{region.width}", height: "{region.height}",
                            vector_effect: "non-scaling-stroke",
                            title { "Box {index + 1}" }
                        }
                    }
                    rect { class: "annotation-box annotation-drag", vector_effect: "non-scaling-stroke", visibility: "hidden" }
                }
                for (index, region) in boxes {
                    span {
                        key: "{index}", class: "annotation-number", aria_hidden: "true",
                        style: "left: {region.x * 100.0}%; top: {region.y * 100.0}%",
                        "{index + 1}"
                    }
                }
            }
        }
    }
}

#[derive(Serialize)]
struct Mark<'a> {
    screen: usize,
    bounds: [u32; 4],
    comment: &'a str,
}

#[derive(Serialize)]
struct Submission<'a> {
    expected: BTreeMap<usize, &'a Screen>,
    viewport: Viewport,
    theme: &'a Theme,
    app_window: &'a Option<AppWindow>,
    marks: Vec<Mark<'a>>,
}

#[derive(Serialize)]
struct AnnotationTurn<'a> {
    content: &'static str,
    design: &'a str,
    action: &'static str,
    annotations: Submission<'a>,
}

#[component]
pub(super) fn AnnotationComments(
    design_id: String,
    screen: usize,
    snapshot: Rc<Design>,
    mut annotations: Signal<Vec<ScreenAnnotation>>,
) -> Element {
    let mut status = use_signal(String::new);
    let mut is_submitting = use_signal(|| false);
    let boxes = annotations.read();
    let has_stale = boxes.iter().any(|entry| !entry.matches(&snapshot));
    let can_submit = !boxes.is_empty()
        && !has_stale
        && boxes.iter().all(|entry| !entry.comment.trim().is_empty());
    rsx! {
        section { class: "annotation-comments", aria_label: "Screen annotations",
            p { "Drag a box around the area you want to change, then add a comment. {boxes.len()} total." }
            if has_stale {
                p { "Some marked screens changed. Remove those boxes and mark the current version before submitting." }
            }
            for (index, entry) in boxes.iter().enumerate().filter(|(_, entry)| entry.screen == screen || !entry.matches(&snapshot)) {
                div { class: "annotation-comment", key: "{index}",
                    label {
                        span { "Box {index + 1}" }
                        if !entry.matches(&snapshot) {
                            span { class: "badge", "Earlier snapshot · Screen {entry.screen + 1}" }
                        }
                        textarea {
                            aria_label: "Comment for box {index + 1}",
                            placeholder: "What should change here?",
                            value: "{entry.comment}",
                            oninput: move |event| {
                                if let Some(entry) = annotations.write().get_mut(index) {
                                    entry.comment = event.value();
                                }
                            },
                        }
                    }
                    button {
                        onclick: move |_| {
                            let mut entries = annotations.write();
                            if index < entries.len() { entries.remove(index); }
                        },
                        "Remove box {index + 1}"
                    }
                }
            }
            if !boxes.is_empty() {
                button {
                    class: "primary",
                    disabled: !can_submit || is_submitting(),
                    onclick: move |_| {
                        if is_submitting() { return; }
                        let entries = annotations.read();
                        let mut expected = BTreeMap::new();
                        let mut marks = Vec::new();
                        for entry in entries.iter() {
                            let Some(screen) = entry.snapshot.screens.get(entry.screen) else { return };
                            expected.insert(entry.screen, screen);
                            let Region { x, y, width, height } = entry.region;
                            let bounds = [x, y, x + width, y + height].map(|part| (part * 10_000.0).round() as u32);
                            marks.push(Mark { screen: entry.screen, bounds, comment: entry.comment.trim() });
                        }
                        let session = artifact_project(&design_id);
                        let request = Request::post(&format!("/sessions/{session}/messages")).json(&AnnotationTurn {
                            content: "Update the marked regions.", design: &design_id, action: "annotate",
                            annotations: Submission { expected, viewport: snapshot.viewport, theme: &snapshot.theme, app_window: &snapshot.app_window, marks },
                        });
                        is_submitting.set(true);
                        status.set(String::new());
                        spawn(async move {
                            let result = match request {
                                Ok(request) => send_checked(request, "Submit annotations").await.map(|_| ()),
                                Err(error) => Err(error.to_string()),
                            };
                            is_submitting.set(false);
                            status.set(match result {
                                Ok(()) => "Annotations submitted. Follow the edit in chat.".into(),
                                Err(error) => error,
                            });
                        });
                    },
                    if is_submitting() { "Submitting…" } else { "Submit annotations" }
                }
            }
            if !status.read().is_empty() {
                p { role: "status", "{status}" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Region;

    #[test]
    fn a_drag_in_either_direction_produces_the_same_normalized_box() {
        let forward = Region::from_drag([0.0, 0.25, 0.5, 1.0]);
        assert_eq!(forward, Region::from_drag([0.5, 1.0, 0.0, 0.25]));
        assert_eq!(
            forward,
            Some(Region {
                x: 0.0,
                y: 0.25,
                width: 0.5,
                height: 0.75
            })
        );
    }

    #[test]
    fn a_zero_size_or_out_of_bounds_region_is_not_added() {
        for points in [
            [0.1, 0.1, 0.1, 0.9],
            [0.1, 0.1, 1.1, 0.9],
            [f64::NAN, 0.1, 0.9, 0.9],
        ] {
            assert!(Region::from_drag(points).is_none());
        }
    }
}
