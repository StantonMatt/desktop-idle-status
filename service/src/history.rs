//! Per-window away intervals and retention, independent of clocks and D-Bus.
use crate::model::Blocker;
use rusqlite::{Connection, params};
use std::{collections::HashMap, path::Path};

pub const RETENTION: i64 = 7 * 24 * 60 * 60;

/// Process-level second guard, including processes on different session buses.
/// Never unlink the lock file: all contenders must lock the same inode.
pub fn lock_database(data_home: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let dir = data_home.join("desktop-idle-status");
    std::fs::create_dir_all(&dir)?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(dir.join("history.lock"))?;
    file.try_lock().map_err(|e| {
        std::io::Error::other(format!(
            "Cannot lock history database (another instance may already be running): {e}"
        ))
    })?;
    Ok(file)
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub start: i64,
    pub end: i64,
    pub blocker: Blocker,
}
#[derive(Default)]
pub struct Tracker {
    active: HashMap<String, (i64, Blocker)>,
    returned: Vec<Entry>,
    pending: Vec<Entry>,
}
impl Tracker {
    pub fn update(&mut self, now: i64, away: bool, blockers: &[Blocker]) -> Vec<Entry> {
        let ids: std::collections::HashSet<_> =
            blockers.iter().map(|b| b.internal_id.as_str()).collect();
        let ended: Vec<_> = self
            .active
            .keys()
            .filter(|id| !away || !ids.contains(id.as_str()))
            .cloned()
            .collect();
        let mut entries = Vec::new();
        for id in ended {
            let (start, blocker) = self.active.remove(&id).expect("existing active interval");
            if now - start >= 60 {
                let entry = Entry {
                    start,
                    end: now,
                    blocker,
                };
                self.pending.push(entry.clone());
                self.returned.push(entry.clone());
                entries.push(entry);
            }
        }
        if away {
            for b in blockers {
                self.active
                    .entry(b.internal_id.clone())
                    .or_insert_with(|| (now, b.clone()));
            }
        }
        entries
    }
    pub fn take_return_notice(&mut self) -> Option<(i64, Vec<Entry>)> {
        let mut entries = std::mem::take(&mut self.returned);
        entries.sort_by(|a, b| {
            (&a.start, &a.blocker.app_name, &a.blocker.caption).cmp(&(
                &b.start,
                &b.blocker.app_name,
                &b.blocker.caption,
            ))
        });
        if entries.is_empty() {
            return None;
        }
        let total = union_seconds(&entries);
        Some((total, entries))
    }
    /// End observations without carrying a notice across an unobserved input gap.
    pub fn reset(&mut self, now: i64) -> Vec<Entry> {
        let entries = self.update(now, false, &[]);
        self.active.clear();
        self.returned.clear();
        entries
    }
    pub fn pending(&self) -> &[Entry] {
        &self.pending
    }
    pub fn committed(&mut self) {
        self.pending.clear();
    }
    pub fn clear(&mut self) {
        self.pending.clear();
        self.active.clear();
        self.returned.clear();
    }
}
/// Union overlapping windows: two blockers for an hour means an hour, not two.
pub fn union_seconds(entries: &[Entry]) -> i64 {
    let mut ranges: Vec<_> = entries.iter().map(|e| (e.start, e.end)).collect();
    ranges.sort_unstable();
    let Some(mut current) = ranges.first().copied() else {
        return 0;
    };
    let mut total = 0;
    for (start, end) in ranges.into_iter().skip(1) {
        if start <= current.1 {
            current.1 = current.1.max(end);
        } else {
            total += current.1 - current.0;
            current = (start, end);
        }
    }
    total + current.1 - current.0
}
pub struct Store {
    db: Connection,
}
impl Store {
    pub fn open(data_home: &Path, now: i64) -> rusqlite::Result<Self> {
        let dir = data_home.join("desktop-idle-status");
        std::fs::create_dir_all(&dir)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        }
        let path = dir.join("history.sqlite3");
        let db = Connection::open(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        }
        db.execute_batch("PRAGMA secure_delete=ON;")?;
        // Wipe pages freed by older versions that used ordinary DELETE.
        let privacy_version: i64 = db.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if privacy_version < 1 {
            db.execute_batch("VACUUM; PRAGMA user_version=1;")?;
        }
        db.execute_batch("CREATE TABLE IF NOT EXISTS history (start INTEGER NOT NULL,end INTEGER NOT NULL,internal_id TEXT NOT NULL,app_id TEXT NOT NULL,app_name TEXT NOT NULL,icon_name TEXT NOT NULL,caption TEXT NOT NULL); CREATE INDEX IF NOT EXISTS history_start ON history(start);")?;
        let this = Self { db };
        this.trim(now)?;
        Ok(this)
    }
    pub fn trim(&self, now: i64) -> rusqlite::Result<bool> {
        Ok(self
            .db
            .execute("DELETE FROM history WHERE end < ?1", [now - RETENTION])?
            > 0)
    }
    pub fn write(&mut self, entries: &[Entry], now: i64) -> rusqlite::Result<bool> {
        let tx = self.db.transaction()?;
        for e in entries {
            tx.execute(
                "INSERT INTO history VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![
                    e.start,
                    e.end,
                    e.blocker.internal_id,
                    e.blocker.app_id,
                    e.blocker.app_name,
                    e.blocker.icon_name,
                    e.blocker.caption
                ],
            )?;
        }
        let removed = tx.execute("DELETE FROM history WHERE end < ?1", [now - RETENTION])?;
        tx.commit()?;
        Ok(!entries.is_empty() || removed > 0)
    }
    pub fn history(&self, days: u32, now: i64) -> rusqlite::Result<Vec<Entry>> {
        let cutoff = now - i64::from(days.min(7)) * 86400;
        let mut stmt=self.db.prepare("SELECT start,end,internal_id,app_id,app_name,icon_name,caption FROM history WHERE end >= ?1 AND end >= ?2 ORDER BY start DESC,app_name,caption")?;
        stmt.query_map([cutoff, now - RETENTION], |row| {
            Ok(Entry {
                start: row.get(0)?,
                end: row.get(1)?,
                blocker: Blocker {
                    internal_id: row.get(2)?,
                    app_id: row.get(3)?,
                    app_name: row.get(4)?,
                    icon_name: row.get(5)?,
                    caption: row.get(6)?,
                    since: row.get(0)?,
                },
            })
        })?
        .collect()
    }
    pub fn clear(&self) -> rusqlite::Result<()> {
        self.db.execute("DELETE FROM history", [])?;
        // secure_delete already erased the deleted cells. Compaction is best
        // effort: its failure cannot undo the committed logical deletion.
        if let Err(e) = self.db.execute_batch("VACUUM;") {
            eprintln!("History deleted; compaction failed: {e}");
        }
        Ok(())
    }
}
pub fn duration(seconds: i64) -> String {
    let minutes = (seconds + 30) / 60;
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}
#[cfg(test)]
pub fn test_dir() -> tempfile::TempDir {
    let base = crate::paths::test_root();
    std::fs::create_dir_all(&base).unwrap();
    tempfile::tempdir_in(base).unwrap()
}
#[cfg(test)]
mod tests {
    use super::*;
    fn blocker(id: &str) -> Blocker {
        Blocker {
            internal_id: id.into(),
            app_id: "app".into(),
            app_name: "App".into(),
            icon_name: "icon".into(),
            caption: "Original".into(),
            since: 0,
        }
    }
    #[test]
    fn clear_commits_delete_even_when_active_statement_prevents_vacuum() {
        let dir = test_dir();
        let mut store = Store::open(dir.path(), 100).unwrap();
        store
            .write(
                &[Entry {
                    start: 100,
                    end: 700,
                    blocker: blocker("a"),
                }],
                700,
            )
            .unwrap();
        let mut statement = store.db.prepare("SELECT 1 UNION ALL SELECT 2").unwrap();
        let mut rows = statement.query([]).unwrap();
        assert!(rows.next().unwrap().is_some());
        assert!(store.db.execute_batch("VACUUM").is_err());
        store.clear().unwrap();
        assert!(store.history(7, 700).unwrap().is_empty());
        drop(rows);
        drop(statement);
        store.db.execute_batch("VACUUM").unwrap();
    }
    #[test]
    fn return_notice_is_consumed_once_and_clear_cancels_pending() {
        let mut t = Tracker::default();
        t.update(100, true, &[blocker("a"), blocker("b")]);
        t.update(700, false, &[]);
        assert_eq!(t.take_return_notice().unwrap().0, 600);
        assert!(t.take_return_notice().is_none());
        t.update(800, true, &[blocker("a")]);
        t.update(1400, false, &[]);
        t.clear();
        assert!(t.take_return_notice().is_none());
    }
    #[test]
    fn titles_are_captured_and_noise_ignored() {
        let mut t = Tracker::default();
        let b = blocker("1");
        assert!(t.update(100, true, std::slice::from_ref(&b)).is_empty());
        let mut changed = b.clone();
        changed.caption = "Changed".into();
        t.update(130, true, &[changed]);
        assert_eq!(t.update(160, false, &[])[0].blocker.caption, "Original");
        assert_eq!(t.take_return_notice().unwrap().0, 60);
        t.update(200, true, &[b]);
        assert!(t.update(259, false, &[]).is_empty());
        assert!(t.take_return_notice().is_none());
    }
    #[test]
    fn releases_new_windows_and_union_duration() {
        let mut t = Tracker::default();
        let a = blocker("a");
        let b = blocker("b");
        t.update(0, true, std::slice::from_ref(&a));
        t.update(30, true, &[a, b.clone()]);
        assert_eq!(t.update(90, true, std::slice::from_ref(&b)).len(), 1);
        assert_eq!(t.update(150, true, &[]).len(), 1);
        assert_eq!(t.take_return_notice().unwrap().0, 150);
    }
    #[test]
    fn disconnect_discards_previous_return_period() {
        let mut t = Tracker::default();
        t.update(100, true, &[blocker("old-released"), blocker("old-active")]);
        t.update(700, true, &[blocker("old-active")]);
        assert_eq!(t.reset(800).len(), 1);
        // Input during the gap is unseen. Reconnection starts a distinct period.
        t.update(1000, true, &[blocker("new")]);
        t.update(1600, false, &[]);
        let (seconds, rows) = t.take_return_notice().unwrap();
        assert_eq!(seconds, 600);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].blocker.internal_id, "new");
    }
    #[test]
    fn deletions_and_migration_wipe_captions() {
        let dir = test_dir();
        let path = dir.path().join("desktop-idle-status/history.sqlite3");
        let mut store = Store::open(dir.path(), 1000).unwrap();
        let mut b = blocker("a");
        b.caption = "PRIVATE-CAPTION-".repeat(300);
        let entry = Entry {
            start: 0,
            end: 1000,
            blocker: b,
        };
        store.write(std::slice::from_ref(&entry), 1000).unwrap();
        assert!(store.write(&[], 1001 + RETENTION).unwrap());
        assert!(!store.write(&[], 1001 + RETENTION).unwrap());
        let contains_title = || {
            std::fs::read(&path)
                .unwrap()
                .windows(b"PRIVATE-CAPTION-P".len())
                .any(|w| w == b"PRIVATE-CAPTION-P")
        };
        assert!(!contains_title(), "retention wipes deleted cells");
        store.write(std::slice::from_ref(&entry), 1000).unwrap();
        assert!(store.trim(1001 + RETENTION).unwrap());
        assert!(!store.trim(1001 + RETENTION).unwrap());
        assert!(!contains_title(), "standalone trim wipes captions");
        store.write(std::slice::from_ref(&entry), 1000).unwrap();
        store.clear().unwrap();
        assert!(!contains_title(), "clear wipes captions");
        store
            .db
            .execute_batch("PRAGMA secure_delete=OFF; PRAGMA user_version=0;")
            .unwrap();
        store.write(&[entry], 1000).unwrap();
        store.db.execute("DELETE FROM history", []).unwrap();
        assert!(contains_title(), "legacy pages contain the fixture");
        drop(store);
        let _store = Store::open(dir.path(), 1000).unwrap();
        assert!(!contains_title(), "migration wipes legacy free pages");
    }
    #[test]
    fn persistence_retention_and_clear() {
        let dir = test_dir();
        let now = RETENTION + 1000;
        let mut store = Store::open(dir.path(), now).unwrap();
        store
            .write(
                &[
                    Entry {
                        start: now - 100,
                        end: now,
                        blocker: blocker("a"),
                    },
                    Entry {
                        start: 0,
                        end: 100,
                        blocker: blocker("b"),
                    },
                ],
                now,
            )
            .unwrap();
        assert_eq!(store.history(7, now).unwrap().len(), 1);
        drop(store);
        let store = Store::open(dir.path(), now).unwrap();
        assert_eq!(
            store.history(7, now).unwrap()[0].blocker.caption,
            "Original"
        );
        store.clear().unwrap();
        assert!(store.history(7, now).unwrap().is_empty());
    }
}
