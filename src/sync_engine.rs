use crate::sync_dav::{Dav, hash, safe_path};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Record {
    pub hash: String,
    pub etag: String,
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct State {
    pub files: BTreeMap<String, Record>,
}
#[derive(Clone)]
pub struct Change {
    pub modified: Option<std::time::SystemTime>,
    pub path: String,
    pub expected: Option<String>,
    pub bytes: Option<Vec<u8>>,
    pub record: Option<Record>,
}
pub struct Outcome {
    pub state: State,
    pub changes: Vec<Change>,
    pub conflicts: Vec<String>,
}

impl Outcome {
    fn queue(&mut self, change: Change, progress: &mut impl FnMut(&Change) -> bool) {
        if progress(&change) {
            if let Some(record) = change.record {
                self.state.files.insert(change.path, record);
            } else {
                self.state.files.remove(&change.path);
            }
        } else {
            self.changes.push(change);
        }
    }
}

pub fn snapshot(root: &Path) -> Result<BTreeMap<String, Vec<u8>>, String> {
    fn visit(
        root: &Path,
        directory: &Path,
        files: &mut BTreeMap<String, Vec<u8>>,
    ) -> Result<(), String> {
        if !directory.exists() {
            return Ok(());
        }
        for entry in std::fs::read_dir(directory).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                visit(root, &entry.path(), files)?;
            }
            if kind.is_file() {
                let path = entry
                    .path()
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace(std::path::MAIN_SEPARATOR, "/");
                if !safe_path(&path) {
                    return Err("Unsafe local filename".into());
                }
                files.insert(
                    path,
                    std::fs::read(entry.path()).map_err(|e| e.to_string())?,
                );
            }
        }
        Ok(())
    }
    let mut files = BTreeMap::new();
    visit(root, root, &mut files)?;
    Ok(files)
}
fn note(path: &str) -> bool {
    matches!(
        Path::new(path).extension().and_then(|ext| ext.to_str()),
        Some("md" | "markdown" | "txt" | "text")
    )
}
fn conflict_path(path: &str, bytes: &[u8]) -> String {
    let path = Path::new(path);
    let stem = path.file_stem().unwrap().to_string_lossy();
    let suffix = path
        .extension()
        .map(|ext| format!(".{}", ext.to_string_lossy()))
        .unwrap_or_default();
    path.with_file_name(format!(
        "{stem}.conflict-remote-{}{suffix}",
        &hash(bytes)[..12]
    ))
    .to_string_lossy()
    .to_string()
}
fn preserve_remote(
    dav: &Dav,
    path: &str,
    bytes: &[u8],
    local: &BTreeMap<String, Vec<u8>>,
    outcome: &mut Outcome,
    progress: &mut impl FnMut(&Change) -> bool,
) -> Result<(), String> {
    let mut copy = conflict_path(path, bytes);
    // Never reuse a conflicting local filename for different content.
    if local.get(&copy).is_some_and(|existing| existing != bytes) {
        copy = format!("{}-{}", copy, gtk::glib::uuid_string_random());
    }
    let response = dav.request("GET", &copy, &[], None)?;
    let etag = if response.status == 404 {
        dav.put(&copy, bytes, None)?
    } else if response.status == 200 && response.body == bytes {
        response.etag.ok_or("Missing conflict-copy ETag")?
    } else {
        return Err(format!(
            "Could not safely preserve the remote conflict for {path}"
        ));
    };
    outcome.queue(
        Change {
            modified: None,
            path: copy.clone(),
            expected: local.get(&copy).map(|bytes| hash(bytes)),
            bytes: Some(bytes.to_vec()),
            record: Some(Record {
                hash: hash(bytes),
                etag,
            }),
        },
        progress,
    );
    outcome.conflicts.push(copy);
    Ok(())
}

#[cfg(test)]
pub fn cycle(dav: &Dav, local: BTreeMap<String, Vec<u8>>, state: State) -> Result<Outcome, String> {
    cycle_with_progress(dav, local, state, |_| false)
}

