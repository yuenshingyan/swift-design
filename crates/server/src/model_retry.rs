//! Bounded, cancellable retries for transient model-provider failures.

use std::future::Future;
use std::time::Duration;

use crate::model_client::LogSink;
use crate::time::unix_now_seconds;

const BACKOFF_SECONDS: [u64; 3] = [5, 10, 20];
const MAX_RETRY_WAIT_SECONDS: u64 = 60;

/// The reason a provider request could not finish.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RequestFailure {
    /// The request can succeed again, optionally after a provider-specified wait.
    Temporary {
        message: String,
        retry_after: Option<Duration>,
    },
    /// Retrying cannot fix this request or account problem.
    Permanent(String),
}

/// Runs one request and at most three retries with cancellable backoff.
pub(crate) async fn with_retries<F, Response>(
    attempt: F,
    log: Option<&LogSink>,
) -> Result<String, String>
where
    F: FnMut() -> Response,
    Response: Future<Output = Result<String, RequestFailure>>,
{
    with_retry_wait(attempt, tokio::time::sleep, log).await
}

async fn with_retry_wait<F, Response, W, Wait>(
    mut attempt: F,
    mut wait: W,
    log: Option<&LogSink>,
) -> Result<String, String>
where
    F: FnMut() -> Response,
    Response: Future<Output = Result<String, RequestFailure>>,
    W: FnMut(Duration) -> Wait,
    Wait: Future<Output = ()>,
{
    for index in 0..=BACKOFF_SECONDS.len() {
        let (message, retry_after) = match attempt().await {
            Ok(text) => return Ok(text),
            Err(RequestFailure::Permanent(message)) => return Err(message),
            Err(RequestFailure::Temporary {
                message,
                retry_after,
            }) => (message, retry_after),
        };
        let Some(backoff) = BACKOFF_SECONDS.get(index) else {
            return Err(format!(
                "{message} (after 4 attempts; automatic retry budget exhausted)"
            ));
        };
        let delay = Duration::from_secs(*backoff).max(retry_after.unwrap_or_default());
        if delay > Duration::from_secs(MAX_RETRY_WAIT_SECONDS) {
            return Err(format!(
                "{message}. The provider requested a retry wait of {} seconds, above the automatic limit of {MAX_RETRY_WAIT_SECONDS} seconds. Retry later.",
                delay.as_secs()
            ));
        }
        if let Some(log) = log {
            log(&format!(
                "model request: waiting to retry attempt {} of 4 in {} seconds after a temporary provider failure",
                index + 2,
                delay.as_secs()
            ));
        }
        // Dropping the run future cancels this wait. No detached request is started.
        wait(delay).await;
        if let Some(log) = log {
            log(&format!(
                "model request: retrying attempt {} of 4",
                index + 2
            ));
        }
    }
    Err("The model request exhausted its retry budget.".to_owned())
}

/// Rejects account failures before classifying a provider message as transient.
pub(crate) fn is_account_failure(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    [
        "insufficient_quota",
        "insufficient quota",
        "exceeded your current quota",
        "billing",
        "credit balance",
        "out of credits",
        "unauthorized",
        "authentication",
        "invalid api key",
    ]
    .iter()
    .any(|marker| message.contains(marker))
}

/// Classifies errors carried inside an otherwise successful provider stream.
pub(crate) fn stream_failure(message: String, code: Option<&str>) -> RequestFailure {
    let normalized = format!("{message} {}", code.unwrap_or_default())
        .replace('_', " ")
        .to_ascii_lowercase();
    let is_temporary = !is_account_failure(&normalized)
        && [
            "overloaded",
            "temporarily unavailable",
            "server error",
            "try again later",
            "rate limit",
            "rate_limit",
            "too many requests",
            "timed out",
        ]
        .iter()
        .any(|marker| normalized.contains(marker));
    if is_temporary {
        RequestFailure::Temporary {
            message,
            retry_after: None,
        }
    } else {
        RequestFailure::Permanent(message)
    }
}

/// Reads Retry-After as delta seconds or an IMF-fixdate without adding a date dependency.
pub(crate) fn retry_after(value: &str) -> Option<Duration> {
    retry_after_at(value, unix_now_seconds())
}

fn retry_after_at(value: &str, now: u64) -> Option<Duration> {
    if let Ok(seconds) = value.trim().parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let timestamp = http_date_seconds(value)?;
    Some(Duration::from_secs(timestamp.saturating_sub(now)))
}

