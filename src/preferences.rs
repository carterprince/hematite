use super::*;
use std::time::Duration;

const SCHEMA_ID: &str = "io.github.hematite.Editor";

fn schema() -> gio::SettingsSchema {
    // Bundle the schema so cargo-run and standalone binaries work without an
    // installation step. Loading maps it into memory; the temporary copy can
    // be removed immediately. Settings themselves use the standard backend.
    let directory =
        std::env::temp_dir().join(format!("hematite-schema-{}", glib::uuid_string_random()));
    std::fs::create_dir(&directory).expect("Could not create schema directory");
    std::fs::write(
        directory.join("gschemas.compiled"),
        include_bytes!(concat!(env!("OUT_DIR"), "/schemas/gschemas.compiled")),
    )
    .expect("Could not load preferences schema");
    let result = gio::SettingsSchemaSource::from_directory(
        &directory,
        None::<&gio::SettingsSchemaSource>,
        true,
    )
    .expect("Could not load preferences schema")
    .lookup(SCHEMA_ID, false)
    .expect("Missing preferences schema");
    let _ = std::fs::remove_dir_all(directory);
    result
}
thread_local! {
    static SETTINGS: gio::Settings = gio::Settings::new_full(&schema(), gio::SettingsBackend::NONE, None);
}
fn settings() -> gio::Settings {
    SETTINGS.with(Clone::clone)
}
pub(super) fn folders_start_expanded() -> bool {
    settings().boolean("folders-start-expanded")
}
pub(super) fn sync_external_deletions() -> bool {
    settings().boolean("sync-external-deletions")
}
fn configured_vault() -> Option<PathBuf> {
    let value = settings().string("vault-directory");
    if value.is_empty() {
        None
    } else {
        Some(PathBuf::from(value.as_str()))
    }
}
pub(super) fn vault_directory() -> PathBuf {
    configured_vault().unwrap_or_else(|| glib::home_dir().join("Documents/Vault"))
}
fn display_directory(path: &std::path::Path) -> String {
    match path.strip_prefix(glib::home_dir()) {
        Ok(relative) if relative.as_os_str().is_empty() => "~".to_string(),
        Ok(relative) => format!("~/{}", relative.to_string_lossy()),
        Err(_) => path.to_string_lossy().into_owned(),
    }
}

struct Preferences {
    editor: Editor,
    enabled: Cell<bool>,
    timer: RefCell<Option<glib::SourceId>>,
    settings: gio::Settings,
}

impl Preferences {
    fn schedule(self: &Rc<Self>) {
        if let Some(timer) = self.timer.borrow_mut().take() {
            timer.remove();
        }
        if !self.enabled.get() {
            return;
        }
        let path = self.editor.document.borrow().path.clone();
        let preferences = self.clone();
        let timer = glib::timeout_add_local_once(Duration::from_secs(1), move || {
            preferences.timer.borrow_mut().take();
            let editor = &preferences.editor;
            if preferences.enabled.get()
                && path.is_some()
                && editor.document.borrow().path == path
                && editor.view.is_editable()
                && editor.buffer.is_modified()
            {
                editor.save();
            }
        });
        *self.timer.borrow_mut() = Some(timer);
    }

    fn show(self: &Rc<Self>) {
        let dialog = adw::PreferencesDialog::builder()
            .title("Preferences")
            .build();
        let page = adw::PreferencesPage::new();
        let vault = adw::PreferencesGroup::builder()
            .title("Vault")
            .description("Directory changes take effect when you reopen Hematite.")
            .build();
        let directory = adw::ActionRow::builder()
            .title("Vault directory")
            .subtitle(display_directory(
                &configured_vault().unwrap_or_else(|| self.editor.root.clone()),
            ))
            .build();
        let choose = gtk::Button::with_label("Choose…");
        choose.set_valign(gtk::Align::Center);
        directory.add_suffix(&choose);
        directory.set_activatable_widget(Some(&choose));
        choose.connect_clicked({
            let preferences = self.clone();
            let directory = directory.clone();
            move |_| {
                let preferences = preferences.clone();
                let directory = directory.clone();
                glib::spawn_future_local(async move {
                    let chooser = gtk::FileDialog::builder()
                        .title("Choose vault directory")
                        .initial_folder(&gio::File::for_path(
                            configured_vault().unwrap_or_else(|| preferences.editor.root.clone()),
                        ))
                        .build();
                    match chooser
                        .select_folder_future(Some(&preferences.editor.window))
                        .await
                    {
                        Ok(folder) => {
                            let Some(path) = folder.path() else {
                                preferences.editor.toasts.add_toast(adw::Toast::new(
                                    "Choose a local directory for the vault.",
                                ));
                                return;
                            };
                            let result = preferences
                                .settings
                                .set_string("vault-directory", &path.to_string_lossy());
                            if let Err(error) = result {
                                preferences
                                    .editor
                                    .toasts
                                    .add_toast(adw::Toast::new(&format!(
                                        "Could not save vault directory: {error}"
                                    )));
                                return;
                            }
                            directory.set_subtitle(&display_directory(&path));
                            preferences.editor.toasts.add_toast(adw::Toast::new(
                                "Vault directory saved. Reopen Hematite to use it.",
                            ));
                        }
                        Err(error)
                            if error.matches(gtk::DialogError::Dismissed)
                                || error.matches(gtk::DialogError::Cancelled) => {}
                        Err(error) => {
                            preferences
                                .editor
                                .toasts
                                .add_toast(adw::Toast::new(&format!(
                                    "Could not choose vault directory: {error}"
                                )))
                        }
                    }
                });
            }
        });
        vault.add(&directory);
        page.add(&vault);
        let group = adw::PreferencesGroup::builder().title("Editing").build();
        let autosave = adw::SwitchRow::builder()
            .title("Autosave")
            .subtitle("Save notes automatically after you stop typing")
            .active(self.enabled.get())
            .build();
        self.settings.bind("autosave", &autosave, "active").build();
        group.add(&autosave);
        page.add(&group);
        let folders = adw::SwitchRow::builder()
            .title("Folders start expanded")
            .subtitle("Expand folders when opening the vault")
            .active(folders_start_expanded())
            .build();
        self.settings
            .bind("folders-start-expanded", &folders, "active")
            .build();
        let browsing = adw::PreferencesGroup::builder().title("Sidebar").build();
        browsing.add(&folders);
        page.add(&browsing);
        let external = adw::SwitchRow::builder()
            .title("Sync files deleted outside Hematite")
            .subtitle("Remove remote copies when files are deleted in another app. A missing vault is always downloaded again.")
            .active(sync_external_deletions()).build();
        self.settings
            .bind("sync-external-deletions", &external, "active")
            .build();
        let sync = adw::PreferencesGroup::builder().title("Sync").build();
        sync.add(&external);
        page.add(&sync);
        dialog.add(&page);
        dialog.present(Some(&self.editor.window));
    }
}

