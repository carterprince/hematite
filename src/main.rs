mod images;
mod markdown;
mod markdown_smoke;
mod markdown_syntax;
mod moves;
mod notes;
mod preferences;
mod search;
mod sync;
mod sync_dav;
mod sync_engine;
mod trash_smoke;
mod vault;
use trash_smoke::run as trash_smoke;

use adw::prelude::*;
use gtk::{gio, glib};
use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    path::PathBuf,
    rc::Rc,
};

#[derive(Default)]
struct Sidebar {
    entries: Vec<vault::Entry>,
    expanded: HashSet<PathBuf>,
    seen_folders: HashSet<PathBuf>,
}

impl Sidebar {
    fn visible(&self, entry: &vault::Entry, root: &std::path::Path) -> bool {
        entry
            .path
            .ancestors()
            .skip(1)
            .take_while(|parent| *parent != root)
            .all(|parent| self.expanded.contains(parent))
    }

    fn refresh(&self, list: &gtk::ListBox) {
        list.invalidate_filter();
        for (index, entry) in self.entries.iter().enumerate() {
            if !entry.directory {
                continue;
            }
            if let Some(icon) = list
                .row_at_index(index as i32)
                .and_then(|row| row.child())
                .and_then(|child| child.first_child())
                .and_then(|child| {
                    if child.is::<gtk::Image>() {
                        Some(child)
                    } else {
                        child.first_child()
                    }
                })
                .and_then(|child| child.downcast::<gtk::Image>().ok())
            {
                icon.set_icon_name(Some(if self.expanded.contains(&entry.path) {
                    "pan-down-symbolic"
                } else {
                    "pan-end-symbolic"
                }));
            }
        }
    }
}

#[derive(Default)]
struct Document {
    path: Option<PathBuf>,
    original: String,
}

#[derive(Clone)]
struct Editor {
    window: adw::ApplicationWindow,
    buffer: gtk::TextBuffer,
    view: gtk::TextView,
    title: adw::WindowTitle,
    status: gtk::Label,
    refresh_button: gtk::Button,
    list: gtk::ListBox,
    results: gtk::ListBox,
    draft_note: gtk::Box,
    document: Rc<RefCell<Document>>,
    root: PathBuf,
    trash_pending: Rc<RefCell<HashSet<PathBuf>>>,
    toasts: adw::ToastOverlay,
    sync: Rc<RefCell<Option<Rc<sync::Controller>>>>,
}

impl Editor {
    fn reveal_match(&self, query: &str) {
        let start = self.buffer.start_iter();
        let flags = gtk::TextSearchFlags::CASE_INSENSITIVE;
        let found = start.forward_search(query.trim(), flags, None).or_else(|| {
            query
                .split_whitespace()
                .filter_map(|word| start.forward_search(word, flags, None))
                .min_by_key(|(start, _)| start.offset())
        });
        if let Some((mut start, end)) = found {
            self.buffer.select_range(&start, &end);
            self.view.scroll_to_iter(&mut start, 0.15, false, 0.0, 0.0);
        }
    }

    fn text(&self) -> String {
        self.buffer
            .text(&self.buffer.start_iter(), &self.buffer.end_iter(), true)
            .into()
    }

    fn update(&self) {
        let document = self.document.borrow();
        let dirty = self.buffer.is_modified();
        let trashing = document.path.as_ref().is_some_and(|path| {
            self.trash_pending
                .borrow()
                .iter()
                .any(|pending| path.starts_with(pending))
        });
        let name = document
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Hematite".into());
        self.title
            .set_title(&format!("{}{}", if dirty { "•  " } else { "" }, name));
        self.title.set_subtitle(
            &document
                .path
                .as_ref()
                .and_then(|p| p.parent())
                .map(|folder| match folder.strip_prefix(glib::home_dir()) {
                    Ok(relative) if relative.as_os_str().is_empty() => "~".to_string(),
                    Ok(relative) => format!("~/{}", relative.to_string_lossy()),
                    Err(_) => folder.to_string_lossy().into_owned(),
                })
                .unwrap_or_else(|| "Select a note to begin".into()),
        );
        self.view.set_editable(document.path.is_some() && !trashing);
        let active = document
            .path
            .as_ref()
            .and_then(|p| p.strip_prefix(&self.root).ok())
            .map(|p| p.to_string_lossy().into_owned());
        for list in [&self.list, &self.results] {
            let mut index = 0;
            while let Some(row) = list.row_at_index(index) {
                if !self.draft_note.is_visible()
                    && row.tooltip_text().as_deref() == active.as_deref()
                {
                    row.add_css_class("active-note");
                } else {
                    row.remove_css_class("active-note");
                }
                index += 1;
            }
        }
        if document.path.is_some() {
            self.status
                .set_text(&format!("{} words", self.text().split_whitespace().count()));
        }
    }

    fn error(&self, message: &str) {
        let dialog = adw::AlertDialog::builder()
            .heading("Could not complete the operation")
            .body(message)
            .build();
        dialog.add_response("ok", "OK");
        dialog.present(Some(&self.window));
    }

    fn save(&self) -> bool {
        let mut document = self.document.borrow_mut();
        let Some(path) = &document.path else {
            return true;
        };
        if self
            .trash_pending
            .borrow()
            .iter()
            .any(|pending| path.starts_with(pending))
        {
            return false;
        }
        let text = self.text();
        if let Err(error) = vault::save(path, &document.original, &text) {
            drop(document);
            self.error(&error.to_string());
            return false;
        }
        document.original = text;
        drop(document);
        self.buffer.set_modified(false);
        self.update();
        self.refresh_button.emit_clicked();
        sync::request(self);
        true
    }

