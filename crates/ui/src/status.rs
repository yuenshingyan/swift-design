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

/// The status text while a run is active: `Working… 45%` when the
/// engine reports progress, else `Working…`.
pub(crate) fn working_label(run: &api::AgentRun) -> String {
    match run.progress {
        Some(percent) => format!("Working… {percent}%"),
        None => "Working…".to_owned(),
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
                    span { "Working…" }
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
            p { class: "error", "The agent exited with code {run.exit_code.unwrap_or_default()}." }
            if let Some(line) = last_log_line(&run.log_tail) {
                p { class: "agent-log", "{line}" }
            }
        } else if run.total_tokens > 0 {
            p { class: "usage-line", "{usage_line(&run)}" }
        }
    }
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
            context_tokens: 25_000,
            total_tokens: 60_000,
            context_window: 200_000,
            progress: None,
            designs: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn token_counts_format_short() {
        assert_eq!(format_tokens(850), "850");
        assert_eq!(format_tokens(42_300), "42k");
        assert_eq!(format_tokens(1_250_000), "1.2M");
    }

    #[test]
    fn working_labels_carry_the_progress() {
        assert_eq!(working_label(&run()), "Working…");
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
