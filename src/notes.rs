use super::*;

fn create_note(root: &std::path::Path, name: &str) -> std::io::Result<PathBuf> {
    let name = name.trim();
    let stem = name.strip_suffix(".md").unwrap_or(name);
    if stem.trim().is_empty() || name.starts_with('.') || name.contains(['/', '\\', '\n', '\r']) {
        return Err(std::io::Error::other(
            "Enter a filename without folders, such as Ideas.md",
        ));
    }
    let filename = if name.ends_with(".md") {
        name.to_string()
    } else {
        format!("{name}.md")
    };
    let path = root.join(filename);
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    Ok(path)
}

fn validate_parent(root: &std::path::Path, parent: &std::path::Path) -> std::io::Result<()> {
    let relative = parent
        .strip_prefix(root)
        .map_err(|_| std::io::Error::other("Choose a folder inside this vault"))?;
    let mut path = root.to_path_buf();
    for part in relative.components() {
        if !matches!(part, std::path::Component::Normal(_)) {
            return Err(std::io::Error::other("Invalid folder path"));
        }
        path.push(part);
        if std::fs::symlink_metadata(&path)?.file_type().is_symlink() {
            return Err(std::io::Error::other(
                "Cannot create notes in a symbolic link",
            ));
        }
    }
    if !parent.is_dir() {
        return Err(std::io::Error::other(
            "The containing folder no longer exists",
        ));
    }
    Ok(())
}

fn create_folder(parent: &std::path::Path, name: &str) -> std::io::Result<PathBuf> {
    let name = name.trim();
    if name.is_empty() || name.starts_with('.') || name.contains(['/', '\\', '\n', '\r']) {
        return Err(std::io::Error::other("Enter a folder name without slashes"));
    }
    let path = parent.join(name);
    std::fs::create_dir(&path)?;
    Ok(path)
}

pub(super) fn reset_draft_parent(editor: &Editor) {
    let root = editor
        .list
        .parent()
        .unwrap()
        .downcast::<gtk::Box>()
        .unwrap();
    if editor.draft_note.parent().as_ref() != Some(root.upcast_ref()) {
        if let Some(parent) = editor
            .draft_note
            .parent()
            .and_then(|parent| parent.downcast::<gtk::Box>().ok())
        {
            parent.remove(&editor.draft_note);
            if let Some(row) = parent
                .parent()
                .and_then(|widget| widget.downcast::<gtk::ListBoxRow>().ok())
            {
                if let Some(header) = parent.first_child() {
                    parent.remove(&header);
                    row.set_child(Some(&header));
                }
            }
        }
        root.prepend(&editor.draft_note);
    }
    editor.draft_note.set_margin_start(0);
}

pub(super) fn position_draft(editor: &Editor) {
    if !editor.draft_note.is_visible() {
        return;
    }
    let Some(destination) = editor
        .draft_note
        .tooltip_text()
        .map(|path| PathBuf::from(path.as_str()))
    else {
        return;
    };
    if destination == editor.root {
        return;
    }
    let Ok(relative) = destination.strip_prefix(&editor.root) else {
        return;
    };
    let mut index = 0;
    while let Some(row) = editor.list.row_at_index(index) {
        if row.tooltip_text().as_deref() == Some(relative.to_string_lossy().as_ref()) {
            let Some(child) = row.child() else {
                return;
            };
            let container = if let Ok(container) = child.clone().downcast::<gtk::Box>() {
                if container.orientation() == gtk::Orientation::Vertical {
                    container
                } else {
                    row.set_child(gtk::Widget::NONE);
                    let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
                    container.append(&child);
                    row.set_child(Some(&container));
                    container
                }
            } else {
                return;
            };
            if editor.draft_note.parent().as_ref() != Some(container.upcast_ref()) {
                if let Some(parent) = editor
                    .draft_note
                    .parent()
                    .and_then(|parent| parent.downcast::<gtk::Box>().ok())
                {
                    parent.remove(&editor.draft_note);
                }
                container.append(&editor.draft_note);
            }
            editor
                .draft_note
                .set_margin_start(relative.components().count() as i32 * 16);
            return;
        }
        index += 1;
    }
}