fn http_date_seconds(value: &str) -> Option<u64> {
    let parts: Vec<&str> = value.split_whitespace().collect();
    if parts.len() != 6
        || !["Mon,", "Tue,", "Wed,", "Thu,", "Fri,", "Sat,", "Sun,"].contains(&parts[0])
        || parts[5] != "GMT"
    {
        return None;
    }
    let month = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ]
    .iter()
    .position(|month| *month == parts[2])?;
    let year: u64 = parts[3].parse().ok()?;
    if !(1970..=9999).contains(&year) {
        return None;
    }
    let is_leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let mut days_in_month = [31u64, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if is_leap {
        days_in_month[1] = 29;
    }
    let day: u64 = parts[1].parse().ok()?;
    if day == 0 || day > days_in_month[month] {
        return None;
    }
    let clock: Vec<u64> = parts[4]
        .split(':')
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    if clock.len() != 3 || clock[0] > 23 || clock[1] > 59 || clock[2] > 59 {
        return None;
    }
    let leap_days = (year - 1) / 4 - 1969 / 4 - ((year - 1) / 100 - 1969 / 100)
        + ((year - 1) / 400 - 1969 / 400);
    let days =
        (year - 1970) * 365 + leap_days + days_in_month[..month].iter().sum::<u64>() + day - 1;
    Some(days * 86_400 + clock[0] * 3600 + clock[1] * 60 + clock[2])
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::future::{pending, ready};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use crate::model_client::LogSink;

    use super::{RequestFailure, retry_after_at, stream_failure, with_retry_wait};

    fn temporary() -> RequestFailure {
        RequestFailure::Temporary {
            message: "provider overloaded".to_owned(),
            retry_after: None,
        }
    }

    #[tokio::test]
    async fn retries_are_bounded_and_wait_exponentially() {
        let calls = Cell::new(0);
        let delays = RefCell::new(Vec::new());
        let result = with_retry_wait(
            || {
                calls.set(calls.get() + 1);
                ready(Err(temporary()))
            },
            |delay| {
                delays.borrow_mut().push(delay.as_secs());
                ready(())
            },
            None,
        )
        .await;
        assert_eq!(calls.get(), 4);
        assert_eq!(*delays.borrow(), [5, 10, 20]);
        assert!(result.unwrap_err().contains("after 4 attempts"));
    }

    #[tokio::test]
    async fn recovery_retries_only_the_same_request_and_logs_the_wait() {
        let calls = Cell::new(0);
        let lines = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&lines);
        let log: LogSink = Arc::new(move |line| captured.lock().unwrap().push(line.to_owned()));
        let result = with_retry_wait(
            || {
                calls.set(calls.get() + 1);
                ready(if calls.get() == 3 {
                    Ok("recovered".to_owned())
                } else {
                    Err(temporary())
                })
            },
            |_| ready(()),
            Some(&log),
        )
        .await;
        assert_eq!(result.unwrap(), "recovered");
        assert_eq!(calls.get(), 3);
        assert!(lines.lock().unwrap()[0].contains("attempt 2 of 4 in 5 seconds"));
    }

    #[tokio::test]
    async fn permanent_errors_are_not_retried() {
        let calls = Cell::new(0);
        let result = with_retry_wait(
            || {
                calls.set(calls.get() + 1);
                ready(Err(RequestFailure::Permanent("invalid api key".to_owned())))
            },
            |_| async { panic!("must not wait") },
            None,
        )
        .await;
        assert_eq!(calls.get(), 1);
        assert_eq!(result.unwrap_err(), "invalid api key");
    }

    #[tokio::test]
    async fn provider_wait_is_respected_or_refused_without_retrying_too_early() {
        for seconds in [30, 61] {
            let calls = Cell::new(0);
            let delays = RefCell::new(Vec::new());
            let result = with_retry_wait(
                || {
                    calls.set(calls.get() + 1);
                    ready(if calls.get() == 2 {
                        Ok("ok".to_owned())
                    } else {
                        Err(RequestFailure::Temporary {
                            message: "overload".to_owned(),
                            retry_after: Some(Duration::from_secs(seconds)),
                        })
                    })
                },
                |delay| {
                    delays.borrow_mut().push(delay.as_secs());
                    ready(())
                },
                None,
            )
            .await;
            if seconds == 30 {
                assert!(result.is_ok());
                assert_eq!(*delays.borrow(), [30]);
            } else {
                assert!(result.unwrap_err().contains("Retry later"));
                assert_eq!(calls.get(), 1);
                assert!(delays.borrow().is_empty());
            }
        }
    }

    #[tokio::test]
    async fn cancelling_a_wait_never_sends_another_request() {
        let calls = Cell::new(0);
        let future = with_retry_wait(
            || {
                calls.set(calls.get() + 1);
                ready(Err(temporary()))
            },
            |_| pending::<()>(),
            None,
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(10), future)
                .await
                .is_err()
        );
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn retry_after_accepts_seconds_dates_and_leap_days_but_rejects_invalid_dates() {
        assert_eq!(retry_after_at("30", 0), Some(Duration::from_secs(30)));
        assert_eq!(
            retry_after_at("Thu, 01 Jan 1970 00:00:30 GMT", 0),
            Some(Duration::from_secs(30))
        );
        assert_eq!(
            retry_after_at("Thu, 29 Feb 2024 00:00:00 GMT", 0),
            Some(Duration::from_secs(1_709_164_800))
        );
        assert_eq!(
            retry_after_at("Thu, 01 Jan 1970 00:00:30 GMT", 40),
            Some(Duration::ZERO)
        );
        for value in [
            "-1",
            "garbage",
            "Mon, 29 Feb 2023 00:00:00 GMT",
            "Mon, 01 Jan 2024 24:00:00 GMT",
            "Mon, 32 Jan 2024 00:00:00 GMT",
        ] {
            assert!(retry_after_at(value, 0).is_none());
        }
    }

    #[test]
    fn stream_overload_is_retryable_but_quota_and_authentication_are_not() {
        assert!(matches!(
            stream_failure(
                "Please try again later.".to_owned(),
                Some("insufficient_quota")
            ),
            RequestFailure::Permanent(_)
        ));
        assert!(matches!(
            stream_failure("Something went wrong.".to_owned(), Some("server_error")),
            RequestFailure::Temporary { .. }
        ));
        assert!(matches!(
            stream_failure("Our servers are currently overloaded.".to_owned(), None),
            RequestFailure::Temporary { .. }
        ));
        for message in [
            "insufficient_quota; try again later",
            "invalid api key",
            "authentication failed",
            "exceeded your current quota",
        ] {
            assert!(matches!(
                stream_failure(message.to_owned(), None),
                RequestFailure::Permanent(_)
            ));
        }
    }
}
