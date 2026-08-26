//! Change notifications: one revision counter behind `GET /events`.
//!
//! Every mutating route bumps the revision. Clients pass the last
//! revision they saw plus a wait time; the request returns when the
//! revision moves past it, or when the wait ends. The browser uses
//! this to update the studio live. Agents use it to wait for the
//! user's answers inside one run.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use tokio::sync::watch;

/// Longest wait `GET /events` accepts, in seconds. Below common proxy
/// and agent-tool timeouts, so a waiting call always returns cleanly.
const WAIT_LIMIT_SECONDS: u64 = 60;

/// Bumps and hands out the shared revision counter.
#[derive(Clone)]
pub struct ChangeNotifier {
    sender: Arc<watch::Sender<u64>>,
}

impl ChangeNotifier {
    /// Creates a notifier starting at revision 0.
    pub fn new() -> Self {
        Self {
            sender: Arc::new(watch::channel(0).0),
        }
    }

    /// Records one change: bumps the revision and wakes every waiter.
    pub fn notify(&self) {
        self.sender.send_modify(|revision| *revision += 1);
    }

    /// A receiver that observes revision changes.
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.sender.subscribe()
    }
}

/// The `/events` route table.
pub fn routes() -> Router<crate::AppState> {
    Router::new().route("/events", get(wait_for_change))
}

/// Query of `GET /events`.
#[derive(Debug, Deserialize)]
struct EventsQuery {
    /// The last revision the client saw. The response waits until the
    /// revision moves past this value.
    #[serde(default)]
    after: u64,
    /// Longest time to wait, in seconds. 0 returns at once.
    #[serde(default)]
    wait: u64,
}

/// Returns the current revision, waiting up to `wait` seconds for it to
/// move past `after`.
async fn wait_for_change(
    State(notifier): State<ChangeNotifier>,
    Query(query): Query<EventsQuery>,
) -> Json<serde_json::Value> {
    let mut receiver = notifier.subscribe();
    let wait = Duration::from_secs(query.wait.min(WAIT_LIMIT_SECONDS));
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        let current = *receiver.borrow_and_update();
        if current > query.after {
            return Json(serde_json::json!({ "revision": current }));
        }
        match tokio::time::timeout_at(deadline, receiver.changed()).await {
            Ok(Ok(())) => {}
            // The wait ended, or the notifier dropped: report where we are.
            Ok(Err(_)) | Err(_) => {
                let current = *receiver.borrow();
                return Json(serde_json::json!({ "revision": current }));
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::events::ChangeNotifier;

    #[tokio::test]
    async fn notify_bumps_the_revision_for_subscribers() {
        let notifier = ChangeNotifier::new();
        let receiver = notifier.subscribe();
        assert_eq!(*receiver.borrow(), 0);
        notifier.notify();
        notifier.notify();
        assert_eq!(*receiver.borrow(), 2);
    }

    #[tokio::test]
    async fn a_waiting_subscriber_wakes_on_notify() {
        let notifier = ChangeNotifier::new();
        let mut receiver = notifier.subscribe();
        let waiter = tokio::spawn(async move {
            receiver.changed().await.unwrap();
            *receiver.borrow()
        });
        notifier.notify();
        assert_eq!(waiter.await.unwrap(), 1);
    }
}