pub(super) fn install(
    editor: &Editor,
    button: &gtk::Button,
    search: &gtk::SearchEntry,
    draft: &gtk::Box,
    scroller: &gtk::ScrolledWindow,
) {
    let parent = Rc::new(RefCell::new(editor.root.clone()));
    let folder = Rc::new(Cell::new(false));
    draft.connect_visible_notify({
        let editor = editor.clone();
        move |draft| {
            if !draft.is_visible() {
                reset_draft_parent(&editor);
            }
            editor.update();
        }
    });
    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    content.add_css_class("note-row");
    content.add_css_class("active-note");
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let icon = gtk::Image::from_icon_name("text-x-generic-symbolic");
    icon.add_css_class("dim-label");
    line.append(&icon);
    let entry = gtk::Entry::new();
    entry.set_width_chars(1);
    entry.set_hexpand(true);
    entry.add_css_class("inline-note-name");
    entry.set_tooltip_text(Some("Type a filename, then press Enter"));
    let error = gtk::Label::new(None);
    error.add_css_class("error");
    error.set_wrap(true);
    error.set_max_width_chars(32);
    error.set_visible(false);
    line.append(&entry);
    content.append(&line);
    content.append(&error);
    draft.append(&content);
    entry.connect_map(|entry| {
        entry.grab_focus();
        entry.set_position(0);
    });
    entry.connect_activate({
        let editor = editor.clone();
        let draft = draft.clone();
        let error = error.clone();
        let search = search.clone();
        let parent = parent.clone();
        let folder = folder.clone();
        move |entry| {
            let destination = parent.borrow().clone();
            let result = validate_parent(&editor.root, &destination).and_then(|()| {
                if folder.get() {
                    create_folder(&destination, &entry.text())
                } else {
                    create_note(&destination, &entry.text())
                }
            });
            match result {
                Ok(path) => {
                    draft.set_visible(false);
                    search.set_text("");
                    search.emit_by_name::<()>("search-changed", &[]);
                    editor.refresh_button.emit_clicked();
                    if destination != editor.root {
                        let relative = destination
                            .strip_prefix(&editor.root)
                            .unwrap()
                            .to_string_lossy();
                        let mut index = 0;
                        while let Some(row) = editor.list.row_at_index(index) {
                            if row.tooltip_text().as_deref() == Some(relative.as_ref()) {
                                let icon = row
                                    .child()
                                    .and_then(|child| child.first_child())
                                    .and_then(|widget| widget.downcast::<gtk::Image>().ok());
                                if icon.is_some_and(|icon| {
                                    icon.icon_name().as_deref() == Some("pan-end-symbolic")
                                }) {
                                    editor.list.emit_by_name::<()>("row-activated", &[&row]);
                                }
                                break;
                            }
                            index += 1;
                        }
                    }
                    if !folder.get() {
                        editor.open(&path);
                        editor.buffer.place_cursor(&editor.buffer.start_iter());
                    }
                    sync::request(&editor);
                    let view = editor.view.clone();
                    glib::idle_add_local_once(move || {
                        view.grab_focus();
                    });
                }
                Err(problem) => {
                    let message = problem.to_string();
                    error.set_text(if problem.kind() == std::io::ErrorKind::AlreadyExists {
                        "An item with this name already exists."
                    } else {
                        &message
                    });
                    error.set_visible(true);
                }
            }
        }
    });
    let keys = gtk::EventControllerKey::new();
    keys.connect_key_pressed({
        let draft = draft.clone();
        let editor = editor.clone();
        move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                draft.set_visible(false);
                editor.view.grab_focus();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        }
    });
    entry.add_controller(keys);
    for name in ["new-note", "new-folder", "new-note-here"] {
        let action = gio::SimpleAction::new(
            name,
            if name == "new-note-here" {
                Some(glib::VariantTy::STRING)
            } else {
                None
            },
        );
        action.connect_activate({
            let editor = editor.clone();
            let draft = draft.clone();
            let search = search.clone();
            let scroller = scroller.clone();
            let entry = entry.clone();
            let error = error.clone();
            let parent = parent.clone();
            let folder = folder.clone();
            let icon = icon.clone();
            move |_, parameter| {
                let destination = if name == "new-note-here" {
                    let Some(path) = parameter.and_then(|value| value.str()).map(PathBuf::from)
                    else {
                        return;
                    };
                    path
                } else {
                    editor.root.clone()
                };
                if let Err(problem) = validate_parent(&editor.root, &destination) {
                    editor.error(&problem.to_string());
                    return;
                }
                let draft = draft.clone();
                let entry = entry.clone();
                let error = error.clone();
                let search = search.clone();
                let scroller = scroller.clone();
                let parent = parent.clone();
                let folder = folder.clone();
                let icon = icon.clone();
                let next = editor.clone();
                editor.confirm(move || {
                    reset_draft_parent(&next);
                    *parent.borrow_mut() = destination.clone();
                    draft.set_tooltip_text(Some(&destination.to_string_lossy()));
                    folder.set(name == "new-folder");
                    icon.set_icon_name(Some(if folder.get() {
                        "folder-symbolic"
                    } else {
                        "text-x-generic-symbolic"
                    }));
                    entry.set_text(if folder.get() { "" } else { ".md" });
                    entry.set_placeholder_text(Some(if folder.get() {
                        "Folder name"
                    } else {
                        "Note name"
                    }));
                    entry.set_tooltip_text(Some(&format!(
                        "Create in {} — press Enter to finish",
                        destination.display()
                    )));
                    entry.set_position(0);
                    error.set_visible(false);
                    search.set_text("");
                    search.emit_by_name::<()>("search-changed", &[]);
                    draft.set_visible(true);
                    if destination != next.root {
                        let relative = destination
                            .strip_prefix(&next.root)
                            .unwrap()
                            .to_string_lossy();
                        let mut index = 0;
                        while let Some(row) = next.list.row_at_index(index) {
                            if row.tooltip_text().as_deref() == Some(relative.as_ref()) {
                                let icon = row
                                    .child()
                                    .and_then(|child| child.first_child())
                                    .and_then(|widget| widget.downcast::<gtk::Image>().ok());
                                if icon.is_some_and(|icon| {
                                    icon.icon_name().as_deref() == Some("pan-end-symbolic")
                                }) {
                                    next.list.emit_by_name::<()>("row-activated", &[&row]);
                                }
                                break;
                            }
                            index += 1;
                        }
                        position_draft(&next);
                    } else {
                        scroller.vadjustment().set_value(0.0);
                    }
                    entry.grab_focus();
                    entry.set_position(0);
                });
            }
        });
        editor.window.add_action(&action);
    }
    let rename = gio::SimpleAction::new("rename-note", Some(glib::VariantTy::STRING));
    rename.connect_activate({
        let editor = editor.clone();
        move |_, parameter| {
            let Some(path) = parameter.and_then(|value| value.str()).map(PathBuf::from) else {
                return;
            };
            if vault::validate_entry(&editor.root, &path).is_err() {
                return;
            }
            let folder = path.is_dir();
            editor.draft_note.set_visible(false);
            let relative = path.strip_prefix(&editor.root).unwrap().to_string_lossy();
            let row = [&editor.list, &editor.results].iter().find_map(|list| {
                let mut index = 0;
                while let Some(row) = list.row_at_index(index) {
                    if row.is_mapped() && row.tooltip_text().as_deref() == Some(relative.as_ref()) {
                        return Some(row);
                    }
                    index += 1;
                }
                None
            });
            let Some(row) = row else { return };
            let Some(original) = row.child() else { return };
            let entry = gtk::Entry::new();
            entry.add_css_class("inline-note-name");
            entry.set_text(&path.file_name().unwrap().to_string_lossy());
            row.set_child(Some(&entry));
            entry.grab_focus();
            let stem = if folder {
                path.file_name()
            } else {
                path.file_stem()
            }
            .unwrap()
            .to_string_lossy()
            .chars()
            .count() as i32;
            entry.select_region(0, stem);
            let keys = gtk::EventControllerKey::new();
            keys.connect_key_pressed({
                let row = row.clone();
                let original = original.clone();
                move |_, key, _, _| {
                    if key == gtk::gdk::Key::Escape {
                        row.set_child(Some(&original));
                        row.grab_focus();
                        glib::Propagation::Stop
                    } else {
                        glib::Propagation::Proceed
                    }
                }
            });
            entry.add_controller(keys);
            entry.connect_activate({
                let editor = editor.clone();
                move |entry| {
                    let name = entry.text();
                    let name = name.trim();
                    if name.is_empty()
                        || name.starts_with('.')
                        || name.contains(['/', '\\', '\n', '\r'])
                        || (!folder
                            && !matches!(
                                std::path::Path::new(name)
                                    .extension()
                                    .and_then(|ext| ext.to_str()),
                                Some("md" | "markdown" | "txt" | "text")
                            ))
                    {
                        entry.add_css_class("error");
                        entry.set_tooltip_text(Some(if folder {
                            "Enter a folder name without slashes"
                        } else {
                            "Enter a note filename ending in .md, .markdown, .txt, or .text"
                        }));
                        return;
                    }
                    let destination = path.parent().unwrap().join(name);
                    if destination != path {
                        if let Err(error) = vault::validate_entry(&editor.root, &path)
                            .map_err(|error| error.to_string())
                            .and_then(|_| {
                                if std::fs::symlink_metadata(&destination).is_ok() {
                                    return Err(
                                        "An item with this name already exists.".to_string()
                                    );
                                }
                                gio::File::for_path(&path)
                                    .move_(
                                        &gio::File::for_path(&destination),
                                        gio::FileCopyFlags::NONE,
                                        gio::Cancellable::NONE,
                                        None,
                                    )
                                    .map_err(|error| error.to_string())
                            })
                        {
                            entry.add_css_class("error");
                            entry.set_tooltip_text(Some(&error));
                            return;
                        }
                    }
                    if destination != path {
                        sync::record_deletion(&editor, &path);
                    }
                    let current = editor.document.borrow().path.clone();
                    if let Some(current) = current.filter(|current| current.starts_with(&path)) {
                        let moved = if current == path {
                            destination.clone()
                        } else {
                            destination.join(current.strip_prefix(&path).unwrap())
                        };
                        editor
                            .view
                            .clone()
                            .downcast::<markdown::View>()
                            .unwrap()
                            .set_note_directory(moved.parent().map(std::path::Path::to_path_buf));
                        editor.document.borrow_mut().path = Some(moved);
                        editor.update();
                    }
                    row.set_child(Some(&original));
                    editor.refresh_button.emit_clicked();
                    sync::request(&editor);
                    editor.view.grab_focus();
                }
            });
        }
    });
    editor.window.add_action(&rename);
    button.set_action_name(Some("win.new-note"));
    editor
        .window
        .application()
        .unwrap()
        .set_accels_for_action("win.new-note", &["<Control>n"]);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn creates_markdown_without_overwriting_existing_notes() {
        let root = std::env::temp_dir().join(format!("hematite-new-note-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = create_note(&root, "My ideas.md").unwrap();
        std::fs::write(&path, "existing note").unwrap();
        assert_eq!(
            create_note(&root, "My ideas.md").unwrap_err().kind(),
            std::io::ErrorKind::AlreadyExists
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), "existing note");
        assert_eq!(
            create_note(&root, "Other").unwrap().file_name().unwrap(),
            "Other.md"
        );
        for invalid in [".md", "", "../outside.md", "nested/note", ".hidden", "a\nb"] {
            assert!(create_note(&root, invalid).is_err());
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}

pub(super) async fn smoke(editor: Editor) {
    async fn frame() {
        glib::timeout_future(std::time::Duration::from_millis(200)).await;
    }
    let folder = editor.root.join("folder");
    std::fs::create_dir_all(&folder).unwrap();
    editor.refresh_button.emit_clicked();
    frame().await;
    gtk::prelude::WidgetExt::activate_action(
        &editor.window,
        "win.new-note-here",
        Some(&folder.to_string_lossy().to_string().to_variant()),
    )
    .unwrap();
    frame().await;
    let entry = editor
        .draft_note
        .first_child()
        .unwrap()
        .first_child()
        .unwrap()
        .last_child()
        .unwrap()
        .downcast::<gtk::Entry>()
        .unwrap();
    let row = entry.ancestor(gtk::ListBoxRow::static_type()).unwrap();
    assert_eq!(row.tooltip_text().as_deref(), Some("folder"));
    assert_eq!(entry.text(), ".md");
    assert_eq!(entry.position(), 0);
    entry.set_text("Inside.md");
    editor.refresh_button.emit_clicked();
    frame().await;
    assert_eq!(entry.text(), "Inside.md");
    assert_eq!(
        entry
            .ancestor(gtk::ListBoxRow::static_type())
            .unwrap()
            .tooltip_text()
            .as_deref(),
        Some("folder")
    );
    entry.emit_by_name::<()>("activate", &[]);
    frame().await;
    assert!(folder.join("Inside.md").is_file());
    assert_eq!(
        editor.document.borrow().path.as_ref(),
        Some(&folder.join("Inside.md"))
    );
    assert!(!editor.draft_note.is_visible());
    gtk::prelude::WidgetExt::activate_action(&editor.window, "win.new-note", None).unwrap();
    frame().await;
    assert!(entry.ancestor(gtk::ListBoxRow::static_type()).is_none());
    editor.draft_note.set_visible(false);
    println!(
        "Folder draft verified: nested placement, caret before extension, refresh preservation, note creation, and root draft restoration."
    );
    if std::env::var_os("HEMATITE_FOLDER_OPERATIONS_SMOKE_TEST").is_some() {
        editor.buffer.insert_at_cursor("Saved after folder rename");
        gtk::prelude::WidgetExt::activate_action(
            &editor.window,
            "win.rename-note",
            Some(&folder.to_string_lossy().to_string().to_variant()),
        )
        .unwrap();
        frame().await;
        let focus = gtk::prelude::GtkWindowExt::focus(&editor.window).unwrap();
        let rename = focus
            .ancestor(gtk::Entry::static_type())
            .unwrap()
            .downcast::<gtk::Entry>()
            .unwrap();
        assert_eq!(rename.text(), "folder");
        std::fs::create_dir(editor.root.join("taken")).unwrap();
        rename.set_text("taken");
        rename.emit_by_name::<()>("activate", &[]);
        assert!(rename.has_css_class("error"));
        assert!(folder.exists());
        rename.set_text("Renamed folder");
        rename.emit_by_name::<()>("activate", &[]);
        frame().await;
        let renamed = editor.root.join("Renamed folder");
        assert!(!folder.exists());
        assert!(renamed.join("Inside.md").exists());
        assert_eq!(
            editor.document.borrow().path.as_ref(),
            Some(&renamed.join("Inside.md"))
        );
        assert_eq!(editor.text(), "Saved after folder rename");
        assert!(editor.buffer.is_modified());
        assert!(editor.save());
        editor
            .buffer
            .insert_at_cursor("discard only after trash succeeds");
        editor.request_trash(renamed.clone());
        let dialog = editor
            .window
            .visible_dialog()
            .unwrap()
            .downcast::<adw::AlertDialog>()
            .unwrap();
        assert_eq!(dialog.heading().as_deref(), Some("Move folder to Trash?"));
        assert!(dialog.body().contains("all notes and files"));
        assert!(dialog.body().contains("unsaved edits"));
        dialog.emit_by_name::<()>("response", &[&"cancel"]);
        dialog.close();
        assert!(renamed.exists());
        assert!(editor.buffer.is_modified());
        editor.request_trash(renamed.clone());
        let dialog = editor
            .window
            .visible_dialog()
            .unwrap()
            .downcast::<adw::AlertDialog>()
            .unwrap();
        dialog.emit_by_name::<()>("response", &[&"trash"]);
        dialog.close();
        for _ in 0..30 {
            frame().await;
            if !renamed.exists() {
                break;
            }
        }
        assert!(!renamed.exists());
        assert!(editor.document.borrow().path.is_none());
        assert!(!editor.buffer.is_modified());
        let trash = PathBuf::from(std::env::var_os("XDG_DATA_HOME").unwrap()).join("Trash/files");
        assert!(
            std::fs::read_dir(trash).unwrap().any(|entry| entry
                .unwrap()
                .path()
                .join("Inside.md")
                .exists())
        );
        println!(
            "Folder operations verified: rename without overwriting, preserved unsaved edits, updated note path, trash cancellation, and trashed contents."
        );
    }
    editor.window.close();
}
