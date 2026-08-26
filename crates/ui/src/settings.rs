//! The model setup panel and shared studio helpers.
//!
//! `SettingsPanel` is the three-step wizard that chooses a provider, an
//! access method (API key or login), and a model. The small helpers
//! (`stepped_screen`, `design_project`, `pause_briefly`) are shared by
//! the session workspace and the canvas.

use dioxus::document;
use dioxus::prelude::*;

use crate::api;
use crate::select::Select;

/// The screen a card shows after one arrow press: `step` of `-1` or
/// `1` from `current`, clamped to `1..=count`.
pub(crate) fn stepped_screen(current: usize, step: i32, count: usize) -> usize {
    let count = count.max(1);
    let next = if step < 0 {
        current.saturating_sub(1)
    } else {
        current + 1
    };
    next.clamp(1, count)
}

/// Waits two seconds. Used to back off after a failed poll.
pub(crate) async fn pause_briefly() {
    let mut sleeper = document::eval("setTimeout(() => dioxus.send(0), 2000);");
    let _ = sleeper.recv::<i32>().await;
}

/// The project a design belongs to: its id up to any candidate suffix.
pub(crate) fn design_project(id: &str) -> String {
    match id.find("-candidate-") {
        Some(position) => id[..position].to_owned(),
        None => id.to_owned(),
    }
}

/// One row of the model list: the catalog entry, plus whether the key
/// can reach the model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ModelRow {
    /// Model id sent to the provider.
    pub id: String,
    /// One short line that tells the user when to pick this model.
    /// Empty for a model the catalog does not list.
    pub description: String,
    /// True for the model the panel selects first.
    pub is_recommended: bool,
    /// True when the live model list holds this id, or when no live
    /// list exists.
    pub is_available: bool,
}

/// Builds the model rows from the catalog and the live model list.
///
/// An empty live list means the fetch did not run, so every catalog
/// model stays available: the panel must not claim a fact it does not
/// have. Order is the recommended model, then catalog order, then the
/// live models the catalog omits, then every unavailable model.
pub(crate) fn model_rows(catalog: &[api::CatalogModel], live: &[String]) -> Vec<ModelRow> {
    let has_live_list = !live.is_empty();
    let mut rows: Vec<ModelRow> = catalog
        .iter()
        .map(|model| ModelRow {
            id: model.id.clone(),
            description: model.description.clone(),
            is_recommended: model.is_recommended,
            is_available: !has_live_list || live.contains(&model.id),
        })
        .collect();
    for id in live {
        if catalog.iter().any(|model| &model.id == id) {
            continue;
        }
        rows.push(ModelRow {
            id: id.clone(),
            description: String::new(),
            is_recommended: false,
            is_available: true,
        });
    }
    // Sort keys only, so catalog order survives inside each group.
    rows.sort_by_key(|row| (!row.is_available, !row.is_recommended));
    rows
}

/// The model the panel selects first: the recommended one when the key
/// can reach it, else the first row it can reach.
fn first_choice(rows: &[ModelRow]) -> Option<String> {
    rows.iter()
        .find(|row| row.is_available)
        .map(|row| row.id.clone())
}

/// Every signal the three setup steps share.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct SetupState {
    /// Which step the panel shows: 1, 2, or 3.
    step: Signal<u8>,
    /// The chosen provider name.
    provider_name: Signal<String>,
    /// The API key the user typed.
    api_key: Signal<String>,
    /// True while the key field shows the key as plain text.
    is_key_shown: Signal<bool>,
    /// True once the provider answered a request made with the key.
    is_key_verified: Signal<bool>,
    /// The chosen model id.
    model: Signal<String>,
    /// A model id typed by hand. It overrides the list.
    custom_model: Signal<String>,
    /// True while the custom model field is open.
    is_custom_model_open: Signal<bool>,
    /// The model ids the provider returned. Empty before a fetch.
    loaded_models: Signal<Vec<String>>,
    /// True once the model step asked the provider for its models.
    has_tried_loading_models: Signal<bool>,
    /// The login page URL, once a login started.
    login_url: Signal<Option<String>>,
    /// The code the user pasted back from the login page.
    login_code: Signal<String>,
    /// The error to show under the form.
    message: Signal<Option<String>>,
    /// True while a request is in flight.
    is_busy: Signal<bool>,
    /// True when the user pressed Next and the panel owes them the
    /// model step once the running key check succeeds.
    is_advance_requested: Signal<bool>,
}

impl SetupState {
    /// Which step the panel shows.
    fn step(&self) -> u8 {
        (self.step)()
    }

