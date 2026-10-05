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
        move |_| editor.update()
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
                editor.confirm(move || {
                    *parent.borrow_mut() = destination.clone();
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
                    scroller.vadjustment().set_value(0.0);
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
            if vault::validate_note(&editor.root, &path).is_err() {
                return;
            }
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
            let stem = path.file_stem().unwrap().to_string_lossy().chars().count() as i32;
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
                        || !matches!(
                            std::path::Path::new(name)
                                .extension()
                                .and_then(|ext| ext.to_str()),
                            Some("md" | "markdown" | "txt" | "text")
                        )
                    {
                        entry.add_css_class("error");
                        entry.set_tooltip_text(Some(
                            "Enter a note filename ending in .md, .markdown, .txt, or .text",
                        ));
                        return;
                    }
                    let destination = path.parent().unwrap().join(name);
                    if destination != path {
                        if let Err(error) = vault::validate_note(&editor.root, &path)
                            .map_err(|error| error.to_string())
                            .and_then(|_| {
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
                    if editor.document.borrow().path.as_ref() == Some(&path) {
                        editor.document.borrow_mut().path = Some(destination);
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
