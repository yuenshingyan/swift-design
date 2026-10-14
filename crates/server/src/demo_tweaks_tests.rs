//! Exercises verified tweaking through real model and browser boundaries.
#![allow(clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use axum::http::StatusCode;
use tempfile::TempDir;

use design_model::{ArtifactKind, Design, Transition};

use crate::designs::DesignStore;
use crate::edit_focus::EditFix;
use crate::events::ChangeNotifier;
use crate::generation::{ArtifactRequest, GenerationEngine};
use crate::model_client::LogSink;
use crate::polish::{DemoAudit, DemoDomRunner, chrome_dom_runner};
use crate::request::SessionRequest;
use crate::screenshots::find_chrome;
use crate::sessions::{RunOptions, SessionStore};
use crate::test_support::{FakeModelServer, SAMPLE_DESIGN};

fn design() -> Design {
    let mut design: Design = serde_json::from_str(SAMPLE_DESIGN).unwrap();
    // Browser layout tests must not depend on Google's font service.
    design.theme.fonts.heading = "Arial".to_owned();
    design.theme.fonts.body = "Arial".to_owned();
    design.theme.fonts.mono = "Courier New".to_owned();
    design.screens[0].html = "<h1>Broken</h1>".to_owned();
    design
}

fn context() -> ArtifactRequest<'static, Design> {
    ArtifactRequest {
        effort: "low".to_owned(),
        label: "test demo".to_owned(),
        parse: Box::new(|text| serde_json::from_str(text).map_err(|error| error.to_string())),
        progress: None,
        live: None,
    }
}

fn engine(fake: &FakeModelServer, directory: &TempDir, runner: DemoDomRunner) -> GenerationEngine {
    GenerationEngine::new(
        fake.configuration(),
        DesignStore::new(directory.path().join("designs")),
        SessionStore::new(directory.path().join("sessions")),
        None,
        "http://127.0.0.1:3000".to_owned(),
        ChangeNotifier::new(),
    )
    .with_demo_dom_runner(runner)
}

fn log() -> LogSink {
    Arc::new(|_| {})
}

fn report(design: &Design, findings: serde_json::Value) -> String {
    let coverage: Vec<usize> = (0..design.screens.len()).collect();
    format!(
        "<html data-swift-design-findings=\"{}\" data-swift-design-audited-screens=\"{}\">",
        findings.to_string().replace('"', "&quot;"),
        serde_json::to_string(&coverage).unwrap()
    )
}

fn changing_browser(measured: Arc<Mutex<Vec<Design>>>, cosmetic_warnings: bool) -> DemoDomRunner {
    Arc::new(move |design, _| {
        measured.lock().unwrap().push(design.clone());
        let findings = if design.screens[0].html.contains("Broken") {
            serde_json::json!([{"screen":0,"kind":"overflow","node":"h1","detail":"clipped text"}])
        } else if cosmetic_warnings {
            serde_json::json!([{"screen":0,"kind":"contrast","detail":"low contrast"},{"screen":0,"kind":"tiny_text","detail":"small label"}])
        } else {
            serde_json::json!([])
        };
        let dom = report(&design, findings);
        Box::pin(async move { Ok(dom) })
    })
}