    fn confirm(&self, next: impl Fn() + 'static) {
        if !self.buffer.is_modified() {
            next();
            return;
        }
        let dialog = adw::AlertDialog::builder()
            .heading("Save your changes?")
            .body("This note has unsaved changes.")
            .build();
        dialog.add_responses(&[
            ("cancel", "Cancel"),
            ("discard", "Discard"),
            ("save", "Save"),
        ]);
        dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
        dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("save"));
        dialog.set_close_response("cancel");
        let editor = self.clone();
        dialog.connect_response(None, move |_, response| {
            if response == "discard" || (response == "save" && editor.save()) {
                next();
            }
        });
        dialog.present(Some(&self.window));
    }

    fn open(&self, path: &PathBuf) {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                self.view
                    .clone()
                    .downcast::<markdown::View>()
                    .unwrap()
                    .set_note_directory(path.parent().map(std::path::Path::to_path_buf));
                *self.document.borrow_mut() = Document {
                    path: Some(path.clone()),
                    original: text.clone(),
                };
                self.buffer.begin_irreversible_action();
                self.buffer.set_text(&text);
                self.buffer.end_irreversible_action();
                self.buffer.set_modified(false);
                self.view.set_editable(true);
                self.view.set_cursor_visible(true);
                self.view.grab_focus();
                self.update();
            }
            Err(error) => self.error(&format!("{}: {error}", path.display())),
        }
    }

    fn request_trash(&self, path: PathBuf) {
        if self
            .trash_pending
            .borrow()
            .iter()
            .any(|pending| path.starts_with(pending))
        {
            return;
        }
        let relative = path
            .strip_prefix(&self.root)
            .unwrap_or(&path)
            .to_string_lossy();
        let folder = path.is_dir();
        let unsaved = self
            .document
            .borrow()
            .path
            .as_ref()
            .is_some_and(|open| open.starts_with(&path))
            && self.buffer.is_modified();
        let dialog = adw::AlertDialog::builder()
            .heading(if folder { "Move folder to Trash?" } else { "Move note to Trash?" })
            .body(format!(
                "{relative}\n\n{}{}",
                if folder { "This includes all notes and files inside the folder. You can restore them from your desktop’s Trash." } else { "You can restore the saved file from your desktop’s Trash." },
                if unsaved {
                    "\n\nThis note has unsaved edits. Moving it to Trash will discard those edits."
                } else {
                    ""
                }
            ))
            .build();
        dialog.add_responses(&[("cancel", "Cancel"), ("trash", "Move to Trash")]);
        dialog.set_response_appearance("trash", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        let editor = self.clone();
        dialog.connect_response(None, move |_, response| {
            if response != "trash" { return; }
            if let Err(error) = vault::validate_entry(&editor.root, &path) {
                editor.error(&error.to_string());
                return;
            }
            if !editor.trash_pending.borrow_mut().insert(path.clone()) { return; }
            editor.update();
            let editor = editor.clone();
            let path = path.clone();
            gio::File::for_path(&path).trash_async(glib::Priority::DEFAULT, gio::Cancellable::NONE, move |result| {
                editor.trash_pending.borrow_mut().remove(&path);
                match result {
                    Ok(()) => {
                        sync::record_deletion(&editor, &path);
                        if editor.document.borrow().path.as_ref().is_some_and(|open| open.starts_with(&path)) {
                            *editor.document.borrow_mut() = Document::default();
                            editor.buffer.begin_irreversible_action();
                            editor.buffer.set_text("Choose a note from the sidebar.");
                            editor.buffer.end_irreversible_action();
                            editor.buffer.set_modified(false);
                            editor.view.set_cursor_visible(false);
                            editor.status.set_text(if folder { "Folder moved to Trash" } else { "Note moved to Trash" });
                        }
                        editor.update();
                        editor.refresh_button.emit_clicked();
                        sync::request(&editor);
                        editor.toasts.add_toast(adw::Toast::new(&format!("{} moved to Trash", path.file_name().unwrap_or_default().to_string_lossy())));
                    }
                    Err(error) => {
                        editor.update();
                        editor.error(&format!("Could not move {} to Trash: {error}\n\nYour note and editor contents have been kept.", path.display()));
                    }
                }
            });
        });
        dialog.present(Some(&self.window));
    }
}

