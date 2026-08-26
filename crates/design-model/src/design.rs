//! The top-level design type.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::screen::Screen;
use crate::theme::Theme;
use crate::transition::Transition;
use crate::viewport::Viewport;

/// A complete design: one theme and one viewport applied to an ordered
/// list of screens.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Design {
    /// Design title, shown in the editor and the session.
    pub title: String,
    /// Visual identity applied to every screen.
    pub theme: Theme,
    /// The px canvas every screen is laid out on. Set it from the
    /// brief's target platform: 1440 by 900 for desktop web, 390 by 844
    /// for a phone, 1024 by 768 for a tablet. Absent means 1440 by 900.
    #[serde(default)]
    pub viewport: Viewport,
    /// Screens in flow order.
    pub screens: Vec<Screen>,
    /// The screen titles of the complete design, in order. A preview design
    /// lists more titles than it has screens: the app continues it from
    /// this list later. Leave it empty when the design is complete.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outline: Vec<String>,
    /// How the presentation moves from one screen to the next. Leave it
    /// out for a design that scrolls instead of animating.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition: Option<Transition>,
}

impl Design {
    /// True when the outline plans more screens than the design has: the
    /// design is a preview that waits for its remaining screens.
    pub fn is_preview(&self) -> bool {
        self.outline.len() > self.screens.len()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::test_support::sample_design;
    use crate::{Design, Transition, TransitionAxis, TransitionEffect};

    #[test]
    fn design_round_trips_through_json() {
        let design = sample_design();
        let json = serde_json::to_string(&design).unwrap();
        let restored: Design = serde_json::from_str(&json).unwrap();
        assert_eq!(design, restored);
    }

    #[test]
    fn a_design_without_a_viewport_loads_with_the_default() {
        let mut value = serde_json::to_value(sample_design()).unwrap();
        assert_eq!(value["viewport"]["width"], 1440);
        value.as_object_mut().unwrap().remove("viewport");
        let design: Design = serde_json::from_value(value).unwrap();
        assert_eq!(design.viewport, crate::Viewport::default());
    }

    #[test]
    fn rejects_unknown_fields() {
        let mut value = serde_json::to_value(sample_design()).unwrap();
        value["extra"] = serde_json::json!(1);
        assert!(serde_json::from_value::<Design>(value).is_err());
    }

    #[test]
    fn an_outline_longer_than_the_screens_marks_a_preview() {
        let mut design = sample_design();
        assert!(!design.is_preview());
        assert!(!serde_json::to_string(&design).unwrap().contains("outline"));
        design.outline = vec!["Sample".to_owned(), "Next".to_owned()];
        assert!(design.is_preview());
        let json = serde_json::to_string(&design).unwrap();
        let restored: Design = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.outline, design.outline);
        design.outline.truncate(1);
        assert!(!design.is_preview());
    }

    #[test]
    fn a_transition_round_trips_and_stays_out_when_absent() {
        let mut design = sample_design();
        assert!(
            !serde_json::to_string(&design)
                .unwrap()
                .contains("transition")
        );
        design.transition = Some(Transition {
            effect: TransitionEffect::Zoom,
            axis: TransitionAxis::Horizontal,
            duration_ms: 300,
        });
        let json = serde_json::to_string(&design).unwrap();
        let restored: Design = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.transition, design.transition);
    }
}
