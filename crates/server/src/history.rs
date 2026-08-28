//! Design save history: one snapshot of the previous file per save.
//!
//! `DesignStore::save` copies the current design file here before it writes
//! the new content, so one bad agent edit is one restore away. A
//! snapshot lives at `<history dir>/<design id>/<stamp>.json`. The store
//! keeps the newest `SNAPSHOT_LIMIT` snapshots per design and deletes the
//! rest.

use std::path::PathBuf;

use serde::Serialize;

use crate::time::{rfc3339, unix_now_seconds};

/// Snapshots kept per design. The next save deletes older ones.
pub const SNAPSHOT_LIMIT: usize = 50;

/// Longest stamp accepted from a request: a time plus a short suffix.
const STAMP_LENGTH_LIMIT: usize = 32;

/// Filesystem-backed snapshot storage: one directory per design.
#[derive(Clone, Debug)]
pub struct HistoryStore {
    directory: PathBuf,
}

/// One row of `GET /designs/{id}/history`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Snapshot {
    /// File stem of the snapshot, used in the restore route.
    pub stamp: String,
    /// When the snapshot was taken, as an RFC 3339 UTC string.
    pub saved_at: String,
    /// Size of the snapshot file.
    pub size_bytes: u64,
}

impl HistoryStore {
    /// Creates a store over `directory`. The directory may not exist
    /// yet; it is created on the first snapshot.
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    fn design_directory(&self, id: &str) -> PathBuf {
        self.directory.join(id)
    }

    fn path_of(&self, id: &str, stamp: &str) -> PathBuf {
        self.design_directory(id).join(format!("{stamp}.json"))
    }

    /// Stores `content` as a snapshot taken now. Returns the stamp.
    pub async fn snapshot(&self, id: &str, content: &[u8]) -> anyhow::Result<String> {
        self.snapshot_at(id, unix_now_seconds(), content).await
    }

    /// Stores `content` as a snapshot taken at `unix_seconds`, then
    /// deletes the snapshots past `SNAPSHOT_LIMIT`. Returns the stamp.
    pub async fn snapshot_at(
        &self,
        id: &str,
        unix_seconds: u64,
        content: &[u8],
    ) -> anyhow::Result<String> {
        tokio::fs::create_dir_all(self.design_directory(id)).await?;
        let stamp = self.free_stamp(id, &stamp_from_unix(unix_seconds)).await;
        tokio::fs::write(self.path_of(id, &stamp), content).await?;
        self.prune(id).await?;
        Ok(stamp)
    }

    /// `base` when no snapshot uses it yet, else `base-2`, `base-3`, …
    /// so two saves in one second keep two snapshots.
    async fn free_stamp(&self, id: &str, base: &str) -> String {
        let mut stamp = base.to_owned();
        let mut suffix = 2;
        while tokio::fs::try_exists(self.path_of(id, &stamp))
            .await
            .unwrap_or(false)
        {
            stamp = format!("{base}-{suffix}");
            suffix += 1;
        }
        stamp
    }

    /// Deletes the oldest snapshots past `SNAPSHOT_LIMIT`.
    async fn prune(&self, id: &str) -> anyhow::Result<()> {
        for stamp in self.stamps(id).await?.iter().skip(SNAPSHOT_LIMIT) {
            tokio::fs::remove_file(self.path_of(id, stamp)).await?;
        }
        Ok(())
    }

