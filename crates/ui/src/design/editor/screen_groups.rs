use std::collections::HashMap;

pub(super) enum StripItem {
    Group {
        name: Option<String>,
        indexes: Vec<usize>,
        matches: usize,
        is_open: bool,
    },
    Screen(usize),
}

pub(super) fn screen_group_name(label: &str) -> Option<&str> {
    let (prefix, suffix) = label.split_once('·')?;
    let prefix = prefix.trim();
    (!prefix.is_empty() && !suffix.trim().is_empty()).then_some(prefix)
}

pub(super) fn strip_items(
    names: &[(&str, &str)],
    indexes: impl IntoIterator<Item = usize>,
    query: &str,
    current_group: &Option<String>,
    overrides: &HashMap<Option<String>, bool>,
) -> Vec<StripItem> {
    let mut positions = HashMap::new();
    let mut groups: Vec<(Option<&str>, Vec<usize>)> = Vec::new();
    for index in indexes {
        let name = screen_group_name(names[index].0);
        let position = *positions.entry(name).or_insert_with(|| {
            groups.push((name, Vec::new()));
            groups.len() - 1
        });
        groups[position].1.push(index);
    }

    let mut items = Vec::new();
    for (name, indexes) in groups {
        let matching: Vec<_> = indexes
            .iter()
            .copied()
            .filter(|&index| {
                let (name, fallback) = names[index];
                query.is_empty()
                    || name.to_lowercase().contains(query)
                    || (name.trim().is_empty() && fallback.to_lowercase().contains(query))
            })
            .collect();
        if matching.is_empty() {
            continue;
        }
        let name = name.map(str::to_owned);
        let is_open = !query.is_empty()
            || overrides
                .get(&name)
                .copied()
                .unwrap_or(&name == current_group);
        items.push(StripItem::Group {
            name,
            indexes,
            matches: matching.len(),
            is_open,
        });
        if is_open {
            items.extend(matching.into_iter().map(StripItem::Screen));
        }
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels() -> [(&'static str, &'static str); 5] {
        [
            "Access · Open",
            "",
            "Activity · Loaded",
            "Access · Save · Failed",
            "Ungrouped · Explicit",
        ]
        .map(|name| (name, "Screen fallback"))
    }

    fn visible(items: &[StripItem]) -> Vec<usize> {
        items
            .iter()
            .filter_map(|item| match item {
                StripItem::Screen(index) => Some(*index),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn explicit_prefix_needs_a_group_and_a_screen_name() {
        assert_eq!(screen_group_name(" Access · Save · Failed"), Some("Access"));
        for label in ["Overview", " · Save", "Access · ", ""] {
            assert_eq!(screen_group_name(label), None);
        }
    }

    #[test]
    fn groups_follow_first_occurrence_and_retain_original_screen_indexes() {
        let items = strip_items(&labels(), 0..5, "", &Some("Access".into()), &HashMap::new());
        assert_eq!(visible(&items), [0, 3]);
        let groups: Vec<_> = items
            .iter()
            .filter_map(|item| match item {
                StripItem::Group { name, indexes, .. } => {
                    Some((name.as_deref(), indexes.as_slice()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            groups,
            [
                (Some("Access"), &[0, 3][..]),
                (None, &[1]),
                (Some("Activity"), &[2]),
                (Some("Ungrouped"), &[4])
            ]
        );
    }

    #[test]
    fn search_reveals_matching_screens_even_in_manually_closed_groups() {
        let overrides = HashMap::from([(Some("Access".into()), false), (None, true)]);
        let current = Some("Access".into());
        let labels = labels();
        assert_eq!(
            visible(&strip_items(&labels, 0..5, "", &current, &overrides)),
            [1]
        );
        let items = strip_items(&labels, 0..5, "failed", &current, &overrides);
        assert_eq!(visible(&items), [3]);
        assert!(
            matches!(&items[0], StripItem::Group { indexes, matches: 1, is_open: true, .. } if indexes == &[0, 3])
        );
        assert!(strip_items(&labels, 0..5, "not found", &current, &overrides).is_empty());
        assert!(strip_items(&labels, [0, 1], "failed", &current, &overrides).is_empty());
        assert_eq!(
            visible(&strip_items(&labels, 0..5, "", &current, &overrides)),
            [1]
        );
    }
}