    /// The chosen provider name.
    fn provider_name(&self) -> String {
        (self.provider_name)()
    }

    /// The API key the user typed.
    fn api_key(&self) -> String {
        (self.api_key)()
    }

    /// True while the key field shows the key as plain text.
    fn is_key_shown(&self) -> bool {
        (self.is_key_shown)()
    }

    /// True once the provider answered a request made with the key.
    fn is_key_verified(&self) -> bool {
        (self.is_key_verified)()
    }

    /// The chosen model id.
    fn model(&self) -> String {
        (self.model)()
    }

    /// A model id typed by hand.
    fn custom_model(&self) -> String {
        (self.custom_model)()
    }

    /// True while the custom model field is open.
    fn is_custom_model_open(&self) -> bool {
        (self.is_custom_model_open)()
    }

    /// The model ids the provider returned.
    fn loaded_models(&self) -> Vec<String> {
        (self.loaded_models)()
    }

    /// True once the model step asked the provider for its models.
    fn has_tried_loading_models(&self) -> bool {
        (self.has_tried_loading_models)()
    }

    /// The login page URL, once a login started.
    fn login_url(&self) -> Option<String> {
        (self.login_url)()
    }

    /// The code the user pasted back from the login page.
    fn login_code(&self) -> String {
        (self.login_code)()
    }

    /// The error to show under the form.
    fn message(&self) -> Option<String> {
        (self.message)()
    }

    /// True while a request is in flight.
    fn is_busy(&self) -> bool {
        (self.is_busy)()
    }

    /// True when the user pressed Next during a running key check.
    fn is_advance_requested(&self) -> bool {
        (self.is_advance_requested)()
    }

    /// Creates the signals. Call it from the panel body only: it
    /// calls `use_signal`, so it obeys the hook rules.
    fn new() -> Self {
        Self {
            step: use_signal(|| 1u8),
            provider_name: use_signal(|| "google".to_owned()),
            api_key: use_signal(String::new),
            is_key_shown: use_signal(|| false),
            is_key_verified: use_signal(|| false),
            model: use_signal(String::new),
            custom_model: use_signal(String::new),
            is_custom_model_open: use_signal(|| false),
            loaded_models: use_signal(Vec::<String>::new),
            has_tried_loading_models: use_signal(|| false),
            login_url: use_signal(|| Option::<String>::None),
            login_code: use_signal(String::new),
            message: use_signal(|| Option::<String>::None),
            is_busy: use_signal(|| false),
            is_advance_requested: use_signal(|| false),
        }
    }

    /// Clears everything the old provider decided.
    fn reset_for_new_provider(&mut self, name: String) {
        self.provider_name.set(name);
        self.api_key.set(String::new());
        self.is_key_shown.set(false);
        self.is_key_verified.set(false);
        self.model.set(String::new());
        self.custom_model.set(String::new());
        self.is_custom_model_open.set(false);
        self.loaded_models.set(Vec::new());
        self.has_tried_loading_models.set(false);
        self.login_url.set(None);
        self.message.set(None);
        self.is_advance_requested.set(false);
    }
}

/// The model picker, as three steps: provider, access, model.
#[component]
pub(crate) fn SettingsPanel(
    settings: Signal<Option<api::SettingsView>>,
    is_configuring: Signal<bool>,
) -> Element {
    let mut state = SetupState::new();

    let providers = settings().map(|view| view.providers).unwrap_or_default();
    let Some(provider) = providers
        .iter()
        .find(|provider| provider.name == state.provider_name())
        .cloned()
        .or_else(|| providers.first().cloned())
    else {
        return rsx! {
            p { "Loading…" }
        };
    };

    // A finished login stores credentials server-side and /events
    // refreshes the settings; move on to the model step.
    use_effect(move || {
        let current = settings().and_then(|view| view.current);
        if state.step() == 2
            && state.login_url().is_some()
            && current.is_some_and(|current| {
                current.provider == state.provider_name() && current.auth != "none"
            })
        {
            state.login_url.set(None);
            state.is_key_verified.set(true);
            state.step.set(3);
        }
    });

    // Entering the model step without a verified key still needs the
    // list: a provider that needs no key, or one with saved
    // credentials, never passes through the key field.
    use_effect(move || {
        if state.step() == 3 && !state.has_tried_loading_models() {
            state.has_tried_loading_models.set(true);
            let provider = state.provider_name();
            let key = state.api_key();
            spawn(async move {
                let key = (!key.trim().is_empty()).then_some(key);
                match api::fetch_provider_models(&provider, key.as_deref()).await {
                    Ok(models) => {
                        state.loaded_models.set(models);
                        state.message.set(None);
                    }
                    Err(text) => state.message.set(Some(text)),
                }
            });
        }
    });

    let rows = model_rows(&provider.models, &state.loaded_models());
    if state.model().is_empty()
        && let Some(first) = first_choice(&rows)
    {
        state.model.set(first);
    }

    rsx! {
        div { class: "settings-panel",
            div { class: "settings-head",
                span { class: "kicker", "Set up" }
                SetupStepRail { step: state.step() }
                button {
                    class: "icon-button",
                    title: "Close",
                    onclick: move |_| is_configuring.set(false),
                    "×"
                }
            }
            div { class: "settings-form",
                if state.step() == 1 {
                    ProviderStep {
                        state,
                        providers: providers.clone(),
                        provider: provider.clone(),
                        settings,
                    }
                } else if state.step() == 2 {
                    AccessStep { state, provider: provider.clone() }
                } else {
                    ModelStep {
                        state,
                        provider: provider.clone(),
                        rows,
                        settings,
                        is_configuring,
                    }
                }
                if let Some(text) = state.message() {
                    p { class: "error", "{text}" }
                }
            }
        }
    }
}

