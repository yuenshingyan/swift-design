//! Bounded parallel unit edits, followed by one serial coordination request.
//!
//! Workers read one immutable artifact and cannot save, reorder units, or edit
//! shared fields. Only the coordinated and audited artifact reaches the store.

use std::collections::{BTreeSet, VecDeque};
use std::sync::Arc;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::task::JoinSet;

use design_model::ArtifactKind;

use crate::edit_focus::{EditInput, focus_note};
use crate::edit_report::unit_names;
use crate::generation::{
    ArtifactRequest, DRAFT_SHARE, GenerationEngine, GenerationStop, Validated,
};
use crate::model_client::LogSink;
use crate::{
    artwork_patch, campaign_patch, deck_patch, document_patch, mailing_patch, patch, print_patch,
    social_patch,
};

/// The most page-writing requests in flight for one candidate edit.
pub(crate) const PARALLEL_EDIT_LIMIT: usize = 4;

/// An artifact whose ordinary patch parser can also apply isolated unit edits.
pub(crate) trait ParallelEditable:
    Clone + Serialize + DeserializeOwned + Validated + Send + Sync + 'static
{
    /// The explicit artifact kind for this pipeline.
    const KIND: ArtifactKind;
    /// Applies the pipeline's patch to its supplied baseline.
    fn apply_edit(original: &Self, content: &str) -> Result<Self, String>;
    /// Moves only the assigned unit into the combined artifact.
    fn merge_unit(&mut self, index: usize, edited: Self) -> Result<(), String>;
}

macro_rules! editable {
    ($type:ty, $kind:ident, $patch:ident, $units:ident) => {
        impl ParallelEditable for $type {
            const KIND: ArtifactKind = ArtifactKind::$kind;

            fn apply_edit(original: &Self, content: &str) -> Result<Self, String> {
                $patch::apply_patch(original, $patch::parse_patch(content)?)
            }

            fn merge_unit(&mut self, index: usize, edited: Self) -> Result<(), String> {
                let replacement = edited
                    .$units
                    .into_iter()
                    .nth(index)
                    .ok_or("a worker result is missing its assigned unit")?;
                let destination = self
                    .$units
                    .get_mut(index)
                    .ok_or("a worker result is outside the edit baseline")?;
                *destination = replacement;
                Ok(())
            }
        }
    };
}

editable!(design_model::Design, Demo, patch, screens);
editable!(design_model::Deck, Deck, deck_patch, slides);
editable!(design_model::Document, Document, document_patch, pages);
editable!(design_model::Social, Social, social_patch, frames);
editable!(design_model::Print, Print, print_patch, sheets);
editable!(design_model::Mailing, Mailing, mailing_patch, emails);
editable!(design_model::Campaign, Campaign, campaign_patch, ads);
editable!(design_model::Artwork, Artwork, artwork_patch, covers);

/// The original artifact, normal edit prompt, scope, and validation context.
pub(crate) struct EditExecution<'a, T> {
    /// The immutable artifact every worker starts from.
    pub(crate) original: &'a T,
    /// The ordinary edit messages, including the user's attachments.
    pub(crate) messages: Vec<Value>,
    /// The exact artifact text and scope note embedded in those messages.
    pub(crate) input: &'a EditInput<'a>,
    /// Explicitly selected indexes; empty means a systemic edit.
    pub(crate) indexes: &'a [usize],
    /// True when workers must not see the old unit markup.
    pub(crate) is_fresh: bool,
    /// The existing effort, label, parser, and progress sink.
    pub(crate) request: &'a ArtifactRequest<'a, T>,
}

/// Owned boundaries and baseline shared by the in-flight workers.
struct WorkerShared<T> {
    engine: GenerationEngine,
    client: reqwest::Client,
    original: Arc<T>,
    effort: String,
    label: String,
    log: LogSink,
}

/// Work that can be spawned without borrowing the caller's edit context.
struct WorkerOrder<T> {
    shared: Arc<WorkerShared<T>>,
    index: usize,
    messages: Vec<Value>,
}

/// A bounded queue whose completed unit results are merged and released at once.
struct WorkerBatch<T> {
    pending: VecDeque<usize>,
    tasks: JoinSet<Result<(usize, T), GenerationStop>>,
    merged: T,
    completed: usize,
}

impl<T: ParallelEditable> WorkerBatch<T> {
    fn start_next(
        &mut self,
        shared: &Arc<WorkerShared<T>>,
        edit: &EditExecution<'_, T>,
        snapshot: &Value,
    ) -> Result<(), GenerationStop> {
        let Some(index) = self.pending.pop_front() else {
            return Ok(());
        };
        let messages = page_messages(edit, snapshot, index).map_err(GenerationStop::Failed)?;
        let order = WorkerOrder {
            shared: Arc::clone(shared),
            index,
            messages,
        };
        self.tasks.spawn(page_worker(order));
        Ok(())
    }
}

