use super::*;

fn press(dialog: &adw::Dialog, label: &str) {
    fn find(widget: &gtk::Widget, label: &str) -> Option<gtk::Button> {
        if let Some(button) = widget.downcast_ref::<gtk::Button>() {
            if button.label().as_deref() == Some(label) {
                return Some(button.clone());
            }
        }
        let mut child = widget.first_child();
        while let Some(widget) = child {
            if let Some(button) = find(&widget, label) {
                return Some(button);
            }
            child = widget.next_sibling();
        }
        None
    }
    find(dialog.upcast_ref(), label)
        .expect("Dialog response button must exist")
        .emit_clicked();
}

pub(super) async fn run(
    editor: Editor,
    tree_menu: gtk::GestureClick,
    results_menu: gtk::GestureClick,
    search: gtk::SearchEntry,
    tree: Rc<RefCell<Sidebar>>,
    hits: Rc<RefCell<Vec<search::Hit>>>,
    update_search: Rc<dyn Fn()>,
) {
    async fn frame() {
        glib::timeout_future(std::time::Duration::from_millis(250)).await;
    }
    let path = editor.root.join("smoke.md");
    let saved = std::fs::read_to_string(&path).unwrap();
    editor.buffer.insert_at_cursor("unsaved trash check");
    let unsaved = editor.text();
    search.set_text("");
    update_search();
    frame().await;

    // Right-click does not switch notes. Cancel preserves the file and edits.
    let row = editor.list.row_at_index(0).unwrap();
    tree_menu.emit_by_name::<()>(
        "pressed",
        &[
            &1i32,
            &16f64,
            &(row.compute_bounds(&editor.list).unwrap().y() as f64 + 8.0),
        ],
    );
    assert_eq!(editor.document.borrow().path.as_ref(), Some(&path));
    gtk::prelude::WidgetExt::activate_action(&editor.list, "note.trash", None).unwrap();
    frame().await;
    let dialog = editor.window.visible_dialog().unwrap();
    assert!(
        dialog
            .downcast_ref::<adw::AlertDialog>()
            .unwrap()
            .body()
            .contains("unsaved edits")
    );
    press(&dialog, "Cancel");
    frame().await;
    assert_eq!(std::fs::read_to_string(&path).unwrap(), saved);
    assert_eq!(editor.text(), unsaved);
    assert!(editor.buffer.is_modified());

    // A source disappearing after confirmation opens must preserve the editor.
    tree_menu.emit_by_name::<()>(
        "pressed",
        &[
            &1i32,
            &16f64,
            &(row.compute_bounds(&editor.list).unwrap().y() as f64 + 8.0),
        ],
    );
    gtk::prelude::WidgetExt::activate_action(&editor.list, "note.trash", None).unwrap();
    frame().await;
    let dialog = editor.window.visible_dialog().unwrap();
    let backup = editor.root.join("smoke-backup.tmp");
    std::fs::rename(&path, &backup).unwrap();
    press(&dialog, "Move to Trash");
    frame().await;
    assert_eq!(editor.text(), unsaved);
    assert!(editor.buffer.is_modified());
    assert!(editor.view.is_editable());
    std::fs::rename(backup, &path).unwrap();
    press(&editor.window.visible_dialog().unwrap(), "OK");
    frame().await;

    // Trashing a different note leaves the open note and its unsaved edits intact.
    let other = editor.root.join("other-trash.md");
    assert!(!other.exists());
    std::fs::write(&other, "Other fixture\n").unwrap();
    editor.refresh_button.emit_clicked();
    search.set_text("fixture");
    update_search();
    frame().await;
    let index = hits
        .borrow()
        .iter()
        .position(|hit| hit.path == other)
        .unwrap();
    let row = editor.results.row_at_index(index as i32).unwrap();
    results_menu.emit_by_name::<()>(
        "pressed",
        &[
            &1i32,
            &16f64,
            &(row.compute_bounds(&editor.results).unwrap().y() as f64 + 8.0),
        ],
    );
    gtk::prelude::WidgetExt::activate_action(&editor.results, "note.trash", None).unwrap();
    frame().await;
    let dialog = editor.window.visible_dialog().unwrap();
    let body = dialog.downcast_ref::<adw::AlertDialog>().unwrap().body();
    assert!(body.contains("other-trash.md"));
    assert!(!body.contains("unsaved edits"));
    press(&dialog, "Move to Trash");
    for _ in 0..40 {
        glib::timeout_future(std::time::Duration::from_millis(50)).await;
        if !other.exists() && editor.trash_pending.borrow().is_empty() {
            break;
        }
    }
    assert!(!other.exists());
    assert_eq!(editor.document.borrow().path.as_ref(), Some(&path));
    assert_eq!(editor.text(), unsaved);
    assert!(editor.buffer.is_modified());
    assert!(editor.view.is_editable());
    let trash = PathBuf::from(std::env::var_os("XDG_DATA_HOME").unwrap()).join("Trash");
    assert_eq!(
        std::fs::read_to_string(trash.join("files/other-trash.md")).unwrap(),
        "Other fixture\n"
    );

    // The search-result menu moves the saved file into the isolated desktop Trash.
    search.set_text("fixture");
    update_search();
    frame().await;
    let index = hits
        .borrow()
        .iter()
        .position(|hit| hit.path == path)
        .unwrap();
    let row = editor.results.row_at_index(index as i32).unwrap();
    results_menu.emit_by_name::<()>(
        "pressed",
        &[
            &1i32,
            &16f64,
            &(row.compute_bounds(&editor.results).unwrap().y() as f64 + 8.0),
        ],
    );
    gtk::prelude::WidgetExt::activate_action(&editor.results, "note.trash", None).unwrap();
    frame().await;
    press(&editor.window.visible_dialog().unwrap(), "Move to Trash");
    for _ in 0..40 {
        glib::timeout_future(std::time::Duration::from_millis(50)).await;
        if !path.exists() && editor.trash_pending.borrow().is_empty() {
            break;
        }
    }
    if path.exists() {
        if let Some(dialog) = editor
            .window
            .visible_dialog()
            .and_then(|dialog| dialog.downcast::<adw::AlertDialog>().ok())
        {
            eprintln!("Trash test error: {}", dialog.body());
        }
    }
    assert!(!path.exists());
    assert!(editor.document.borrow().path.is_none());
    assert!(!editor.buffer.is_modified());
    assert!(!editor.view.is_editable());
    assert!(!editor.title.title().starts_with("•"));
    assert!(!tree.borrow().entries.iter().any(|entry| entry.path == path));
    assert!(!hits.borrow().iter().any(|hit| hit.path == path));
    assert_eq!(
        std::fs::read_to_string(trash.join("files/smoke.md")).unwrap(),
        saved
    );
    assert!(trash.join("info/smoke.md.trashinfo").exists());
    assert!(editor.root.join("folder/nested note.md").exists());
    editor.window.close();
}
