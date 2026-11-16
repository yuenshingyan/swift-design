use super::*;

/// The reasoning effort for requests that write screen HTML: one level
/// under the run's effort, never below `low`. Writing markup gains
/// little from long reasoning, and the effort level still sets the fix
/// and polish rounds. `minimal` is not a value every model accepts.
pub(crate) fn writing_effort(effort: &str) -> &'static str {
    match effort {
        "high" => "medium",
        _ => "low",
    }
}

/// The complete objects of the JSON array under `key` in a partial
/// JSON text: the index of the array's `[`, and each complete `{…}`
/// item. `None` until the array has started.
pub(crate) fn complete_array_items<'text>(
    text: &'text str,
    key: &str,
) -> Option<(usize, Vec<&'text str>)> {
    let quoted = format!("\"{key}\"");
    let key_position = text.find(&quoted)?;
    let after_key = &text[key_position + quoted.len()..];
    let bracket_offset = after_key.find('[')?;
    // Only a colon and spaces may sit between the key and its array.
    if !after_key[..bracket_offset]
        .trim()
        .trim_start_matches(':')
        .trim()
        .is_empty()
    {
        return None;
    }
    let array_start = key_position + quoted.len() + bracket_offset;
    let mut items = Vec::new();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut is_escaped = false;
    let mut item_start = None;
    for (offset, character) in text[array_start + 1..].char_indices() {
        let position = array_start + 1 + offset;
        if in_string {
            if is_escaped {
                is_escaped = false;
            } else if character == '\\' {
                is_escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }
        match character {
            '"' => in_string = true,
            '{' => {
                if depth == 0 {
                    item_start = Some(position);
                }
                depth += 1;
            }
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0
                    && let Some(start) = item_start.take()
                {
                    items.push(&text[start..=position]);
                }
            }
            ']' if depth == 0 => break,
            _ => {}
        }
    }
    Some((array_start, items))
}

/// One continuation chunk: the zero-based outline index of its first
/// title and how many titles it covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ContinueChunk {
    pub(crate) first: usize,
    pub(crate) count: usize,
}

/// The chunks that cover outline titles `start..planned`, each at most
/// `CONTINUE_CHUNK_SCREENS` long.
pub(crate) fn continue_chunks(start: usize, planned: usize) -> Vec<ContinueChunk> {
    (start..planned)
        .step_by(CONTINUE_CHUNK_SCREENS)
        .map(|first| ContinueChunk {
            first,
            count: (planned - first).min(CONTINUE_CHUNK_SCREENS),
        })
        .collect()
}