impl GenerationEngine {
    /// Edits multiple units concurrently, then reconciles their combined result.
    pub(crate) async fn request_edit<T: ParallelEditable>(
        &self,
        client: &reqwest::Client,
        edit: EditExecution<'_, T>,
        log: &LogSink,
    ) -> Result<T, GenerationStop> {
        let snapshot = serde_json::to_value(edit.original)
            .map_err(|_| GenerationStop::Failed("could not read the edit baseline".to_owned()))?;
        let indexes =
            selected_indexes(&snapshot, T::KIND, edit.indexes).map_err(GenerationStop::Failed)?;
        if indexes.len() < 2 {
            return self
                .request_valid(client, edit.messages, edit.request, log)
                .await;
        }
        let (_, plural) = unit_names(T::KIND);
        log(&format!(
            "{}: editing {} {plural} in parallel (at most {PARALLEL_EDIT_LIMIT} requests)",
            edit.request.label,
            indexes.len()
        ));
        let shared = Arc::new(WorkerShared {
            engine: self.clone(),
            client: client.clone(),
            original: Arc::new(edit.original.clone()),
            effort: edit.request.effort.clone(),
            label: edit.request.label.clone(),
            log: Arc::clone(log),
        });
        let merged = collect_workers(shared, &edit, &snapshot, &indexes).await?;
        log(&format!(
            "{}: coordinating the parallel {plural} edits",
            edit.request.label
        ));
        self.coordinate_edit(client, &edit, merged, log).await
    }

    /// Uses the merged artifact, not the original baseline, for the serial patch.
    async fn coordinate_edit<T: ParallelEditable>(
        &self,
        client: &reqwest::Client,
        edit: &EditExecution<'_, T>,
        merged: T,
        log: &LogSink,
    ) -> Result<T, GenerationStop> {
        let messages = coordination_messages(edit, &merged).map_err(GenerationStop::Failed)?;
        let indexes = edit.indexes.to_vec();
        let request = ArtifactRequest {
            effort: edit.request.effort.clone(),
            label: format!("{}: coordination", edit.request.label),
            parse: Box::new(move |content| {
                if !indexes.is_empty() {
                    guard_page_patch(content, T::KIND, &indexes)?;
                }
                T::apply_edit(&merged, content)
            }),
            progress: None,
            live: None,
        };
        let coordinated = self.request_valid(client, messages, &request, log).await?;
        edit.request.report(DRAFT_SHARE);
        Ok(coordinated)
    }
}

/// Collects successful workers without retaining a full artifact per completed page.
async fn collect_workers<T: ParallelEditable>(
    shared: Arc<WorkerShared<T>>,
    edit: &EditExecution<'_, T>,
    snapshot: &Value,
    indexes: &[usize],
) -> Result<T, GenerationStop> {
    let mut batch = WorkerBatch {
        pending: indexes.iter().copied().collect(),
        tasks: JoinSet::new(),
        merged: edit.original.clone(),
        completed: 0,
    };
    for _ in 0..PARALLEL_EDIT_LIMIT.min(indexes.len()) {
        batch.start_next(&shared, edit, snapshot)?;
    }
    while let Some(joined) = batch.tasks.join_next().await {
        let result = joined.unwrap_or_else(|_| {
            Err(GenerationStop::Failed(
                "a parallel edit worker stopped unexpectedly".to_owned(),
            ))
        });
        let (index, result) = match result {
            Ok(result) => result,
            Err(error) => {
                batch.tasks.shutdown().await;
                return Err(error);
            }
        };
        batch.merged = merge_worker_results(batch.merged, vec![(index, result)])
            .map_err(GenerationStop::Failed)?;
        batch.completed += 1;
        edit.request
            .report(DRAFT_SHARE * 0.75 * batch.completed as f32 / indexes.len() as f32);
        (shared.log)(&format!(
            "{}: unit {} finished ({}/{})",
            shared.label,
            index + 1,
            batch.completed,
            indexes.len()
        ));
        batch.start_next(&shared, edit, snapshot)?;
    }
    Ok(batch.merged)
}

