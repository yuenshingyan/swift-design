//! What an edit turn shows the model. A change that names screens or
//! slides through references like `[slide 3, node 0/1 <h2>]` is about
//! those units: the model gets only them, with their layout problems
//! measured in Chrome. A change that names none is systemic: the model
//! gets the whole artifact.

use crate::generation::ArtifactRequest;
use crate::request::SessionRequest;

/// The zero-based indexes named by references like `[{unit} 3, …]`.
/// Sorted, without repeats.
pub(crate) fn referenced_indexes(instruction: &str, unit: &str) -> Vec<usize> {
    let marker = format!("[{unit} ");
    let mut indexes: Vec<usize> = instruction
        .match_indices(&marker)
        .filter_map(|(start, _)| {
            let digits: String = instruction[start + marker.len()..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            digits.parse::<usize>().ok()?.checked_sub(1)
        })
        .collect();
    indexes.sort_unstable();
    indexes.dedup();
    indexes
}

/// The findings that belong to `indexes`, by their `{plural}[i]`
/// prefix. Every finding when `indexes` is empty: a systemic change
/// sees the whole audit.
pub(crate) fn findings_for(findings: &[String], plural: &str, indexes: &[usize]) -> Vec<String> {
    if indexes.is_empty() {
        return findings.to_vec();
    }
    findings
        .iter()
        .filter(|finding| {
            indexes
                .iter()
                .any(|index| finding.starts_with(&format!("{plural}[{index}] ")))
        })
        .cloned()
        .collect()
}

/// The prompt block that explains a focused view.
pub(crate) fn focus_note(unit: &str, plural: &str, indexes: &[usize], total: usize) -> String {
    let shown: Vec<String> = indexes
        .iter()
        .map(|index| (index + 1).to_string())
        .collect();
    format!(
        "The {plural} shown are {unit} {} of {total}. Only the {plural} the change names are \
         shown, each with its zero-based index in the whole set. Patch indexes refer to the \
         whole set. Change only these {plural}. Do not replace, delete, or insert any other \
         {unit}.\n",
        shown.join(", ")
    )
}

/// The prompt block that explains a fresh view: the units named are
/// shown without their markup, and the model writes them anew.
pub(crate) fn fresh_note(unit: &str, plural: &str, indexes: &[usize], total: usize) -> String {
    let shown: Vec<String> = indexes
        .iter()
        .map(|index| (index + 1).to_string())
        .collect();
    format!(
        "Write {unit} {} of {total} anew. The old html and css of these {plural} are removed \
         from the view: write new markup for each, do not repeat the old one. Keep the title \
         and the notes as the purpose of each {unit}. Keep the theme and every other {unit}. \
         Patch indexes refer to the whole set. Replace only these {plural}. Do not delete or \
         insert any {unit}.\n",
        shown.join(", ")
    )
}

/// The prompt block with the measured findings. Empty when there are
/// none.
pub(crate) fn findings_note(findings: &[String]) -> String {
    if findings.is_empty() {
        return String::new();
    }
    let lines: Vec<String> = findings
        .iter()
        .map(|finding| format!("- {finding}"))
        .collect();
    format!(
        "Chrome measured these layout problems before your change. Fix them where the change \
         asks:\n{}\n",
        lines.join("\n")
    )
}

/// The units an edit touched: the ones the change named, and every
/// index whose content differs after the edit. Sorted, without repeats,
/// and within `after`.
pub(crate) fn touched_indexes<T: PartialEq>(
    before: &[T],
    after: &[T],
    named: &[usize],
) -> Vec<usize> {
    let mut indexes: Vec<usize> = named
        .iter()
        .copied()
        .chain(
            after
                .iter()
                .enumerate()
                .filter(|(index, unit)| before.get(*index) != Some(*unit))
                .map(|(index, _)| index),
        )
        .filter(|index| *index < after.len())
        .collect();
    indexes.sort_unstable();
    indexes.dedup();
    indexes
}

/// The change a fix round asks for.
pub(crate) fn fix_instruction(plural: &str) -> String {
    format!(
        "Fix the measured layout problems in the {plural} shown. Keep the content and the \
         design. Change only what the problems need."
    )
}

/// What a fix loop after an edit works from.
pub(crate) struct EditFix<'a, T> {
    /// The session request, for the prompt.
    pub(crate) request: &'a SessionRequest,
    /// The edit's effort, label, and progress share.
    pub(crate) context: &'a ArtifactRequest<'a, T>,
    /// The units the edit touched: the ones measured and fixed.
    pub(crate) indexes: Vec<usize>,
}

/// What an edit turn does: the artifacts to change, the change, and
/// whether the named units are written anew.
pub(crate) struct EditOrder<'a> {
    /// The artifacts to edit, each in turn.
    pub(crate) artifact_ids: &'a [String],
    /// The user's change, with its references.
    pub(crate) instruction: &'a str,
    /// True for a regenerate: the units the instruction names are shown
    /// without their markup, and the model writes them from scratch.
    pub(crate) is_fresh: bool,
}

