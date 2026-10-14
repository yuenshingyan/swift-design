use super::*;

/// The paths an inspector command goes to: the command's own path
/// first, then every other node in the selection.
pub(crate) fn selection_paths(selection: &[SelectionEntry], primary: &str) -> Vec<String> {
    let mut paths = vec![primary.to_owned()];
    paths.extend(
        selection
            .iter()
            .map(|entry| entry.path.clone())
            .filter(|path| path != primary),
    );
    paths
}

/// The chat reference for one node: `[screen 3, node 0/1 <h2.title>: text]`.
/// `unit` is `screen` or `slide`.
pub(crate) fn node_reference(unit: &str, screen_index: usize, node: &SelectedNode) -> String {
    let class = node
        .classes
        .split_whitespace()
        .next()
        .map(|class| format!(".{class}"))
        .unwrap_or_default();
    let text: String = node.text.chars().take(40).collect();
    if text.is_empty() {
        format!(
            "[{unit} {}, node {} <{}{class}>]",
            screen_index + 1,
            node.path,
            node.tag
        )
    } else {
        format!(
            "[{unit} {}, node {} <{}{class}>: {text}]",
            screen_index + 1,
            node.path,
            node.tag
        )
    }
}

/// The picker entries for an image node: the session's image uploads
/// as `/uploads/{name}` paths.
pub(super) fn image_upload_paths(uploads: &[api::UploadSummary]) -> Vec<String> {
    uploads
        .iter()
        .filter(|upload| upload.is_image)
        .map(|upload| format!("/uploads/{}", upload.name))
        .collect()
}

/// The selected node's quick fields. Every change is sent into the
/// preview, which applies it and posts the screen's HTML back.
#[component]
pub(crate) fn NodeInspector(
    screen: usize,
    node: Option<SelectedNode>,
    on_apply: EventHandler<NodeCommand>,
    scope: String,
) -> Element {
    // The hook runs before the empty-state return, so the hook count
    // stays the same whether a node is selected or not.
    let uploads = use_resource(move || {
        let scope = scope.clone();
        async move { api::fetch_uploads(&scope).await.unwrap_or_default() }
    });
    let Some(node) = node else {
        return rsx! {
            div { class: "sheet-section",
                div { class: "head", "Node" }
                div { class: "inspector-empty",
                    span {
                        class: "glyph",
                        dangerous_inner_html: icons::DASHED_SQUARE,
                    }
                    p { class: "inspector-hint",
                        "Click a node in the preview to edit its text, size, color, alignment, or padding here."
                    }
                }
            }
        };
    };
    let path = node.path.clone();
    let command = move |property: &'static str, value: String| NodeCommand {
        kind: "swift-design-apply",
        screen,
        path: path.clone(),
        property,
        value,
    };
    let class = node
        .classes
        .split_whitespace()
        .next()
        .map(|class| format!(".{class}"))
        .unwrap_or_default();
    let color = if node.styles.color.is_empty() {
        "#000000".to_owned()
    } else {
        node.styles.color.clone()
    };
    let heading = format!("Node <{}{class}> · {}", node.tag, node.path);
    rsx! {
        div { class: "sheet-section inspector",
            div { class: "head mono-head", "{heading}" }
            div { class: "screen-actions",
                button {
                    onclick: {
                        let command = command.clone();
                        move |_| on_apply.call(command("select_parent", String::new()))
                    },
                    "Select parent"
                }
            }
            if node.styles.is_leaf {
                label {
                    "Text"
                    textarea {
                        value: "{node.text}",
                        oninput: {
                            let command = command.clone();
                            move |event| on_apply.call(command("text", event.value()))
                        },
                    }
                }
            }
            div { class: "frame-grid",
                label {
                    "Font size"
                    input {
                        value: "{node.styles.font_size}",
                        oninput: {
                            let command = command.clone();
                            move |event| on_apply.call(command("font_size", event.value()))
                        },
                    }
                }
                label {
                    "Color"
                    div { class: "color-field",
                        input {
                            r#type: "color",
                            value: "{color}",
                            oninput: {
                                let command = command.clone();
                                move |event| on_apply.call(command("text_color", event.value()))
                            },
                        }
                        span { class: "color-code mono", "{color}" }
                    }
                }
                div { class: "field",
                    span { class: "field-label", "Align" }
                    Select {
                        value: node.styles.text_align.clone(),
                        options: plain_options(["start", "left", "center", "right", "justify"]),
                        on_change: {
                            let command = command.clone();
                            move |align: String| on_apply.call(command("text_align", align))
                        },
                    }
                }
                label {
                    "Padding"
                    input {
                        value: "{node.styles.padding}",
                        oninput: {
                            let command = command.clone();
                            move |event| on_apply.call(command("padding", event.value()))
                        },
                    }
                }
                label {
                    "Width"
                    input {
                        value: "{node.styles.width}",
                        oninput: {
                            let command = command.clone();
                            move |event| on_apply.call(command("width", event.value()))
                        },
                    }
                }
                label {
                    "Height"
                    input {
                        value: "{node.styles.height}",
                        oninput: {
                            let command = command.clone();
                            move |event| on_apply.call(command("height", event.value()))
                        },
                    }
                }
                if node.tag == "img" {
                    if let Some(images) = uploads.read().as_deref().map(image_upload_paths) {
                        if !images.is_empty() {
                            div { class: "upload-picker",
                                for image in images {
                                    button {
                                        class: if node.styles.src == image { "selected" } else { "" },
                                        title: "{image}",
                                        onclick: {
                                            let command = command.clone();
                                            let image = image.clone();
                                            move |_| on_apply.call(command("src", image.clone()))
                                        },
                                        img { src: "{image}" }
                                        span { class: "upload-name", "{image}" }
                                    }
                                }
                            }
                        }
                    }
                    label {
                        "Image source: click a file, or type a /uploads/… path"
                        input {
                            value: "{node.styles.src}",
                            oninput: {
                                let command = command.clone();
                                move |event| on_apply.call(command("src", event.value()))
                            },
                        }
                    }
                }
            }
        }
    }
}