/// The three step names, with a tick on every finished step.
#[component]
fn SetupStepRail(step: u8) -> Element {
    rsx! {
        div { class: "step-rail",
            for (number, name) in [(1u8, "Provider"), (2, "Access"), (3, "Model")] {
                if number > 1 {
                    span { class: "sep" }
                }
                span { class: if step == number { "step current" } else if step > number { "step done" } else { "step" },
                    span { class: "n",
                        if step > number {
                            "✓"
                        } else {
                            "{number}"
                        }
                    }
                    "{name}"
                }
            }
        }
    }
}

/// Step 1: pick the provider.
#[component]
fn ProviderStep(
    state: SetupState,
    providers: Vec<api::CatalogProvider>,
    provider: api::CatalogProvider,
    settings: Signal<Option<api::SettingsView>>,
) -> Element {
    let mut state = state;

    // A saved login or key for this provider skips the access step:
    // the model step loads the list with the stored credentials.
    let has_saved_credentials = settings()
        .and_then(|view| view.current)
        .is_some_and(|current| current.provider == provider.name && current.auth != "none");
    let needs_access_step = provider.needs_api_key && !has_saved_credentials;

    rsx! {
        div { class: "field provider-field",
            span { class: "field-label", "Provider" }
            Select {
                value: provider.name.clone(),
                options: providers
                    .iter()
                    .map(|entry| (entry.name.clone(), entry.label.clone()))
                    .collect::<Vec<_>>(),
                on_change: move |name| state.reset_for_new_provider(name),
            }
        }
        p { class: "agent-log", "runs on your own account · nothing leaves this machine" }
        div { class: "settings-actions",
            button {
                class: "primary",
                onclick: move |_| {
                    state.message.set(None);
                    state.step.set(if needs_access_step { 2 } else { 3 });
                },
                "Next"
            }
            if !provider.needs_api_key {
                span { class: "agent-log", "{provider.label} needs no sign-in." }
            }
            span { class: "step-count", "Step 1 of 3" }
        }
    }
}

