use super::*;

/// Forwards preview-iframe messages into the Dioxus event loop, turns
/// arrow keys on the editor page into navigate messages, and tracks
/// thumbnail drags. Same-origin messages only.
///
/// The drag runs in JS with `elementFromPoint` because Safari does not
/// fire `pointerenter` on other elements while a mouse button is held.
pub(crate) const PREVIEW_LISTENER: &str = "\
window.addEventListener('message', (event) => {
  if (event.origin !== window.location.origin) { return; }
  const data = event.data;
  if (data && (data.type === 'swift-design-html' || data.type === 'swift-design-select'
      || data.type === 'swift-design-action' || data.type === 'swift-design-navigate')) {
    dioxus.send(data);
  }
});
document.addEventListener('keydown', (event) => {
  const target = event.target;
  if (target && (target.tagName === 'TEXTAREA' || target.tagName === 'INPUT' || target.tagName === 'SELECT' || target.isContentEditable)) { return; }
  if (event.key === 'ArrowRight' || event.key === 'PageDown') {
    event.preventDefault();
    dioxus.send({ type: 'swift-design-navigate', step: 1 });
  } else if (event.key === 'ArrowLeft' || event.key === 'PageUp') {
    event.preventDefault();
    dioxus.send({ type: 'swift-design-navigate', step: -1 });
  }
});
document.addEventListener('keydown', (event) => {
  if (event.key === 'Escape') { dioxus.send({ type: 'swift-design-escape' }); }
});
let dragFrom = null;
let dragOver = null;
const thumbnailIndex = (element) => {
  const thumbnail = element && element.closest ? element.closest('.thumbnail[data-index]') : null;
  return thumbnail ? Number(thumbnail.dataset.index) : null;
};
const endDrag = () => {
  if (dragFrom === null) { return; }
  dragFrom = null;
  dragOver = null;
  dioxus.send({ type: 'swift-design-drag', action: 'end', screen: 0 });
};
document.addEventListener('pointerdown', (event) => {
  if (event.button !== 0 || (event.target.closest && event.target.closest('button'))) { return; }
  const index = thumbnailIndex(event.target);
  if (index === null) { return; }
  dragFrom = index;
  dragOver = index;
  dioxus.send({ type: 'swift-design-drag', action: 'start', screen: index });
});
document.addEventListener('pointermove', (event) => {
  if (dragFrom === null) { return; }
  const over = thumbnailIndex(document.elementFromPoint(event.clientX, event.clientY));
  if (over !== null && over !== dragOver) {
    dragOver = over;
    dioxus.send({ type: 'swift-design-drag', action: 'over', screen: over });
  }
});
document.addEventListener('pointerup', endDrag);
document.addEventListener('pointercancel', endDrag);
window.addEventListener('blur', endDrag);
";

/// Posts one command from the inspector into the preview iframe. Only
/// the live pane of the double-buffered preview carries `true`.
pub(crate) const APPLY_TO_PREVIEW: &str = "\
const message = await dioxus.recv();
const frame = document.querySelector('iframe[data-preview=true]');
if (frame && frame.contentWindow) {
  frame.contentWindow.postMessage(message, window.location.origin);
}
";

/// The pane addresses of the double-buffered preview: the document on
/// screen, then the incoming one while it still loads.
pub(crate) fn preview_panes(loaded_address: &str, address: &str) -> Vec<String> {
    if loaded_address == address {
        return vec![address.to_owned()];
    }
    vec![loaded_address.to_owned(), address.to_owned()]
}

/// The main preview, double-buffered. A changed address loads in an
/// invisible pane of the same size and swaps in once loaded, so a mode
/// switch or a save never flashes a half-rendered page. The panes are
/// keyed by address: a swap only retargets attributes and never moves
/// or reloads an iframe.
#[component]
pub(crate) fn PreviewFrame(title: String, ratio: String, src: String) -> Element {
    let mut loaded_src = use_signal(|| src.clone());
    rsx! {
        div { class: "preview-swap",
            for pane_src in preview_panes(&loaded_src(), &src) {
                iframe {
                    key: "{pane_src}",
                    title: "{title}",
                    class: if pane_src == loaded_src() { "" } else { "preview-loading" },
                    "data-preview": if pane_src == loaded_src() { "true" } else { "false" },
                    tabindex: if pane_src == loaded_src() { "0" } else { "-1" },
                    style: "aspect-ratio: {ratio}",
                    src: "{pane_src}",
                    onload: {
                        let pane_src = pane_src.clone();
                        move |_| loaded_src.set(pane_src.clone())
                    },
                }
            }
        }
    }
}

/// The computed styles of the selected node, as the preview reports
/// them.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub(crate) struct NodeStyles {
    #[serde(default)]
    pub(crate) font_size: String,
    #[serde(default)]
    pub(crate) color: String,
    #[serde(default)]
    pub(crate) text_align: String,
    #[serde(default)]
    pub(crate) padding: String,
    #[serde(default)]
    pub(crate) src: String,
    #[serde(default)]
    pub(crate) width: String,
    #[serde(default)]
    pub(crate) height: String,
    #[serde(default)]
    pub(crate) is_leaf: bool,
}

/// How the preview takes a click: as a selection, or as the user of the
/// demo would.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PreviewMode {
    /// A click selects a node, a double-click edits its text.
    Edit,
    /// A click acts: a link opens its screen, a menu opens, a toggle
    /// flips.
    Play,
}

impl PreviewMode {
    /// Both modes, in tab order. Play comes first: it is the default,
    /// so a demo opens clickable.
    pub(crate) const ALL: [PreviewMode; 2] = [PreviewMode::Play, PreviewMode::Edit];