/// What a merge writes from: the candidates to combine, each with its
/// number and its JSON, and the user's instruction.
pub(crate) struct MergeInput {
    /// The source candidates, as (candidate number, artifact JSON).
    pub(crate) sources: Vec<(usize, String)>,
    /// What to take from each, in the user's words.
    pub(crate) instruction: String,
}

/// The prompt block for a merge: the sources and the rule for taking
/// parts from them. `unit` is `design` or `deck`.
pub(crate) fn merge_note(unit: &str, input: &MergeInput) -> String {
    let mut note = format!(
        "Combine these candidates into one {unit}, as the user asks:\n{}\n",
        input.instruction.trim()
    );
    for (number, json) in &input.sources {
        note.push_str(&format!("Candidate {number}:\n{json}\n"));
    }
    note.push_str(&format!(
        "Take each part from the candidate the user names for it. Keep the markup of a part you \
         take. Where the user names no source for a part, take it from the first candidate. Use \
         one theme for the whole {unit}, and adapt the parts to it so the {unit} reads as one. \
         Write the whole {unit}.\n"
    ));
    note
}

/// The merge sources as the prompt names them: each loaded artifact
/// with its candidate number, or its position when the id has none.
pub(crate) fn merge_sources<T: serde::Serialize>(
    loaded: &[(&str, T)],
) -> Result<Vec<(usize, String)>, String> {
    loaded
        .iter()
        .enumerate()
        .map(|(position, (id, artifact))| {
            let number = crate::candidates::candidate_number_of(id).unwrap_or(position + 1);
            let json = serde_json::to_string(artifact).map_err(|error| error.to_string())?;
            Ok((number, json))
        })
        .collect()
}

/// What one edit prompt is built from.
pub(crate) struct EditInput<'a> {
    /// The user's change, with its references.
    pub(crate) instruction: &'a str,
    /// The artifact JSON: whole, or the focused view.
    pub(crate) artifact_json: &'a str,
    /// The focus note, or empty for a systemic change.
    pub(crate) note: &'a str,
    /// The measured findings for the units shown.
    pub(crate) findings: &'a [String],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_name_the_units_zero_based_without_repeats() {
        let instruction =
            "[slide 4, node 0/4/3 <span>: x] [slide 2, node 1 <h2>] [slide 4, node 1 <p>] fix";
        assert_eq!(referenced_indexes(instruction, "slide"), vec![1, 3]);
        assert!(referenced_indexes(instruction, "screen").is_empty());
        // The page alone, without a node.
        assert_eq!(
            referenced_indexes("[slide 4] More margin at the bottom.", "slide"),
            vec![3]
        );
        assert!(referenced_indexes("make every title bigger", "slide").is_empty());
    }

    #[test]
    fn a_focused_change_sees_only_its_units_findings() {
        let findings = vec![
            "slides[1] h2 (0/1): too tall: cut".to_owned(),
            "slides[3] p (0/2): overflow: shorten".to_owned(),
            "slides[10] p (0/2): overflow: shorten".to_owned(),
        ];
        assert_eq!(
            findings_for(&findings, "slides", &[1]),
            vec![findings[0].clone()]
        );
        assert_eq!(findings_for(&findings, "slides", &[]), findings);
    }

    #[test]
    fn the_touched_units_are_the_named_and_the_changed_ones() {
        let before = vec!["a", "b", "c"];
        let after = vec!["a", "B", "c", "d"];
        assert_eq!(touched_indexes(&before, &after, &[2, 9]), vec![1, 2, 3]);
        assert!(touched_indexes(&before, &before, &[]).is_empty());
        assert!(fix_instruction("slides").contains("in the slides shown"));
    }

    #[test]
    fn the_notes_name_the_units_and_the_findings() {
        let note = focus_note("slide", "slides", &[1, 3], 12);
        assert!(note.contains("slide 2, 4 of 12"));
        assert!(note.contains("Do not replace, delete, or insert any other slide."));
        assert_eq!(findings_note(&[]), "");
        assert!(
            findings_note(&["slides[1] p: overflow".to_owned()])
                .contains("- slides[1] p: overflow")
        );
    }

    #[test]
    fn the_fresh_note_asks_for_new_markup_and_keeps_the_rest() {
        let note = fresh_note("screen", "screens", &[2], 5);
        assert!(note.contains("Write screen 3 of 5 anew."));
        assert!(note.contains("do not repeat the old one"));
        assert!(note.contains("Do not delete or insert any screen."));
    }

    #[test]
    fn the_merge_note_lists_every_source_with_its_number() {
        let input = MergeInput {
            sources: vec![(1, "{\"a\":1}".to_owned()), (3, "{\"c\":3}".to_owned())],
            instruction: "Hero from 1, pricing from 3.".to_owned(),
        };
        let note = merge_note("design", &input);
        assert!(note.starts_with("Combine these candidates into one design"));
        assert!(note.contains("Hero from 1, pricing from 3."));
        assert!(note.contains("Candidate 1:\n{\"a\":1}\n"));
        assert!(note.contains("Candidate 3:\n{\"c\":3}\n"));
        assert!(note.contains("Write the whole design."));
    }
}
