use super::*;

/// Google font families offered for headings and body text. The server
/// loads any of them from Google Fonts.
pub(super) const TEXT_FONTS: [&str; 30] = [
    "Inter",
    "Roboto",
    "Open Sans",
    "Lato",
    "Montserrat",
    "Poppins",
    "Raleway",
    "Nunito",
    "Work Sans",
    "DM Sans",
    "Manrope",
    "Outfit",
    "Sora",
    "Plus Jakarta Sans",
    "Space Grotesk",
    "IBM Plex Sans",
    "Source Sans 3",
    "Fira Sans",
    "Rubik",
    "Archivo",
    "Oswald",
    "Bebas Neue",
    "Josefin Sans",
    "Playfair Display",
    "Lora",
    "Merriweather",
    "Libre Baskerville",
    "EB Garamond",
    "Cormorant Garamond",
    "Crimson Text",
];

/// Monospace families offered for code and figures.
pub(super) const MONO_FONTS: [&str; 9] = [
    "JetBrains Mono",
    "Fira Code",
    "IBM Plex Mono",
    "Source Code Pro",
    "Roboto Mono",
    "Space Mono",
    "DM Mono",
    "Inconsolata",
    "Courier Prime",
];

/// System families that need no download.
const SYSTEM_FONTS: [&str; 4] = ["system-ui", "sans-serif", "serif", "monospace"];

/// The options for one font select: the current value first when it is
/// not in the list, then `families`, then the system families.
pub(super) fn font_options(current: &str, families: &[&str]) -> Vec<String> {
    let mut options: Vec<String> = Vec::with_capacity(families.len() + SYSTEM_FONTS.len() + 1);
    let is_listed = families
        .iter()
        .chain(SYSTEM_FONTS.iter())
        .any(|family| family.eq_ignore_ascii_case(current));
    if !is_listed && !current.trim().is_empty() {
        options.push(current.to_owned());
    }
    options.extend(families.iter().map(|family| (*family).to_owned()));
    options.extend(SYSTEM_FONTS.iter().map(|family| (*family).to_owned()));
    options
}

/// A font family select. A family outside the list, written by the
/// agent, stays selectable so the value never changes on open.
#[component]
fn FontField(
    label: &'static str,
    value: String,
    families: &'static [&'static str],
    on_change: EventHandler<String>,
) -> Element {
    let options = font_options(&value, families);
    // The listed spelling of the current family, so a lower-case value
    // written by the agent still shows as chosen.
    let chosen = options
        .iter()
        .find(|family| family.eq_ignore_ascii_case(&value))
        .cloned()
        .unwrap_or_else(|| value.clone());
    rsx! {
        div { class: "field",
            span { class: "field-label", "{label}" }
            Select {
                value: chosen,
                options: plain_options(options),
                on_change: move |family| on_change.call(family),
            }
        }
    }
}

/// Theme fields: the four palette colors and the three fonts. The
/// theme name stays the agent's label for the design; the user never
/// needs to edit it.
#[component]
pub(crate) fn ThemeForm(theme: Theme, on_change: EventHandler<Theme>) -> Element {
    let source = theme.clone();
    let edit = use_callback(move |(change, value): (fn(&mut Theme, String), String)| {
        let mut edited = source.clone();
        change(&mut edited, value);
        on_change.call(edited);
    });
    rsx! {
        div { class: "sheet-section theme",
            div { class: "head", "Theme" }
            div { class: "color-list",
                RequiredColorField {
                    label: "Background color",
                    value: theme.colors.background.clone(),
                    on_change: move |value: String| {
                        edit.call((|theme, value| theme.colors.background = value, value))
                    },
                }
                RequiredColorField {
                    label: "Text color",
                    value: theme.colors.text.clone(),
                    on_change: move |value: String| edit.call((|theme, value| theme.colors.text = value, value)),
                }
                RequiredColorField {
                    label: "Accent color",
                    value: theme.colors.accent.clone(),
                    on_change: move |value: String| edit.call((|theme, value| theme.colors.accent = value, value)),
                }
                RequiredColorField {
                    label: "Muted color",
                    value: theme.colors.muted.clone(),
                    on_change: move |value: String| edit.call((|theme, value| theme.colors.muted = value, value)),
                }
            }
            div { class: "font-grid",
                FontField {
                    label: "Heading font",
                    value: theme.fonts.heading.clone(),
                    families: &TEXT_FONTS,
                    on_change: move |value: String| edit.call((|theme, value| theme.fonts.heading = value, value)),
                }
                FontField {
                    label: "Body font",
                    value: theme.fonts.body.clone(),
                    families: &TEXT_FONTS,
                    on_change: move |value: String| edit.call((|theme, value| theme.fonts.body = value, value)),
                }
                FontField {
                    label: "Mono font",
                    value: theme.fonts.mono.clone(),
                    families: &MONO_FONTS,
                    on_change: move |value: String| edit.call((|theme, value| theme.fonts.mono = value, value)),
                }
            }
        }
    }
}

/// One row of the palette list: the swatch, the colour's name, and its
/// hex code. The colour always has a value.
#[component]
fn RequiredColorField(
    label: &'static str,
    value: String,
    on_change: EventHandler<String>,
) -> Element {
    rsx! {
        label { class: "color-field",
            input {
                r#type: "color",
                value: "{value}",
                oninput: move |event| on_change.call(event.value()),
            }
            span { class: "color-name", "{label}" }
            span { class: "color-code mono", "{value}" }
        }
    }
}
