use super::*;
use pulldown_cmark::{Event, Parser, Tag};
use std::{
    ops::Range,
    path::{Component, Path},
};

fn relative_url(uri: &str, from: &Path, to: &Path) -> Option<String> {
    if uri.is_empty() || uri.starts_with(['/', '#']) || uri.split('/').next()?.contains(':') {
        return None;
    }
    let suffix = uri.find(['?', '#']).unwrap_or(uri.len());
    let path = glib::uri_unescape_string(&uri[..suffix], None::<&str>)?;
    let mut target = from.to_path_buf();
    for part in Path::new(path.as_str()).components() {
        match part {
            Component::Normal(name) => target.push(name),
            Component::ParentDir => {
                target.pop();
            }
            Component::CurDir => (),
            _ => return None,
        }
    }
    let source: Vec<_> = to.components().collect();
    let destination: Vec<_> = target.components().collect();
    let common = source
        .iter()
        .zip(&destination)
        .take_while(|(a, b)| a == b)
        .count();
    let mut relative = PathBuf::new();
    for _ in common..source.len() {
        relative.push("..");
    }
    for part in &destination[common..] {
        relative.push(part.as_os_str());
    }
    Some(format!(
        "{}{}",
        sync_dav::encode(&relative.to_string_lossy()),
        &uri[suffix..]
    ))
}

fn destination_range(source: &str, start: usize) -> Option<Range<usize>> {
    let start = start + source[start..].len() - source[start..].trim_start().len();
    if source[start..].starts_with('<') {
        return Some(start + 1..start + 1 + source[start + 1..].find('>')?);
    }
    let mut depth = 0;
    let mut escaped = false;
    for (index, ch) in source[start..].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '(' {
            depth += 1;
        }
        if ch == ')' {
            if depth == 0 {
                return Some(start..start + index);
            }
            depth -= 1;
        }
        if ch.is_whitespace() {
            return Some(start..start + index);
        }
    }
    Some(start..source.len())
}

fn inline_destination(source: &str) -> Option<Range<usize>> {
    let start = source.find('[')?;
    let mut depth = 1;
    let mut escaped = false;
    for (index, ch) in source[start + 1..].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '[' {
            depth += 1;
        }
        if ch == ']' {
            depth -= 1;
            if depth == 0 {
                let end = start + 1 + index + 1;
                return if source[end..].starts_with('(') {
                    destination_range(source, end + 1)
                } else {
                    None
                };
            }
        }
    }
    None
}

fn link_changes(text: &str, from: &Path, to: &Path) -> Vec<(Range<usize>, String)> {
    let parser = Parser::new(text);
    let mut changes = Vec::new();
    for (_, definition) in parser.reference_definitions().iter() {
        let source = &text[definition.span.clone()];
        if let Some(start) = source.find("]:") {
            if let (Some(range), Some(uri)) = (
                destination_range(source, start + 2),
                relative_url(&definition.dest, from, to),
            ) {
                changes.push((
                    definition.span.start + range.start..definition.span.start + range.end,
                    uri,
                ));
            }
        }
    }
    for (event, range) in parser.into_offset_iter() {
        let Event::Start(Tag::Image { dest_url, .. } | Tag::Link { dest_url, .. }) = event else {
            continue;
        };
        let source = &text[range.clone()];
        // Reference-style links are rewritten at their definition, not at each use.
        if let Some(destination) = inline_destination(source) {
            if let Some(uri) = relative_url(&dest_url, from, to) {
                changes.push((
                    range.start + destination.start..range.start + destination.end,
                    uri,
                ));
            }
        }
    }
    changes.sort_by_key(|(range, _)| range.start);
    changes.dedup_by(|a, b| a.0 == b.0);
    changes
}

fn rewritten(text: &str, changes: &[(Range<usize>, String)]) -> String {
    let mut result = text.to_string();
    for (range, value) in changes.iter().rev() {
        result.replace_range(range.clone(), value);
    }
    result
}