pub fn cycle_with_progress(
    dav: &Dav,
    local: BTreeMap<String, Vec<u8>>,
    state: State,
    mut progress: impl FnMut(&Change) -> bool,
) -> Result<Outcome, String> {
    let remote = dav.list()?; // Never infer deletion from a failed/partial listing.
    let mut paths: Vec<_> = local
        .keys()
        .chain(remote.keys())
        .chain(state.files.keys())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    paths.sort_by_key(|path| (!local.contains_key(path), note(path), path.clone())); // Attachments before referencing notes.
    let mut outcome = Outcome {
        state,
        changes: Vec::new(),
        conflicts: Vec::new(),
    };
    for path in paths {
        if !safe_path(&path) {
            return Err("Invalid path in sync history".into());
        }
        let bytes = local.get(&path);
        let remote_file = remote.get(&path);
        let base = outcome.state.files.get(&path).cloned();
        let local_hash = bytes.map(|bytes| hash(bytes));
        match (bytes, remote_file, base) {
            (Some(bytes), Some(remote), base) => {
                let local_changed = base
                    .as_ref()
                    .is_none_or(|base| Some(&base.hash) != local_hash.as_ref());
                let remote_changed = base.as_ref().is_none_or(|base| base.etag != remote.etag);
                if !local_changed && !remote_changed {
                    continue;
                }
                if remote_changed {
                    let (remote_bytes, etag) = dav.get(&path)?;
                    let remote_hash = hash(&remote_bytes);
                    if remote_hash == local_hash.clone().unwrap() {
                        outcome.state.files.insert(
                            path,
                            Record {
                                hash: remote_hash,
                                etag,
                            },
                        );
                        continue;
                    }
                    if !local_changed {
                        outcome.queue(
                            Change {
                                modified: (etag == remote.etag)
                                    .then_some(remote.modified)
                                    .flatten(),
                                path,
                                expected: local_hash,
                                bytes: Some(remote_bytes),
                                record: Some(Record {
                                    hash: remote_hash,
                                    etag,
                                }),
                            },
                            &mut progress,
                        );
                        continue;
                    }
                    // Both changed: preserve the remote version in BOTH vaults
                    // before publishing the local version at its original name.
                    preserve_remote(
                        dav,
                        &path,
                        &remote_bytes,
                        &local,
                        &mut outcome,
                        &mut progress,
                    )?;
                    let uploaded = dav.put(&path, bytes, Some(&etag))?;
                    outcome.state.files.insert(
                        path,
                        Record {
                            hash: local_hash.unwrap(),
                            etag: uploaded,
                        },
                    );
                } else {
                    let etag = dav.put(&path, bytes, Some(&remote.etag))?;
                    outcome.state.files.insert(
                        path,
                        Record {
                            hash: local_hash.unwrap(),
                            etag,
                        },
                    );
                }
            }
            (Some(_), None, Some(base)) if local_hash.as_ref() == Some(&base.hash) => {
                outcome.queue(
                    Change {
                        modified: None,
                        path,
                        expected: local_hash,
                        bytes: None,
                        record: None,
                    },
                    &mut progress,
                );
            }
            (Some(bytes), None, base) => {
                if base.is_some() {
                    outcome
                        .conflicts
                        .push(format!("{path} (local edit kept after remote deletion)"));
                }
                let etag = dav.put(&path, bytes, None)?;
                outcome.state.files.insert(
                    path,
                    Record {
                        hash: hash(bytes),
                        etag,
                    },
                );
            }
            (None, Some(remote), Some(base)) if base.etag == remote.etag => {
                dav.delete(&path, &base.etag)?;
                outcome.state.files.remove(&path);
            }
            (None, Some(remote), base) => {
                let (bytes, etag) = dav.get(&path)?;
                if base.is_some() {
                    outcome
                        .conflicts
                        .push(format!("{path} (remote edit kept after local deletion)"));
                }
                outcome.queue(
                    Change {
                        modified: (etag == remote.etag).then_some(remote.modified).flatten(),
                        path,
                        expected: None,
                        record: Some(Record {
                            hash: hash(&bytes),
                            etag,
                        }),
                        bytes: Some(bytes),
                    },
                    &mut progress,
                );
            }
            (None, None, _) => {
                outcome.state.files.remove(&path);
            }
        }
    }
    Ok(outcome)
}

