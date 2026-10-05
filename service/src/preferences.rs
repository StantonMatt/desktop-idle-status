// SPDX-FileCopyrightText: 2026 Matthew Stanton
// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    collections::BTreeSet,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

#[derive(Default)]
pub struct Preferences {
    path: Option<PathBuf>,
    pub ignored: BTreeSet<String>,
}
impl Preferences {
    pub fn load(config_home: &Path) -> io::Result<Self> {
        let path = config_home.join("desktop-idle-statusrc");
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e),
        };
        let mut ignored = BTreeSet::new();
        for line in text
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#') && *l != "[IgnoredApps]")
        {
            let encoded = line.strip_suffix("=true").ok_or_else(invalid)?;
            if encoded.len() % 2 != 0 {
                return Err(invalid());
            }
            let bytes = (0..encoded.len())
                .step_by(2)
                .map(|i| {
                    encoded
                        .get(i..i + 2)
                        .and_then(|v| u8::from_str_radix(v, 16).ok())
                        .ok_or_else(invalid)
                })
                .collect::<io::Result<Vec<_>>>()?;
            let id = String::from_utf8(bytes).map_err(|_| invalid())?;
            if !valid_id(&id) {
                return Err(invalid());
            }
            ignored.insert(id);
        }
        Ok(Self {
            path: Some(path),
            ignored,
        })
    }
    pub fn set(&mut self, id: &str, ignored: bool) -> io::Result<bool> {
        if !valid_id(id) {
            return Err(invalid());
        }
        let mut next = self.ignored.clone();
        if ignored {
            next.insert(id.into());
        } else {
            next.remove(id);
        }
        if next == self.ignored {
            return Ok(false);
        }
        if let Some(path) = &self.path {
            fs::create_dir_all(path.parent().unwrap())?;
            let temporary = path.with_extension(format!(
                "tmp-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            let mut created = false;
            let result = (|| {
                use std::os::unix::fs::OpenOptionsExt;
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&temporary)?;
                created = true;
                file.write_all(b"[IgnoredApps]\n")?;
                for id in &next {
                    for byte in id.as_bytes() {
                        write!(file, "{byte:02x}")?;
                    }
                    file.write_all(b"=true\n")?;
                }
                file.sync_all()?;
                fs::rename(&temporary, path)
            })();
            if created && result.is_err() {
                let _ = fs::remove_file(&temporary);
            }
            result?;
        }
        self.ignored = next;
        Ok(true)
    }
}
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 4096 && !id.chars().any(char::is_control)
}
fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid ignored app identity")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trip_and_failed_write_are_transactional() {
        let root = crate::paths::test_root();
        fs::create_dir_all(&root).unwrap();
        let dir = tempfile::tempdir_in(root).unwrap();
        let mut p = Preferences::load(dir.path()).unwrap();
        for id in ["org.example.App", "app,=é%"] {
            assert!(p.set(id, true).unwrap());
        }
        assert!(!p.set("org.example.App", true).unwrap());
        assert_eq!(Preferences::load(dir.path()).unwrap().ignored, p.ignored);
        p.set("org.example.App", false).unwrap();
        assert!(
            !Preferences::load(dir.path())
                .unwrap()
                .ignored
                .contains("org.example.App")
        );
        assert!(p.set("", true).is_err());
        fs::remove_file(dir.path().join("desktop-idle-statusrc")).unwrap();
        fs::create_dir(dir.path().join("desktop-idle-statusrc")).unwrap();
        assert!(p.set("another", true).is_err());
        assert!(!p.ignored.contains("another"));
    }
}
