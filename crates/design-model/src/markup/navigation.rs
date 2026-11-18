//! Validation for numbered page links and local prototype controls.

use std::collections::BTreeMap;

use crate::design::Design;

use super::{HtmlChecker, decode_entities, screen_link_target};

#[derive(Default)]
pub(super) struct DemoTag {
    href: Option<String>,
    panel: Option<String>,
    action: Option<String>,
    hidden: bool,
}

impl DemoTag {
    pub(super) fn read(&mut self, name: &str, value: Option<&str>) {
        let read_value = || decode_entities(value.unwrap_or_default());
        match name {
            "href" if self.href.is_none() => self.href = Some(read_value()),
            "data-demo-panel" if self.panel.is_none() => self.panel = Some(read_value()),
            "data-demo-action" if self.action.is_none() => self.action = Some(read_value()),
            "hidden" => self.hidden = true,
            _ => {}
        }
    }
}

#[derive(Default)]
pub(super) struct DemoMarkup {
    tags: Vec<(String, DemoTag)>,
}

impl DemoMarkup {
    pub(super) fn record(&mut self, name: &str, tag: DemoTag) {
        if tag.panel.is_some() || tag.action.is_some() || (name == "a" && tag.href.is_some()) {
            self.tags.push((name.to_owned(), tag));
        }
    }

    fn validate(&self, last: usize, old_last: Option<usize>) -> Result<(), String> {
        let mut groups = BTreeMap::<&str, BTreeMap<&str, bool>>::new();
        for (tag, markup) in &self.tags {
            let Some(panel) = &markup.panel else { continue };
            let Some((group, state)) = local_target(panel).filter(|(_, state)| key(state)) else {
                return Err("a prototype panel needs a group and state, such as filter:all".into());
            };
            if !matches!(tag.as_str(), "div" | "section" | "article") || markup.action.is_some() {
                return Err(
                    "put each prototype panel on a div, section, or article without an action"
                        .into(),
                );
            }
            if groups
                .entry(group)
                .or_default()
                .insert(state, !markup.hidden)
                .is_some()
            {
                return Err(format!("prototype panel {panel} occurs twice"));
            }
        }
        for (group, states) in &groups {
            if states.len() < 2 || states.values().filter(|visible| **visible).count() != 1 {
                return Err(format!(
                    "prototype group {group} needs at least two panels and exactly one visible default panel"
                ));
            }
        }
        for (tag, markup) in &self.tags {
            let href = markup.href.as_deref().unwrap_or_default();
            if let Some(action) = &markup.action {
                if tag != "a" || href != "#demo-panel" {
                    return Err("a prototype action needs an anchor with href='#demo-panel'".into());
                }
                let supported = local_target(action).is_some_and(|(group, target)| {
                    groups.get(group).is_some_and(|states| {
                        states.contains_key(target)
                            || matches!(target, "@previous" | "@next" | "@first" | "@last")
                    })
                });
                if !supported {
                    return Err(
                        "a prototype action points to a missing group or state on this screen"
                            .into(),
                    );
                }
            } else if tag == "a" && href == "#demo-panel" {
                return Err("a prototype anchor is missing data-demo-action".into());
            } else if tag == "a" && href.starts_with("#screen-") {
                let target = screen_link_target(href);
                let valid = |max| target.is_some_and(|target| (1..=max).contains(&target));
                if !(valid(last) || old_last.is_some_and(|old_last| !valid(old_last))) {
                    return Err(format!(
                        "an internal link has no destination: use a screen from 1 to {last}"
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Checks demo page destinations and prototype controls before built-in generation saves.
/// Draft outlines may contain planned destinations. Unchanged legacy page links may remain.
pub fn validate_demo_navigation(design: &Design, before: Option<&Design>) -> Result<(), String> {
    let last = design.screens.len().max(design.outline.len());
    for (index, screen) in design.screens.iter().enumerate() {
        let old_last = before
            .filter(|old| {
                old.screens
                    .get(index)
                    .is_some_and(|page| page.html == screen.html)
            })
            .map(|old| old.screens.len().max(old.outline.len()));
        let mut checker = HtmlChecker::new(&screen.html);
        checker.demo = Some(DemoMarkup::default());
        checker.run();
        if let Some(problem) = checker.problems.first() {
            return Err(format!("Screen {}: {problem}.", index + 1));
        }
        if let Some(demo) = checker.demo {
            demo.validate(last, old_last)
                .map_err(|error| format!("Screen {}: {error}.", index + 1))?;
        }
    }
    Ok(())
}

fn local_target(value: &str) -> Option<(&str, &str)> {
    let (group, state) = value.split_once(':')?;
    key(group).then_some((group, state))
}

fn key(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safely_links_local_panels_without_accepting_missing_states_or_page_links() {
        let mut demo: Design =
            serde_json::from_str(include_str!("../../../../fixtures/sample-design.json"))
                .expect("sample demo should parse");
        let panels = "<section data-demo-panel='chooser:first'>First</section><section hidden data-demo-panel='chooser:second'>Second</section>";
        for (control, expected) in [
            (
                "<a href='#demo-panel' data-demo-action='chooser:second'>Show</a>",
                None,
            ),
            (
                "<a href='#demo-panel' data-demo-action='chooser:@next'>Next</a>",
                None,
            ),
            ("<a href='#screen-2'>Open page</a>", None),
            (
                "<a href='#demo-panel' data-demo-action='chooser:missing'>Show</a>",
                Some("missing group or state"),
            ),
            (
                "<a href='#demo-panel' data-demo-action='other:second'>Show</a>",
                Some("missing group or state"),
            ),
            (
                "<div data-demo-action='chooser:second'>Show</div>",
                Some("needs an anchor"),
            ),
            (
                "<a href='#screen-999'>Open page</a>",
                Some("has no destination"),
            ),
            (
                "<a href='#demo-panel'>Show</a>",
                Some("missing data-demo-action"),
            ),
        ] {
            demo.screens[0].html = format!("{panels}{control}");
            match expected {
                Some(message) => assert!(
                    validate_demo_navigation(&demo, None)
                        .expect_err("invalid markup should fail navigation checks")
                        .contains(message)
                ),
                None => {
                    validate_demo_navigation(&demo, None).expect("valid navigation should pass")
                }
            }
        }
        demo.screens[0].html = panels.replace(" hidden", "");
        assert!(
            validate_demo_navigation(&demo, None)
                .expect_err("two default panels should fail")
                .contains("one visible default")
        );
    }
}