    /// Every stamp stored for `id`, newest first. Empty when the design
    /// has no history directory.
    async fn stamps(&self, id: &str) -> anyhow::Result<Vec<String>> {
        let mut entries = match tokio::fs::read_dir(self.design_directory(id)).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut stamps = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
                continue;
            }
            if let Some(stamp) = path.file_stem().and_then(|stem| stem.to_str())
                && is_valid_stamp(stamp)
            {
                stamps.push(stamp.to_owned());
            }
        }
        stamps.sort_by(|left, right| right.cmp(left));
        Ok(stamps)
    }

    /// Lists the snapshots of `id`, newest first.
    pub async fn list(&self, id: &str) -> anyhow::Result<Vec<Snapshot>> {
        let mut snapshots = Vec::new();
        for stamp in self.stamps(id).await? {
            let size_bytes = tokio::fs::metadata(self.path_of(id, &stamp)).await?.len();
            snapshots.push(Snapshot {
                saved_at: saved_at_of(&stamp),
                stamp,
                size_bytes,
            });
        }
        Ok(snapshots)
    }

    /// Reads one snapshot. `Ok(None)` when no such stamp exists.
    pub async fn read(&self, id: &str, stamp: &str) -> anyhow::Result<Option<Vec<u8>>> {
        if !is_valid_stamp(stamp) {
            return Ok(None);
        }
        match tokio::fs::read(self.path_of(id, stamp)).await {
            Ok(content) => Ok(Some(content)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Deletes every snapshot of `id`. A design with no history has
    /// nothing to delete, which is not an error.
    pub async fn delete(&self, id: &str) -> anyhow::Result<()> {
        match tokio::fs::remove_dir_all(self.design_directory(id)).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    /// Moves the history of `old_id` to `new_id`. A design with no history
    /// has nothing to move, which is not an error.
    pub async fn rename(&self, old_id: &str, new_id: &str) -> anyhow::Result<()> {
        let source = self.design_directory(old_id);
        if !tokio::fs::try_exists(&source).await? {
            return Ok(());
        }
        tokio::fs::create_dir_all(&self.directory).await?;
        tokio::fs::rename(source, self.design_directory(new_id)).await?;
        Ok(())
    }
}

/// True for a stamp this store could have written: digits, `T`, `Z`,
/// and hyphens only. That excludes path separators and `.` segments.
pub fn is_valid_stamp(stamp: &str) -> bool {
    !stamp.is_empty()
        && stamp.len() <= STAMP_LENGTH_LIMIT
        && stamp
            .chars()
            .all(|character| character.is_ascii_digit() || matches!(character, '-' | 'T' | 'Z'))
}

/// The file stem for a snapshot taken at `unix_seconds`: the RFC 3339
/// UTC time with every `:` replaced by `-`, so it is a safe file name.
pub fn stamp_from_unix(unix_seconds: u64) -> String {
    rfc3339(unix_seconds).replace(':', "-")
}

/// The RFC 3339 time a stamp encodes. A same-second suffix is dropped.
pub fn saved_at_of(stamp: &str) -> String {
    match (stamp.get(..10), stamp.get(11..20)) {
        (Some(date), Some(time)) => format!("{date}T{}", time.replace('-', ":")),
        _ => stamp.to_owned(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_delete_removes_every_snapshot_of_one_id() {
        let directory = tempfile::tempdir().unwrap();
        let store = HistoryStore::new(directory.path().join("history"));
        store
            .snapshot_at("talk", 1_000_000_000, b"{}")
            .await
            .unwrap();
        store
            .snapshot_at("talking", 1_000_000_000, b"{}")
            .await
            .unwrap();
        store.delete("talk").await.unwrap();
        assert!(store.list("talk").await.unwrap().is_empty());
        assert_eq!(store.list("talking").await.unwrap().len(), 1);
        // Deleting an id with no history is not an error.
        store.delete("talk").await.unwrap();
    }

    #[test]
    fn stamps_are_rfc3339_times_with_hyphens() {
        assert_eq!(stamp_from_unix(1_000_000_000), "2001-09-09T01-46-40Z");
        assert_eq!(saved_at_of("2001-09-09T01-46-40Z"), "2001-09-09T01:46:40Z");
        assert_eq!(
            saved_at_of("2001-09-09T01-46-40Z-2"),
            "2001-09-09T01:46:40Z"
        );
        assert_eq!(saved_at_of("odd"), "odd");
    }

    #[test]
    fn stamp_validation_blocks_paths() {
        assert!(is_valid_stamp("2026-08-25T10-14-02Z"));
        assert!(is_valid_stamp("2026-08-25T10-14-02Z-2"));
        assert!(!is_valid_stamp(""));
        assert!(!is_valid_stamp(".."));
        assert!(!is_valid_stamp("../x"));
        assert!(!is_valid_stamp("a/b"));
        assert!(!is_valid_stamp("2026-08-25T10-14-02Z.json"));
        assert!(!is_valid_stamp(&"1".repeat(33)));
    }

    #[tokio::test]
    async fn snapshots_list_newest_first_with_sizes() {
        let directory = tempfile::tempdir().unwrap();
        let store = HistoryStore::new(directory.path().to_path_buf());
        store.snapshot_at("design", 100, b"one").await.unwrap();
        store.snapshot_at("design", 300, b"three").await.unwrap();
        store.snapshot_at("design", 200, b"two").await.unwrap();
        let rows = store.list("design").await.unwrap();
        let stamps: Vec<&str> = rows.iter().map(|row| row.stamp.as_str()).collect();
        assert_eq!(
            stamps,
            [
                "1970-01-01T00-05-00Z",
                "1970-01-01T00-03-20Z",
                "1970-01-01T00-01-40Z"
            ]
        );
        assert_eq!(rows[0].saved_at, "1970-01-01T00:05:00Z");
        assert_eq!(rows[0].size_bytes, 5);
        assert!(store.list("other").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn two_snapshots_in_one_second_both_survive() {
        let directory = tempfile::tempdir().unwrap();
        let store = HistoryStore::new(directory.path().to_path_buf());
        let first = store.snapshot_at("design", 60, b"a").await.unwrap();
        let second = store.snapshot_at("design", 60, b"b").await.unwrap();
        assert_eq!(first, "1970-01-01T00-01-00Z");
        assert_eq!(second, "1970-01-01T00-01-00Z-2");
        let rows = store.list("design").await.unwrap();
        assert_eq!(rows[0].stamp, second);
        assert_eq!(store.read("design", &second).await.unwrap().unwrap(), b"b");
    }

    #[tokio::test]
    async fn the_store_keeps_fifty_snapshots_and_deletes_the_oldest() {
        let directory = tempfile::tempdir().unwrap();
        let store = HistoryStore::new(directory.path().to_path_buf());
        for second in 1..=51u64 {
            store
                .snapshot_at("design", second, second.to_string().as_bytes())
                .await
                .unwrap();
        }
        let rows = store.list("design").await.unwrap();
        assert_eq!(rows.len(), SNAPSHOT_LIMIT);
        assert_eq!(rows[0].stamp, stamp_from_unix(51));
        assert_eq!(rows[49].stamp, stamp_from_unix(2));
        assert!(
            store
                .read("design", &stamp_from_unix(1))
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn read_returns_none_for_unknown_or_unsafe_stamps() {
        let directory = tempfile::tempdir().unwrap();
        let store = HistoryStore::new(directory.path().to_path_buf());
        store.snapshot_at("design", 5, b"x").await.unwrap();
        assert!(
            store
                .read("design", "2000-01-01T00-00-00Z")
                .await
                .unwrap()
                .is_none()
        );
        assert!(store.read("design", "../design").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn rename_moves_the_history_directory() {
        let directory = tempfile::tempdir().unwrap();
        let store = HistoryStore::new(directory.path().to_path_buf());
        store.snapshot_at("old", 7, b"x").await.unwrap();
        store.rename("old", "new").await.unwrap();
        assert!(store.list("old").await.unwrap().is_empty());
        assert_eq!(store.list("new").await.unwrap().len(), 1);
        store.rename("absent", "elsewhere").await.unwrap();
        assert!(!directory.path().join("elsewhere").exists());
    }
}