fn install_note_menu(
    list: &gtk::ListBox,
    editor: &Editor,
    resolve: impl Fn(&gtk::ListBoxRow) -> Option<PathBuf> + 'static,
) -> gtk::GestureClick {
    let resolve = Rc::new(resolve);
    let target = Rc::new(RefCell::new(None::<PathBuf>));
    let model = gio::Menu::new();
    model.append(Some("Rename"), Some("note.rename"));
    model.append(Some("Move to Trash"), Some("note.trash"));
    let menu = gtk::PopoverMenu::from_model(Some(&model));
    menu.set_has_arrow(false);
    menu.set_position(gtk::PositionType::Bottom);
    menu.set_halign(gtk::Align::Start);
    menu.set_parent(list);
    let action = gio::SimpleAction::new("trash", None);
    action.connect_activate({
        let target = target.clone();
        let menu = menu.clone();
        let editor = editor.clone();
        move |_, _| {
            let Some(path) = target.borrow_mut().take() else {
                return;
            };
            menu.popdown();
            editor.request_trash(path);
        }
    });
    let group = gio::SimpleActionGroup::new();
    let rename = gio::SimpleAction::new("rename", None);
    rename.connect_activate({
        let target = target.clone();
        let menu = menu.clone();
        let editor = editor.clone();
        move |_, _| {
            if let Some(path) = target.borrow_mut().take() {
                menu.popdown();
                gtk::prelude::WidgetExt::activate_action(
                    &editor.window,
                    "win.rename-note",
                    Some(&path.to_string_lossy().to_string().to_variant()),
                )
                .unwrap();
            }
        }
    });
    group.add_action(&rename);
    group.add_action(&action);
    let new_here = gio::SimpleAction::new("new-here", None);
    new_here.connect_activate({
        let target = target.clone();
        let menu = menu.clone();
        let editor = editor.clone();
        move |_, _| {
            if let Some(path) = target.borrow_mut().take() {
                menu.popdown();
                gtk::prelude::WidgetExt::activate_action(
                    &editor.window,
                    "win.new-note-here",
                    Some(&path.to_string_lossy().to_string().to_variant()),
                )
                .unwrap();
            }
        }
    });
    group.add_action(&new_here);
    let prepare_menu: Rc<dyn Fn(&std::path::Path)> = Rc::new(move |path| {
        model.remove_all();
        if path.is_dir() {
            model.append(Some("New note here"), Some("note.new-here"));
        }
        model.append(Some("Rename"), Some("note.rename"));
        model.append(Some("Move to Trash"), Some("note.trash"));
    });
    list.insert_action_group("note", Some(&group));
    let gesture = gtk::GestureClick::new();
    gesture.set_button(3);
    gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
    gesture.connect_pressed({
        let list = list.clone();
        let menu = menu.clone();
        let target = target.clone();
        let resolve = resolve.clone();
        let prepare_menu = prepare_menu.clone();
        move |gesture, _, x, y| {
            let Some(path) = list.row_at_y(y as i32).and_then(|row| resolve(&row)) else {
                return;
            };
            prepare_menu(&path);
            *target.borrow_mut() = Some(path);
            gesture.set_state(gtk::EventSequenceState::Claimed);
            menu.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            menu.popup();
        }
    });
    list.add_controller(gesture.clone());
    let keys = gtk::EventControllerKey::new();
    keys.connect_key_pressed({
        let list = list.clone();
        let menu = menu.clone();
        move |_, key, _, modifiers| {
            if key != gtk::gdk::Key::Menu
                && !(key == gtk::gdk::Key::F10
                    && modifiers.contains(gtk::gdk::ModifierType::SHIFT_MASK))
            {
                return glib::Propagation::Proceed;
            }
            let Some(row) = list
                .focus_child()
                .and_then(|child| child.downcast::<gtk::ListBoxRow>().ok())
            else {
                return glib::Propagation::Proceed;
            };
            let Some(path) = resolve(&row) else {
                return glib::Propagation::Proceed;
            };
            prepare_menu(&path);
            *target.borrow_mut() = Some(path);
            let Some(bounds) = row.compute_bounds(&list) else {
                return glib::Propagation::Proceed;
            };
            menu.set_pointing_to(Some(&gtk::gdk::Rectangle::new(
                bounds.x() as i32,
                bounds.y() as i32,
                bounds.width() as i32,
                bounds.height() as i32,
            )));
            menu.popup();
            glib::Propagation::Stop
        }
    });
    list.add_controller(keys);
    list.connect_unrealize(move |_| menu.unparent());
    gesture
}

