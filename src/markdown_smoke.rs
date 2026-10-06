use super::*;
use gtk::glib::translate::{IntoGlib, ToGlibPtr, ToGlibPtrMut};
use gtk::subclass::prelude::ObjectSubclassIsExt;

pub(super) async fn run(
    editor: Editor,
    search: gtk::SearchEntry,
    opened_links: Rc<RefCell<Vec<String>>>,
) {
    async fn frame() {
        glib::timeout_future(std::time::Duration::from_millis(200)).await;
    }
    for index in 0..30 {
        std::fs::write(
            editor.root.join(format!("b-search-{index:02}.md")),
            "Search fixture",
        )
        .unwrap();
    }
    std::fs::write(editor.root.join("bbo-search.md"), "Unique search fixture").unwrap();
    editor.refresh_button.emit_clicked();
    search.set_text("b-search");
    search.emit_by_name::<()>("search-changed", &[]);
    for _ in 0..30 {
        frame().await;
        if editor.results.row_at_index(29).is_some() {
            break;
        }
    }
    assert!(editor.results.row_at_index(29).is_some());
    let scroller = editor
        .results
        .ancestor(gtk::ScrolledWindow::static_type())
        .unwrap()
        .downcast::<gtk::ScrolledWindow>()
        .unwrap();
    let adjustment = scroller.vadjustment();
    adjustment.set_value(adjustment.upper() - adjustment.page_size());
    frame().await;
    assert!(adjustment.value() > 0.0);
    search.set_text("bbo-search");
    search.emit_by_name::<()>("search-changed", &[]);
    frame().await;
    assert!(editor.results.row_at_index(1).is_none());
    let row = editor.results.row_at_index(0).unwrap();
    let bounds = row.compute_bounds(&scroller).unwrap();
    assert!(bounds.y() >= 0.0 && bounds.y() < scroller.height() as f32);
    assert_eq!(adjustment.value(), adjustment.lower());
    println!("Search verified: narrowing a scrolled list leaves the matching note visible.");
    const SOURCE: &str = "Formatting fixture\n\n**bold** and *italic* with café\n- First item\n  - Nested item\n[Example](https://example.org)\nBare https://example.org/path\n\n`**not bold**`\n\n# Heading one\n## Heading two\n### Heading three\n#### Heading four\n\n* Star bullet\n\n- [ ] Task unchecked\n- [X] Task checked\n\nEditing line";
    let path = editor.root.join("formatting.md");
    assert!(!path.exists());
    std::fs::write(&path, SOURCE).unwrap();
    search.set_text("");
    search.emit_by_name::<()>("search-changed", &[]);
    editor.refresh_button.emit_clicked();
    editor.open(&path);
    editor.buffer.place_cursor(&editor.buffer.end_iter());
    frame().await;
    // Exercise the triple-click selection vfunc on a genuinely wrapped line.
    let long_line = format!("1. https://example.org/{}", "café-long-path-".repeat(100));
    let fixture = format!("Previous line\n{long_line}\nFinal line without newline");
    editor.buffer.set_text(&fixture);
    frame().await;
    let view = editor.view.clone().downcast::<markdown::View>().unwrap();
    let line_start = editor.buffer.iter_at_line(1).unwrap();
    let mut location = line_start;
    assert!(view.forward_display_line(&mut location));
    assert_eq!(location.line(), 1);
    assert!(location.offset() > line_start.offset());
    let mut start = location;
    let mut end = location;
    // Use the real GTK signal, including the handled flag that its mouse
    // selection code checks before accepting these output iterators.
    let extend = |granularity: gtk::TextExtendSelection,
                  location: &gtk::TextIter,
                  start: &mut gtk::TextIter,
                  end: &mut gtk::TextIter| {
        let mut handled: glib::ffi::gboolean = 0;
        unsafe {
            glib::gobject_ffi::g_signal_emit_by_name(
                view.as_ptr() as *mut glib::gobject_ffi::GObject,
                c"extend-selection".as_ptr(),
                granularity.into_glib(),
                location.to_glib_none().0,
                start.to_glib_none_mut().0,
                end.to_glib_none_mut().0,
                &mut handled,
            );
        }
        assert_ne!(handled, 0, "GTK would discard an unhandled selection");
        editor.buffer.select_range(start, end);
    };
    extend(
        gtk::TextExtendSelection::Line,
        &location,
        &mut start,
        &mut end,
    );
    let (selected_start, selected_end) = editor.buffer.selection_bounds().unwrap();
    assert_eq!(
        editor.buffer.text(&selected_start, &selected_end, true),
        format!("{long_line}\n")
    );
    location = editor.buffer.end_iter();
    extend(
        gtk::TextExtendSelection::Line,
        &location,
        &mut start,
        &mut end,
    );
    assert_eq!(
        editor.buffer.text(&start, &end, true),
        "Final line without newline"
    );
    extend(
        gtk::TextExtendSelection::Word,
        &location,
        &mut start,
        &mut end,
    );
    assert_eq!(editor.buffer.text(&start, &end, true), "newline");
    editor.buffer.set_text(SOURCE);
    editor.buffer.set_modified(false);
    editor.buffer.place_cursor(&editor.buffer.end_iter());
    frame().await;
    assert_eq!(editor.text(), SOURCE);
    assert!(!editor.buffer.is_modified());
    let visible = editor.buffer.text(
        &editor.buffer.start_iter(),
        &editor.buffer.end_iter(),
        false,
    );
    assert!(visible.contains("bold and italic with café"));
    assert!(!visible.contains("**bold**"));
    assert!(!visible.contains("- First item"));
    assert!(visible.contains("Example\n"));
    assert!(!visible.contains("[Example]"));
    assert!(visible.contains("`**not bold**`"));
    for (level, label) in [
        (1, "Heading one"),
        (2, "Heading two"),
        (3, "Heading three"),
        (4, "Heading four"),
    ] {
        let tag = editor
            .buffer
            .tag_table()
            .lookup(&format!("md-heading-{level}"))
            .unwrap();
        let position = SOURCE[..SOURCE.find(label).unwrap()].chars().count() as i32;
        assert!(editor.buffer.iter_at_offset(position).has_tag(&tag));
        assert!(visible.contains(label));
    }
    assert!(!visible.contains("# Heading"));
    assert!(!visible.contains("* Star bullet"));
    let heading_position = SOURCE[..SOURCE.find("Heading one").unwrap()]
        .chars()
        .count() as i32;
    editor
        .buffer
        .place_cursor(&editor.buffer.iter_at_offset(heading_position));
    frame().await;
    assert!(
        editor
            .buffer
            .text(
                &editor.buffer.start_iter(),
                &editor.buffer.end_iter(),
                false
            )
            .contains("# Heading one")
    );
    editor.buffer.place_cursor(&editor.buffer.end_iter());
    frame().await;
    let bold = editor.buffer.tag_table().lookup("md-bold").unwrap();
    let italic = editor.buffer.tag_table().lookup("md-italic").unwrap();
    let hidden = editor.buffer.tag_table().lookup("md-conceal").unwrap();
    let offset = |needle: &str| SOURCE[..SOURCE.find(needle).unwrap()].chars().count() as i32;
    assert!(editor.buffer.iter_at_offset(offset("bold")).has_tag(&bold));
    assert!(
        editor
            .buffer
            .iter_at_offset(offset("italic"))
            .has_tag(&italic)
    );
    assert!(
        editor
            .buffer
            .iter_at_offset(offset("- First"))
            .has_tag(&hidden)
    );

    // Reveal only the logical line at the caret, without marking the note dirty.
    editor
        .buffer
        .place_cursor(&editor.buffer.iter_at_offset(offset("bold")));
    assert!(
        !editor
            .buffer
            .iter_at_offset(offset("**bold"))
            .has_tag(&hidden)
    );
    assert!(
        editor
            .buffer
            .iter_at_offset(offset("- First"))
            .has_tag(&hidden)
    );
    editor
        .buffer
        .place_cursor(&editor.buffer.iter_at_offset(offset("First")));
    assert!(
        !editor
            .buffer
            .iter_at_offset(offset("- First"))
            .has_tag(&hidden)
    );
    assert!(
        editor
            .buffer
            .iter_at_offset(offset("**bold"))
            .has_tag(&hidden)
    );
    editor.buffer.place_cursor(&editor.buffer.end_iter());
    frame().await;

    // Test click handling with a fake URI opener; no browser is launched.
    let controllers = editor.view.observe_controllers();
    let click = (0..controllers.n_items())
        .filter_map(|i| controllers.item(i)?.downcast::<gtk::GestureClick>().ok())
        .find(|gesture| {
            gesture.button() == 1 && gesture.propagation_phase() == gtk::PropagationPhase::Capture
        })
        .unwrap();
    let point = || {
        let rect = editor
            .view
            .iter_location(&editor.buffer.iter_at_offset(offset("Example")));
        let (x, y) = editor.view.buffer_to_window_coords(
            gtk::TextWindowType::Widget,
            rect.x() + 4,
            rect.y() + rect.height() / 2,
        );
        (x as f64, y as f64)
    };
    let (x, y) = point();
    click.emit_by_name::<()>("pressed", &[&1i32, &x, &y]);
    click.emit_by_name::<()>("released", &[&1i32, &x, &y]);
    assert_eq!(*opened_links.borrow(), ["https://example.org"]);
    editor
        .buffer
        .place_cursor(&editor.buffer.iter_at_offset(offset("Example")));
    frame().await;
    let (x, y) = point();
    click.emit_by_name::<()>("pressed", &[&1i32, &x, &y]);
    click.emit_by_name::<()>("released", &[&1i32, &x, &y]);
    assert_eq!(opened_links.borrow().len(), 1);
    let keys = (0..controllers.n_items())
        .filter_map(|i| {
            controllers
                .item(i)?
                .downcast::<gtk::EventControllerKey>()
                .ok()
        })
        .next()
        .unwrap();
    let handled: bool = keys.emit_by_name(
        "key-pressed",
        &[
            &gtk::gdk::Key::Return.into_glib(),
            &0u32,
            &gtk::gdk::ModifierType::CONTROL_MASK,
        ],
    );
    assert!(handled);
    assert_eq!(opened_links.borrow().len(), 2);
    assert_eq!(editor.text(), SOURCE);
    assert!(!editor.buffer.is_modified());

    // Formatting shortcuts are one undoable edit, and saving preserves Markdown.
    let start = offset("Editing line");
    editor.buffer.select_range(
        &editor.buffer.iter_at_offset(start),
        &editor.buffer.end_iter(),
    );
    let previous_selection = editor
        .buffer
        .selection_bounds()
        .map(|(start, end)| (start.offset(), end.offset()));
    editor.buffer.place_cursor(&editor.buffer.end_iter());
    frame().await;
    for (marker, expected) in [
        ("- [ ] Task unchecked", "- [X] Task unchecked"),
        ("- [X] Task checked", "- [ ] Task checked"),
    ] {
        let position = SOURCE[..SOURCE.find(marker).unwrap()].chars().count() as i32;
        assert!(
            editor
                .buffer
                .iter_at_offset(position)
                .has_tag(&editor.buffer.tag_table().lookup("md-conceal").unwrap())
        );
        let rect = editor
            .view
            .iter_location(&editor.buffer.iter_at_offset(position));
        let (x, y) = editor.view.buffer_to_window_coords(
            gtk::TextWindowType::Widget,
            rect.x() + 2,
            rect.y() + 2,
        );
        click.emit_by_name::<()>("pressed", &[&1i32, &(x as f64), &(y as f64)]);
        assert!(editor.text().contains(expected));
        editor.buffer.undo();
        frame().await;
        assert_eq!(editor.text(), SOURCE);
        editor
            .buffer
            .place_cursor(&editor.buffer.iter_at_offset(position + 6));
        frame().await;
        assert!(
            editor
                .buffer
                .text(
                    &editor.buffer.start_iter(),
                    &editor.buffer.end_iter(),
                    false
                )
                .contains(marker)
        );
        editor.buffer.place_cursor(&editor.buffer.end_iter());
        frame().await;
    }
    if let Some((start, end)) = previous_selection {
        editor.buffer.select_range(
            &editor.buffer.iter_at_offset(start),
            &editor.buffer.iter_at_offset(end),
        );
    }
    frame().await;
    println!(
        "Tasks verified: checkbox clicks toggle source, undo restores it, and editing reveals markers."
    );
    gtk::prelude::WidgetExt::activate_action(&editor.window, "win.bold", None).unwrap();
    assert!(editor.text().ends_with("**Editing line**"));
    editor.buffer.undo();
    assert_eq!(editor.text(), SOURCE);
    editor.buffer.redo();
    editor.buffer.select_range(
        &editor.buffer.iter_at_offset(start + 2),
        &editor
            .buffer
            .iter_at_offset(start + 2 + "Editing line".chars().count() as i32),
    );
    gtk::prelude::WidgetExt::activate_action(&editor.window, "win.bold", None).unwrap();
    assert_eq!(editor.text(), SOURCE);
    editor.buffer.select_range(
        &editor.buffer.iter_at_offset(start),
        &editor.buffer.end_iter(),
    );
    gtk::prelude::WidgetExt::activate_action(&editor.window, "win.italic", None).unwrap();
    assert!(editor.text().ends_with("*Editing line*"));
    search.grab_focus();
    frame().await;
    assert!(
        editor
            .buffer
            .iter_at_offset(offset("**bold"))
            .has_tag(&hidden)
    );
    let expected = editor.text();
    assert!(editor.save());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), expected);
    editor.view.grab_focus();
    editor.buffer.place_cursor(&editor.buffer.end_iter());
    frame().await;
    if let Some(path) = std::env::var_os("HEMATITE_MARKDOWN_SCREENSHOT") {
        let snapshot = gtk::Snapshot::new();
        gtk::WidgetPaintable::new(Some(&editor.window)).snapshot(
            &snapshot,
            editor.window.width().into(),
            editor.window.height().into(),
        );
        editor
            .window
            .renderer()
            .unwrap()
            .render_texture(&snapshot.to_node().unwrap(), None)
            .save_to_png(PathBuf::from(path))
            .unwrap();
    }
    // Actual clipboard image -> paste action, including nested relative paths.
    let nested = editor.root.join("folder/image paste.md");
    std::fs::write(&nested, "Before\n").unwrap();
    editor.open(&nested);
    editor.buffer.place_cursor(&editor.buffer.end_iter());
    let clipboard = editor.view.clipboard();
    let previous = clipboard.content();
    let texture = gtk::gdk::MemoryTexture::new(
        1,
        1,
        gtk::gdk::MemoryFormat::R8g8b8a8,
        &glib::Bytes::from_static(&[255, 0, 0, 255]),
        4,
    );
    clipboard.set_texture(&texture);
    editor.view.emit_by_name::<()>("paste-clipboard", &[]);
    for _ in 0..30 {
        frame().await;
        if editor.text().contains("![](") {
            break;
        }
    }
    let pasted = editor.text();
    assert!(pasted.starts_with("Before\n![](../paste-"));
    let relative = pasted
        .split("![](")
        .nth(1)
        .unwrap()
        .strip_suffix(')')
        .unwrap();
    let attachment = nested.parent().unwrap().join(relative);
    let decoded = gtk::gdk::Texture::from_filename(&attachment).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (1, 1));
    search.grab_focus();
    frame().await;
    let previews = view.imp().images.borrow();
    assert_eq!(previews.len(), 1);
    let (offset, _, width, height) = &previews[0];
    let available = editor.view.width() - editor.view.left_margin() - editor.view.right_margin();
    let surface = editor.view.native().unwrap().surface().unwrap();
    let screen = editor
        .view
        .display()
        .monitor_at_surface(&surface)
        .unwrap()
        .geometry()
        .width();
    let expected_width = (available as f32 * 0.7).min(screen as f32 * 0.33);
    assert!((*width - expected_width).abs() < 1.0);
    assert_eq!(width, height);
    assert!(
        editor
            .view
            .iter_location(&editor.buffer.iter_at_offset(*offset))
            .y() as f32
            >= *height
    );
    drop(previews);
    assert_eq!(editor.text(), pasted);
    if let Some(path) = std::env::var_os("HEMATITE_IMAGE_SCREENSHOT") {
        let snapshot = gtk::Snapshot::new();
        gtk::WidgetPaintable::new(Some(&editor.window)).snapshot(
            &snapshot,
            editor.window.width().into(),
            editor.window.height().into(),
        );
        editor
            .window
            .renderer()
            .unwrap()
            .render_texture(&snapshot.to_node().unwrap(), None)
            .save_to_png(PathBuf::from(path))
            .unwrap();
    }
    // Click in the blank space beside the preview, through the real handler.
    let (image_offset, _, _, image_height) = view.imp().images.borrow()[0].clone();
    let rect = editor
        .view
        .iter_location(&editor.buffer.iter_at_offset(image_offset));
    let (click_x, click_y) = editor.view.buffer_to_window_coords(
        gtk::TextWindowType::Widget,
        editor.view.width() - editor.view.right_margin() - 4,
        rect.y() - (image_height / 2.0) as i32,
    );
    click.emit_by_name::<()>("pressed", &[&1i32, &(click_x as f64), &(click_y as f64)]);
    click.emit_by_name::<()>("released", &[&1i32, &(click_x as f64), &(click_y as f64)]);
    frame().await;
    assert!(view.imp().images.borrow().is_empty());
    assert!(editor.buffer.selection_bounds().is_none());
    assert_eq!(editor.buffer.cursor_position(), editor.buffer.char_count());
    assert_eq!(editor.text(), pasted);
    assert!(
        !vault::entries(&editor.root)
            .unwrap()
            .iter()
            .any(|entry| entry.path.extension().is_some_and(|ext| ext == "png"))
    );
    editor.buffer.undo();
    assert_eq!(editor.text(), "Before\n");
    assert!(attachment.exists());
    editor.buffer.redo();
    assert_eq!(editor.text(), pasted);
    assert!(editor.save());
    assert_eq!(std::fs::read_to_string(&nested).unwrap(), pasted);
    clipboard.set_text("ordinary text");
    editor.view.emit_by_name::<()>("paste-clipboard", &[]);
    frame().await;
    assert!(editor.text().ends_with("ordinary text"));
    clipboard.set_content(previous.as_ref()).unwrap();
    assert!(editor.save());
    gtk::prelude::WidgetExt::activate_action(&editor.window, "win.new-note", None).unwrap();
    frame().await;
    let focus = gtk::prelude::GtkWindowExt::focus(&editor.window).unwrap();
    let filename = focus
        .clone()
        .downcast::<gtk::Entry>()
        .ok()
        .or_else(|| {
            focus
                .ancestor(gtk::Entry::static_type())
                .and_then(|widget| widget.downcast().ok())
        })
        .expect("New-note filename field must have focus");
    assert_eq!(filename.text(), ".md");
    assert_eq!(filename.position(), 0);
    let mut index = 0;
    while let Some(row) = editor.list.row_at_index(index) {
        assert!(!row.has_css_class("active-note"));
        index += 1;
    }
    assert!(filename.ancestor(gtk::Popover::static_type()).is_none());
    assert!(filename.ancestor(gtk::Stack::static_type()).is_some());
    filename.set_text("Created note.md");
    filename.emit_by_name::<()>("activate", &[]);
    frame().await;
    assert_eq!(
        editor.document.borrow().path.as_ref(),
        Some(&editor.root.join("Created note.md"))
    );
    assert_eq!(editor.text(), "");
    assert_eq!(editor.buffer.cursor_position(), 0);
    assert!(editor.view.has_focus());
    assert_eq!(
        std::fs::read_to_string(editor.root.join("Created note.md")).unwrap(),
        ""
    );
    editor.buffer.insert_at_cursor("unsaved rename content");
    let created = editor.root.join("Created note.md");
    gtk::prelude::WidgetExt::activate_action(
        &editor.window,
        "win.rename-note",
        Some(&created.to_string_lossy().to_string().to_variant()),
    )
    .unwrap();
    frame().await;
    let focus = gtk::prelude::GtkWindowExt::focus(&editor.window).unwrap();
    let filename = focus
        .ancestor(gtk::Entry::static_type())
        .unwrap()
        .downcast::<gtk::Entry>()
        .unwrap();
    assert!(filename.ancestor(gtk::ListBoxRow::static_type()).is_some());
    assert_eq!(filename.text(), "Created note.md");
    filename.set_text("smoke.md");
    filename.emit_by_name::<()>("activate", &[]);
    assert!(created.exists());
    assert!(filename.has_css_class("error"));
    filename.set_text("Renamed note.md");
    filename.emit_by_name::<()>("activate", &[]);
    assert!(!created.exists());
    assert_eq!(
        editor.document.borrow().path.as_ref(),
        Some(&editor.root.join("Renamed note.md"))
    );
    assert_eq!(editor.text(), "unsaved rename content");
    assert!(editor.buffer.is_modified());
    assert!(editor.save());
    assert_eq!(
        std::fs::read_to_string(editor.root.join("Renamed note.md")).unwrap(),
        "unsaved rename content"
    );
    gtk::prelude::WidgetExt::activate_action(&editor.window, "win.new-folder", None).unwrap();
    frame().await;
    let focus = gtk::prelude::GtkWindowExt::focus(&editor.window).unwrap();
    let folder_entry = focus
        .ancestor(gtk::Entry::static_type())
        .unwrap()
        .downcast::<gtk::Entry>()
        .unwrap();
    assert_eq!(folder_entry.text(), "");
    folder_entry.set_text("folder");
    folder_entry.emit_by_name::<()>("activate", &[]);
    assert!(editor.draft_note.is_visible());
    folder_entry.set_text("Created folder");
    folder_entry.emit_by_name::<()>("activate", &[]);
    frame().await;
    assert!(editor.root.join("Created folder").is_dir());
    assert!(!editor.draft_note.is_visible());
    let mut index = 0;
    let folder_row = loop {
        let row = editor
            .list
            .row_at_index(index)
            .expect("Created folder must be listed");
        if row.tooltip_text().as_deref() == Some("Created folder") {
            break row;
        }
        index += 1;
    };
    let bounds = folder_row.compute_bounds(&editor.list).unwrap();
    let controllers = editor.list.observe_controllers();
    let gesture = (0..controllers.n_items())
        .find_map(|index| {
            controllers
                .item(index)
                .and_then(|item| item.downcast::<gtk::GestureClick>().ok())
                .filter(|gesture| gesture.button() == 3)
        })
        .unwrap();
    gesture.emit_by_name::<()>(
        "pressed",
        &[
            &1i32,
            &(bounds.x() as f64 + 8.0),
            &(bounds.y() as f64 + 8.0),
        ],
    );
    gtk::prelude::WidgetExt::activate_action(&editor.list, "note.new-here", None).unwrap();
    frame().await;
    let focus = gtk::prelude::GtkWindowExt::focus(&editor.window).unwrap();
    let note_entry = focus
        .ancestor(gtk::Entry::static_type())
        .unwrap()
        .downcast::<gtk::Entry>()
        .unwrap();
    assert_eq!(note_entry.text(), ".md");
    let draft_row = note_entry.ancestor(gtk::ListBoxRow::static_type()).unwrap();
    assert_eq!(draft_row.tooltip_text().as_deref(), Some("Created folder"));
    note_entry.set_text("Inside.md");
    editor.refresh_button.emit_clicked();
    frame().await;
    assert_eq!(note_entry.text(), "Inside.md");
    assert_eq!(
        note_entry
            .ancestor(gtk::ListBoxRow::static_type())
            .unwrap()
            .tooltip_text()
            .as_deref(),
        Some("Created folder")
    );
    note_entry.emit_by_name::<()>("activate", &[]);
    frame().await;
    let nested = editor.root.join("Created folder/Inside.md");
    assert!(nested.is_file());
    assert_eq!(editor.document.borrow().path.as_ref(), Some(&nested));
    assert!(editor.view.has_focus());
    let mut index = 0;
    let nested_row = loop {
        let row = editor.list.row_at_index(index).unwrap();
        if row.tooltip_text().as_deref() == Some("Created folder/Inside.md") {
            break row;
        }
        index += 1;
    };
    assert!(nested_row.is_mapped());
    println!(
        "Creation verified: inline new folder, collision protection, folder context action, and visible nested note."
    );
    if std::env::var_os("HEMATITE_SYNC_SMOKE_TEST").is_some() {
        sync::smoke(&editor).await;
    }
    if std::env::var_os("HEMATITE_PREFERENCES_SMOKE_TEST").is_some() {
        preferences::smoke(&editor).await;
    }
    editor.window.close();
}
