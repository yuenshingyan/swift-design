//! Atomic file writes for the stores.
//!
//! `tokio::fs::write` truncates the file, then fills it. A reader that
//! arrives inside that window reads an empty file, and the studio polls
//! the stores while a run writes them. Writing to a temporary file and
//! renaming it over the target closes the window: a reader sees either
//! the old file or the new one, never a half-written one.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Counter behind each temporary name, so two writers to one path
/// cannot pick the same temporary file.
static NEXT_WRITE: AtomicU64 = AtomicU64::new(0);

/// The temporary path a write to `path` uses.
///
/// The suffix keeps the name out of every listing: the stores select
/// files whose extension is `json`, and this one's is `writing`.
fn temporary_path(path: &Path) -> PathBuf {
    let ticket = NEXT_WRITE.fetch_add(1, Ordering::Relaxed);
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    path.with_file_name(format!("{name}.{ticket}.writing"))
}

/// Writes `contents` to `path`, replacing it in one step.
///
/// The temporary file is removed when the rename fails, so a failed
/// write leaves nothing behind.
pub async fn write_atomically(path: &Path, contents: &str) -> anyhow::Result<()> {
    let temporary = temporary_path(path);
    tokio::fs::write(&temporary, contents).await?;
    if let Err(error) = tokio::fs::rename(&temporary, path).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(error.into());
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::{temporary_path, write_atomically};
    use std::path::Path;

    #[tokio::test]
    async fn a_write_replaces_the_file_and_leaves_no_temporary() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("design.json");
        write_atomically(&path, "{\"a\":1}").await.unwrap();
        assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), "{\"a\":1}");
        write_atomically(&path, "{\"a\":2}").await.unwrap();
        assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), "{\"a\":2}");
        let mut entries = tokio::fs::read_dir(directory.path()).await.unwrap();
        let mut names = Vec::new();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
        assert_eq!(names, vec!["design.json"]);
    }

    #[test]
    fn two_writers_to_one_path_pick_different_temporaries() {
        let path = Path::new("/tmp/design.json");
        let first = temporary_path(path);
        let second = temporary_path(path);
        assert_ne!(first, second);
        // The name stays out of a `*.json` listing.
        for temporary in [first, second] {
            assert_eq!(temporary.extension().unwrap(), "writing");
            assert!(
                temporary
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("design.json.")
            );
        }
    }
}