fn build(app: &adw::Application, root: PathBuf) {
    let css = gtk::CssProvider::new();
    css.load_from_string(include_str!("style.css"));
    gtk::style_context_add_provider_for_display(
        &gtk::gdk::Display::default().expect("No display available"),
        &css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Hematite")
        .default_width(1100)
        .default_height(760)
        .build();
    let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 0);
    sidebar.add_css_class("vault-sidebar");
    sidebar.set_size_request(230, -1);
    let refresh = gtk::Button::from_icon_name("view-refresh-symbolic");
    refresh.set_tooltip_text(Some("Refresh notes"));
    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Search notes…"));
    search.set_tooltip_text(Some(
        "Search titles, paths, and contents. All words must match; title matches rank first.",
    ));
    search.set_margin_start(12);
    search.set_margin_end(12);
    search.set_margin_top(10);
    search.set_margin_bottom(10);
    sidebar.append(&search);
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    list.add_css_class("navigation-sidebar");
    let results = gtk::ListBox::new();
    results.set_selection_mode(gtk::SelectionMode::None);
    results.add_css_class("navigation-sidebar");
    let stack = gtk::Stack::new();
    stack.set_vhomogeneous(false);
    let tree_page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let draft_note = gtk::Box::new(gtk::Orientation::Vertical, 0);
    draft_note.set_visible(false);
    tree_page.append(&draft_note);
    tree_page.append(&list);
    stack.add_named(&tree_page, Some("tree"));
    stack.add_named(&results, Some("search"));
    let search_status = gtk::Label::new(None);
    search_status.set_xalign(0.0);
    search_status.set_wrap(true);
    search_status.add_css_class("search-status");
    search_status.set_visible(false);
    sidebar.append(&search_status);
    let scroller = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&stack)
        .build();
    sidebar.append(&scroller);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let header = adw::HeaderBar::new();
    let options = gtk::MenuButton::new();
    options.set_icon_name("open-menu-symbolic");
    options.set_tooltip_text(Some("Options"));
    header.pack_end(&options);
    let new_note = gtk::Button::from_icon_name("tab-new-symbolic");
    new_note.set_tooltip_text(Some("New note (Ctrl+N)"));
    let create_buttons = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    create_buttons.add_css_class("linked");
    create_buttons.append(&new_note);
    let create_menu = gio::Menu::new();
    create_menu.append(Some("New note"), Some("win.new-note"));
    create_menu.append(Some("New folder"), Some("win.new-folder"));
    let create_dropdown = gtk::MenuButton::new();
    create_dropdown.set_icon_name("pan-down-symbolic");
    create_dropdown.set_tooltip_text(Some("Create a note or folder"));
    create_dropdown.set_menu_model(Some(&create_menu));
    create_buttons.append(&create_dropdown);
    header.pack_start(&create_buttons);
    header.pack_start(&refresh);
    let title = adw::WindowTitle::new("Hematite", "Select a note to begin");
    header.set_title_widget(Some(&title));
    let buffer = gtk::TextBuffer::builder().enable_undo(true).build();
    let markdown_view = markdown::View::new(&buffer);
    let view: gtk::TextView = markdown_view.clone().upcast();
    view.set_wrap_mode(gtk::WrapMode::WordChar);
    view.set_left_margin(40);
    view.set_right_margin(40);
    view.set_top_margin(30);
    view.set_bottom_margin(30);
    view.set_editable(false);
    view.set_cursor_visible(false);
    view.add_css_class("editor");
    view.set_monospace(true);
    buffer.set_text("Choose a note from the sidebar.\n\nYour vault is opened without changing any files.\nSave edits with Ctrl+S.");
    buffer.set_modified(false);
    let editor_scroll = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .hexpand(true)
        .child(&view)
        .build();
    content.append(&editor_scroll);
    let status = gtk::Label::new(Some("Select a note to begin"));
    status.set_xalign(0.0);
    status.add_css_class("editor-status");
    content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    status.set_hexpand(true);
    footer.append(&status);
    let sync_status = gtk::Button::new();
    sync_status.set_margin_end(10);
    footer.append(&sync_status);
    content.append(&footer);
    let split = gtk::Paned::new(gtk::Orientation::Horizontal);
    split.set_start_child(Some(&sidebar));
    split.set_end_child(Some(&content));
    split.set_position(280);
    split.set_resize_start_child(false);
    split.set_shrink_start_child(false);
    split.set_shrink_end_child(false);
    split.set_vexpand(true);
    let layout = gtk::Box::new(gtk::Orientation::Vertical, 0);
    layout.append(&header);
    layout.append(&split);
    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&layout));
    window.set_content(Some(&toasts));
    let editor = Editor {
        window: window.clone(),
        buffer: buffer.clone(),
        view,
        title,
        status,
        refresh_button: refresh.clone(),
        list: list.clone(),
        results: results.clone(),
        draft_note: draft_note.clone(),
        document: Rc::default(),
        root: root.clone(),
        trash_pending: Rc::default(),
        toasts,
        sync: Rc::default(),
    };
    let opened_links = Rc::new(RefCell::new(Vec::<String>::new()));
    images::install(&markdown_view, &editor);
    notes::install(&editor, &new_note, &search, &draft_note, &scroller);
    moves::install_root(&editor, &scroller);
    preferences::install(&editor);
    sync::install(&editor, &options, &sync_status);
    markdown::install(&markdown_view, {
        let editor = editor.clone();
        let opened_links = opened_links.clone();
        let test_links = std::env::var_os("HEMATITE_MARKDOWN_SMOKE_TEST").is_some();
        move |uri| {
            if test_links {
                opened_links.borrow_mut().push(uri.into());
                return;
            }
            let error_editor = editor.clone();
            gtk::UriLauncher::new(uri).launch(
                Some(&editor.window),
                gio::Cancellable::NONE,
                move |result| {
                    if let Err(error) = result {
                        error_editor.error(&format!("Could not open link: {error}"));
                    }
                },
            );
        }
    });

    let tree = Rc::new(RefCell::new(Sidebar::default()));
    let indexed = Rc::new(RefCell::new(Vec::<search::Note>::new()));
    let hits = Rc::new(RefCell::new(Vec::<search::Hit>::new()));
    let indexing = Rc::new(Cell::new(true));
    let unreadable = Rc::new(Cell::new(0usize));
    let generation = Rc::new(Cell::new(0u64));
    let update_search: Rc<dyn Fn()> = {
        let indexed = indexed.clone();
        let hits = hits.clone();
        let indexing = indexing.clone();
        let unreadable = unreadable.clone();
        let search = search.clone();
        let results = results.clone();
        let stack = stack.clone();
        let search_status = search_status.clone();
        let editor = editor.clone();
        let scroller = scroller.clone();
        let previous_query = RefCell::new(String::new());
        Rc::new(move || {
            let query = search.text();
            let query_changed = previous_query.borrow().as_str() != query.as_str();
            *previous_query.borrow_mut() = query.to_string();
            if query.trim().is_empty() {
                stack.set_visible_child_name("tree");
                search_status.set_visible(false);
                return;
            }
            stack.set_visible_child_name("search");
            let text = editor.text();
            let path = editor.document.borrow().path.clone();
            let matches = search::find(
                &indexed.borrow(),
                &query,
                path.as_deref().map(|path| (path, text.as_str())),
            );
            while let Some(row) = results.row_at_index(0) {
                results.remove(&row);
            }
            for hit in &matches {
                let row = gtk::ListBoxRow::new();
                row.add_css_class("note-row");
                row.set_tooltip_text(Some(&hit.relative));
                let labels = gtk::Box::new(gtk::Orientation::Vertical, 4);
                let title = gtk::Label::new(Some(&hit.title));
                title.set_xalign(0.0);
                title.set_ellipsize(gtk::pango::EllipsizeMode::End);
                labels.append(&title);
                let location = gtk::Label::new(Some(&hit.relative));
                location.set_xalign(0.0);
                location.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
                location.add_css_class("search-preview");
                labels.append(&location);
                if let Some(preview) = &hit.preview {
                    let preview = gtk::Label::new(Some(preview));
                    preview.set_xalign(0.0);
                    preview.set_wrap(true);
                    preview.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                    preview.set_lines(2);
                    preview.set_ellipsize(gtk::pango::EllipsizeMode::End);
                    preview.add_css_class("search-preview");
                    labels.append(&preview);
                }
                row.set_child(Some(&labels));
                moves::install_row(&editor, &row, &hit.path, false);
                results.append(&row);
            }
            let count = matches.len();
            *hits.borrow_mut() = matches;
            let message = if indexing.get() {
                format!("Indexing contents… · {count} matches")
            } else if count == 0 {
                "No matching notes".into()
            } else {
                format!(
                    "{count} matching {}",
                    if count == 1 { "note" } else { "notes" }
                )
            };
            search_status.set_text(&if unreadable.get() > 0 {
                format!(
                    "{message} · {} notes searchable by name only",
                    unreadable.get()
                )
            } else {
                message
            });
            search_status.set_visible(true);
            if query_changed {
                let adjustment = scroller.vadjustment();
                adjustment.set_value(adjustment.lower());
            }
            editor.update();
        })
    };
    let populate: Rc<dyn Fn()> = {
        let editor = editor.clone();
        let list = list.clone();
        let tree = tree.clone();
        let indexed = indexed.clone();
        let indexing = indexing.clone();
        let unreadable = unreadable.clone();
        let generation = generation.clone();
        let update_search = update_search.clone();
        Rc::new(move || {
            notes::reset_draft_parent(&editor);
            while let Some(row) = list.row_at_index(0) {
                list.remove(&row);
            }
            if !editor.root.is_dir() {
                sync::vault_missing(&editor);
            }
            match std::fs::create_dir_all(&editor.root).and_then(|()| vault::entries(&editor.root))
            {
                Ok(entries) => {
                    let live: HashSet<_> = entries
                        .iter()
                        .filter(|entry| !entry.directory)
                        .map(|entry| entry.path.clone())
                        .collect();
                    indexed
                        .borrow_mut()
                        .retain(|note| live.contains(&note.path));
                    {
                        let mut state = tree.borrow_mut();
                        let expand = preferences::folders_start_expanded();
                        for entry in entries.iter().filter(|entry| entry.directory) {
                            if state.seen_folders.insert(entry.path.clone()) && expand {
                                state.expanded.insert(entry.path.clone());
                            }
                        }
                        state.entries = entries.clone();
                    }
                    for entry in &entries {
                        let path = &entry.path;
                        let row = gtk::ListBoxRow::new();
                        row.add_css_class("note-row");
                        let labels = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                        labels.set_margin_start(entry.depth as i32 * 16);
                        let icon = gtk::Image::from_icon_name(if entry.directory {
                            "pan-end-symbolic"
                        } else {
                            "text-x-generic-symbolic"
                        });
                        icon.add_css_class("dim-label");
                        labels.append(&icon);
                        let name = gtk::Label::new(
                            if entry.directory {
                                path.file_name()
                            } else {
                                path.file_stem()
                            }
                            .and_then(|s| s.to_str()),
                        );
                        name.set_xalign(0.0);
                        name.set_ellipsize(gtk::pango::EllipsizeMode::End);
                        labels.append(&name);
                        let relative = path
                            .strip_prefix(&editor.root)
                            .unwrap_or(path)
                            .to_string_lossy();
                        row.set_tooltip_text(Some(&relative));
                        row.set_child(Some(&labels));
                        moves::install_row(&editor, &row, path, entry.directory);
                        list.append(&row);
                    }
                    if !entries.iter().any(|entry| !entry.directory) {
                        editor
                            .status
                            .set_text("No .md or .txt notes found in this vault");
                    }
                    tree.borrow().refresh(&list);
                    notes::position_draft(&editor);
                    if editor.document.borrow().path.is_some() {
                        editor.update();
                    }
                    indexing.set(true);
                    generation.set(generation.get() + 1);
                    let version = generation.get();
                    let (sender, receiver) = std::sync::mpsc::channel();
                    let root = editor.root.clone();
                    std::thread::spawn(move || {
                        let _ = sender.send(search::index(&entries, &root));
                    });
                    let indexed = indexed.clone();
                    let indexing = indexing.clone();
                    let unreadable = unreadable.clone();
                    let generation = generation.clone();
                    let timer_update_search = update_search.clone();
                    glib::timeout_add_local(std::time::Duration::from_millis(25), move || {
                        if generation.get() != version {
                            return glib::ControlFlow::Break;
                        }
                        match receiver.try_recv() {
                            Ok((notes, failures)) => {
                                *indexed.borrow_mut() = notes;
                                unreadable.set(failures);
                                indexing.set(false);
                                timer_update_search();
                                glib::ControlFlow::Break
                            }
                            Err(std::sync::mpsc::TryRecvError::Empty) => {
                                glib::ControlFlow::Continue
                            }
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                indexing.set(false);
                                timer_update_search();
                                glib::ControlFlow::Break
                            }
                        }
                    });
                    update_search();
                }
                Err(error) => editor.error(&format!(
                    "Could not read vault {}: {error}",
                    editor.root.display()
                )),
            }
        })
    };
    list.set_filter_func({
        let tree = tree.clone();
        let root = root.clone();
        move |row| {
            let tree = tree.borrow();
            tree.entries
                .get(row.index() as usize)
                .is_some_and(|entry| tree.visible(entry, &root))
        }
    });
    search.connect_search_changed({
        let update_search = update_search.clone();
        move |_| update_search()
    });
    search.connect_activate({
        let update_search = update_search.clone();
        let results = results.clone();
        move |search| {
            if search.text().trim().is_empty() {
                return;
            }
            update_search();
            if let Some(row) = results.row_at_index(0) {
                results.emit_by_name::<()>("row-activated", &[&row]);
            }
        }
    });
    search.connect_stop_search({
        let update_search = update_search.clone();
        let view = editor.view.clone();
        move |search| {
            search.set_text("");
            update_search();
            view.grab_focus();
        }
    });
    results.connect_row_activated({
        let editor = editor.clone();
        let hits = hits.clone();
        let search = search.clone();
        move |_, row| {
            let Some(path) = hits
                .borrow()
                .get(row.index() as usize)
                .map(|hit| hit.path.clone())
            else {
                return;
            };
            let query = search.text().to_string();
            if editor.document.borrow().path.as_ref() == Some(&path) {
                editor.reveal_match(&query);
                return;
            }
            let next = editor.clone();
            editor.confirm(move || {
                next.open(&path);
                if next.document.borrow().path.as_ref() == Some(&path) {
                    next.reveal_match(&query);
                }
            });
        }
    });
    let tree_menu = install_note_menu(&list, &editor, {
        let tree = tree.clone();
        move |row| {
            tree.borrow()
                .entries
                .get(row.index() as usize)
                .map(|entry| entry.path.clone())
        }
    });
    let results_menu = install_note_menu(&results, &editor, {
        let hits = hits.clone();
        move |row| {
            hits.borrow()
                .get(row.index() as usize)
                .map(|hit| hit.path.clone())
        }
    });
    refresh.connect_clicked({
        let populate = populate.clone();
        move |_| populate()
    });
    list.connect_row_activated({
        let editor = editor.clone();
        let tree = tree.clone();
        move |_, row| {
            let Some(entry) = tree.borrow().entries.get(row.index() as usize).cloned() else {
                return;
            };
            let path = entry.path;
            if entry.directory {
                {
                    let mut state = tree.borrow_mut();
                    if !state.expanded.remove(&path) {
                        state.expanded.insert(path);
                    }
                }
                tree.borrow().refresh(&editor.list);
                return;
            }
            if editor.document.borrow().path.as_ref() == Some(&path) {
                return;
            }
            let next = editor.clone();
            editor.confirm(move || next.open(&path));
        }
    });
    buffer.connect_changed({
        let editor = editor.clone();
        let update_search = update_search.clone();
        let search = search.clone();
        let pending = Rc::new(RefCell::new(None::<glib::SourceId>));
        move |_| {
            editor.update();
            if search.text().trim().is_empty() {
                return;
            }
            if let Some(source) = pending.borrow_mut().take() {
                source.remove();
            }
            let timer_pending = pending.clone();
            let update_search = update_search.clone();
            let source =
                glib::timeout_add_local_once(std::time::Duration::from_millis(150), move || {
                    timer_pending.borrow_mut().take();
                    update_search();
                });
            *pending.borrow_mut() = Some(source);
        }
    });
    buffer.connect_modified_changed({
        let editor = editor.clone();
        move |_| editor.update()
    });
    let save_action = gio::SimpleAction::new("save", None);
    save_action.connect_activate({
        let editor = editor.clone();
        move |_, _| {
            if editor.buffer.is_modified() {
                editor.save();
            }
        }
    });
    window.add_action(&save_action);
    app.set_accels_for_action("win.save", &["<Control>s"]);
    let search_action = gio::SimpleAction::new("search", None);
    search_action.connect_activate({
        let search = search.clone();
        move |_, _| {
            search.grab_focus();
            search.select_region(0, -1);
        }
    });
    window.add_action(&search_action);
    app.set_accels_for_action("win.search", &["<Control>f"]);
    for (name, marker, accelerator) in [("bold", "**", "<Control>b"), ("italic", "*", "<Control>i")]
    {
        let action = gio::SimpleAction::new(name, None);
        action.connect_activate({
            let editor = editor.clone();
            move |_, _| {
                if editor.view.is_editable() && editor.view.has_focus() {
                    markdown::toggle(&editor.buffer, marker);
                }
            }
        });
        window.add_action(&action);
        app.set_accels_for_action(&format!("win.{name}"), &[accelerator]);
    }
    window.connect_close_request({
        let editor = editor.clone();
        move |_| {
            if !editor.buffer.is_modified() {
                return glib::Propagation::Proceed;
            }
            let next = editor.clone();
            editor.confirm(move || {
                next.buffer.set_modified(false);
                next.window.close();
            });
            glib::Propagation::Stop
        }
    });
    window.present();
    populate();

    if std::env::var_os("HEMATITE_SMOKE_TEST").is_some()
        && std::env::var_os("HEMATITE_HOVER_SMOKE_TEST").is_some()
    {
        glib::MainContext::default().spawn_local(markdown::hover_smoke(editor));
        return;
    }

    if std::env::var_os("HEMATITE_SMOKE_TEST").is_some()
        && std::env::var_os("HEMATITE_MOVE_SMOKE_TEST").is_some()
    {
        glib::MainContext::default().spawn_local(moves::smoke(editor));
        return;
    }

    if std::env::var_os("HEMATITE_SMOKE_TEST").is_some()
        && std::env::var_os("HEMATITE_FOLDER_DRAFT_SMOKE_TEST").is_some()
    {
        glib::MainContext::default().spawn_local(notes::smoke(editor));
        return;
    }

    if std::env::var_os("HEMATITE_SMOKE_TEST").is_some()
        && std::env::var_os("HEMATITE_TABLE_SMOKE_TEST").is_some()
    {
        glib::MainContext::default().spawn_local(markdown::table_smoke(editor));
        return;
    }

    // Opt-in UI integration check; require an explicitly supplied scratch vault.
    if std::env::var_os("HEMATITE_SMOKE_TEST").is_some() {
        glib::timeout_add_local_once(std::time::Duration::from_millis(500), move || {
            let path = root.join("smoke.md");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "smoke fixture\n");
            let folder_index = tree
                .borrow()
                .entries
                .iter()
                .position(|entry| entry.directory);
            if let Some(folder_index) = folder_index {
                let folder = list.row_at_index(folder_index as i32).unwrap();
                let child = list.row_at_index(folder_index as i32 + 1).unwrap();
                if child.is_child_visible() {
                    list.emit_by_name::<()>("row-activated", &[&folder]);
                }
                assert!(!child.is_child_visible());
                list.emit_by_name::<()>("row-activated", &[&folder]);
                assert!(child.is_child_visible());
                list.emit_by_name::<()>("row-activated", &[&folder]);
                assert!(!child.is_child_visible());
                search.set_text("nested note");
                search.emit_by_name::<()>("search-changed", &[]);
                assert_eq!(stack.visible_child_name().as_deref(), Some("search"));
                assert!(hits.borrow().iter().any(|hit| hit.title == "nested note"));
                assert!(!child.is_child_visible());
                search.set_text("");
                search.emit_by_name::<()>("search-changed", &[]);
                assert!(!child.is_child_visible());
                list.emit_by_name::<()>("row-activated", &[&folder]);
            }
            let note_index = tree
                .borrow()
                .entries
                .iter()
                .position(|entry| entry.path == path)
                .unwrap();
            let row = list
                .row_at_index(note_index as i32)
                .expect("Fixture must appear in the sidebar");
            search.set_text("no matching note");
            search.emit_by_name::<()>("search-changed", &[]);
            assert!(results.row_at_index(0).is_none());
            assert_eq!(search_status.text(), "No matching notes");
            search.set_text("fixture");
            search.emit_by_name::<()>("search-changed", &[]);
            assert!(hits.borrow().iter().all(|hit| {
                hit.preview
                    .as_ref()
                    .is_some_and(|text| text.to_lowercase().contains("fixture"))
            }));
            let result_index = hits
                .borrow()
                .iter()
                .position(|hit| hit.path == path)
                .expect("Content match must appear");
            let result = results.row_at_index(result_index as i32).unwrap();
            results.emit_by_name::<()>("row-activated", &[&result]);
            assert_eq!(editor.document.borrow().path.as_ref(), Some(&path));
            let (start, end) = editor
                .buffer
                .selection_bounds()
                .expect("Content match should be selected");
            assert_eq!(editor.buffer.text(&start, &end, true), "fixture");
            let top_result = hits.borrow()[0].path.clone();
            search.emit_by_name::<()>("activate", &[]);
            assert_eq!(editor.document.borrow().path.as_ref(), Some(&top_result));
            search.emit_by_name::<()>("stop-search", &[]);
            assert!(search.text().is_empty());
            assert_eq!(stack.visible_child_name().as_deref(), Some("tree"));
            assert!(row.is_child_visible());
            list.emit_by_name::<()>("row-activated", &[&row]);
            assert_eq!(editor.document.borrow().path.as_ref(), Some(&path));
            editor.buffer.insert_at_cursor("edited\n");
            assert!(editor.buffer.is_modified());
            search.set_text("edited");
            search.emit_by_name::<()>("search-changed", &[]);
            assert_eq!(hits.borrow().len(), 1);
            assert_eq!(hits.borrow()[0].path, path);
            search.set_text("");
            search.emit_by_name::<()>("search-changed", &[]);
            assert!(editor.buffer.can_undo());
            editor.buffer.undo();
            assert_eq!(editor.text(), "smoke fixture\n");
            editor.buffer.redo();
            assert!(editor.title.title().starts_with("•  "));
            assert!(!editor.status.text().contains("Unsaved"));
            gtk::prelude::WidgetExt::activate_action(&editor.window, "win.save", None).unwrap();
            assert!(!editor.title.title().starts_with("•"));
            assert!(!editor.buffer.is_modified());
            assert!(std::fs::read_to_string(&path).unwrap().contains("edited"));
            assert_eq!(tree.borrow().entries.first().unwrap().path, path);
            assert!(list.row_at_index(0).unwrap().has_css_class("active-note"));
            glib::timeout_add_local_once(std::time::Duration::from_millis(250), move || {
                search.set_text("fixture");
                search.emit_by_name::<()>("search-changed", &[]);
                glib::timeout_add_local_once(std::time::Duration::from_millis(300), move || {
                    if let Some(path) = std::env::var_os("HEMATITE_SCREENSHOT") {
                        let snapshot = gtk::Snapshot::new();
                        let paintable = gtk::WidgetPaintable::new(Some(&editor.window));
                        paintable.snapshot(
                            &snapshot,
                            editor.window.width().into(),
                            editor.window.height().into(),
                        );
                        let node = snapshot.to_node().expect("Window must have rendered");
                        editor
                            .window
                            .renderer()
                            .unwrap()
                            .render_texture(&node, None)
                            .save_to_png(PathBuf::from(path))
                            .unwrap();
                    }
                    if std::env::var_os("HEMATITE_TRASH_SMOKE_TEST").is_some() {
                        glib::MainContext::default().spawn_local(trash_smoke(
                            editor,
                            tree_menu,
                            results_menu,
                            search,
                            tree,
                            hits,
                            update_search,
                        ));
                    } else if std::env::var_os("HEMATITE_MARKDOWN_SMOKE_TEST").is_some() {
                        glib::MainContext::default().spawn_local(markdown_smoke::run(
                            editor,
                            search,
                            opened_links,
                        ));
                    } else {
                        editor.window.close();
                    }
                });
            });
        });
    }
}