/// Validates an isolated patch with the ordinary retry and clarification handling.
async fn page_worker<T: ParallelEditable>(
    order: WorkerOrder<T>,
) -> Result<(usize, T), GenerationStop> {
    let shared = order.shared;
    let original = Arc::clone(&shared.original);
    let index = order.index;
    let (unit, _) = unit_names(T::KIND);
    let request = ArtifactRequest {
        effort: shared.effort.clone(),
        label: format!("{}: {unit} {}", shared.label, index + 1),
        parse: Box::new(move |content| {
            guard_page_patch(content, T::KIND, &[index])?;
            T::apply_edit(&original, content)
        }),
        progress: None,
        live: None,
    };
    let edited = shared
        .engine
        .request_valid(&shared.client, order.messages, &request, &shared.log)
        .await?;
    Ok((index, edited))
}

/// Resolves a systemic scope to every index and keeps a focused scope sorted and unique.
fn selected_indexes(
    snapshot: &Value,
    kind: ArtifactKind,
    indexes: &[usize],
) -> Result<Vec<usize>, String> {
    let (_, plural) = unit_names(kind);
    let units = snapshot[plural]
        .as_array()
        .ok_or("the edit baseline has no unit list")?;
    if indexes.is_empty() {
        return Ok((0..units.len()).collect());
    }
    let selected: BTreeSet<usize> = indexes
        .iter()
        .copied()
        .filter(|index| *index < units.len())
        .collect();
    Ok(selected.into_iter().collect())
}

/// Builds an indexed one-unit view while preserving shared settings and a sibling inventory.
fn focused_worker_json(
    original: &Value,
    kind: ArtifactKind,
    index: usize,
    is_fresh: bool,
) -> Result<String, String> {
    let (unit, plural) = unit_names(kind);
    let units = original[plural]
        .as_array()
        .ok_or("the edit baseline has no unit list")?;
    let mut assigned = units
        .get(index)
        .cloned()
        .ok_or("the assigned unit does not exist")?;
    if is_fresh {
        let object = assigned
            .as_object_mut()
            .ok_or("the assigned unit is not an object")?;
        object.insert("html".to_owned(), Value::String(String::new()));
        object.remove("css");
    }
    let inventory: Vec<Value> = units.iter().enumerate().map(|(index, value)| {
        serde_json::json!({ "index": index, "name": value["name"], "notes": value["notes"] })
    }).collect();
    let shared = original
        .as_object()
        .ok_or("the edit baseline is not an object")?;
    let mut view = Value::Object(
        shared
            .iter()
            .filter(|(key, _)| key.as_str() != plural)
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    );
    view[plural] = serde_json::json!([{ "index": index, unit: assigned }]);
    view[format!("{unit}_count")] = serde_json::json!(units.len());
    view["unit_inventory"] = Value::Array(inventory);
    serde_json::to_string(&view).map_err(|_| "could not render the worker input".to_owned())
}

/// Reuses the ordinary edit prompt and attachments, replacing only the artifact and scope.
fn page_messages<T: ParallelEditable>(
    edit: &EditExecution<'_, T>,
    snapshot: &Value,
    index: usize,
) -> Result<Vec<Value>, String> {
    let (unit, plural) = unit_names(T::KIND);
    let artifact = focused_worker_json(snapshot, T::KIND, index, edit.is_fresh)?;
    let fresh = if edit.is_fresh {
        "The old markup is removed. Write the assigned unit anew.\n"
    } else {
        ""
    };
    let note = format!(
        "{}\nParallel edit worker. Assigned unit index (zero-based): {index}.\n{fresh}\
         Apply the user's change to this {unit} only. Return its patch or an empty patch if no change is needed. \
         Keep shared values unchanged. Do not insert, delete, or reorder units. \
         Defer shared theme, navigation reconciliation, and new or removed units to the coordination pass.\n",
        focus_note(
            unit,
            plural,
            &[index],
            snapshot[plural].as_array().map_or(0, Vec::len)
        )
    );
    let note = if T::KIND == ArtifactKind::Demo {
        format!(
            "{note}Preserve the candidate's shared app_window and its shell marker. Do not choose new window bounds for the assigned screen. Keep window chrome, header height, and sidebar width consistent.\n"
        )
    } else {
        note
    };
    rewrite_messages(&edit.messages, edit.input, &artifact, &note)
}

