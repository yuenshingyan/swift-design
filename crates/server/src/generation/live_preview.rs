//! Progressive, read-only demo previews, independent of accepted persistence.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use design_model::Design;
use design_model::markup::preview_html_prefix;
use serde_json::{Value, json};

use crate::designs::LivePreview;
use crate::model_client::TextSink;
use crate::patch::{apply_patch, parse_patch};

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;

/// Latest isolated screen operation from each parallel worker.
pub(crate) type PreviewBoard = Arc<Mutex<BTreeMap<usize, Value>>>;

/// Closes JSON containers and a partial string for display only. The ordinary
/// reply parser still requires the exact, complete model output for acceptance.
fn prefix_json(text: &str) -> Option<Value> {
    let start = text.find('{')?;
    let mut text = text[start..].to_owned();
    let mut closers = Vec::new();
    let mut in_string = false;
    let mut escaped = false;
    for byte in text.bytes() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => closers.push(b'}'),
            b'[' => closers.push(b']'),
            b'}' | b']' if closers.pop() != Some(byte) => return None,
            _ => {}
        }
    }
    if in_string {
        if escaped {
            text.pop();
        }
        text.push('"');
    }
    text.extend(closers.into_iter().rev().map(char::from));
    serde_json::from_str(&text).ok()
}

fn close_screen(screen: &mut Value) -> bool {
    let Some(html) = screen
        .get("html")
        .and_then(Value::as_str)
        .and_then(preview_html_prefix)
    else {
        return false;
    };
    screen["html"] = Value::String(html);
    true
}

/// Builds a safe visual draft even while the last HTML string is incomplete.
pub(crate) fn preview_design(text: &str) -> Option<Design> {
    let mut value = prefix_json(text)?;
    let screens = value.get_mut("screens")?.as_array_mut()?;
    for index in 0..screens.len() {
        if !close_screen(&mut screens[index]) {
            if index + 1 != screens.len() {
                return None;
            }
            screens.truncate(index);
            break;
        }
    }
    let design: Design = serde_json::from_value(value).ok()?;
    design.validate().is_empty().then_some(design)
}

/// Applies renderable patch prefixes without ever relaxing saved validation.
pub(crate) fn preview_patch(base: &Design, text: &str) -> Option<Design> {
    let mut value = prefix_json(text)?;
    if let Some(screens) = value.get_mut("screens").and_then(Value::as_array_mut) {
        screens.retain_mut(|operation| {
            if operation.get("screen") == Some(&Value::Null) {
                return true;
            }
            operation.get_mut("screen").is_some_and(close_screen)
        });
    }
    let patch = parse_patch(&serde_json::to_string(&value).ok()?).ok()?;
    let design = apply_patch(base, patch).ok()?;
    design.validate().is_empty().then_some(design)
}

/// New continuation screens, including a safe prefix of the screen being written.
pub(crate) fn preview_continuation_screens(
    written: usize,
    text: &str,
) -> Option<Vec<design_model::Screen>> {
    let mut value = prefix_json(text)?;
    let mut result = Vec::new();
    for operation in value.get_mut("screens")?.as_array_mut()? {
        if operation.get("index").and_then(Value::as_u64) != Some((written + result.len()) as u64)
            || operation.get("insert").and_then(Value::as_bool) != Some(true)
        {
            break;
        }
        let Some(screen) = operation.get_mut("screen") else {
            break;
        };
        if !close_screen(screen) {
            break;
        }
        let Ok(screen) = serde_json::from_value(screen.clone()) else {
            break;
        };
        result.push(screen);
    }
    Some(result)
}

impl LivePreview {
    /// Streams an edit against this round's baseline, not the saved original.
    pub(crate) fn edit_sink(&self, base: &Design) -> Arc<TextSink> {
        let preview = self.clone();
        let base = base.clone();
        Arc::new(move |text| {
            if !preview.should_update() {
                return;
            }
            if let Some(design) = preview_design(text).or_else(|| preview_patch(&base, text)) {
                preview.show(&design);
            }
        })
    }
}

/// Adapts tweak/coordinator patches against their latest merged baseline.
pub(crate) fn patch_sink(base: &Design, sink: Arc<TextSink>) -> Arc<TextSink> {
    let base = base.clone();
    let last = Mutex::new(Instant::now() - Duration::from_secs(1));
    Arc::new(move |text| {
        let Ok(mut last) = last.lock() else { return };
        if last.elapsed() < Duration::from_millis(200) {
            return;
        }
        *last = Instant::now();
        drop(last);
        if let Some(design) = preview_patch(&base, text)
            && let Ok(text) = serde_json::to_string(&design)
        {
            sink(&text);
        }
    })
}

/// Combines isolated worker progress instead of making their previews overwrite
/// each other. Workers cannot preview shared changes or another worker's screen.
pub(crate) fn worker_sink(
    base: Design,
    index: usize,
    board: PreviewBoard,
    sink: Arc<TextSink>,
) -> Arc<TextSink> {
    let last = Mutex::new(Instant::now() - Duration::from_secs(1));
    Arc::new(move |text| {
        let Ok(mut last) = last.lock() else { return };
        if last.elapsed() < Duration::from_millis(200) {
            return;
        }
        *last = Instant::now();
        drop(last);
        let Some(value) = prefix_json(text) else {
            return;
        };
        let Some(operation) = value
            .get("screens")
            .and_then(Value::as_array)
            .and_then(|screens| {
                screens.iter().find(|operation| {
                    operation.get("index").and_then(Value::as_u64) == Some(index as u64)
                        && operation.get("insert").and_then(Value::as_bool) != Some(true)
                        && operation.get("screen").is_some_and(Value::is_object)
                })
            })
        else {
            return;
        };
        let text = {
            let Ok(mut board) = board.lock() else { return };
            board.insert(index, operation.clone());
            json!({"screens": board.values().collect::<Vec<_>>()}).to_string()
        };
        if let Some(design) = preview_patch(&base, &text)
            && let Ok(text) = serde_json::to_string(&design)
        {
            sink(&text);
        }
    })
}
