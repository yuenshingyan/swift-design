//! The run status card: what the agent is doing right now.
//!
//! One block under the conversation while a run is active: a pulsing
//! dot and a sentence, the progress track, the last log line, then the
//! token usage with a Stop button. After the run it shrinks to the
//! usage line, or to the error when the agent exited with a code.

use dioxus::prelude::*;

use crate::api;

/// A short token count: `850`, `42k`, `1.2M`.
fn format_tokens(count: u64) -> String {
    if count >= 1_000_000 {
        format!("{:.1}M", count as f64 / 1_000_000.0)
    } else if count >= 1_000 {
        format!("{}k", count / 1_000)
    } else {
        count.to_string()
    }
}

/// The token line under the status: live context size and run total.
pub(crate) fn usage_line(run: &api::AgentRun) -> String {
    let context = format_tokens(run.context_tokens);
    if run.context_window == 0 {
        return format!("context {context} tokens");
    }
    let percent = run.context_tokens as f64 * 100.0 / run.context_window as f64;
    let percent = if percent < 10.0 {
        format!("{percent:.1}")
    } else {
        format!("{percent:.0}")
    };
    format!("context {context} tokens · {percent}% used this run")
}

/// What the run is doing, from what the record shows: the designs it
/// writes, else its last log line. A run that has only planned so far
/// is `Thinking…`: the model may be answering a question, and no
/// artifact is touched until the planner decides.
pub(crate) fn phase_name(run: &api::AgentRun) -> String {
    match run.designs.len() {
        0 => {}
        1 => return "Writing a candidate…".to_owned(),
        count => return format!("Writing {count} candidates…"),
    }
    let last = last_log_line(&run.log_tail)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if last.starts_with("merging") || last.starts_with("merge") {
        "Merging…".to_owned()
    } else if last.starts_with("edit ") {
        "Editing…".to_owned()
    } else if last.starts_with("continue ") {
        "Writing the rest…".to_owned()
    } else if last.contains("fix round") || last.contains("polish") || last.contains("audit") {
        "Polishing…".to_owned()
    } else if last.contains("concept") {
        "Planning candidates…".to_owned()
    } else if run.progress.is_none() {
        "Thinking…".to_owned()
    } else {
        "Working…".to_owned()
    }
}

/// The status text while a run is active: the phase, with the
/// percent when the engine reports progress: `Writing 2 candidates… 45%`.
pub(crate) fn working_label(run: &api::AgentRun) -> String {
    match run.progress {
        Some(percent) => format!("{} {percent}%", phase_name(run)),
        None => phase_name(run),
    }
}

/// The last non-empty line of the agent log, for the status area.
pub(crate) fn last_log_line(log_tail: &str) -> Option<String> {
    log_tail
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_owned)
}

/// The status card for `run`: the live card while it runs, the usage
/// line or the exit error after.
#[component]
pub(crate) fn RunStatusCard(run: api::AgentRun) -> Element {
    let stop_agent = move |_| {
        spawn(async move {
            let _ = api::stop_agent_run().await;
        });
    };
    if run.is_running {
        return rsx! {
            div { class: "status-card",
                div { class: "status-line",
                    span { class: "status-dot" }
                    span { "{phase_name(&run)}" }
                    if let Some(percent) = run.progress {
                        span { class: "pct", "{percent}%" }
                    }
                }
                if let Some(percent) = run.progress {
                    div { class: "progress-track",
                        div {
                            class: "progress-fill",
                            style: "width: {percent}%",
                        }
                    }
                }
                if let Some(line) = last_log_line(&run.log_tail) {
                    p { class: "agent-log", "{line}" }
                }
                div { class: "usage-line",
                    span { "{usage_line(&run)}" }
                    button { onclick: stop_agent, "Stop" }
                }
            }
        };
    }
    rsx! {
        if run.exit_code == Some(127) {
            p { class: "error",
                "The custom command was not found. Check "
                "SWIFT_DESIGN_AGENT_COMMAND and restart the server."
            }
        } else if run.exit_code.is_some_and(|code| code != 0) {
            p { class: "error", "{failure_title(&run)}" }
            if let Some(line) = last_log_line(&run.log_tail) {
                p { class: "agent-log wrapped", "{line}" }
            }
        } else if run.total_tokens > 0 {
            p { class: "usage-line", "{usage_line(&run)}" }
        }
    }
}

