//! Hash routing: the view is kept in `window.location.hash`, so a
//! reload or a Back gesture lands on the same screen. There is no
//! router crate; the app reads and writes the hash through `eval`.

/// The page the app is showing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum View {
    /// The landing page: the composer plus the session list.
    Home,
    /// One session's workspace.
    Session(String),
    /// The editor for one design.
    Design(String),
}

/// The view for `hash`. Unknown or empty hashes land on Home.
pub(crate) fn route_from_hash(hash: &str) -> View {
    let trimmed = hash.trim_start_matches('#').trim_start_matches('/');
    if trimmed.is_empty() {
        return View::Home;
    }
    let mut parts = trimmed.split('/');
    match (parts.next(), parts.next(), parts.next()) {
        (Some("sessions"), Some(id), None) if is_slug(id) => View::Session(id.to_owned()),
        (Some("designs"), Some(id), None) if is_slug(id) => View::Design(id.to_owned()),
        _ => View::Home,
    }
}

/// The hash for `view`: `#/`, `#/sessions/{id}`, or `#/designs/{id}`.
pub(crate) fn hash_for(view: &View) -> String {
    match view {
        View::Home => "#/".to_owned(),
        View::Session(id) => format!("#/sessions/{id}"),
        View::Design(id) => format!("#/designs/{id}"),
    }
}

/// True for a non-empty id of the slug characters the server allows.
fn is_slug(id: &str) -> bool {
    !id.is_empty()
        && id.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
}

/// Sends the current hash once, then again on every `hashchange`.
pub(crate) const HASH_LISTENER: &str = "\
dioxus.send(window.location.hash);
window.addEventListener('hashchange', () => dioxus.send(window.location.hash));
";

/// Writes a hash the app chose, unless the URL already shows it, and
/// unless Home is asked for on a bare URL (so the first load adds no
/// history entry).
pub(crate) const WRITE_HASH: &str = "\
const hash = await dioxus.recv();
const current = window.location.hash;
if (current === hash) { return; }
if (hash === '#/' && (current === '' || current === '#')) { return; }
window.location.hash = hash;
";

#[cfg(test)]
mod tests {
    use super::{View, hash_for, route_from_hash};

    #[test]
    fn empty_and_root_hashes_route_home() {
        assert_eq!(route_from_hash(""), View::Home);
        assert_eq!(route_from_hash("#"), View::Home);
        assert_eq!(route_from_hash("#/"), View::Home);
    }

    #[test]
    fn session_hashes_route_to_the_session() {
        assert_eq!(
            route_from_hash("#/sessions/finance-app"),
            View::Session("finance-app".to_owned())
        );
    }

    #[test]
    fn design_hashes_route_to_the_editor() {
        assert_eq!(
            route_from_hash("#/designs/finance-app-candidate-2"),
            View::Design("finance-app-candidate-2".to_owned())
        );
    }

    #[test]
    fn unknown_hashes_fall_back_to_home() {
        assert_eq!(route_from_hash("#/x"), View::Home);
        assert_eq!(route_from_hash("#/sessions/"), View::Home);
        assert_eq!(route_from_hash("#/sessions/a/b"), View::Home);
        assert_eq!(route_from_hash("#/sessions/Bad_Id"), View::Home);
    }

    #[test]
    fn hashes_round_trip_every_view() {
        for view in [
            View::Home,
            View::Session("talk".to_owned()),
            View::Design("talk-candidate-1".to_owned()),
        ] {
            assert_eq!(route_from_hash(&hash_for(&view)), view);
        }
    }
}
