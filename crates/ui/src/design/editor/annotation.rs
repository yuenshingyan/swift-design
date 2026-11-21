use std::rc::Rc;

use design_model::Design;
use dioxus::prelude::*;

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

#[component]
pub(super) fn AnnotationComments(
    screen: usize,
    snapshot: Rc<Design>,
    mut annotations: Signal<Vec<ScreenAnnotation>>,
) -> Element {
    let boxes = annotations.read();
    rsx! {
        section { class: "annotation-comments", aria_label: "Screen annotations",
            p { "Drag a box around the area you want to change, then add a comment. {boxes.len()} total." }
            for (index, entry) in boxes.iter().enumerate().filter(|(_, entry)| entry.screen == screen) {
                div { class: "annotation-comment", key: "{index}",
                    label {
                        span { "Box {index + 1}" }
                        if !entry.matches(&snapshot) {
                            span { class: "badge", "Earlier snapshot" }
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
