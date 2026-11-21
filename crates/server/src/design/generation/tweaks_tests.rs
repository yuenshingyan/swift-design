//! Exercises verified tweaking through real model and browser boundaries.

use std::sync::{Arc, Mutex};

use axum::http::StatusCode;
use tempfile::TempDir;

use design_model::{ArtifactKind, Design, Transition};

use crate::design::core::DesignStore;
use crate::design::polish::{DemoAudit, DemoDomRunner, chrome_dom_runner};
use crate::edit_focus::EditFix;
use crate::events::ChangeNotifier;
use crate::generation::{ArtifactRequest, GenerationEngine};
use crate::model_client::LogSink;
use crate::request::SessionRequest;
use crate::screenshots::find_chrome;
use crate::sessions::{RunOptions, SessionStore};
use crate::test_support::{FakeModelServer, SAMPLE_DESIGN};

fn design() -> Design {
    let mut design: Design =
        serde_json::from_str(SAMPLE_DESIGN).expect("test JSON should deserialize");
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
        serde_json::to_string(&coverage).expect("test value should serialize to JSON")
    )
}

fn changing_browser(measured: Arc<Mutex<Vec<Design>>>, cosmetic_warnings: bool) -> DemoDomRunner {
    Arc::new(move |design, _| {
        measured
            .lock()
            .expect("test mutex should not be poisoned")
            .push(design.clone());
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
    let directory = TempDir::new().expect("temporary test directory should be created");
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
        .expect("test design should complete visual review");
    assert_eq!(result.screens[0].html, "<h1>Fixed</h1>");
    assert_eq!(fake.requests().len(), 1);
    let measured = measured.lock().expect("test mutex should not be poisoned");
    assert_eq!(measured.len(), 3);
    assert_eq!(
        measured
            .last()
            .expect("test collection should contain a last item"),
        &result
    );
    assert!(fake.requests()[0].to_string().contains("clipped text"));
}

#[tokio::test]
async fn unresolved_layout_errors_block_completion_when_the_tweak_budget_runs_out() {
    let directory = TempDir::new().expect("temporary test directory should be created");
    let fake = FakeModelServer::start().await;
    fake.push_text("{}");
    let measured = Arc::new(Mutex::new(Vec::new()));
    let engine = engine(&fake, &directory, changing_browser(measured, false));
    let error = engine
        .polish_design(&reqwest::Client::new(), design(), &context(), &log())
        .await
        .expect_err("unresolved layout errors should block completion after the tweak budget");
    assert!(error.contains("Layout verification failed"));
    assert_eq!(fake.requests().len(), 1);
}

#[tokio::test]
async fn cosmetic_warnings_cannot_outweigh_fixing_a_hard_layout_error() {
    let directory = TempDir::new().expect("temporary test directory should be created");
    let fake = FakeModelServer::start().await;
    fake.push_text(r#"{"screens":[{"index":0,"screen":{"html":"<h1>Fixed</h1>"}}]}"#);
    let runner = changing_browser(Arc::new(Mutex::new(Vec::new())), true);
    let result = engine(&fake, &directory, runner)
        .polish_design(&reqwest::Client::new(), design(), &context(), &log())
        .await
        .expect("test design should complete visual review");
    assert_eq!(result.screens[0].html, "<h1>Fixed</h1>");
}

#[tokio::test]
async fn unavailable_or_incomplete_browser_audits_never_count_as_clean() {
    for response in [
        Err("Chrome unavailable".to_owned()),
        Ok("<html data-swift-design-findings=\"[]\">".to_owned()),
    ] {
        let directory = TempDir::new().expect("temporary test directory should be created");
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
    let directory = TempDir::new().expect("temporary test directory should be created");
    let fake = FakeModelServer::start().await;
    let count = Arc::new(Mutex::new(0));
    let runner: DemoDomRunner = Arc::new(move |design, _| {
        let mut count = count.lock().expect("test mutex should not be poisoned");
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
    let directory = TempDir::new().expect("temporary test directory should be created");
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
        .expect("valid test edit should pass correction");
    assert_eq!(result.0.screens[2].html, "<h1>Fixed</h1>");
    assert_eq!(result.0.screens[0], original.screens[0]);
    assert_eq!(fake.requests().len(), 1);
}

#[tokio::test]
async fn one_tweak_round_repairs_blocked_screens_concurrently_and_audits_the_merge() {
    let directory = TempDir::new().expect("temporary test directory should be created");
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let provider = axum::Router::new().route(
        "/chat",
        axum::routing::post(move |axum::Json(body): axum::Json<serde_json::Value>| {
            let barrier = Arc::clone(&barrier);
            async move {
                let prompt = body["messages"][1]["content"]
                    .as_str()
                    .expect("the repair should send a text prompt");
                let first = prompt.contains("first repair evidence");
                let third = prompt.contains("third repair evidence");
                assert_ne!(first, third, "each worker should receive only its own finding");
                assert!(!prompt.contains("untouched clipping"));
                barrier.wait().await;
                let index = if first { 0 } else { 2 };
                let patch = serde_json::json!({"screens": [{"index": index, "screen": {"html": format!("<h1>Fixed {index}</h1>")}}]});
                axum::Json(serde_json::json!({ "choices": [{ "message": { "content": patch.to_string() }}] }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("test provider should listen on an available port");
    let address = listener
        .local_addr()
        .expect("test provider should expose its address");
    let provider_task = tokio::spawn(async move { axum::serve(listener, provider).await });
    let runner: DemoDomRunner = Arc::new(|design, _| {
        let mut findings =
            vec![serde_json::json!({"screen":1,"kind":"overflow","detail":"untouched clipping"})];
        for (index, detail) in [(0, "first repair evidence"), (2, "third repair evidence")] {
            if !design.screens[index].html.contains("Fixed") {
                findings
                    .push(serde_json::json!({"screen":index,"kind":"overflow","detail":detail}));
            }
        }
        let dom = report(&design, serde_json::json!(findings));
        Box::pin(async move { Ok(dom) })
    });
    let engine = GenerationEngine::new(
        crate::model_client::ModelConfiguration {
            provider: "fake".to_owned(),
            chat_url: format!("http://{address}/chat"),
            auth: crate::model_client::ProviderAuth::None,
            wire: crate::model_client::WireFormat::OpenAiChat,
            model: "fake-model".to_owned(),
        },
        DesignStore::new(directory.path().join("designs")),
        SessionStore::new(directory.path().join("sessions")),
        None,
        "http://127.0.0.1:3000".to_owned(),
        ChangeNotifier::new(),
    )
    .with_demo_dom_runner(runner);
    let original = design();
    let fix = EditFix {
        request: &SessionRequest {
            request: "Fix clipping on the first and third screens".to_owned(),
            conversation: String::new(),
            kind: ArtifactKind::Demo,
            answers: Vec::new(),
            options: RunOptions::default(),
        },
        context: &context(),
        indexes: vec![0, 2],
    };
    let (merged, audit) = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        engine.fix_edited_design(&reqwest::Client::new(), original.clone(), &fix, &log()),
    )
    .await
    .expect("both requests must reach the barrier in the same round")
    .expect("the merged design should complete layout verification");
    provider_task.abort();
    assert_eq!(merged.screens[0].html, "<h1>Fixed 0</h1>");
    assert_eq!(merged.screens[2].html, "<h1>Fixed 2</h1>");
    assert_eq!(merged.screens[1], original.screens[1]);
    assert_eq!(merged.theme, original.theme);
    assert!(audit.require_layout_clean_on(Some(&fix.indexes)).is_ok());
    assert!(audit.require_layout_clean_on(None).is_err());
}

#[tokio::test]
async fn a_failed_repair_keeps_usable_edits_but_blocks_unusable_edits() {
    for kind in ["overflow", "empty"] {
        let directory = TempDir::new().expect("temporary test directory should be created");
        let fake = FakeModelServer::start().await;
        for _ in 0..4 {
            fake.push_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "Our servers are currently overloaded. Please try again later.",
            );
        }
        let runner: DemoDomRunner = Arc::new(move |design, _| {
            let dom = report(
                &design,
                serde_json::json!([{"screen":2,"node":"a.control (0/2)","kind":kind,"detail":"content needs 41px tall but the box is 38px tall"}]),
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
        let original = design();
        let result = engine
            .fix_edited_design(&reqwest::Client::new(), original.clone(), &fix, &log())
            .await;
        if kind == "empty" {
            let error = result
                .expect_err("failed repair of an unusable edit should report the provider error");
            assert!(error.contains("The layout repair could not finish"));
            assert!(error.contains("servers are currently overloaded"));
            assert!(error.contains("a.control (0/2)"));
            assert!(error.contains("automatic retry budget exhausted"));
        } else {
            let (kept, audit) = result.expect("failed optional polish should keep the usable edit");
            assert_eq!(kept, original);
            assert!(
                audit.instructions(Some(&[2]))[0].contains("41px tall but the box is 38px tall")
            );
        }
        assert_eq!(fake.requests().len(), 4);
    }
}

#[tokio::test]
async fn layout_repair_continues_when_the_same_overflow_gets_smaller() {
    let directory = TempDir::new().expect("temporary test directory should be created");
    let fake = FakeModelServer::start().await;
    fake.push_text(r#"{"screens":[{"index":2,"screen":{"html":"<h1>Closer</h1>"}}]}"#);
    fake.push_text(r#"{"screens":[{"index":2,"screen":{"html":"<h1>Fits</h1>"}}]}"#);
    let mut original = design();
    original.screens[2].html = "<h1>Overflow</h1>".into();
    let runner: DemoDomRunner = Arc::new(|design, _| {
        let excess = match design.screens[2].html.as_str() {
            "<h1>Overflow</h1>" => 43,
            "<h1>Closer</h1>" => 28,
            _ => 0,
        };
        let findings = if excess == 0 {
            serde_json::json!([])
        } else {
            serde_json::json!([{"screen":2,"node":"div.scrim (0/3)","kind":"overflow","overflow_px":[0,excess],
                "detail":format!("content needs {}px tall but the box is 806px tall",806+excess)}])
        };
        let dom = report(&design, findings);
        Box::pin(async move { Ok(dom) })
    });
    let engine = engine(&fake, &directory, runner);
    let request = SessionRequest {
        request: "Make the dialog fit".into(),
        conversation: String::new(),
        kind: ArtifactKind::Demo,
        answers: Vec::new(),
        options: RunOptions::default(),
    };
    let context = ArtifactRequest {
        effort: "medium".into(),
        ..context()
    };
    let fix = EditFix {
        request: &request,
        context: &context,
        indexes: vec![2],
    };
    let (fixed, audit) = engine
        .fix_edited_design(&reqwest::Client::new(), original.clone(), &fix, &log())
        .await
        .expect("a smaller overflow should get another repair round");
    assert_eq!(fixed.screens[2].html, "<h1>Fits</h1>");
    assert_eq!(fixed.screens[..2], original.screens[..2]);
    assert!(audit.require_layout_clean().is_ok());
    let requests = fake.requests();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[0]
            .to_string()
            .contains("let the body scroll inside the dialog")
    );
    assert!(
        requests[1]
            .to_string()
            .contains("834px tall but the box is 806px tall")
    );
}

#[tokio::test]
async fn a_temporary_failure_retries_the_repair_without_restarting_the_edit() {
    let directory = TempDir::new().expect("temporary test directory should be created");
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
        .expect("valid test edit should pass correction");
    assert_eq!(result.0.screens[2].html, "<h1>Fixed</h1>");
    assert_eq!(result.0.screens[0], original.screens[0]);
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
        .expect("Chrome should produce the fixture DOM audit");
    let audit =
        DemoAudit::from_dom(&dom, 3).expect("fixture DOM should contain a valid audit result");
    assert!(
        audit.require_layout_clean_on(Some(&[0])).is_ok(),
        "{:?}",
        audit.instructions(Some(&[0]))
    );
    assert!(audit.require_layout_clean_on(Some(&[1])).is_ok());
    let warnings = audit.instructions(Some(&[1])).join("; ");
    assert!(
        warnings.contains("41px tall but the box is 38px tall"),
        "{warnings}"
    );
    assert!(warnings.contains("div.clip"));
    assert!(dom.contains("&quot;overflow_px&quot;:[0,3]"));
    assert_eq!(audit.score(Some(&[1])).0, 0);
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
        .expect("Chrome should produce the fixture DOM audit");
    let audit =
        DemoAudit::from_dom(&dom, 3).expect("fixture DOM should contain a valid audit result");
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
            true,
        ),
    ];
    for (html, css, should_pass) in cases {
        let mut design = design();
        design.screens.truncate(1);
        design.screens[0].html = html.to_owned();
        design.screens[0].css = Some(css.to_owned());
        let dom = chrome_dom_runner()(design, "http://127.0.0.1:3000".to_owned())
            .await
            .expect("Chrome should produce the fixture DOM audit");
        let audit =
            DemoAudit::from_dom(&dom, 1).expect("fixture DOM should contain a valid audit result");
        assert_eq!(
            audit.require_layout_clean().is_ok(),
            should_pass,
            "{html}: {:?}",
            audit.instructions(None)
        );
    }
}

#[tokio::test]
async fn real_chrome_marks_small_hidden_and_scrolling_groups_as_unmeasured_not_broken() {
    if find_chrome().is_none() {
        return;
    }
    let mut design = design();
    design.screens[0].html =
        "<div class='group' data-layout-gap='y'><p>One</p><p>Two</p></div>".into();
    design.screens[1].html =
        "<div class='group' data-layout-gap='y'><p>One</p><p>Two</p><p hidden>Hidden</p></div>"
            .into();
    design.screens[2].html = "<div class='group scroll' data-layout-gap='y'><p>One</p><p>Two</p><p class='later'>Below the canvas</p></div>".into();
    for screen in &mut design.screens {
        screen.css = Some(".group{display:grid;gap:16px;width:400px;}p{margin:0;font-size:16px;line-height:24px;}.scroll{display:block;height:800px;overflow:auto;}.later{margin-top:1000px;}".into());
    }
    let dom = chrome_dom_runner()(design, "http://127.0.0.1:3000".into())
        .await
        .expect("Chrome should produce the fixture DOM audit");
    let audit =
        DemoAudit::from_dom(&dom, 3).expect("fixture DOM should contain a valid audit result");
    assert!(
        audit.require_layout_clean().is_ok(),
        "{:?}",
        audit.instructions(None)
    );
    for index in 0..3 {
        assert!(
            audit
                .instructions(Some(&[index]))
                .join(" ")
                .contains("cannot be measured with fewer than 3")
        );
    }
}

#[tokio::test]
async fn real_chrome_measures_alignment_then_verifies_the_models_css_tweak() {
    if find_chrome().is_none() {
        return;
    }
    let directory = TempDir::new().expect("temporary test directory should be created");
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
        .expect("test design should complete visual review");
    assert_eq!(result.screens[0].css.as_deref(), Some(css));
    assert_eq!(fake.requests().len(), 1);
    assert!(fake.requests()[0].to_string().contains("alignment"));
}