fn main() -> glib::ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let root = match args.as_slice() {
        [] => preferences::vault_directory(),
        [flag, path] if flag == "--vault" => PathBuf::from(path),
        _ => {
            eprintln!("Usage: hematite [--vault PATH]");
            return glib::ExitCode::FAILURE;
        }
    };
    if std::env::var_os("HEMATITE_PREFERENCES_SMOKE_TEST").is_some()
        && (std::env::var_os("HEMATITE_MARKDOWN_SMOKE_TEST").is_none()
            || std::env::var_os("HEMATITE_SMOKE_TEST").is_none()
            || std::env::var("GSETTINGS_BACKEND").as_deref() != Ok("memory")
            || args.is_empty()
            || !std::env::var_os("XDG_CONFIG_HOME")
                .is_some_and(|path| PathBuf::from(path).starts_with(&root)))
    {
        eprintln!(
            "Preferences testing requires smoke and Markdown testing, GSETTINGS_BACKEND=memory, an explicit scratch vault, and isolated XDG_CONFIG_HOME."
        );
        return glib::ExitCode::FAILURE;
    }
    if std::env::var_os("HEMATITE_SYNC_SMOKE_TEST").is_some()
        && (std::env::var_os("HEMATITE_MARKDOWN_SMOKE_TEST").is_none()
            || std::env::var_os("HEMATITE_SMOKE_TEST").is_none()
            || args.is_empty()
            || !std::env::var_os("XDG_DATA_HOME")
                .is_some_and(|path| PathBuf::from(path).starts_with(&root))
            || !std::env::var("HEMATITE_TEST_WEBDAV_URL")
                .is_ok_and(|url| url.contains("/hematite-sync-test-")))
    {
        eprintln!(
            "Sync testing requires smoke and Markdown testing, an explicit scratch vault, isolated XDG_DATA_HOME, and a temporary WebDAV folder."
        );
        return glib::ExitCode::FAILURE;
    }
    if std::env::var_os("HEMATITE_SMOKE_TEST").is_some() && args.is_empty() {
        eprintln!("Smoke testing requires --vault pointing to a scratch directory.");
        return glib::ExitCode::FAILURE;
    }
    if (std::env::var_os("HEMATITE_TRASH_SMOKE_TEST").is_some()
        || std::env::var_os("HEMATITE_FOLDER_OPERATIONS_SMOKE_TEST").is_some())
        && (std::env::var_os("HEMATITE_SMOKE_TEST").is_none()
            || args.is_empty()
            || !std::env::var_os("XDG_DATA_HOME")
                .is_some_and(|data| PathBuf::from(data).starts_with(&root)))
    {
        eprintln!(
            "Trash testing requires smoke testing, an explicit scratch vault, and XDG_DATA_HOME inside that vault."
        );
        return glib::ExitCode::FAILURE;
    }
    if std::env::var_os("HEMATITE_MARKDOWN_SMOKE_TEST").is_some()
        && (std::env::var_os("HEMATITE_SMOKE_TEST").is_none() || args.is_empty())
    {
        eprintln!("Markdown testing requires smoke testing and an explicit scratch vault.");
        return glib::ExitCode::FAILURE;
    }
    if std::env::var_os("HEMATITE_SMOKE_TEST").is_some() {
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            hook(info);
            std::process::exit(101);
        }));
    }
    let app = adw::Application::builder()
        .application_id("io.github.hematite.Editor")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    gio::resources_register_include!("hematite.gresource")
        .expect("Could not register the app icon");
    app.connect_startup(|_| {
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::IconTheme::for_display(&display)
                .add_resource_path("/io/github/hematite/Editor/icons");
            if std::env::var_os("HEMATITE_SMOKE_TEST").is_some() {
                assert!(
                    gtk::IconTheme::for_display(&display).has_icon("io.github.hematite.Editor")
                );
            }
        }
        gtk::Window::set_default_icon_name("io.github.hematite.Editor");
    });
    app.connect_activate(move |app| build(app, root.clone()));
    app.run_with_args::<&str>(&[])
}