/// The first line of a failed run. A custom command has an exit code
/// worth reading; the built-in engine's code is only a flag, so the
/// line says the run failed and leaves the reason to the log line.
fn failure_title(run: &api::AgentRun) -> String {
    if run.active_agent.as_deref() == Some("custom") {
        return format!(
            "The agent exited with code {}.",
            run.exit_code.unwrap_or_default()
        );
    }
    "The run failed.".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run() -> api::AgentRun {
        api::AgentRun {
            is_running: true,
            exit_code: None,
            log_tail: String::new(),
            active_agent: None,
            session_id: None,
            mode: None,
            context_tokens: 25_000,
            total_tokens: 60_000,
            context_window: 200_000,
            progress: None,
            designs: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn a_failed_run_names_the_exit_code_only_for_a_custom_command() {
        let mut failed = run();
        failed.exit_code = Some(1);
        assert_eq!(failure_title(&failed), "The run failed.");
        failed.active_agent = Some("custom".to_owned());
        assert_eq!(failure_title(&failed), "The agent exited with code 1.");
    }

    #[test]
    fn token_counts_format_short() {
        assert_eq!(format_tokens(850), "850");
        assert_eq!(format_tokens(42_300), "42k");
        assert_eq!(format_tokens(1_250_000), "1.2M");
    }

    #[test]
    fn a_run_that_has_only_planned_is_thinking() {
        let mut planning = run();
        planning.log_tail = "planning the turn\n".to_owned();
        assert_eq!(phase_name(&planning), "Thinking…");
        assert_eq!(phase_name(&run()), "Thinking…");
        let mut editing = run();
        editing.log_tail =
            "planning the turn\nedit talk-candidate-2: requesting (round 1)".to_owned();
        assert_eq!(phase_name(&editing), "Editing…");
        let mut merging = run();
        merging.log_tail =
            "merging talk-candidate-1, talk-candidate-3 into talk-candidate-4".to_owned();
        assert_eq!(phase_name(&merging), "Merging…");
        let mut polishing = run();
        polishing.log_tail = "candidate 1: fix round 2 failed: overfull".to_owned();
        assert_eq!(phase_name(&polishing), "Polishing…");
        let mut concepts = run();
        concepts.log_tail = "planning 3 concepts".to_owned();
        assert_eq!(phase_name(&concepts), "Planning candidates…");
        let mut writing = run();
        writing.designs.insert("talk-candidate-1".to_owned(), 10);
        writing.designs.insert("talk-candidate-2".to_owned(), 0);
        assert_eq!(phase_name(&writing), "Writing 2 candidates…");
        writing.designs.remove("talk-candidate-2");
        assert_eq!(phase_name(&writing), "Writing a candidate…");
        assert_eq!(working_label(&writing), "Writing a candidate…");
        writing.progress = Some(45);
        assert_eq!(working_label(&writing), "Writing a candidate… 45%");
    }

    #[test]
    fn working_labels_carry_the_progress() {
        assert_eq!(working_label(&run()), "Thinking…");
        assert_eq!(
            working_label(&api::AgentRun {
                progress: Some(45),
                ..run()
            }),
            "Working… 45%"
        );
    }

    #[test]
    fn usage_lines_show_the_context_share() {
        assert_eq!(usage_line(&run()), "context 25k tokens · 12% used this run");
        let small = api::AgentRun {
            context_tokens: 376,
            ..run()
        };
        assert_eq!(
            usage_line(&small),
            "context 376 tokens · 0.2% used this run"
        );
        let unknown = api::AgentRun {
            context_window: 0,
            ..run()
        };
        assert_eq!(usage_line(&unknown), "context 25k tokens");
    }

    #[test]
    fn the_last_log_line_skips_blank_lines() {
        assert_eq!(last_log_line("one\ntwo\n\n  \n").as_deref(), Some("two"));
        assert_eq!(last_log_line("\n\n"), None);
    }
}