    /// The tab label.
    pub(crate) fn label(self) -> &'static str {
        match self {
            PreviewMode::Edit => "Edit",
            PreviewMode::Play => "Play",
        }
    }

    /// The tab tooltip.
    pub(crate) fn title(self) -> &'static str {
        match self {
            PreviewMode::Edit => "Click a node to select it",
            PreviewMode::Play => "Click buttons, links, and menus as a user would",
        }
    }

    /// The query that asks the render for the editing script. Play mode
    /// asks for nothing, so a click reaches the content.
    pub(crate) fn render_query(self) -> &'static str {
        match self {
            PreviewMode::Edit => "&editable=true",
            PreviewMode::Play => "",
        }
    }
}

/// One message posted by the editable preview page.
#[derive(Debug, Deserialize)]
pub(crate) struct PreviewMessage {
    /// `swift-design-html`, `swift-design-select`, `swift-design-action`,
    /// `swift-design-navigate`, `swift-design-drag`, or `swift-design-escape`.
    #[serde(rename = "type")]
    pub(crate) kind: String,
    /// Screen or slide index the message is about. Absent on navigate
    /// messages.
    #[serde(default)]
    pub(crate) screen: usize,
    /// Node path from the root, like `0/2/1`. `None` for the root.
    #[serde(default)]
    pub(crate) path: Option<String>,
    /// Node tag name.
    #[serde(default)]
    pub(crate) tag: Option<String>,
    /// Node class attribute.
    #[serde(default)]
    pub(crate) classes: Option<String>,
    /// Start of the node text.
    #[serde(default)]
    pub(crate) text: Option<String>,
    /// Computed styles of the node.
    #[serde(default)]
    pub(crate) styles: Option<NodeStyles>,
    /// The root's HTML after a change.
    #[serde(default)]
    pub(crate) html: Option<String>,
    /// True when the change should be saved at once.
    #[serde(default)]
    pub(crate) save: bool,
    /// The menu action: `pin`, `ask`, `properties`, or `delete-screen`.
    #[serde(default)]
    pub(crate) action: Option<String>,
    /// Screens to move by, on navigate messages: 1 or -1.
    #[serde(default)]
    pub(crate) step: i32,
    /// The screen to open, on a navigate message from a screen link.
    #[serde(default)]
    pub(crate) target: Option<usize>,
    /// Every node selected, on select and pin messages, the primary
    /// one last. A command-click adds to it.
    #[serde(default)]
    pub(crate) selection: Vec<SelectionEntry>,
}

/// One node of a multi-node selection, as the preview names it.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub(crate) struct SelectionEntry {
    #[serde(default)]
    pub(crate) path: String,
    #[serde(default)]
    pub(crate) tag: String,
    #[serde(default)]
    pub(crate) classes: String,
    #[serde(default)]
    pub(crate) text: String,
}

impl SelectionEntry {
    /// The entry as a selected node, without computed styles.
    fn to_node(&self) -> SelectedNode {
        SelectedNode {
            path: self.path.clone(),
            tag: self.tag.clone(),
            classes: self.classes.clone(),
            text: self.text.clone(),
            styles: NodeStyles::default(),
        }
    }
}

/// The nodes a select message carries: its selection list, or the one
/// node it describes when the list is absent.
pub(crate) fn selection_of(message: &PreviewMessage) -> Vec<SelectionEntry> {
    if !message.selection.is_empty() {
        return message.selection.clone();
    }
    SelectedNode::from_message(message)
        .map(|node| SelectionEntry {
            path: node.path,
            tag: node.tag,
            classes: node.classes,
            text: node.text,
        })
        .into_iter()
        .collect()
}

/// The chat reference for a selection. One node gets the full node
/// reference, with its text. Several nodes share one bracket without
/// the texts, so the chip stays short:
/// `[slide 9, nodes 0/5/0 <p>; 0/5/1 <div.rule>]`.
pub(crate) fn selection_reference(
    unit: &str,
    screen_index: usize,
    entries: &[SelectionEntry],
) -> String {
    if let [entry] = entries {
        return node_reference(unit, screen_index, &entry.to_node());
    }
    let nodes: Vec<String> = entries
        .iter()
        .map(|entry| {
            let class = entry
                .classes
                .split_whitespace()
                .next()
                .map(|class| format!(".{class}"))
                .unwrap_or_default();
            format!("{} <{}{class}>", entry.path, entry.tag)
        })
        .collect();
    format!("[{unit} {}, nodes {}]", screen_index + 1, nodes.join("; "))
}

/// The chat reference the Pin to chat menu item sets: every node in
/// the selection, or the bare unit when the message names no node.
pub(crate) fn pin_reference(unit: &str, screen_index: usize, entries: &[SelectionEntry]) -> String {
    if entries.is_empty() {
        return format!("[{unit} {}]", screen_index + 1);
    }
    selection_reference(unit, screen_index, entries)
}

/// The node the user selected in the preview.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SelectedNode {
    pub(crate) path: String,
    pub(crate) tag: String,
    pub(crate) classes: String,
    pub(crate) text: String,
    pub(crate) styles: NodeStyles,
}

impl SelectedNode {
    pub(crate) fn from_message(message: &PreviewMessage) -> Option<Self> {
        Some(Self {
            path: message.path.clone()?,
            tag: message.tag.clone().unwrap_or_default(),
            classes: message.classes.clone().unwrap_or_default(),
            text: message.text.clone().unwrap_or_default(),
            styles: message.styles.clone().unwrap_or_default(),
        })
    }
}

/// A command the inspector sends into the preview.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct NodeCommand {
    #[serde(rename = "type")]
    pub(crate) kind: &'static str,
    pub(crate) screen: usize,
    pub(crate) path: String,
    pub(crate) property: &'static str,
    pub(crate) value: String,
}