#[tokio::test]
async fn the_last_allowed_tweak_is_measured_and_the_exact_returned_version_is_rechecked() {
    let directory = TempDir::new().unwrap();
    let fake = FakeModelServer::start().await;
    fake.push_text(r#"{"screens":[{"index":0,"screen":{"html":"<h1>Fixed</h1>"}}]}"#);
    let measured = Arc::new(Mutex::new(Vec::new()));
    let engine = engine(
        &fake,
        &directory,
        changing_browser(Arc::clone(&measured), false),
    );
    let result = engine
        .polish_design(&reqwest::Client::new(), design(), &context(), &log())
        .await
        .unwrap();
    assert_eq!(result.screens[0].html, "<h1>Fixed</h1>");
    assert_eq!(fake.requests().len(), 1);
    let measured = measured.lock().unwrap();
    assert_eq!(measured.len(), 3);
    assert_eq!(measured.last().unwrap(), &result);
    assert!(fake.requests()[0].to_string().contains("clipped text"));
}

#[tokio::test]
async fn unresolved_layout_errors_block_completion_when_the_tweak_budget_runs_out() {
    let directory = TempDir::new().unwrap();
    let fake = FakeModelServer::start().await;
    fake.push_text("{}");
    let measured = Arc::new(Mutex::new(Vec::new()));
    let engine = engine(&fake, &directory, changing_browser(measured, false));
    let error = engine
        .polish_design(&reqwest::Client::new(), design(), &context(), &log())
        .await
        .unwrap_err();
    assert!(error.contains("Layout verification failed"));
    assert_eq!(fake.requests().len(), 1);
}

#[tokio::test]
async fn cosmetic_warnings_cannot_outweigh_fixing_a_hard_layout_error() {
    let directory = TempDir::new().unwrap();
    let fake = FakeModelServer::start().await;
    fake.push_text(r#"{"screens":[{"index":0,"screen":{"html":"<h1>Fixed</h1>"}}]}"#);
    let runner = changing_browser(Arc::new(Mutex::new(Vec::new())), true);
    let result = engine(&fake, &directory, runner)
        .polish_design(&reqwest::Client::new(), design(), &context(), &log())
        .await
        .unwrap();
    assert_eq!(result.screens[0].html, "<h1>Fixed</h1>");
}

#[tokio::test]
async fn unavailable_or_incomplete_browser_audits_never_count_as_clean() {
    for response in [
        Err("Chrome unavailable".to_owned()),
        Ok("<html data-swift-design-findings=\"[]\">".to_owned()),
    ] {
        let directory = TempDir::new().unwrap();
        let fake = FakeModelServer::start().await;
        let runner: DemoDomRunner = Arc::new(move |_, _| {
            let response = response.clone();
            Box::pin(async move { response })
        });
        assert!(
            engine(&fake, &directory, runner)
                .polish_design(&reqwest::Client::new(), design(), &context(), &log())
                .await
                .is_err()
        );
        assert!(fake.requests().is_empty());
    }
}

#[tokio::test]
async fn a_failed_final_recheck_cannot_complete_an_initially_clean_design() {
    let directory = TempDir::new().unwrap();
    let fake = FakeModelServer::start().await;
    let count = Arc::new(Mutex::new(0));
    let runner: DemoDomRunner = Arc::new(move |design, _| {
        let mut count = count.lock().unwrap();
        *count += 1;
        let response = if *count == 1 {
            Ok(report(&design, serde_json::json!([])))
        } else {
            Err("browser timed out".to_owned())
        };
        Box::pin(async move { response })
    });
    assert!(
        engine(&fake, &directory, runner)
            .polish_design(&reqwest::Client::new(), design(), &context(), &log())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn a_focused_repair_verifies_screen_three_without_rewriting_existing_errors_on_screen_one() {
    let directory = TempDir::new().unwrap();
    let fake = FakeModelServer::start().await;
    fake.push_text(r#"{"screens":[{"index":2,"screen":{"html":"<h1>Fixed</h1>"}}]}"#);
    let mut original = design();
    original.screens[2].html = "<h1>Broken</h1>".to_owned();
    let runner: DemoDomRunner = Arc::new(|design, _| {
        let mut findings =
            vec![serde_json::json!({"screen":0,"kind":"overflow","detail":"existing clipping"})];
        if design.screens[2].html.contains("Broken") {
            findings.push(
                serde_json::json!({"screen":2,"kind":"overflow","detail":"screen three clipping"}),
            );
        }
        let dom = report(&design, serde_json::json!(findings));
        Box::pin(async move { Ok(dom) })
    });
    let engine = engine(&fake, &directory, runner);
    let request = SessionRequest {
        request: "Page 3 is still overflowed.".to_owned(),
        conversation: String::new(),
        kind: ArtifactKind::Demo,
        answers: Vec::new(),
        options: RunOptions::default(),
    };
    let context = context();
    let fix = EditFix {
        request: &request,
        context: &context,
        indexes: vec![2],
    };
    let result = engine
        .fix_edited_design(&reqwest::Client::new(), original.clone(), &fix, &log())
        .await
        .unwrap();
    assert_eq!(result.screens[2].html, "<h1>Fixed</h1>");
    assert_eq!(result.screens[0], original.screens[0]);
    assert_eq!(fake.requests().len(), 1);
}

#[tokio::test]
async fn a_failed_layout_repair_preserves_the_provider_cause_and_element() {
    let directory = TempDir::new().unwrap();
    let fake = FakeModelServer::start().await;
    for _ in 0..4 {
        fake.push_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Our servers are currently overloaded. Please try again later.",
        );
    }
    let runner: DemoDomRunner = Arc::new(|design, _| {
        let dom = report(
            &design,
            serde_json::json!([{"screen":2,"node":"a.control (0/2)","kind":"overflow","detail":"content needs 41px tall but the box is 38px tall"}]),
        );
        Box::pin(async move { Ok(dom) })
    });
    let engine = engine(&fake, &directory, runner);
    let request = SessionRequest {
        request: "Page 3 is still overflowed.".to_owned(),
        conversation: String::new(),
        kind: ArtifactKind::Demo,
        answers: Vec::new(),
        options: RunOptions::default(),
    };
    let context = context();
    let fix = EditFix {
        request: &request,
        context: &context,
        indexes: vec![2],
    };
    let error = engine
        .fix_edited_design(&reqwest::Client::new(), design(), &fix, &log())
        .await
        .unwrap_err();
    assert!(error.contains("The layout repair could not finish"));
    assert!(error.contains("servers are currently overloaded"));
    assert!(error.contains("a.control (0/2)"));
    assert!(error.contains("41px tall but the box is 38px tall"));
    assert_eq!(fake.requests().len(), 4);
    assert!(error.contains("automatic retry budget exhausted"));
}

#[tokio::test]
async fn a_temporary_failure_retries_the_repair_without_restarting_the_edit() {
    let directory = TempDir::new().unwrap();
    let fake = FakeModelServer::start().await;
    fake.push_error(StatusCode::SERVICE_UNAVAILABLE, "overloaded");
    fake.push_text(r#"{"screens":[{"index":2,"screen":{"html":"<h1>Fixed</h1>"}}]}"#);
    let mut original = design();
    original.screens[2].html = "<h1>Broken</h1>".to_owned();
    let runner: DemoDomRunner = Arc::new(|design, _| {
        let findings = if design.screens[2].html.contains("Broken") {
            serde_json::json!([{"screen":2,"kind":"overflow","detail":"clipping"}])
        } else {
            serde_json::json!([])
        };
        let dom = report(&design, findings);
        Box::pin(async move { Ok(dom) })
    });
    let engine = engine(&fake, &directory, runner);
    let request = SessionRequest {
        request: "Page 3".to_owned(),
        conversation: String::new(),
        kind: ArtifactKind::Demo,
        answers: Vec::new(),
        options: RunOptions::default(),
    };
    let context = context();
    let fix = EditFix {
        request: &request,
        context: &context,
        indexes: vec![2],
    };
    let result = engine
        .fix_edited_design(&reqwest::Client::new(), original.clone(), &fix, &log())
        .await
        .unwrap();
    assert_eq!(result.screens[2].html, "<h1>Fixed</h1>");
    assert_eq!(result.screens[0], original.screens[0]);
    let requests = fake.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0], requests[1]);
}

#[tokio::test]
async fn real_chrome_distinguishes_padding_only_scroll_extents_from_three_pixel_visible_clipping() {
    if find_chrome().is_none() {
        return;
    }
    let mut design = design();
    design.screens[0].html = "<a class='control' href='#screen-1'>Account</a>".to_owned();
    design.screens[0].css = Some(".control {display:inline-block;height:40px;padding:11px 18px;border:1px solid black;line-height:19px;font-size:16px;box-sizing:border-box;overflow:hidden;}".to_owned());
    design.screens[1].html =
        "<div class='clip'><div class='tall'>Actual clipping</div></div>".to_owned();
    design.screens[1].css = Some(".clip {width:200px;height:40px;border:1px solid black;overflow:hidden;box-sizing:border-box;} .tall {height:41px;background:black;color:white;font-size:16px;line-height:19px;}".to_owned());
    let dom = chrome_dom_runner()(design, "http://127.0.0.1:3000".to_owned())
        .await
        .unwrap();
    let audit = DemoAudit::from_dom(&dom, 3).unwrap();
    assert!(
        audit.require_layout_clean_on(Some(&[0])).is_ok(),
        "{:?}",
        audit.instructions(Some(&[0]))
    );
    let error = audit.require_layout_clean_on(Some(&[1])).unwrap_err();
    assert!(
        error.contains("41px tall but the box is 38px tall"),
        "{error}"
    );
    assert!(error.contains("div.clip"));
}

#[tokio::test]
async fn real_chrome_audits_inactive_transition_screens_instead_of_reporting_them_as_empty() {
    if find_chrome().is_none() {
        return;
    }
    let mut design = design();
    design.transition = Some(Transition::default());
    for screen in &mut design.screens {
        screen.html = "<h1>Visible page content</h1>".to_owned();
        screen.css = Some("h1 {font-size:20px;}".to_owned());
    }
    design.screens[2].html =
        "<div class='clip'>AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA</div>".to_owned();
    design.screens[2].css = Some(
        ".clip {width:80px;height:30px;overflow:hidden;white-space:nowrap;font-size:16px;}"
            .to_owned(),
    );
    let dom = chrome_dom_runner()(design, "http://127.0.0.1:3000".to_owned())
        .await
        .unwrap();
    let audit = DemoAudit::from_dom(&dom, 3).unwrap();
    assert_eq!(audit.score(Some(&[0])).0, 0);
    assert_eq!(audit.score(Some(&[1])).0, 0);
    assert!(audit.require_layout_clean_on(Some(&[2])).is_err());
    assert!(
        audit
            .instructions(Some(&[2]))
            .iter()
            .any(|finding| finding.contains("wide but the box"))
    );
    assert!(
        !audit
            .instructions(None)
            .iter()
            .any(|finding| finding.contains("no visible text"))
    );
}

#[tokio::test]
async fn real_chrome_distinguishes_clipping_and_uneven_gaps_from_scrolling_and_hidden_menus() {
    if find_chrome().is_none() {
        return;
    }
    let cases = [
        (
            "<div class='clip'>AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA</div>",
            ".clip {position:absolute;left:40px;top:40px;width:80px;height:30px;white-space:nowrap;overflow:hidden;font-size:16px;}",
            false,
        ),
        (
            "<div class='shell'><div class='scroll'><p>First</p><p class='later'>Later</p></div></div>",
            ".shell {width:1440px;height:900px;overflow:hidden;} .scroll {width:600px;height:400px;overflow:auto;} p {margin:0;font-size:16px;} .later {margin-top:1200px;}",
            true,
        ),
        (
            "<details><summary>Menu</summary><p class='hidden'>Hidden menu</p></details>",
            "details {font-size:16px;} .hidden {position:absolute;top:1200px;}",
            true,
        ),
        (
            "<div class='row' data-layout-gap='x'><p>One</p><p>Two</p><p>Three</p></div>",
            ".row {position:absolute;left:40px;top:40px;width:800px;height:40px;} p {position:absolute;width:50px;margin:0;font-size:16px;} p:nth-child(2) {left:80px;} p:nth-child(3) {left:200px;}",
            false,
        ),
    ];
    for (html, css, should_pass) in cases {
        let mut design = design();
        design.screens.truncate(1);
        design.screens[0].html = html.to_owned();
        design.screens[0].css = Some(css.to_owned());
        let dom = chrome_dom_runner()(design, "http://127.0.0.1:3000".to_owned())
            .await
            .unwrap();
        let audit = DemoAudit::from_dom(&dom, 1).unwrap();
        assert_eq!(
            audit.require_layout_clean().is_ok(),
            should_pass,
            "{html}: {:?}",
            audit.instructions(None)
        );
    }
}

#[tokio::test]
async fn real_chrome_measures_alignment_then_verifies_the_models_css_tweak() {
    if find_chrome().is_none() {
        return;
    }
    let directory = TempDir::new().unwrap();
    let fake = FakeModelServer::start().await;
    let mut design = design();
    design.screens.truncate(1);
    design.screens[0].html =
        "<div class='stack' data-layout-align='left'><p>First</p><p>Second</p></div>".to_owned();
    let css = ".stack { position:absolute;left:40px;top:40px;width:600px;display:grid;gap:16px;color:#111;background:#fff; } p {margin:0;font-size:16px;line-height:24px;}";
    design.screens[0].css = Some(format!("{css} p:nth-child(2) {{margin-left:20px;}}"));
    fake.push_text(&serde_json::json!({"screens":[{"index":0,"screen":{"html":design.screens[0].html,"css":css}}]}).to_string());
    let engine = engine(&fake, &directory, chrome_dom_runner());
    let result = engine
        .polish_design(&reqwest::Client::new(), design, &context(), &log())
        .await
        .unwrap();
    assert_eq!(result.screens[0].css.as_deref(), Some(css));
    assert_eq!(fake.requests().len(), 1);
    assert!(fake.requests()[0].to_string().contains("alignment"));
}
