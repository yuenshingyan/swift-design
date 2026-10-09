//! Saved edit reports derived from the original artifact and the final result.

use std::collections::BTreeSet;

use serde::Serialize;
use serde_json::Value;

use design_model::ArtifactKind;

use crate::sessions::ChatMessage;

/// The prefix of an app-generated edit completion message.
pub(crate) const EDIT_REPORT_PREFIX: &str = "Saved edit report:\n";

/// Compares whole saved artifacts, including fixes applied after the model's edit.
pub(crate) fn saved_edit_report<T: Serialize>(
    id: &str,
    kind: ArtifactKind,
    before: &T,
    after: &T,
) -> Result<String, String> {
    let before = serde_json::to_value(before)
        .map_err(|_| "could not compare the original artifact".to_owned())?;
    let after = serde_json::to_value(after)
        .map_err(|_| "could not compare the edited artifact".to_owned())?;
    let (unit, plural) = unit_names(kind);
    let mut lines = Vec::new();
    unit_changes(&before[plural], &after[plural], unit, &mut lines);
    shared_changes(&before, &after, plural, &mut lines);
    if lines.is_empty() {
        lines.push("No saved changes.".to_owned());
    }
    Ok(format!("Candidate `{id}`:\n{}", lines.join("\n")))
}

/// Names the units without inferring a kind from the artifact id or contents.
fn unit_names(kind: ArtifactKind) -> (&'static str, &'static str) {
    match kind {
        ArtifactKind::Demo => ("screen", "screens"),
        ArtifactKind::Deck => ("slide", "slides"),
        ArtifactKind::Document => ("page", "pages"),
        ArtifactKind::Social => ("frame", "frames"),
        ArtifactKind::Print => ("sheet", "sheets"),
        ArtifactKind::Mailing => ("email", "emails"),
        ArtifactKind::Campaign => ("ad", "ads"),
        ArtifactKind::Artwork => ("cover", "covers"),
    }
}

/// Records changed indexes, not merely the indexes requested or audited.
fn unit_changes(before: &Value, after: &Value, unit: &str, lines: &mut Vec<String>) {
    let before = before.as_array().map(Vec::as_slice).unwrap_or_default();
    let after = after.as_array().map(Vec::as_slice).unwrap_or_default();
    for index in 0..before.len().max(after.len()) {
        let original = before.get(index);
        let saved = after.get(index);
        if original == saved {
            continue;
        }
        let (action, value) = match (original, saved) {
            (None, Some(value)) => ("Added", value),
            (Some(value), None) => ("Removed", value),
            (Some(original), Some(saved)) if only_notes_changed(original, saved) => {
                ("Updated notes only for", saved)
            }
            (_, Some(value)) => ("Modified", value),
            _ => continue,
        };
        lines.push(format!("{action} {}.", unit_label(unit, index, value)));
    }
}

/// Distinguishes handoff-note changes from changes to rendered content.
fn only_notes_changed(before: &Value, after: &Value) -> bool {
    let mut before = before.clone();
    let mut after = after.clone();
    if let Some(object) = before.as_object_mut() {
        object.remove("notes");
    }
    if let Some(object) = after.as_object_mut() {
        object.remove("notes");
    }
    before == after
}

/// Labels one-based indexes and includes a saved name when one exists.
fn unit_label(unit: &str, index: usize, value: &Value) -> String {
    let name = value["name"].as_str().unwrap_or_default();
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    let name: String = name.chars().take(80).collect();
    if name.is_empty() {
        return format!("{unit} {}", index + 1);
    }
    format!("{unit} {} (\"{name}\")", index + 1)
}

/// Separately reports theme and other artifact-wide settings.
fn shared_changes(before: &Value, after: &Value, plural: &str, lines: &mut Vec<String>) {
    let keys: BTreeSet<&str> = [before, after]
        .into_iter()
        .filter_map(Value::as_object)
        .flat_map(|object| object.keys().map(String::as_str))
        .collect();
    let mut settings = Vec::new();
    for key in keys {
        if key == plural || before.get(key) == after.get(key) {
            continue;
        }
        if key == "theme" {
            lines.push(format!("Shared theme changed (affects all {plural})."));
        } else {
            settings.push(key);
        }
    }
    if !settings.is_empty() {
        lines.push(format!("Shared settings changed: {}.", settings.join(", ")));
    }
}