/// Step 2: sign in, or paste an API key.
#[component]
fn AccessStep(state: SetupState, provider: api::CatalogProvider) -> Element {
    let mut state = state;
    let is_openrouter = provider.name == "openrouter";
    let is_openai = provider.name == "openai";
    let uses_callback_login = is_openrouter || is_openai;

    let get_login_link = move |_| {
        // Open a blank tab now, inside the click gesture, so the popup
        // blocker allows it. The tab is named, so the URL is loaded into
        // this same tab once the server returns it. Doing the open after
        // the async round trip would be blocked as a non-gesture popup.
        let _ = dioxus::document::eval("window.open('', 'swiftDesignLogin');");
        spawn(async move {
            let started = if is_openrouter {
                api::start_openrouter_login().await
            } else if is_openai {
                api::start_openai_login().await
            } else {
                api::start_login().await
            };
            match started {
                Ok(url) => {
                    // Point the already-open tab at the login page.
                    let opener = dioxus::document::eval(
                        "const url = await dioxus.recv(); window.open(url, 'swiftDesignLogin');",
                    );
                    let _ = opener.send(url.clone());
                    state.login_url.set(Some(url));
                    state.message.set(None);
                }
                Err(text) => state.message.set(Some(text)),
            }
        });
    };

    let finish_login = move |_| {
        let code = state.login_code();
        spawn(async move {
            match api::complete_login(&code, None).await {
                Ok(()) => {
                    state.login_url.set(None);
                    state.login_code.set(String::new());
                    state.message.set(None);
                    state.is_key_verified.set(true);
                    state.step.set(3);
                }
                Err(text) => state.message.set(Some(text)),
            }
        });
    };

    // The server has no verify route. Asking the provider for its model
    // list is the one round trip that proves the key works, and the
    // model step needs that list anyway. `should_advance` is false for
    // the blur check, which only reports whether the key works.
    let verify_key = use_callback(move |should_advance: bool| {
        if state.api_key().trim().is_empty() {
            if should_advance {
                state
                    .message
                    .set(Some("enter the API key first".to_owned()));
            }
            return;
        }
        if state.is_key_verified() {
            if should_advance {
                state.step.set(3);
            }
            return;
        }
        if should_advance {
            state.is_advance_requested.set(true);
        }
        // Leaving the field starts a check of its own, so a click on
        // Next lands while that check runs. Let the running check carry
        // the request instead of starting a second one.
        if state.is_busy() {
            return;
        }
        state.message.set(None);
        state.is_busy.set(true);
        let provider = state.provider_name();
        let key = state.api_key();
        spawn(async move {
            match api::fetch_provider_models(&provider, Some(&key)).await {
                Ok(models) => {
                    state.loaded_models.set(models);
                    state.has_tried_loading_models.set(true);
                    state.is_key_verified.set(true);
                    state.model.set(String::new());
                    if state.is_advance_requested() {
                        state.step.set(3);
                    }
                }
                Err(text) => {
                    state.is_key_verified.set(false);
                    state.message.set(Some(text));
                }
            }
            state.is_advance_requested.set(false);
            state.is_busy.set(false);
        });
    });

    rsx! {
        p { class: "provider-name", "{provider.label}" }
        if provider.supports_login {
            div { class: "settings-actions",
                button { class: "primary", onclick: get_login_link,
                    if is_openrouter {
                        "Log in with OpenRouter"
                    } else if is_openai {
                        "Log in with ChatGPT"
                    } else {
                        "Log in with Claude"
                    }
                }
            }
            if state.login_url().is_some() {
                div { class: "settings-login",
                    if uses_callback_login {
                        p { class: "agent-log",
                            "Finish in the new tab; this page moves on by itself."
                        }
                    } else {
                        label {
                            "Paste the code the login page shows"
                            input {
                                value: "{state.login_code()}",
                                oninput: move |event| state.login_code.set(event.value()),
                            }
                        }
                        button { class: "primary", onclick: finish_login, "Complete login" }
                    }
                }
            }
            div { class: "settings-divider", "or use an API key" }
        }
        div { class: "field",
            span { class: "field-label", "API key" }
            div { class: "key-field",
                input {
                    r#type: if state.is_key_shown() { "text" } else { "password" },
                    placeholder: "paste your {provider.label} API key",
                    value: "{state.api_key()}",
                    oninput: move |event| {
                        state.api_key.set(event.value());
                        state.is_key_verified.set(false);
                    },
                    onblur: move |_| verify_key.call(false),
                }
                button {
                    class: "link-button",
                    onclick: move |_| state.is_key_shown.toggle(),
                    if state.is_key_shown() {
                        "Hide"
                    } else {
                        "Show"
                    }
                }
            }
        }
        if state.is_key_verified() {
            p { class: "key-status", "✓ key verified · stored on this machine" }
        }
        p { class: "lede",
            "Swift Design calls the provider directly from this machine. The key stays "
            "in a local settings file that git ignores. It goes to no other service."
        }
        div { class: "settings-actions",
            button { class: "primary", onclick: move |_| verify_key.call(true),
                if state.is_busy() {
                    "Checking…"
                } else {
                    "Next"
                }
            }
            button {
                onclick: move |_| {
                    state.message.set(None);
                    state.step.set(1);
                },
                "Back"
            }
            span { class: "step-count", "Step 2 of 3" }
        }
    }
}

