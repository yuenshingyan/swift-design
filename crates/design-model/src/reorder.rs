//! Reorders units without changing the destinations of numbered screen links.

use crate::markup::remap_screen_links;
use crate::{Ad, Cover, Email, Frame, Page, Screen, Sheet, Slide};

/// A unit whose HTML can contain numbered screen links.
pub trait LinkedUnit {
    /// Returns the HTML that carries the unit's navigation links.
    fn html_mut(&mut self) -> &mut String;
}

macro_rules! linked_units {
    ($($type:ty),+ $(,)?) => {
        $(impl LinkedUnit for $type {
            fn html_mut(&mut self) -> &mut String {
                &mut self.html
            }
        })+
    };
}

linked_units!(Screen, Slide, Page, Frame, Sheet, Email, Ad, Cover);

/// Moves a unit and remaps all local screen links to their original destinations.
/// Moves the corresponding preview outline title when the outline covers the units.
/// Returns false for an invalid move or a move to the same position.
pub fn reorder_units<T: LinkedUnit>(
    units: &mut Vec<T>,
    outline: &mut Vec<String>,
    from: usize,
    to: usize,
) -> bool {
    if from == to || from >= units.len() || to >= units.len() {
        return false;
    }
    let mut order: Vec<usize> = (0..units.len()).collect();
    let original_index = order.remove(from);
    order.insert(to, original_index);
    let mut old_to_new = vec![0; units.len()];
    for (new_index, old_index) in order.into_iter().enumerate() {
        old_to_new[old_index] = new_index;
    }
    let unit = units.remove(from);
    units.insert(to, unit);
    for unit in units.iter_mut() {
        let html = unit.html_mut();
        *html = remap_screen_links(html, &old_to_new);
    }
    if outline.len() >= units.len() {
        let title = outline.remove(from);
        outline.insert(to, title);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screens() -> Vec<Screen> {
        (1..=3).map(|number| Screen {
            name: format!("Screen {number}"),
            html: "<a href='#screen-1'>One</a><a href='#screen-2'>Two</a><a href='#screen-3'>Three</a>".to_owned(),
            css: Some("a { color: red; }".to_owned()),
            notes: Some(format!("Purpose {number}")),
        }).collect()
    }

    #[test]
    fn forward_moves_preserve_incoming_outgoing_and_self_links() {
        let mut units = screens();
        let mut outline = vec![
            "One".to_owned(),
            "Two".to_owned(),
            "Three".to_owned(),
            "Future".to_owned(),
        ];
        assert!(reorder_units(&mut units, &mut outline, 0, 2));
        assert_eq!(
            units
                .iter()
                .map(|unit| unit.name.as_str())
                .collect::<Vec<_>>(),
            ["Screen 2", "Screen 3", "Screen 1"]
        );
        for unit in &units {
            assert_eq!(
                unit.html,
                "<a href='#screen-3'>One</a><a href='#screen-1'>Two</a><a href='#screen-2'>Three</a>"
            );
            assert_eq!(unit.css.as_deref(), Some("a { color: red; }"));
        }
        assert_eq!(outline, ["Two", "Three", "One", "Future"]);
        assert_eq!(units[2].notes.as_deref(), Some("Purpose 1"));
    }

    #[test]
    fn backward_moves_and_inverse_moves_preserve_the_graph() {
        let original = screens();
        let mut units = original.clone();
        let mut outline = Vec::new();
        assert!(reorder_units(&mut units, &mut outline, 2, 0));
        assert_eq!(units[0].name, "Screen 3");
        assert_eq!(
            units[0].html,
            "<a href='#screen-2'>One</a><a href='#screen-3'>Two</a><a href='#screen-1'>Three</a>"
        );
        assert!(reorder_units(&mut units, &mut outline, 0, 2));
        assert_eq!(units, original);
    }

    #[test]
    fn every_move_preserves_destinations_with_multi_digit_screen_numbers() {
        for from in 0..12 {
            for to in 0..12 {
                let mut units: Vec<Screen> = (1..=12)
                    .map(|number| Screen {
                        name: number.to_string(),
                        html: (1..=12)
                            .map(|target| format!("<a href='#screen-{target}'>Target {target}</a>"))
                            .collect(),
                        css: None,
                        notes: None,
                    })
                    .collect();
                reorder_units(&mut units, &mut Vec::new(), from, to);
                for unit in &units {
                    for target in 1..=12 {
                        let destination = units
                            .iter()
                            .position(|unit| unit.name == target.to_string());
                        let Some(destination) = destination else {
                            panic!("the destination was lost");
                        };
                        assert!(unit.html.contains(&format!(
                            "href='#screen-{}'>Target {target}",
                            destination + 1
                        )));
                    }
                }
            }
        }
    }

    #[test]
    fn invalid_and_no_op_moves_change_nothing() {
        let mut units = screens();
        let original = units.clone();
        let mut outline = vec!["Incomplete".to_owned()];
        assert!(!reorder_units(&mut units, &mut outline, 4, 0));
        assert!(!reorder_units(&mut units, &mut outline, 0, 4));
        assert!(!reorder_units(&mut units, &mut outline, 1, 1));
        assert_eq!(units, original);
        assert!(reorder_units(&mut units, &mut outline, 0, 2));
        assert_eq!(outline, ["Incomplete"]);
    }

    #[test]
    fn all_eight_unit_types_support_navigation_preserving_reordering() {
        fn assert_linked<T: LinkedUnit>() {}
        assert_linked::<Screen>();
        assert_linked::<Slide>();
        assert_linked::<Page>();
        assert_linked::<Frame>();
        assert_linked::<Sheet>();
        assert_linked::<Email>();
        assert_linked::<Ad>();
        assert_linked::<Cover>();
    }
}