pub fn apply(root: &Path, change: &Change) -> Result<bool, String> {
    if !safe_path(&change.path) {
        return Err("Unsafe sync destination".into());
    }
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let path = root.join(&change.path);
    // Reject symlinks in every component, including the target file.
    let mut ancestor = root.to_path_buf();
    for part in Path::new(&change.path).components() {
        ancestor.push(part);
        if std::fs::symlink_metadata(&ancestor)
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            return Err("Sync destination contains a symbolic link".into());
        }
    }
    let actual = match std::fs::read(&path) {
        Ok(bytes) => Some(hash(&bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.to_string()),
    };
    if actual != change.expected {
        return Ok(false);
    } // A newer local save wins.
    if let Some(bytes) = &change.bytes {
        std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        use gtk::gio::prelude::*;
        gtk::gio::File::for_path(&path)
            .replace_contents(
                bytes,
                None,
                false,
                gtk::gio::FileCreateFlags::REPLACE_DESTINATION,
                gtk::gio::Cancellable::NONE,
            )
            .map_err(|e| e.to_string())?;
        if let Some(modified) = change.modified {
            std::fs::File::options()
                .write(true)
                .open(&path)
                .and_then(|file| file.set_times(std::fs::FileTimes::new().set_modified(modified)))
                .map_err(|e| format!("Could not preserve modification time: {e}"))?;
        }
    } else if actual.is_some() {
        use gtk::gio::prelude::*;
        gtk::gio::File::for_path(&path)
            .trash(gtk::gio::Cancellable::NONE)
            .map_err(|e| format!("Could not safely trash remotely deleted note: {e}"))?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn download_preserves_remote_modification_order() {
        let root = std::env::temp_dir().join(format!(
            "hematite-times-{}",
            gtk::glib::uuid_string_random()
        ));
        let old = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        let new = old + std::time::Duration::from_secs(100);
        for (name, modified) in [("new.md", new), ("old.md", old)] {
            assert!(
                apply(
                    &root,
                    &Change {
                        path: name.into(),
                        modified: Some(modified),
                        expected: None,
                        bytes: Some(b"note".to_vec()),
                        record: None
                    }
                )
                .unwrap()
            );
            assert_eq!(
                std::fs::metadata(root.join(name))
                    .unwrap()
                    .modified()
                    .unwrap(),
                modified
            );
        }
        let entries = crate::vault::entries(&root).unwrap();
        assert_eq!(entries[0].path.file_name().unwrap(), "new.md");
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn applying_download_does_not_replace_a_newer_local_save() {
        let root = std::env::temp_dir().join(format!("hematite-sync-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("note.md"), b"new local edit").unwrap();
        let change = Change {
            modified: None,
            path: "note.md".into(),
            expected: Some(hash(b"old")),
            bytes: Some(b"remote".to_vec()),
            record: None,
        };
        assert!(!apply(&root, &change).unwrap());
        assert_eq!(
            std::fs::read(root.join("note.md")).unwrap(),
            b"new local edit"
        );
        let malicious = Change {
            modified: None,
            path: "../outside".into(),
            expected: None,
            bytes: Some(vec![]),
            record: None,
        };
        assert!(apply(&root, &malicious).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn conflict_names_keep_extension() {
        assert!(
            conflict_path("nested/note.md", b"remote").starts_with("nested/note.conflict-remote-")
        );
        assert!(conflict_path("nested/note.md", b"remote").ends_with(".md"));
    }

    #[test]
    #[ignore = "Requires an explicitly supplied scratch WebDAV server"]
    fn live_webdav_round_trip() {
        let url = std::env::var("HEMATITE_TEST_WEBDAV_URL").unwrap();
        let username = std::env::var("HEMATITE_TEST_WEBDAV_USER").unwrap();
        let password = std::env::var("HEMATITE_TEST_WEBDAV_PASSWORD").unwrap();
        let folder = format!("hematite-sync-test-{}", gtk::glib::uuid_string_random());
        let base = Dav {
            url,
            username,
            password,
            lock_writes: false,
        };
        base.checked("MKCOL", &folder, &[], None).unwrap();
        let root = std::env::temp_dir().join(&folder);
        std::fs::create_dir(&root).unwrap();
        let test = std::panic::catch_unwind(|| {
            let mut dav = base.clone();
            dav.url.push_str(&format!("{folder}/"));
            dav.probe().unwrap();
            assert!(
                dav.lock_writes,
                "This test expects the provided rclone server's lock protection"
            );
            let mut state = State::default();
            let update = |state: &mut State| {
                let mut outcome = cycle(&dav, snapshot(&root).unwrap(), state.clone()).unwrap();
                for change in &outcome.changes {
                    if change.bytes.is_none() {
                        // Assert remote deletion is planned, then emulate successful
                        // local trash in this /tmp test (desktop Trash is unavailable).
                        let path = root.join(&change.path);
                        assert_eq!(
                            std::fs::read(&path).ok().as_ref().map(|bytes| hash(bytes)),
                            change.expected
                        );
                        std::fs::remove_file(path).unwrap();
                    } else {
                        assert!(apply(&root, change).unwrap());
                    }
                    if let Some(record) = &change.record {
                        outcome
                            .state
                            .files
                            .insert(change.path.clone(), record.clone());
                    } else {
                        outcome.state.files.remove(&change.path);
                    }
                }
                *state =
                    serde_json::from_slice(&serde_json::to_vec(&outcome.state).unwrap()).unwrap();
                outcome.conflicts
            };
            dav.put("remote.md", b"remote initial", None).unwrap();
            update(&mut state);
            assert_eq!(
                std::fs::read(root.join("remote.md")).unwrap(),
                b"remote initial"
            );
            std::fs::write(root.join("remote.md"), b"local save").unwrap();
            update(&mut state);
            assert_eq!(dav.get("remote.md").unwrap().0, b"local save");
            let stale = state.files["remote.md"].etag.clone();
            let tag = dav.get("remote.md").unwrap().1;
            dav.put("remote.md", b"another device", Some(&tag)).unwrap();
            assert!(
                dav.put("remote.md", b"stale overwrite", Some(&stale))
                    .is_err()
            );
            assert_eq!(dav.get("remote.md").unwrap().0, b"another device");
            update(&mut state);
            assert_eq!(
                std::fs::read(root.join("remote.md")).unwrap(),
                b"another device"
            );
            std::fs::write(root.join("remote.md"), b"local conflict").unwrap();
            let tag = dav.get("remote.md").unwrap().1;
            dav.put("remote.md", b"remote conflict", Some(&tag))
                .unwrap();
            let conflicts = update(&mut state);
            assert_eq!(conflicts.len(), 1);
            assert_eq!(dav.get("remote.md").unwrap().0, b"local conflict");
            assert_eq!(dav.get(&conflicts[0]).unwrap().0, b"remote conflict");
            assert_eq!(
                std::fs::read(root.join(&conflicts[0])).unwrap(),
                b"remote conflict"
            );
            std::fs::create_dir(root.join("nested")).unwrap();
            std::fs::write(root.join("nested/café & [name].md"), b"unicode path").unwrap();
            update(&mut state);
            assert_eq!(
                dav.get("nested/café & [name].md").unwrap().0,
                b"unicode path"
            );
            update(&mut state); // Verify subsequent XML listing decodes these names.
            std::fs::rename(root.join("remote.md"), root.join("renamed.md")).unwrap();
            update(&mut state);
            assert_eq!(
                dav.request("GET", "remote.md", &[], None).unwrap().status,
                404
            );
            assert_eq!(dav.get("renamed.md").unwrap().0, b"local conflict");
            let tag = dav.get("renamed.md").unwrap().1;
            dav.delete("renamed.md", &tag).unwrap();
            update(&mut state);
            assert!(!root.join("renamed.md").exists());
            std::fs::write(root.join("offline.md"), b"pending local save").unwrap();
            let mut unavailable = dav.clone();
            unavailable.url = "http://127.0.0.1:1/".into();
            assert!(cycle(&unavailable, snapshot(&root).unwrap(), state.clone()).is_err());
            assert_eq!(
                std::fs::read(root.join("offline.md")).unwrap(),
                b"pending local save"
            );
            update(&mut state);
            assert_eq!(dav.get("offline.md").unwrap().0, b"pending local save");
            println!(
                "Verified bootstrap, upload, download, stale-write protection, conflict copies, Unicode paths, rename, deletion, and offline recovery."
            );
        });
        let cleanup = base.request("DELETE", &folder, &[], None).unwrap();
        assert!(
            (200..300).contains(&cleanup.status),
            "Remote scratch cleanup failed"
        );
        std::fs::remove_dir_all(root).unwrap();
        if let Err(panic) = test {
            std::panic::resume_unwind(panic);
        }
    }
}