pub(super) fn install(editor: &Editor) {
    let settings = settings();
    let preferences = Rc::new(Preferences {
        editor: editor.clone(),
        enabled: Cell::new(settings.boolean("autosave")),
        timer: RefCell::new(None),
        settings,
    });
    preferences.settings.connect_changed(Some("autosave"), {
        let preferences = Rc::downgrade(&preferences);
        move |settings, _| {
            if let Some(preferences) = preferences.upgrade() {
                preferences.enabled.set(settings.boolean("autosave"));
                preferences.schedule();
            }
        }
    });
    editor.buffer.connect_changed({
        let preferences = preferences.clone();
        move |_| preferences.schedule()
    });
    let action = gio::SimpleAction::new("preferences", None);
    action.connect_activate(move |_, _| preferences.show());
    editor.window.add_action(&action);
}

pub(super) async fn smoke(editor: &Editor) {
    fn switch(widget: &gtk::Widget) -> Option<adw::SwitchRow> {
        if let Ok(row) = widget.clone().downcast::<adw::SwitchRow>() {
            return Some(row);
        }
        let mut child = widget.first_child();
        while let Some(widget) = child {
            if let Some(row) = switch(&widget) {
                return Some(row);
            }
            child = widget.next_sibling();
        }
        None
    }
    gtk::prelude::WidgetExt::activate_action(&editor.window, "win.preferences", None).unwrap();
    let dialog = editor.window.visible_dialog().unwrap();
    assert!(dialog.is::<adw::PreferencesDialog>());
    let row = switch(dialog.upcast_ref()).unwrap();
    assert!(row.is_active());
    row.set_active(false);
    row.set_active(true);
    editor.buffer.insert_at_cursor("\nautosave fixture");
    glib::timeout_future(Duration::from_millis(1300)).await;
    let path = editor.document.borrow().path.clone().unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), editor.text());
    assert!(!editor.buffer.is_modified());
    row.set_active(false);
    assert!(!settings().boolean("autosave"));
    let saved = editor.text();
    editor.buffer.insert_at_cursor("\nmanual save fixture");
    glib::timeout_future(Duration::from_millis(1300)).await;
    assert!(editor.buffer.is_modified());
    assert_eq!(std::fs::read_to_string(path).unwrap(), saved);
    assert!(editor.save());
    dialog.close();
    println!("Preferences verified: autosave enabled saves; disabled retains unsaved changes.");
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn schema_defaults_and_independent_keys() {
        let schema = schema();
        let backend = gio::memory_settings_backend_new();
        let settings = gio::Settings::new_full(&schema, Some(&backend), None);
        assert!(settings.boolean("autosave"));
        assert!(settings.boolean("folders-start-expanded"));
        assert!(!settings.boolean("sync-external-deletions"));
        assert!(settings.string("vault-directory").is_empty());
        settings
            .set_string("vault-directory", "/tmp/chosen-vault")
            .unwrap();
        settings.set_boolean("autosave", false).unwrap();
        let reopened = gio::Settings::new_full(&schema, Some(&backend), None);
        assert_eq!(reopened.string("vault-directory"), "/tmp/chosen-vault");
        assert!(!reopened.boolean("autosave"));
        assert!(reopened.boolean("folders-start-expanded"));
    }
}