pub(super) fn move_note(editor: &Editor, path: &Path, folder: &Path) -> Result<(), String> {
    vault::validate_entry(&editor.root, path).map_err(|error| error.to_string())?;
    if !path.is_file()
        || !matches!(
            path.extension()
                .and_then(|extension| extension.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some("md" | "markdown" | "txt" | "text")
        )
    {
        return Err("Only notes can be dragged.".into());
    }
    if folder != editor.root {
        vault::validate_entry(&editor.root, folder).map_err(|error| error.to_string())?;
    }
    if !folder.is_dir() {
        return Err("Drop the note onto a folder.".into());
    }
    if editor
        .trash_pending
        .borrow()
        .iter()
        .any(|pending| path.starts_with(pending) || folder.starts_with(pending))
    {
        return Err("Wait for the Trash operation to finish.".into());
    }
    let destination = folder.join(path.file_name().ok_or("Invalid note filename")?);
    if destination == path {
        return Ok(());
    }
    if std::fs::symlink_metadata(&destination).is_ok() {
        return Err("A note with this name already exists in that folder.".into());
    }
    let current = editor.document.borrow().path.as_deref() == Some(path);
    let original = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    if current && editor.document.borrow().original != original {
        return Err(
            "This note changed on disk. Save or preserve your edits before moving it.".into(),
        );
    }
    let parent = path.parent().ok_or("Invalid note path")?;
    let disk = rewritten(&original, &link_changes(&original, parent, folder));
    let text = if current {
        editor.text()
    } else {
        String::new()
    };
    let changes = if current {
        link_changes(&text, parent, folder)
    } else {
        Vec::new()
    };
    gio::File::for_path(path)
        .move_(
            &gio::File::for_path(&destination),
            gio::FileCopyFlags::NONE,
            gio::Cancellable::NONE,
            None,
        )
        .map_err(|error| error.to_string())?;
    if disk != original {
        if let Err(error) = vault::save(&destination, &original, &disk) {
            let rollback = gio::File::for_path(&destination).move_(
                &gio::File::for_path(path),
                gio::FileCopyFlags::NONE,
                gio::Cancellable::NONE,
                None,
            );
            return Err(match rollback {
                Ok(()) => format!("Could not update relative links; the move was undone: {error}"),
                Err(rollback) => format!(
                    "The note is at {}. Updating links failed: {error}. Moving it back failed: {rollback}",
                    destination.display()
                ),
            });
        }
    }
    if current {
        editor.document.borrow_mut().path = Some(destination.clone());
        editor.document.borrow_mut().original = disk.clone();
        editor
            .view
            .clone()
            .downcast::<markdown::View>()
            .unwrap()
            .set_note_directory(Some(folder.to_path_buf()));
        editor.buffer.begin_user_action();
        for (range, value) in changes.iter().rev() {
            let start = text[..range.start].chars().count() as i32;
            let end = text[..range.end].chars().count() as i32;
            let mut start = editor.buffer.iter_at_offset(start);
            let mut end = editor.buffer.iter_at_offset(end);
            editor.buffer.delete(&mut start, &mut end);
            editor.buffer.insert(&mut start, value);
        }
        editor.buffer.end_user_action();
        editor.buffer.set_modified(editor.text() != disk);
        editor.update();
    }
    editor.refresh_button.emit_clicked();
    if folder != editor.root {
        let relative = folder.strip_prefix(&editor.root).unwrap().to_string_lossy();
        let mut index = 0;
        while let Some(row) = editor.list.row_at_index(index) {
            if row.tooltip_text().as_deref() == Some(relative.as_ref()) {
                let icon = row
                    .child()
                    .and_then(|child| child.first_child())
                    .and_then(|child| child.downcast::<gtk::Image>().ok());
                if icon.is_some_and(|icon| icon.icon_name().as_deref() == Some("pan-end-symbolic"))
                {
                    editor.list.emit_by_name::<()>("row-activated", &[&row]);
                }
                break;
            }
            index += 1;
        }
    }
    sync::record_deletion(editor, path);
    sync::request(editor);
    Ok(())
}

fn drop_target(widget: &impl IsA<gtk::Widget>, editor: &Editor, folder: PathBuf, root: bool) {
    let target = gtk::DropTarget::new(String::static_type(), gtk::gdk::DragAction::MOVE);
    let widget = widget.clone().upcast::<gtk::Widget>();
    target.connect_motion({
        let widget = widget.clone();
        move |_, x, y| {
            if root
                && widget
                    .pick(x, y, gtk::PickFlags::DEFAULT)
                    .is_some_and(|picked| picked.ancestor(gtk::ListBoxRow::static_type()).is_some())
            {
                widget.remove_css_class("drop-note-target");
                return gtk::gdk::DragAction::empty();
            }
            widget.add_css_class("drop-note-target");
            gtk::gdk::DragAction::MOVE
        }
    });
    target.connect_leave({
        let widget = widget.clone();
        move |_| widget.remove_css_class("drop-note-target")
    });
    target.connect_drop({
        let widget = widget.clone();
        let editor = editor.clone();
        move |_, value, x, y| {
            widget.remove_css_class("drop-note-target");
            if root
                && widget
                    .pick(x, y, gtk::PickFlags::DEFAULT)
                    .is_some_and(|picked| picked.ancestor(gtk::ListBoxRow::static_type()).is_some())
            {
                return false;
            }
            let Ok(path) = value.get::<String>() else {
                return false;
            };
            match move_note(&editor, Path::new(&path), &folder) {
                Ok(()) => true,
                Err(error) => {
                    editor.toasts.add_toast(adw::Toast::new(&error));
                    false
                }
            }
        }
    });
    widget.add_controller(target);
}

pub(super) fn install_root(editor: &Editor, scroller: &gtk::ScrolledWindow) {
    drop_target(scroller, editor, editor.root.clone(), true);
}

pub(super) fn install_row(editor: &Editor, row: &gtk::ListBoxRow, path: &Path, folder: bool) {
    if folder {
        drop_target(row, editor, path.to_path_buf(), false);
        return;
    }
    let source = gtk::DragSource::new();
    source.set_actions(gtk::gdk::DragAction::MOVE);
    source.connect_prepare({
        let path = path.to_string_lossy().to_string();
        let row = row.clone();
        move |_, _, _| {
            if row.child().is_some_and(|child| child.is::<gtk::Entry>()) {
                return None;
            }
            Some(gtk::gdk::ContentProvider::for_value(&path.to_value()))
        }
    });
    source.connect_drag_begin({
        let row = row.clone();
        move |source, _| source.set_icon(Some(&gtk::WidgetPaintable::new(Some(&row))), 0, 0)
    });
    row.add_controller(source);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rebases_relative_links_and_images_without_touching_code_or_external_urls() {
        let text = "![image](paste.png) [note](other.md#heading)\n![ref][pic]\n\n[pic]: <images/a%20b.png> \"caption\"\n\n`![literal](paste.png)` [web](https://example.org)";
        let result = rewritten(
            text,
            &link_changes(text, Path::new("/vault"), Path::new("/vault/folder")),
        );
        assert!(result.contains("![image](../paste.png)"));
        assert!(result.contains("[note](../other.md#heading)"));
        assert!(result.contains("[pic]: <../images/a%20b.png> \"caption\""));
        assert!(result.contains("`![literal](paste.png)` [web](https://example.org)"));
        assert_eq!(
            rewritten(
                &result,
                &link_changes(&result, Path::new("/vault/folder"), Path::new("/vault"))
            ),
            text
        );
        let nested = "![a [nested] label](image.png \"caption ]( untouched\")";
        assert_eq!(
            rewritten(
                nested,
                &link_changes(nested, Path::new("/vault"), Path::new("/vault/folder"))
            ),
            "![a [nested] label](../image.png \"caption ]( untouched\")"
        );
    }
}

pub(super) async fn smoke(editor: Editor) {
    async fn frame() {
        glib::timeout_future(std::time::Duration::from_millis(200)).await;
    }
    fn row(editor: &Editor, relative: &str) -> gtk::ListBoxRow {
        let mut index = 0;
        loop {
            let row = editor.list.row_at_index(index).unwrap();
            if row.tooltip_text().as_deref() == Some(relative) {
                return row;
            }
            index += 1;
        }
    }
    fn target(widget: &impl IsA<gtk::Widget>) -> gtk::DropTarget {
        let controllers = widget.observe_controllers();
        (0..controllers.n_items())
            .find_map(|index| controllers.item(index)?.downcast::<gtk::DropTarget>().ok())
            .unwrap()
    }
    let folder = editor.root.join("folder");
    std::fs::create_dir(&folder).unwrap();
    use gtk::subclass::prelude::ObjectSubclassIsExt;
    let source = editor.root.join("Move.md");
    std::fs::write(
        &source,
        "![image](Asset%20Folder/paste.png)\n[other](Other.md)\n",
    )
    .unwrap();
    std::fs::create_dir(editor.root.join("Asset Folder")).unwrap();
    let texture = gtk::gdk::MemoryTexture::new(
        1,
        1,
        gtk::gdk::MemoryFormat::R8g8b8a8,
        &glib::Bytes::from_static(&[255, 0, 0, 255]),
        4,
    );
    texture
        .save_to_png(editor.root.join("Asset Folder/paste.png"))
        .unwrap();
    editor.refresh_button.emit_clicked();
    frame().await;
    editor.open(&source);
    editor.buffer.place_cursor(&editor.buffer.end_iter());
    editor.buffer.insert_at_cursor("Unsaved edits\n");
    let text = editor.text();
    let folder_row = row(&editor, "folder");
    let controllers = row(&editor, "Move.md").observe_controllers();
    let drag = (0..controllers.n_items())
        .find_map(|index| controllers.item(index)?.downcast::<gtk::DragSource>().ok())
        .unwrap();
    let provider: Option<gtk::gdk::ContentProvider> =
        drag.emit_by_name("prepare", &[&0.0f64, &0.0f64]);
    assert!(provider.is_some());
    let moved: bool = target(&folder_row).emit_by_name(
        "drop",
        &[
            &glib::BoxedValue(source.to_string_lossy().to_string().to_value()),
            &0.0f64,
            &0.0f64,
        ],
    );
    assert!(moved);
    frame().await;
    let nested = folder.join("Move.md");
    assert!(!source.exists());
    assert!(nested.exists());
    assert_eq!(editor.document.borrow().path.as_ref(), Some(&nested));
    assert!(editor.buffer.is_modified());
    assert!(
        editor
            .text()
            .contains("![image](../Asset%20Folder/paste.png)")
    );
    assert!(editor.text().contains("Unsaved edits"));
    assert!(
        !std::fs::read_to_string(&nested)
            .unwrap()
            .contains("Unsaved edits")
    );
    assert!(row(&editor, "folder/Move.md").is_mapped());
    assert_eq!(
        editor
            .view
            .clone()
            .downcast::<markdown::View>()
            .unwrap()
            .imp()
            .images
            .borrow()
            .len(),
        1
    );
    std::fs::write(&source, "Existing destination").unwrap();
    assert!(move_note(&editor, &nested, &editor.root).is_err());
    assert_eq!(
        std::fs::read_to_string(&source).unwrap(),
        "Existing destination"
    );
    assert!(nested.exists());
    std::fs::remove_file(&source).unwrap();
    let scroller = editor
        .list
        .ancestor(gtk::ScrolledWindow::static_type())
        .unwrap()
        .downcast::<gtk::ScrolledWindow>()
        .unwrap();
    let root_target = target(&scroller);
    let moved: bool = root_target.emit_by_name(
        "drop",
        &[
            &glib::BoxedValue(nested.to_string_lossy().to_string().to_value()),
            &0.0f64,
            &((scroller.height() - 2) as f64),
        ],
    );
    assert!(moved);
    frame().await;
    assert!(source.exists());
    assert!(!nested.exists());
    assert_eq!(editor.text(), text);
    assert!(editor.buffer.is_modified());
    assert!(editor.save());
    assert_eq!(std::fs::read_to_string(source).unwrap(), text);
    assert!(editor.root.join("Asset Folder/paste.png").exists());
    println!(
        "Note moves verified: drag provider, folder drop, root drop, collision protection, relative links, unsaved edits, and save after moving."
    );
    editor.window.close();
}