/// Keeps the latest report for each selected candidate available beyond the chat window.
pub(crate) fn latest_edit_report_note(messages: &[ChatMessage], targets: &[String]) -> String {
    let mut indexes = BTreeSet::new();
    for target in targets {
        if let Some(index) = messages.iter().rposition(|message| {
            message.role == "assistant"
                && message.artifacts.contains(target)
                && message.content.starts_with(EDIT_REPORT_PREFIX)
        }) {
            indexes.insert(index);
        }
    }
    if indexes.is_empty() {
        return String::new();
    }
    let reports: Vec<&str> = indexes
        .into_iter()
        .map(|index| messages[index].content.as_str())
        .collect();
    format!(
        "Latest recorded edit reports for the selected candidates. These reports come from \
         saved before/after comparisons, not assistant promises. Use them to answer which \
         units changed in those edits. They are not a visual or backend test.\n{}\n",
        reports.join("\n")
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn artifact(kind: ArtifactKind) -> Value {
        let (_, plural) = unit_names(kind);
        serde_json::json!({
            "title": "Example", "theme": { "colors": { "background": "#ffffff" } },
            plural: [
                { "name": "Overview", "html": "<h1>Overview</h1>" },
                { "name": "Tasks", "html": "<h1>Tasks</h1>", "css": "h1 { color: black; }" }
            ]
        })
    }

    #[test]
    fn reports_actual_one_based_changes_for_every_kind() {
        for kind in ArtifactKind::ALL {
            let (unit, plural) = unit_names(kind);
            let before = artifact(kind);
            let mut after = before.clone();
            after[plural][1]["html"] = Value::String("<h1>Add Task</h1>".to_owned());
            let report = saved_edit_report("example-candidate-4", kind, &before, &after).unwrap();
            assert!(report.contains(&format!("Modified {unit} 2 (\"Tasks\")")));
            assert!(!report.contains(&format!("Modified {unit} 1")));
        }
    }

    #[test]
    fn reports_added_and_removed_units() {
        let before = artifact(ArtifactKind::Demo);
        let mut after = before.clone();
        after["screens"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({ "html": "<h1>New</h1>" }));
        let report = saved_edit_report("a", ArtifactKind::Demo, &before, &after).unwrap();
        assert!(report.contains("Added screen 3."));
        let report = saved_edit_report("a", ArtifactKind::Demo, &after, &before).unwrap();
        assert!(report.contains("Removed screen 3."));
    }

    #[test]
    fn distinguishes_notes_only_theme_only_and_no_change() {
        let before = artifact(ArtifactKind::Document);
        let mut after = before.clone();
        after["pages"][0]["notes"] = Value::String("Updated handoff".to_owned());
        let report = saved_edit_report("a", ArtifactKind::Document, &before, &after).unwrap();
        assert!(report.contains("Updated notes only for page 1"));
        assert!(!report.contains("Modified page"));
        let mut after = before.clone();
        after["theme"]["colors"]["background"] = Value::String("#101418".to_owned());
        after["title"] = Value::String("New title".to_owned());
        let report = saved_edit_report("a", ArtifactKind::Document, &before, &after).unwrap();
        assert!(report.contains("Shared theme changed (affects all pages)"));
        assert!(report.contains("Shared settings changed: title"));
        assert!(!report.contains("Modified page"));
        let report = saved_edit_report("a", ArtifactKind::Document, &before, &before).unwrap();
        assert!(report.contains("No saved changes."));
    }

    #[test]
    fn css_and_name_changes_count_and_names_stay_on_one_line() {
        let before = artifact(ArtifactKind::Demo);
        let mut after = before.clone();
        after["screens"][1]["css"] = Value::String("h1 { color: teal; }".to_owned());
        after["screens"][1]["name"] = Value::String("Task\n  creation".to_owned());
        let report = saved_edit_report("a", ArtifactKind::Demo, &before, &after).unwrap();
        assert!(report.contains("Modified screen 2 (\"Task creation\")"));
    }

    #[test]
    fn retains_only_the_latest_saved_report_for_each_target() {
        let old = ChatMessage::assistant(&format!("{EDIT_REPORT_PREFIX}Old report"))
            .with_artifacts(vec!["a".to_owned()]);
        let latest = ChatMessage::assistant(&format!("{EDIT_REPORT_PREFIX}Modified screen 3."))
            .with_artifacts(vec!["a".to_owned(), "b".to_owned()]);
        let mut messages = vec![old, latest];
        for number in 0..12 {
            messages.push(ChatMessage::user(&format!("Unrelated {number}"), None));
        }
        messages.push(ChatMessage::assistant(&format!(
            "{EDIT_REPORT_PREFIX}Unverified promise"
        )));
        let note = latest_edit_report_note(&messages, &["a".to_owned(), "b".to_owned()]);
        assert!(note.contains("Modified screen 3"));
        assert!(!note.contains("Old report"));
        assert!(!note.contains("Unverified promise"));
        assert_eq!(note.matches(EDIT_REPORT_PREFIX).count(), 1);
        assert!(latest_edit_report_note(&messages, &["other".to_owned()]).is_empty());
        assert!(latest_edit_report_note(&messages, &[]).is_empty());
    }
}
