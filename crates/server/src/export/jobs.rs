use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::body::Bytes;
use axum::routing::post;
use axum::{Extension, Json};
use serde::Serialize;
use tokio::sync::Mutex;

use super::core::{
    DesignZipQuery, capture_design_images, load_design_for_export, pack_design_images,
    selected_screen_indexes,
};
use super::*;

const RETENTION: Duration = Duration::from_secs(600);
const MAX_JOBS: usize = 8;
const MAX_PNG_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Default)]
struct Jobs(Arc<Mutex<HashMap<String, Job>>>);

struct Job {
    design_id: String,
    total: usize,
    completed: Arc<AtomicUsize>,
    outcome: Option<Result<Bytes, ()>>,
}

#[derive(Serialize)]
struct JobStatus {
    job_id: String,
    state: &'static str,
    completed: usize,
    total: usize,
}

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/designs/{id}/exports", post(start))
        .route("/designs/{id}/exports/{job_id}", get(status))
        .route("/designs/{id}/exports/{job_id}/download", get(download))
        .layer(Extension(Jobs::default()))
}

async fn start(
    State(designs): State<DesignStore>,
    State(settings): State<SettingsStore>,
    Extension(jobs): Extension<Jobs>,
    Path(id): Path<String>,
    Json(query): Json<DesignZipQuery>,
) -> Response {
    let design = match load_design_for_export(&designs, &id).await {
        Ok(design) => design,
        Err(response) => return response,
    };
    let indexes = match selected_screen_indexes(query.screens.as_deref(), design.screens.len()) {
        Ok(indexes) if !indexes.is_empty() => indexes,
        Ok(_) => {
            return api_error::error_response(
                StatusCode::BAD_REQUEST,
                "Cannot export an empty demo.",
                Vec::new(),
            );
        }
        Err(message) => {
            return api_error::error_response(StatusCode::BAD_REQUEST, &message, Vec::new());
        }
    };
    if screenshots::find_chrome().is_none() {
        return screenshots::chrome_missing_response("PNG exports");
    }
    let mut random = [0; 16];
    if let Err(error) = getrandom::getrandom(&mut random) {
        return api_error::internal_error(&anyhow::anyhow!("Cannot create PNG export ID: {error}"));
    }
    let job_id = random
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let total = indexes.len();
    let completed = Arc::new(AtomicUsize::new(0));
    {
        let mut entries = jobs.0.lock().await;
        if entries.len() >= MAX_JOBS || entries.values().any(|job| job.outcome.is_none()) {
            return api_error::error_response(
                StatusCode::TOO_MANY_REQUESTS,
                "Cannot start PNG export: another export is running or too many recent ZIPs are still available; try again shortly.",
                Vec::new(),
            );
        }
        entries.insert(
            job_id.clone(),
            Job {
                design_id: id.clone(),
                total,
                completed: Arc::clone(&completed),
                outcome: None,
            },
        );
    }
    let worker_id = job_id.clone();
    let base_url = format!("http://{}", settings.address());
    tokio::spawn(async move {
        let start = std::time::Instant::now();
        let result = async {
            let images =
                capture_design_images(&design, &base_url, indexes, Some(MAX_PNG_BYTES), |count| {
                    completed.store(count, Ordering::Relaxed)
                })
                .await?;
            tokio::task::spawn_blocking(move || pack_design_images(&id, &images)).await?
        }
        .await;
        let outcome = match result {
            Ok(zip) => {
                tracing::info!(job_id = %worker_id, screen_count = total, elapsed_ms = start.elapsed().as_millis(), size_bytes = zip.len(), "background PNG export ready");
                Ok(Bytes::from(zip))
            }
            Err(error) => {
                tracing::warn!(job_id = %worker_id, %error, "background PNG export failed");
                Err(())
            }
        };
        if let Some(job) = jobs.0.lock().await.get_mut(&worker_id) {
            job.outcome = Some(outcome);
        }
        tokio::time::sleep(RETENTION).await;
        jobs.0.lock().await.remove(&worker_id);
    });
    (
        StatusCode::ACCEPTED,
        [(header::CACHE_CONTROL, "no-store")],
        Json(JobStatus {
            job_id,
            state: "preparing",
            completed: 0,
            total,
        }),
    )
        .into_response()
}

async fn status(
    Extension(jobs): Extension<Jobs>,
    Path((id, job_id)): Path<(String, String)>,
) -> Response {
    let entries = jobs.0.lock().await;
    let Some(job) = entries.get(&job_id).filter(|job| job.design_id == id) else {
        return unavailable();
    };
    let state = match &job.outcome {
        None => "preparing",
        Some(Ok(_)) => "ready",
        Some(Err(())) => "failed",
    };
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(JobStatus {
            job_id,
            state,
            completed: job.completed.load(Ordering::Relaxed),
            total: job.total,
        }),
    )
        .into_response()
}

async fn download(
    Extension(jobs): Extension<Jobs>,
    Path((id, job_id)): Path<(String, String)>,
) -> Response {
    let entries = jobs.0.lock().await;
    let Some(job) = entries.get(&job_id).filter(|job| job.design_id == id) else {
        return unavailable();
    };
    match &job.outcome {
        Some(Ok(zip)) => (
            [
                (header::CONTENT_TYPE, "application/zip".to_owned()),
                (
                    header::CONTENT_DISPOSITION,
                    attachment_disposition(&id, "zip"),
                ),
                (header::CACHE_CONTROL, "no-store".to_owned()),
            ],
            zip.clone(),
        )
            .into_response(),
        _ => api_error::error_response(
            StatusCode::CONFLICT,
            "No PNG ZIP is ready for this export.",
            Vec::new(),
        ),
    }
}

fn unavailable() -> Response {
    api_error::error_response(
        StatusCode::NOT_FOUND,
        "This PNG export is unavailable or expired; start a new export.",
        Vec::new(),
    )
}