/// Step 3: pick the model, then save.
#[component]
fn ModelStep(
    state: SetupState,
    provider: api::CatalogProvider,
    rows: Vec<ModelRow>,
    settings: Signal<Option<api::SettingsView>>,
    is_configuring: Signal<bool>,
) -> Element {
    let mut state = state;
    let live_count = state.loaded_models().len();

    let has_saved_credentials = settings()
        .and_then(|view| view.current)
        .is_some_and(|current| current.provider == provider.name && current.auth != "none");
    let needs_access_step = provider.needs_api_key && !has_saved_credentials;

    let provider_for_save = provider.name.clone();
    let save = move |_| {
        let provider = provider_for_save.clone();
        let custom = state.custom_model();
        let model = if custom.trim().is_empty() {
            state.model()
        } else {
            custom.trim().to_owned()
        };
        let key = state.api_key();
        state.is_busy.set(true);
        spawn(async move {
            let key = (!key.trim().is_empty()).then_some(key);
            match api::save_settings(&provider, &model, key.as_deref()).await {
                Ok(()) => {
                    is_configuring.set(false);
                    state.message.set(None);
                }
                Err(text) => state.message.set(Some(text)),
            }
            state.is_busy.set(false);
        });
    };

    rsx! {
        div { class: "field",
            div { class: "field-heading",
                span { "Model" }
                span { class: "model-count",
                    if live_count > 0 {
                        "{live_count} available on this key"
                    } else {
                        "curated list"
                    }
                    button {
                        class: "link-button",
                        title: "Reload the model list from {provider.label}",
                        onclick: move |_| state.has_tried_loading_models.set(false),
                        "Reload"
                    }
                }
            }
            div { class: "model-list", role: "radiogroup",
                for row in rows.iter().cloned() {
                    button {
                        key: "{row.id}",
                        class: if state.model() == row.id { "model-option selected" } else { "model-option" },
                        role: "radio",
                        aria_checked: state.model() == row.id,
                        disabled: !row.is_available,
                        onclick: {
                            let id = row.id.clone();
                            move |_| state.model.set(id.clone())
                        },
                        span { class: "model-radio" }
                        span { class: "model-option-text",
                            span { class: "model-id", "{row.id}" }
                            if !row.is_available {
                                span { class: "model-desc", "Not enabled on this key." }
                            } else if !row.description.is_empty() {
                                span { class: "model-desc", "{row.description}" }
                            }
                        }
                        if row.is_recommended {
                            span { class: "badge", "Recommended" }
                        }
                    }
                }
            }
        }
        if state.is_custom_model_open() {
            label {
                "Custom model (overrides the list)"
                input {
                    value: "{state.custom_model()}",
                    oninput: move |event| state.custom_model.set(event.value()),
                }
            }
        } else {
            div {
                button {
                    class: "link-button",
                    onclick: move |_| state.is_custom_model_open.set(true),
                    "Use another model id"
                }
            }
        }
        div { class: "settings-actions",
            button { class: "primary", disabled: state.is_busy(), onclick: save,
                "Start using Swift Design"
            }
            button {
                onclick: move |_| {
                    state.message.set(None);
                    state.step.set(if needs_access_step { 2 } else { 1 });
                },
                "Back"
            }
            span { class: "step-count", "Step 3 of 3" }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::api::CatalogModel;
    use crate::settings::{ModelRow, design_project, model_rows, stepped_screen};

    fn catalog() -> Vec<CatalogModel> {
        vec![
            CatalogModel {
                id: "flash".to_owned(),
                description: "Fast drafts and quick edits.".to_owned(),
                is_recommended: false,
            },
            CatalogModel {
                id: "pro".to_owned(),
                description: "Best structure and copy.".to_owned(),
                is_recommended: true,
            },
        ]
    }

    fn ids(rows: &[ModelRow]) -> Vec<&str> {
        rows.iter().map(|row| row.id.as_str()).collect()
    }

    #[test]
    fn card_pager_steps_stay_inside_the_design() {
        assert_eq!(stepped_screen(1, 1, 8), 2);
        assert_eq!(stepped_screen(8, 1, 8), 8);
        assert_eq!(stepped_screen(1, -1, 8), 1);
        assert_eq!(stepped_screen(3, 1, 0), 1);
    }

    #[test]
    fn design_projects_strip_candidate_suffixes() {
        assert_eq!(design_project("talk-candidate-2"), "talk");
        assert_eq!(design_project("talk"), "talk");
    }

    #[test]
    fn puts_the_recommended_model_first() {
        let rows = model_rows(&catalog(), &["flash".to_owned(), "pro".to_owned()]);
        assert_eq!(ids(&rows), ["pro", "flash"]);
        assert!(rows.iter().all(|row| row.is_available));
    }

    #[test]
    fn marks_a_catalog_model_the_key_omits_as_unavailable() {
        let rows = model_rows(&catalog(), &["flash".to_owned()]);
        assert_eq!(ids(&rows), ["flash", "pro"]);
        assert!(rows[0].is_available);
        assert!(!rows[1].is_available);
    }

    #[test]
    fn treats_every_model_as_available_when_the_live_list_is_empty() {
        let rows = model_rows(&catalog(), &[]);
        assert_eq!(ids(&rows), ["pro", "flash"]);
        assert!(rows.iter().all(|row| row.is_available));
    }
}
