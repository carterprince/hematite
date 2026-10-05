use gtk::gio::{self, prelude::*};
use std::{
    cmp::Reverse,
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct Entry {
    pub path: PathBuf,
    pub directory: bool,
    pub depth: usize,
}

pub fn entries(root: &Path) -> io::Result<Vec<Entry>> {
    fn visit(root: &Path, depth: usize, files: &mut Vec<Entry>) -> io::Result<()> {
        let mut children = fs::read_dir(root)?.collect::<io::Result<Vec<_>>>()?;
        children.sort_by_cached_key(|entry| {
            (
                Reverse(
                    entry
                        .metadata()
                        .and_then(|metadata| metadata.modified())
                        .ok(),
                ),
                entry.file_name().to_string_lossy().to_lowercase(),
                entry.file_name(),
            )
        });
        for entry in children {
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let kind = entry.file_type()?;
            if kind.is_dir() {
                files.push(Entry {
                    path: entry.path(),
                    directory: true,
                    depth,
                });
                visit(&entry.path(), depth + 1, files)?;
            } else if kind.is_file()
                && matches!(
                    entry
                        .path()
                        .extension()
                        .and_then(|s| s.to_str())
                        .map(str::to_ascii_lowercase)
                        .as_deref(),
                    Some("md" | "txt" | "markdown" | "text")
                )
            {
                files.push(Entry {
                    path: entry.path(),
                    directory: false,
                    depth,
                });
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    visit(root, 0, &mut files)?;
    Ok(files)
}

// Refuse to overwrite a note changed by another application since it was opened.
pub fn save(path: &Path, original: &str, text: &str) -> io::Result<()> {
    if fs::read_to_string(path)? != original {
        return Err(io::Error::other(
            "This note changed on disk. Your edits are still in the editor; copy them before reopening the note.",
        ));
    }
    // GIO replaces the local file through a temporary file rather than truncating
    // the original before the write succeeds.
    gio::File::for_path(path)
        .replace_contents(
            text.as_bytes(),
            None,
            false,
            gio::FileCreateFlags::NONE,
            gio::Cancellable::NONE,
        )
        .map(|_| ())
        .map_err(io::Error::other)
}

pub fn validate_note(root: &Path, path: &Path) -> io::Result<()> {
    if !fs::symlink_metadata(path)?.is_file()
        || !path.canonicalize()?.starts_with(root.canonicalize()?)
    {
        return Err(io::Error::other(
            "Only regular notes inside this vault can be moved to Trash.",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sorts_newest_first_within_each_folder() {
        use std::{
            fs::{File, FileTimes},
            time::{Duration, UNIX_EPOCH},
        };
        let root = std::env::temp_dir().join(format!("hematite-sort-test-{}", std::process::id()));
        fs::create_dir_all(root.join("folder")).unwrap();
        for (name, seconds) in [
            ("old.md", 10),
            ("new.md", 30),
            ("b.md", 20),
            ("a.md", 20),
            ("folder/old.md", 10),
            ("folder/new.md", 30),
        ] {
            fs::write(root.join(name), name).unwrap();
            File::open(root.join(name))
                .unwrap()
                .set_times(FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(seconds)))
                .unwrap();
        }
        File::open(root.join("folder"))
            .unwrap()
            .set_times(FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(25)))
            .unwrap();
        let ordered: Vec<_> = entries(&root)
            .unwrap()
            .iter()
            .map(|entry| {
                entry
                    .path
                    .strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(
            ordered,
            [
                "new.md",
                "folder",
                "folder/new.md",
                "folder/old.md",
                "a.md",
                "b.md",
                "old.md"
            ]
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn discovers_notes_and_prevents_external_overwrites() {
        let root = std::env::temp_dir().join(format!("hematite-test-{}", std::process::id()));
        fs::create_dir_all(root.join("folder")).unwrap();
        fs::create_dir_all(root.join(".hidden")).unwrap();
        fs::write(root.join("folder/note.md"), "original").unwrap();
        fs::write(root.join("image.png"), "ignore").unwrap();
        fs::write(root.join(".hidden/note.md"), "ignore").unwrap();
        let files = entries(&root).unwrap();
        assert_eq!(files.len(), 2);
        assert!(files[0].directory);
        assert_eq!(files[0].path, root.join("folder"));
        assert_eq!(files[1].path, root.join("folder/note.md"));
        assert_eq!(files[1].depth, 1);
        assert!(validate_note(&root, &files[1].path).is_ok());
        assert!(validate_note(&root, &root.join("folder")).is_err());
        assert!(validate_note(&root.join("folder"), &root.join("image.png")).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&files[1].path, root.join("link.md")).unwrap();
            assert!(validate_note(&root, &root.join("link.md")).is_err());
        }
        save(&files[1].path, "original", "edited").unwrap();
        assert!(save(&files[1].path, "original", "stale edit").is_err());
        assert_eq!(fs::read_to_string(&files[1].path).unwrap(), "edited");
        fs::remove_dir_all(root).unwrap();
    }
}