/// Gives the serial coordinator the combined pages and the original user instruction.
fn coordination_messages<T: ParallelEditable>(
    edit: &EditExecution<'_, T>,
    merged: &T,
) -> Result<Vec<Value>, String> {
    let artifact = serde_json::to_string(merged)
        .map_err(|_| "could not render the combined edits".to_owned())?;
    let (unit, plural) = unit_names(T::KIND);
    let snapshot =
        serde_json::to_value(merged).map_err(|_| "could not read the combined edits".to_owned())?;
    let scope = if edit.indexes.is_empty() {
        "Reconcile shared theme, navigation, and any unit additions or removals the user requested.\n".to_owned()
    } else {
        focus_note(
            unit,
            plural,
            edit.indexes,
            snapshot[plural].as_array().map_or(0, Vec::len),
        )
    };
    let note = format!(
        "Coordinate the parallel edits. The shown artifact already contains the completed worker edits.\n\
         {scope}Keep their completed work. Fix only inconsistencies and unfinished cross-unit requirements. \
         Do not rewrite every {unit} again. Return only a coordination patch. Return an empty patch if the combined result needs no reconciliation.\n"
    );
    let note = if T::KIND == ArtifactKind::Demo {
        format!(
            "{note}Check that every app screen retains its top-level data-swift-design-window shell and the candidate's shared app_window. Keep window chrome, header height, and sidebar width consistent. Change shared bounds only when the user requests a resize.\n"
        )
    } else {
        note
    };
    rewrite_messages(&edit.messages, edit.input, &artifact, &note)
}

/// Rewrites the main prompt without discarding image or file attachment parts.
fn rewrite_messages(
    messages: &[Value],
    input: &EditInput<'_>,
    artifact: &str,
    note: &str,
) -> Result<Vec<Value>, String> {
    let mut messages = messages.to_vec();
    let message = messages
        .iter_mut()
        .find(|message| message["role"] == "user")
        .ok_or("the edit request has no user prompt")?;
    let text = match &mut message["content"] {
        Value::String(text) => text,
        Value::Array(parts) => parts
            .iter_mut()
            .find(|part| part["type"] == "text")
            .and_then(|part| part.get_mut("text"))
            .and_then(|value| match value {
                Value::String(text) => Some(text),
                _ => None,
            })
            .ok_or("the edit request has no text prompt")?,
        _ => return Err("the edit request has no text prompt".to_owned()),
    };
    if input.artifact_json.is_empty() || !text.contains(input.artifact_json) {
        return Err("the edit request is missing its artifact baseline".to_owned());
    }
    *text = text.replacen(input.artifact_json, artifact, 1);
    if !input.note.is_empty() {
        *text = text.replacen(input.note, "", 1);
    }
    text.push('\n');
    text.push_str(note);
    Ok(messages)
}

/// Rejects cross-unit and shared-value operations before the normal parser applies a patch.
fn guard_page_patch(content: &str, kind: ArtifactKind, indexes: &[usize]) -> Result<(), String> {
    let (unit, plural) = unit_names(kind);
    let start = content.find('{').ok_or("return a JSON patch")?;
    let end = content.rfind('}').ok_or("return a JSON patch")?;
    if end < start {
        return Err("return a valid JSON patch".to_owned());
    }
    let patch: Value =
        serde_json::from_str(&content[start..=end]).map_err(|_| "return a valid JSON patch")?;
    let object = patch.as_object().ok_or("return a JSON object patch")?;
    if object.keys().any(|key| key != plural) {
        return Err(
            "isolated edits must not change shared values; defer them to coordination".to_owned(),
        );
    }
    let Some(operations) = object.get(plural) else {
        return Ok(());
    };
    let operations = operations
        .as_array()
        .ok_or("return unit operations as an array")?;
    guard_unit_operations(operations, unit, indexes)
}

/// Allows one complete replacement per assigned unit and no structural operations.
fn guard_unit_operations(
    operations: &[Value],
    unit: &str,
    indexes: &[usize],
) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for operation in operations {
        let index = operation["index"]
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .ok_or("each unit operation needs a zero-based index")?;
        if !indexes.contains(&index) || !seen.insert(index) {
            return Err(
                "replace each assigned unit at most once; do not edit other units".to_owned(),
            );
        }
        if operation
            .get("insert")
            .is_some_and(|value| value != &Value::Bool(false))
            || !operation[unit].is_object()
        {
            return Err(
                "isolated edits cannot insert or delete units; return complete replacements"
                    .to_owned(),
            );
        }
    }
    Ok(())
}

/// Copies only assigned unit results and leaves their order and shared baseline intact.
fn merge_worker_results<T: ParallelEditable>(
    mut original: T,
    results: Vec<(usize, T)>,
) -> Result<T, String> {
    let mut seen = BTreeSet::new();
    for (index, result) in results {
        if !seen.insert(index) {
            return Err("a unit has more than one worker result".to_owned());
        }
        original.merge_unit(index, result)?;
    }
    Ok(original)
}

#[cfg(test)]
#[path = "parallel_edit_tests.rs"]
mod tests;
