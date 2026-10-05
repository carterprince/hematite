use super::*;
use std::{io::Write, path::Path};

fn save_image(root: &Path, note: &Path, png: &[u8]) -> std::io::Result<(PathBuf, String)> {
    let parent = note
        .parent()
        .unwrap()
        .strip_prefix(root)
        .map_err(|_| std::io::Error::other("The note must be inside the vault"))?;
    let stamp = glib::DateTime::now_local()
        .and_then(|time| time.format("%Y-%m-%d-%H%M%S"))
        .map_err(std::io::Error::other)?;
    for suffix in 0..10000 {
        let name = if suffix == 0 {
            format!("paste-{stamp}.png")
        } else {
            format!("paste-{stamp}-{suffix}.png")
        };
        let path = root.join(&name);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                if let Err(error) = file.write_all(png) {
                    drop(file);
                    let _ = std::fs::remove_file(&path);
                    return Err(error);
                }
                let relative = format!("{}{}", "../".repeat(parent.components().count()), name);
                return Ok((path, format!("![]({relative})")));
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::other(
        "Could not choose a unique image filename",
    ))
}

pub(super) fn install(view: &markdown::View, editor: &Editor) {
    let editor = editor.clone();
    view.set_image_paste_handler(move || {
        let clipboard = editor.view.clipboard();
        let formats = clipboard.formats().union_deserialize_types();
        if !formats.contains_type(gtk::gdk::Texture::static_type()) {
            return false;
        }
        if !editor.view.is_editable() {
            return true;
        }
        let Some(note) = editor.document.borrow().path.clone() else {
            return true;
        };
        let source = editor.text();
        let selection = editor.buffer.selection_bounds().unwrap_or_else(|| {
            let caret = editor.buffer.iter_at_mark(&editor.buffer.get_insert());
            (caret, caret)
        });
        let selection = (selection.0.offset(), selection.1.offset());
        let editor = editor.clone();
        glib::spawn_future_local(async move {
            let texture = match clipboard.read_texture_future().await {
                Ok(Some(texture)) => texture,
                Ok(None) => {
                    editor.error("The clipboard did not contain a readable image.");
                    return;
                }
                Err(error) => {
                    editor.error(&format!("Could not paste image: {error}"));
                    return;
                }
            };
            if editor.document.borrow().path.as_ref() != Some(&note)
                || !editor.view.is_editable()
                || editor.text() != source
            {
                editor.toasts.add_toast(adw::Toast::new(
                    "Note changed while reading the image. Paste again.",
                ));
                return;
            }
            let png = texture.save_to_png_bytes();
            let root = editor.root.clone();
            let destination_note = note.clone();
            let saved =
                gio::spawn_blocking(move || save_image(&root, &destination_note, png.as_ref()))
                    .await;
            match saved {
                Ok(Ok((path, markdown))) => {
                    // Recheck after disk I/O, before using the captured selection.
                    if editor.text() != source
                        || !editor.view.is_editable()
                        || editor.document.borrow().path.as_ref() != Some(&note)
                    {
                        let _ = std::fs::remove_file(path);
                        editor.toasts.add_toast(adw::Toast::new(
                            "Note changed while saving the image. Paste again.",
                        ));
                        return;
                    }
                    editor.buffer.begin_user_action();
                    let mut start = editor.buffer.iter_at_offset(selection.0);
                    let mut end = editor.buffer.iter_at_offset(selection.1);
                    let markdown = format!(
                        "{}{}{}",
                        if start.starts_line() { "" } else { "\n" },
                        markdown,
                        if end.ends_line() { "" } else { "\n" },
                    );
                    editor.buffer.delete(&mut start, &mut end);
                    editor.buffer.insert(&mut start, &markdown);
                    editor.buffer.place_cursor(&start);
                    editor.buffer.end_user_action();
                }
                Ok(Err(error)) => editor.error(&format!("Could not save pasted image: {error}")),
                Err(_) => editor.error("Could not save pasted image: the image writer failed."),
            }
        });
        true
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_unique_root_attachments_and_relative_links() {
        let root = std::env::temp_dir().join(format!("hematite-images-{}", std::process::id()));
        std::fs::create_dir_all(root.join("one/two")).unwrap();
        let note = root.join("one/two/note.md");
        let (first, link) = save_image(&root, &note, b"first").unwrap();
        let (second, _) = save_image(&root, &note, b"second").unwrap();
        assert_ne!(first, second);
        assert_eq!(first.parent(), Some(root.as_path()));
        assert_eq!(
            link,
            format!(
                "![](../../{})",
                first.file_name().unwrap().to_str().unwrap()
            )
        );
        assert_eq!(std::fs::read(&first).unwrap(), b"first");
        assert_eq!(std::fs::read(&second).unwrap(), b"second");
        let (_, link) = save_image(&root, &root.join("root.md"), b"third").unwrap();
        assert!(link.starts_with("![](paste-"));
        assert!(save_image(&root, &root.parent().unwrap().join("outside.md"), b"no").is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
