use crate::markdown_syntax::{self as syntax, Document, Style};
use gtk::{glib, prelude::*, subclass::prelude::*};
use std::{
    cell::{Cell, RefCell},
    ops::Range,
    rc::Rc,
};

fn local_image_path(directory: &std::path::Path, destination: &str) -> std::path::PathBuf {
    let decoded = glib::uri_unescape_string(destination, None::<&str>);
    directory.join(decoded.as_deref().unwrap_or(destination))
}

// Concealed markers have a subpixel font. Position overlays vertically from
// the following normal-sized spacer, while keeping the marker's horizontal origin.
fn marker_rect(view: &View, offset: i32, spacer: i32) -> gtk::gdk::Rectangle {
    let buffer = view.buffer();
    let marker = view.iter_location(&buffer.iter_at_offset(offset));
    let text =
        view.iter_location(&buffer.iter_at_offset((offset + spacer).min(buffer.char_count())));
    gtk::gdk::Rectangle::new(marker.x(), text.y(), marker.width(), text.height())
}

fn checkbox_layout(view: &View, checked: bool) -> gtk::pango::Layout {
    let layout = view.create_pango_layout(Some(if checked { "☑" } else { "☐" }));
    let attributes = gtk::pango::AttrList::new();
    attributes.insert(gtk::pango::AttrFloat::new_scale(1.4));
    layout.set_attributes(Some(&attributes));
    layout
}

fn bullet_layout(view: &View) -> gtk::pango::Layout {
    let layout = view.create_pango_layout(Some("•"));
    let attributes = gtk::pango::AttrList::new();
    attributes.insert(gtk::pango::AttrFloat::new_scale(1.2));
    layout.set_attributes(Some(&attributes));
    layout
}

pub struct TablePreview {
    offset: i32,
    end: i32,
    layouts: Vec<gtk::pango::Layout>,
    width: f32,
    height: f32,
    header: bool,
}

mod imp {
    use super::*;
    #[derive(Default)]
    pub struct View {
        pub bullets: RefCell<Vec<i32>>,
        pub tasks: RefCell<Vec<(i32, bool)>>,
        pub tables: RefCell<Vec<TablePreview>>,
        pub paste_image: RefCell<Option<Box<dyn Fn() -> bool>>>,
        pub directory: RefCell<Option<std::path::PathBuf>>,
        pub image_revision: Cell<u64>,
        pub images: RefCell<Vec<(i32, gtk::gdk::Texture, f32, f32)>>,
    }
    #[glib::object_subclass]
    impl ObjectSubclass for View {
        const NAME: &'static str = "HematiteMarkdownView";
        type Type = super::View;
        type ParentType = gtk::TextView;
    }
    impl ObjectImpl for View {}
    impl WidgetImpl for View {}
    impl TextViewImpl for View {
        fn paste_clipboard(&self) {
            if !self
                .paste_image
                .borrow()
                .as_ref()
                .is_some_and(|paste| paste())
            {
                self.parent_paste_clipboard();
            }
        }
        fn extend_selection(
            &self,
            granularity: gtk::TextExtendSelection,
            location: &gtk::TextIter,
            start: &mut gtk::TextIter,
            end: &mut gtk::TextIter,
        ) -> glib::ControlFlow {
            if granularity != gtk::TextExtendSelection::Line {
                return self.parent_extend_selection(granularity, location, start, end);
            }
            // Triple-click selects the source line, regardless of soft wrapping.
            let buffer = self.obj().buffer();
            *start = buffer.iter_at_line(location.line()).unwrap();
            *end = buffer
                .iter_at_line(location.line() + 1)
                .unwrap_or_else(|| buffer.end_iter());
            // This vfunc uses a gboolean handled flag: Continue maps to true.
            glib::ControlFlow::Continue
        }

        fn snapshot_layer(&self, layer: gtk::TextViewLayer, snapshot: gtk::Snapshot) {
            self.parent_snapshot_layer(layer, snapshot.clone());
            if layer != gtk::TextViewLayer::AboveText {
                return;
            }
            let view = self.obj();
            let layout = bullet_layout(&view);
            let (_, height) = layout.pixel_size();
            let color = view.color();
            for &offset in self.bullets.borrow().iter() {
                let rect = marker_rect(&view, offset, 1);
                let y = rect.y() + (rect.height() - height) / 2;
                snapshot.save();
                snapshot.translate(&gtk::graphene::Point::new(rect.x() as f32, y as f32));
                snapshot.append_layout(&layout, &color);
                snapshot.restore();
            }
            for &(offset, checked) in self.tasks.borrow().iter() {
                let rect = marker_rect(&view, offset, 5);
                let layout = checkbox_layout(&view, checked);
                let (_, height) = layout.pixel_size();
                let y = rect.y() + (rect.height() - height) / 2;
                snapshot.save();
                snapshot.translate(&gtk::graphene::Point::new(rect.x() as f32, y as f32));
                snapshot.append_layout(&layout, &color);
                snapshot.restore();
            }
            let content_width = (view.width() - view.left_margin() - view.right_margin()) as f32;
            for row in self.tables.borrow().iter() {
                let rect = view.iter_location(&view.buffer().iter_at_offset(row.offset));
                let x = view.left_margin() as f32;
                let y = rect.y() as f32 - row.height;
                let columns = row.layouts.len().max(1) as f32;
                let cell_width = row.width / columns;
                let mut border = color;
                border.set_alpha(0.25);
                if row.header {
                    let mut background = color;
                    background.set_alpha(0.08);
                    snapshot.append_color(
                        &background,
                        &gtk::graphene::Rect::new(x, y, row.width, row.height),
                    );
                }
                for (column, layout) in row.layouts.iter().enumerate() {
                    let left = x + column as f32 * cell_width;
                    snapshot.save();
                    snapshot.translate(&gtk::graphene::Point::new(left + 10.0, y + 8.0));
                    snapshot.append_layout(layout, &color);
                    snapshot.restore();
                    snapshot
                        .append_color(&border, &gtk::graphene::Rect::new(left, y, 1.0, row.height));
                }
                snapshot.append_color(
                    &border,
                    &gtk::graphene::Rect::new(x + row.width - 1.0, y, 1.0, row.height),
                );
                snapshot.append_color(&border, &gtk::graphene::Rect::new(x, y, row.width, 1.0));
                snapshot.append_color(
                    &border,
                    &gtk::graphene::Rect::new(x, y + row.height - 1.0, row.width, 1.0),
                );
            }
            for (offset, texture, width, height) in self.images.borrow().iter() {
                let rect = view.iter_location(&view.buffer().iter_at_offset(*offset));
                snapshot.append_texture(
                    texture,
                    &gtk::graphene::Rect::new(
                        view.left_margin() as f32 + (content_width - width) / 2.0,
                        rect.y() as f32 - height - 8.0,
                        *width,
                        *height,
                    ),
                );
            }
        }
    }
}

glib::wrapper! {
    pub struct View(ObjectSubclass<imp::View>)
        @extends gtk::TextView, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Scrollable;
}

impl View {
    pub fn refresh_images(&self) {
        self.imp()
            .image_revision
            .set(self.imp().image_revision.get().wrapping_add(1));
        self.queue_draw();
    }
    pub(super) fn screen_width(&self) -> Option<i32> {
        let surface = self.native()?.surface()?;
        self.display()
            .monitor_at_surface(&surface)
            .map(|monitor| monitor.geometry().width())
    }
    pub fn set_note_directory(&self, directory: Option<std::path::PathBuf>) {
        *self.imp().directory.borrow_mut() = directory;
    }
    pub fn set_image_paste_handler(&self, handler: impl Fn() -> bool + 'static) {
        *self.imp().paste_image.borrow_mut() = Some(Box::new(handler));
    }

    pub fn new(buffer: &gtk::TextBuffer) -> Self {
        glib::Object::builder().property("buffer", buffer).build()
    }
}

struct Renderer {
    view: View,
    document: RefCell<Document>,
    tags: [gtk::TextTag; 11],
    busy: Cell<bool>,
    image_tags: RefCell<Vec<gtk::TextTag>>,
    textures: RefCell<std::collections::HashMap<std::path::PathBuf, Option<gtk::gdk::Texture>>>,
}

impl Renderer {
    fn editing_range(&self) -> Option<Range<i32>> {
        if !self.view.has_focus() || !self.view.is_editable() {
            return None;
        }
        let buffer = self.view.buffer();
        let (first, last) = buffer.selection_bounds().unwrap_or_else(|| {
            let caret = buffer.iter_at_mark(&buffer.get_insert());
            (caret, caret)
        });
        let start = buffer.iter_at_line(first.line()).unwrap().offset();
        let end = buffer
            .iter_at_line(last.line() + 1)
            .unwrap_or_else(|| buffer.end_iter())
            .offset();
        Some(start..end)
    }

    fn refresh(&self, reparse: bool) {
        if self.busy.replace(true) {
            return;
        }
        let buffer = self.view.buffer();
        if reparse {
            let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), true);
            *self.document.borrow_mut() = syntax::parse(&text);
            let directory = self.view.imp().directory.borrow();
            self.textures.borrow_mut().retain(|path, texture| {
                texture.is_some()
                    && directory.as_ref().is_some_and(|directory| {
                        self.document
                            .borrow()
                            .images
                            .iter()
                            .any(|(_, destination)| {
                                local_image_path(directory, destination) == *path
                            })
                    })
            });
        }
        for tag in &self.tags {
            buffer.remove_tag(tag, &buffer.start_iter(), &buffer.end_iter());
        }
        let document = self.document.borrow();
        let editing = self.editing_range();
        for tag in self.image_tags.borrow_mut().drain(..) {
            buffer.remove_tag(&tag, &buffer.start_iter(), &buffer.end_iter());
            buffer.tag_table().remove(&tag);
        }
        self.view.imp().images.borrow_mut().clear();
        self.view.imp().tables.borrow_mut().clear();
        let apply = |tag: &gtk::TextTag, range: Range<i32>| {
            if range.start < range.end {
                buffer.apply_tag(
                    tag,
                    &buffer.iter_at_offset(range.start),
                    &buffer.iter_at_offset(range.end),
                );
            }
        };
        for span in &document.styles {
            apply(
                &self.tags[match span.style {
                    Style::Bold => 0,
                    Style::Italic => 1,
                    Style::Strikethrough => 10,
                    Style::Heading(level) => 4 + level as usize,
                }],
                span.range.clone(),
            );
        }
        for link in &document.links {
            apply(&self.tags[2], link.label.clone());
        }
        for marker in &document.markers {
            if let Some(editing) = &editing {
                apply(&self.tags[3], marker.start..marker.end.min(editing.start));
                apply(&self.tags[3], marker.start.max(editing.end)..marker.end);
            } else {
                apply(&self.tags[3], marker.clone());
            }
        }
        if let Some(directory) = self.view.imp().directory.borrow().as_ref() {
            for (source, destination) in &document.images {
                if destination.contains("://")
                    || destination.starts_with("data:")
                    || editing
                        .as_ref()
                        .is_some_and(|line| line.start < source.end && source.start < line.end)
                {
                    continue;
                }
                let start = buffer.iter_at_offset(source.start);
                let end = buffer.iter_at_offset(source.end);
                let mut line_start = start;
                line_start.set_line_offset(0);
                let mut line_end = end;
                if !line_end.ends_line() {
                    line_end.forward_to_line_end();
                }
                if !buffer.text(&line_start, &start, true).trim().is_empty()
                    || !buffer.text(&end, &line_end, true).trim().is_empty()
                {
                    continue;
                }
                let path = local_image_path(directory, destination);
                let texture = self
                    .textures
                    .borrow_mut()
                    .entry(path.clone())
                    .or_insert_with(|| gtk::gdk::Texture::from_filename(&path).ok())
                    .clone();
                let Some(texture) = texture else { continue };
                let width = ((self.view.width()
                    - self.view.left_margin()
                    - self.view.right_margin()) as f32
                    * 0.7)
                    .min(
                        self.view
                            .screen_width()
                            .map(|width| width as f32 * 0.33)
                            .unwrap_or(f32::INFINITY),
                    )
                    .max(1.0);
                let height = width * texture.height() as f32 / texture.width() as f32;
                let tag = gtk::TextTag::builder()
                    .size(1)
                    .pixels_above_lines(height.ceil() as i32 + 8)
                    .pixels_below_lines(8)
                    .foreground_rgba(&gtk::gdk::RGBA::new(0.0, 0.0, 0.0, 0.0))
                    .build();
                buffer.tag_table().add(&tag);
                apply(&tag, source.start..source.start + 1);
                apply(&self.tags[3], source.start + 1..source.end);
                self.image_tags.borrow_mut().push(tag);
                self.view
                    .imp()
                    .images
                    .borrow_mut()
                    .push((source.start, texture, width, height));
            }
        }
        let table_width =
            (self.view.width() - self.view.left_margin() - self.view.right_margin()).max(80) as f32;
        for table in &document.tables {
            if let Some(header) = table.rows.first() {
                let line = buffer.iter_at_offset(header.range.start).line();
                if let Some(start) = buffer.iter_at_line(line + 1) {
                    let end = buffer
                        .iter_at_line(line + 2)
                        .unwrap_or_else(|| buffer.end_iter());
                    let separator = start.offset()..end.offset().min(table.range.end);
                    if !editing.as_ref().is_some_and(|range| {
                        range.start < separator.end && range.end > separator.start
                    }) {
                        apply(&self.tags[3], separator);
                    }
                }
            }
            for row in &table.rows {
                if row.range.is_empty()
                    || editing.as_ref().is_some_and(|range| {
                        range.start < row.range.end && range.end > row.range.start
                    })
                {
                    continue;
                }
                let cell_width = table_width / table.alignments.len().max(1) as f32;
                let layouts: Vec<_> = row
                    .cells
                    .iter()
                    .enumerate()
                    .map(|(column, cell)| {
                        let layout = self.view.create_pango_layout(None);
                        layout.set_markup(&if row.header {
                            format!("<b>{cell}</b>")
                        } else {
                            cell.clone()
                        });
                        layout.set_width(
                            ((cell_width - 20.0).max(1.0) * gtk::pango::SCALE as f32) as i32,
                        );
                        layout.set_wrap(gtk::pango::WrapMode::WordChar);
                        layout.set_alignment(match table.alignments.get(column) {
                            Some(pulldown_cmark::Alignment::Center) => {
                                gtk::pango::Alignment::Center
                            }
                            Some(pulldown_cmark::Alignment::Right) => gtk::pango::Alignment::Right,
                            _ => gtk::pango::Alignment::Left,
                        });
                        layout
                    })
                    .collect();
                let height = layouts
                    .iter()
                    .map(|layout| layout.pixel_size().1)
                    .max()
                    .unwrap_or(0) as f32
                    + 16.0;
                let tag = gtk::TextTag::builder()
                    .size(1)
                    .pixels_above_lines(height.ceil() as i32)
                    .foreground_rgba(&gtk::gdk::RGBA::new(0.0, 0.0, 0.0, 0.0))
                    .build();
                buffer.tag_table().add(&tag);
                apply(
                    &tag,
                    row.range.start..(row.range.end + 1).min(buffer.char_count()),
                );
                apply(&self.tags[3], row.range.start + 1..row.range.end);
                self.image_tags.borrow_mut().push(tag);
                self.view.imp().tables.borrow_mut().push(TablePreview {
                    offset: row.range.start,
                    end: row.range.end,
                    layouts,
                    width: table_width,
                    height,
                    header: row.header,
                });
            }
        }
        let bullets: Vec<_> = document
            .bullets
            .iter()
            .copied()
            .filter(|position| {
                !editing
                    .as_ref()
                    .is_some_and(|range| range.contains(position))
            })
            .collect();
        let (bullet_ink, _) = bullet_layout(&self.view).pixel_extents();
        self.tags[4].set_letter_spacing(
            (bullet_ink.x() + bullet_ink.width()) * gtk::pango::SCALE,
        );
        for &position in &bullets {
            if buffer.iter_at_offset(position + 1).char() == ' ' {
                apply(&self.tags[4], position + 1..position + 2);
            }
        }
        *self.view.imp().bullets.borrow_mut() = bullets;
        let tasks: Vec<_> = document
            .tasks
            .iter()
            .copied()
            .filter(|(position, _)| {
                !editing
                    .as_ref()
                    .is_some_and(|range| range.contains(position))
            })
            .collect();
        // Reserve the checkbox's actual width in addition to the source space.
        // Measuring it keeps that gap at one space even when the editor is zoomed.
        let checkbox_width = checkbox_layout(&self.view, false)
            .pixel_size().0
            .max(checkbox_layout(&self.view, true).pixel_size().0);
        self.tags[9].set_letter_spacing(checkbox_width * gtk::pango::SCALE);
        for &(position, _) in &tasks {
            apply(&self.tags[9], position + 5..position + 6);
        }
        *self.view.imp().tasks.borrow_mut() = tasks;
        // Hang wrapped lines of a list item under its text rather than its
        // marker. Measure the rendered prefix so concealment and zoom count.
        let mut hanging = std::collections::HashMap::new();
        for item in &document.items {
            let start = buffer.iter_at_offset(item.start);
            let width = self.view.iter_location(&buffer.iter_at_offset(item.end)).x()
                - self.view.iter_location(&start).x();
            if width <= 0 {
                continue;
            }
            let tag = hanging.entry(width).or_insert_with(|| {
                let tag = gtk::TextTag::builder()
                    .left_margin(self.view.left_margin() + width)
                    .indent(-width)
                    .build();
                buffer.tag_table().add(&tag);
                self.image_tags.borrow_mut().push(tag.clone());
                tag
            });
            let mut end = start;
            end.forward_to_line_end();
            apply(tag, item.start..end.offset());
        }
        self.view.queue_draw();
        self.busy.set(false);
    }

    fn task_at(&self, x: f64, y: f64) -> Option<(i32, bool)> {
        if !self.view.is_editable() {
            return None;
        }
        let (x, y) =
            self.view
                .window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
        self.view
            .imp()
            .tasks
            .borrow()
            .iter()
            .copied()
            .find(|(position, checked)| {
                let rect = marker_rect(&self.view, *position, 5);
                let layout = checkbox_layout(&self.view, *checked);
                let (width, height) = layout.pixel_size();
                let top = rect.y() + (rect.height() - height) / 2;
                x >= rect.x() && x < rect.x() + width && y >= top && y < top + height
            })
    }

    fn table_row_at(&self, x: f64, y: f64) -> Option<i32> {
        let (x, y) =
            self.view
                .window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
        self.view
            .imp()
            .tables
            .borrow()
            .iter()
            .find(|row| {
                let rect = self
                    .view
                    .iter_location(&self.view.buffer().iter_at_offset(row.offset));
                x >= self.view.left_margin()
                    && x as f32 <= self.view.left_margin() as f32 + row.width
                    && y as f32 >= rect.y() as f32 - row.height
                    && y <= rect.y()
            })
            .map(|row| row.end)
    }

    fn toggle_task_at(&self, x: f64, y: f64) -> bool {
        let task = self.task_at(x, y);
        let Some((position, checked)) = task else {
            return false;
        };
        let buffer = self.view.buffer();
        buffer.begin_user_action();
        let mut start = buffer.iter_at_offset(position + 3);
        let mut end = buffer.iter_at_offset(position + 4);
        buffer.delete(&mut start, &mut end);
        buffer.insert(&mut start, if checked { " " } else { "X" });
        buffer.end_user_action();
        true
    }

    fn link_at(&self, x: f64, y: f64, control: bool) -> Option<String> {
        let (x, y) =
            self.view
                .window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
        let (iter, _) = self.view.iter_at_position(x, y)?;
        let offset = iter.offset();
        if !control
            && self
                .editing_range()
                .is_some_and(|range| range.contains(&offset))
        {
            return None;
        }
        self.document
            .borrow()
            .links
            .iter()
            .find(|link| link.label.contains(&offset) || (control && link.source.contains(&offset)))
            .map(|link| link.uri.clone())
    }

    fn image_line_end_at(&self, x: f64, y: f64) -> Option<i32> {
        let (_, y) =
            self.view
                .window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
        let images = self.view.imp().images.borrow();
        let (offset, _, _, _) = images.iter().find(|(offset, _, _, height)| {
            let rect = self
                .view
                .iter_location(&self.view.buffer().iter_at_offset(*offset));
            y as f32 >= rect.y() as f32 - height - 8.0 && y <= rect.y() + rect.height() + 8
        })?;
        let mut end = self.view.buffer().iter_at_offset(*offset);
        end.forward_to_line_end();
        Some(end.offset())
    }
}

pub fn install(view: &View, open_link: impl Fn(&str) + 'static) {
    let dark = adw::StyleManager::default().is_dark();
    let tags = [
        gtk::TextTag::builder().name("md-bold").weight(700).build(),
        gtk::TextTag::builder()
            .name("md-italic")
            .style(gtk::pango::Style::Italic)
            .build(),
        gtk::TextTag::builder()
            .name("md-link")
            .underline(gtk::pango::Underline::Single)
            .foreground(if dark { "#78aeed" } else { "#1c71d8" })
            .build(),
        gtk::TextTag::builder()
            .name("md-conceal")
            // Keep source characters in GTK's layout index space. Invisible
            // runs can make GTK 4.22 abort while mapping Pango hit-test offsets
            // back to the buffer ("byte index off the end of the line").
            // One Pango unit is subpixel; transparent glyphs still conceal
            // markers while native cursor/selection mapping remains intact.
            .size(1)
            .foreground_rgba(&gtk::gdk::RGBA::new(0.0, 0.0, 0.0, 0.0))
            .build(),
        gtk::TextTag::builder()
            .name("md-bullet-space")
            .build(),
        gtk::TextTag::builder()
            .name("md-heading-1")
            .weight(700)
            .scale(1.8)
            .pixels_above_lines(12)
            .pixels_below_lines(6)
            .build(),
        gtk::TextTag::builder()
            .name("md-heading-2")
            .weight(700)
            .scale(1.5)
            .pixels_above_lines(10)
            .pixels_below_lines(5)
            .build(),
        gtk::TextTag::builder()
            .name("md-heading-3")
            .weight(700)
            .scale(1.3)
            .pixels_above_lines(8)
            .pixels_below_lines(4)
            .build(),
        gtk::TextTag::builder()
            .name("md-heading-4")
            .weight(700)
            .scale(1.15)
            .pixels_above_lines(6)
            .pixels_below_lines(3)
            .build(),
        gtk::TextTag::builder()
            .name("md-checkbox-space")
            .build(),
        gtk::TextTag::builder()
            .name("md-strikethrough")
            .strikethrough(true)
            .build(),
    ];
    for tag in &tags {
        view.buffer().tag_table().add(tag);
    }
    adw::StyleManager::default().connect_dark_notify({
        let link_tag = tags[2].clone();
        move |manager| {
            link_tag.set_foreground(Some(if manager.is_dark() {
                "#78aeed"
            } else {
                "#1c71d8"
            }))
        }
    });
    let renderer = Rc::new(Renderer {
        view: view.clone(),
        document: RefCell::default(),
        tags,
        busy: Cell::new(false),
        image_tags: RefCell::default(),
        textures: RefCell::default(),
    });
    view.buffer().connect_changed({
        let renderer = renderer.clone();
        move |_| renderer.refresh(true)
    });
    view.buffer().connect_mark_set({
        let renderer = renderer.clone();
        move |_, _, mark| {
            if matches!(mark.name().as_deref(), Some("insert" | "selection_bound")) {
                renderer.refresh(false);
            }
        }
    });
    view.connect_has_focus_notify({
        let renderer = renderer.clone();
        move |_| renderer.refresh(false)
    });
    view.connect_editable_notify({
        let renderer = renderer.clone();
        move |_| renderer.refresh(false)
    });
    view.add_tick_callback({
        let renderer = renderer.clone();
        let dimensions = RefCell::new(None);
        move |view, _| {
            let current = (
                view.width(),
                view.screen_width(),
                view.imp().image_revision.get(),
                view.pango_context().font_description(),
            );
            let previous = dimensions.replace(Some(current.clone()));
            if previous.as_ref() != Some(&current) {
                if previous.as_ref().is_some_and(|previous| previous.2 != current.2) {
                    renderer.textures.borrow_mut().clear();
                }
                renderer.refresh(false);
            }
            glib::ControlFlow::Continue
        }
    });

    let gesture = gtk::GestureClick::new();
    gesture.set_button(1);
    gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
    let pressed = Rc::new(RefCell::new(None::<(String, f64, f64)>));
    gesture.connect_pressed({
        let renderer = renderer.clone();
        let pressed = pressed.clone();
        move |gesture, count, x, y| {
            pressed.borrow_mut().take();
            let modifiers = gesture.current_event_state();
            let control = modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK);
            if count == 1
                && !modifiers.intersects(
                    gtk::gdk::ModifierType::SHIFT_MASK
                        | gtk::gdk::ModifierType::CONTROL_MASK
                        | gtk::gdk::ModifierType::ALT_MASK
                        | gtk::gdk::ModifierType::SUPER_MASK,
                )
            {
                if renderer.toggle_task_at(x, y) {
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                    return;
                }
                if let Some(end) = renderer.table_row_at(x, y) {
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                    renderer.view.grab_focus();
                    renderer
                        .view
                        .buffer()
                        .place_cursor(&renderer.view.buffer().iter_at_offset(end));
                    return;
                }
                if let Some(end) = renderer.image_line_end_at(x, y) {
                    // Claim before revealing source: GTK's selection drag would
                    // otherwise use coordinates from the collapsed preview.
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                    renderer.view.grab_focus();
                    renderer
                        .view
                        .buffer()
                        .place_cursor(&renderer.view.buffer().iter_at_offset(end));
                    return;
                }
            }
            if count != 1
                || (!control
                    && modifiers.intersects(
                        gtk::gdk::ModifierType::SHIFT_MASK | gtk::gdk::ModifierType::ALT_MASK,
                    ))
            {
                return;
            }
            if let Some(uri) = renderer.link_at(x, y, control) {
                *pressed.borrow_mut() = Some((uri, x, y));
                gesture.set_state(gtk::EventSequenceState::Claimed);
            }
        }
    });
    let open_link: Rc<dyn Fn(&str)> = Rc::new(open_link);
    gesture.connect_released({
        let open_link = open_link.clone();
        move |_, _, x, y| {
            if let Some((uri, px, py)) = pressed.borrow_mut().take() {
                if (x - px).abs() <= 4.0 && (y - py).abs() <= 4.0 {
                    open_link(&uri);
                }
            }
        }
    });
    view.add_controller(gesture);
    let motion = gtk::EventControllerMotion::new();
    motion.connect_motion({
        let renderer = renderer.clone();
        move |motion, x, y| {
            let control = motion
                .current_event_state()
                .contains(gtk::gdk::ModifierType::CONTROL_MASK);
            let uri = renderer.link_at(x, y, control);
            let task = renderer.task_at(x, y);
            renderer
                .view
                .set_cursor_from_name(Some(if uri.is_some() || task.is_some() {
                    "pointer"
                } else {
                    "text"
                }));
            renderer.view.set_tooltip_text(
                task.map(|(_, checked)| {
                    if checked {
                        "Mark incomplete".to_string()
                    } else {
                        "Mark complete".to_string()
                    }
                })
                .or_else(|| {
                    uri.as_ref()
                        .map(|uri| format!("{uri}\nCtrl+click to open while editing"))
                })
                .as_deref(),
            );
        }
    });
    view.add_controller(motion);
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed({
        let renderer = renderer.clone();
        move |_, key, _, modifiers| {
            if matches!(key, gtk::gdk::Key::Tab | gtk::gdk::Key::ISO_Left_Tab)
                && !modifiers.intersects(
                    gtk::gdk::ModifierType::CONTROL_MASK
                        | gtk::gdk::ModifierType::ALT_MASK
                        | gtk::gdk::ModifierType::SUPER_MASK,
                )
                && renderer.view.is_editable()
                && crate::lists::tab(
                    &renderer.view.buffer(),
                    key == gtk::gdk::Key::ISO_Left_Tab
                        || modifiers.contains(gtk::gdk::ModifierType::SHIFT_MASK),
                )
            {
                return glib::Propagation::Stop;
            }
            if matches!(key, gtk::gdk::Key::Return | gtk::gdk::Key::KP_Enter)
                && !modifiers.intersects(
                    gtk::gdk::ModifierType::CONTROL_MASK
                        | gtk::gdk::ModifierType::SHIFT_MASK
                        | gtk::gdk::ModifierType::ALT_MASK
                        | gtk::gdk::ModifierType::SUPER_MASK,
                )
                && renderer.view.is_editable()
                && crate::lists::enter(&renderer.view.buffer())
            {
                return glib::Propagation::Stop;
            }
            if key == gtk::gdk::Key::Return
                && modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK)
            {
                let position = renderer
                    .view
                    .buffer()
                    .iter_at_mark(&renderer.view.buffer().get_insert())
                    .offset();
                if let Some(uri) = renderer
                    .document
                    .borrow()
                    .links
                    .iter()
                    .find(|link| link.source.contains(&position))
                    .map(|link| link.uri.clone())
                {
                    open_link(&uri);
                    return glib::Propagation::Stop;
                }
            }
            glib::Propagation::Proceed
        }
    });
    view.add_controller(keys);
    renderer.refresh(true);
}

pub fn toggle(buffer: &gtk::TextBuffer, marker: &str) {
    let (start, end) = buffer.selection_bounds().unwrap_or_else(|| {
        let caret = buffer.iter_at_mark(&buffer.get_insert());
        (caret, caret)
    });
    let selected = buffer.text(&start, &end, true).to_string();
    let width = marker.chars().count() as i32;
    let mut begin = start.offset();
    let mut finish = end.offset();
    let (replacement, selection_start, selection_length) = if selected.len() >= marker.len() * 2
        && selected.starts_with(marker)
        && selected.ends_with(marker)
    {
        let inner = &selected[marker.len()..selected.len() - marker.len()];
        (inner.to_string(), begin, inner.chars().count() as i32)
    } else if begin >= width
        && finish + width <= buffer.char_count()
        && buffer.text(&buffer.iter_at_offset(begin - width), &start, true) == marker
        && buffer.text(&end, &buffer.iter_at_offset(finish + width), true) == marker
    {
        begin -= width;
        finish += width;
        (selected.clone(), begin, selected.chars().count() as i32)
    } else {
        (
            format!("{marker}{selected}{marker}"),
            begin + width,
            selected.chars().count() as i32,
        )
    };
    buffer.begin_user_action();
    buffer.delete(
        &mut buffer.iter_at_offset(begin),
        &mut buffer.iter_at_offset(finish),
    );
    buffer.insert(&mut buffer.iter_at_offset(begin), &replacement);
    buffer.select_range(
        &buffer.iter_at_offset(selection_start),
        &buffer.iter_at_offset(selection_start + selection_length),
    );
    buffer.end_user_action();
}

pub(super) async fn table_smoke(editor: crate::Editor) {
    async fn frame() {
        glib::timeout_future(std::time::Duration::from_millis(250)).await;
    }
    const SOURCE: &str = "| Claim | Prediction | Observation |\n| :--- | :---: | ---: |\n| A cache reduces repeated network requests | Repeated requests for unchanged documents should finish sooner because the client can reuse previously fetched data | Measurements show shorter response times after the first request, while changed documents are fetched again to keep the cache current. |\n| Offline editing preserves local changes | Documents edited without a connection should remain available and upload when the network returns | Local edits survive restarting the application and are synchronized after the connection is restored |\n| Conflict copies preserve competing edits | Concurrent changes on separate devices should retain both versions instead of silently discarding one | A conflict copy contains the remote version and the original document retains the local version |\n\nAfter the table";
    let path = editor.root.join("table.md");
    std::fs::write(&path, SOURCE).unwrap();
    if let Some(source) = std::env::var_os("HEMATITE_HOVER_FIXTURE") {
        std::fs::copy(source, &path).unwrap();
    }
    editor.open(&path);
    editor.buffer.place_cursor(&editor.buffer.end_iter());
    frame().await;
    let view = editor.view.clone().downcast::<View>().unwrap();
    assert_eq!(view.imp().tables.borrow().len(), 4);
    assert!(view.imp().tables.borrow()[0].header);
    assert!(view.imp().tables.borrow()[1].height > 60.0);
    assert_eq!(editor.text(), SOURCE);
    assert!(!editor.buffer.is_modified());
    if let Some(path) = std::env::var_os("HEMATITE_TABLE_SCREENSHOT") {
        use gtk::gdk::prelude::PaintableExt;
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
            .save_to_png(std::path::PathBuf::from(path))
            .unwrap();
    }
    let (offset, end, height) = {
        let rows = view.imp().tables.borrow();
        (rows[1].offset, rows[1].end, rows[1].height)
    };
    let rect = view.iter_location(&editor.buffer.iter_at_offset(offset));
    let (x, y) = view.buffer_to_window_coords(
        gtk::TextWindowType::Widget,
        view.left_margin() + 20,
        rect.y() - height as i32 + 12,
    );
    let controllers = view.observe_controllers();
    let gesture = (0..controllers.n_items())
        .filter_map(|i| controllers.item(i)?.downcast::<gtk::GestureClick>().ok())
        .find(|gesture| {
            gesture.button() == 1 && gesture.propagation_phase() == gtk::PropagationPhase::Capture
        })
        .unwrap();
    gesture.emit_by_name::<()>("pressed", &[&1i32, &(x as f64), &(y as f64)]);
    frame().await;
    assert_eq!(editor.buffer.cursor_position(), end);
    assert_eq!(view.imp().tables.borrow().len(), 3);
    assert!(
        editor
            .buffer
            .text(
                &editor.buffer.iter_at_offset(offset),
                &editor.buffer.iter_at_offset(end),
                false
            )
            .contains("| A cache reduces")
    );
    editor.buffer.insert_at_cursor(" edited");
    editor.buffer.undo();
    assert_eq!(editor.text(), SOURCE);
    assert!(editor.save());
    assert_eq!(std::fs::read_to_string(path).unwrap(), SOURCE);
    println!(
        "Tables verified: wrapped preview, header styling, row click editing, undo, and unchanged saved Markdown."
    );
    editor.window.close();
}

pub async fn hover_smoke(editor: crate::Editor) {
    let path = editor.root.join("hover.md");
    std::fs::write(
        &path,
        format!(
            "Intro\n\n**1. what is time? implications:**\n\n- First bullet\n- Second bullet\n- Long {}\n\nEnd",
            "word ".repeat(200)
        ),
    )
    .unwrap();
    if let Some(source) = std::env::var_os("HEMATITE_HOVER_FIXTURE") {
        std::fs::copy(source, &path).unwrap();
    }
    editor.open(&path);
    editor.buffer.place_cursor(&editor.buffer.end_iter());
    glib::timeout_future(std::time::Duration::from_millis(500)).await;
    let view = editor.view.clone().downcast::<View>().unwrap();
    for &offset in view.imp().bullets.borrow().iter() {
        let overlay = marker_rect(&view, offset, 1);
        let text = view.iter_location(&editor.buffer.iter_at_offset(offset + 2));
        assert_eq!(overlay.y(), text.y(), "Bullet must align with its text");
        assert_eq!(overlay.height(), text.height());
    }
    let long = editor.text().find("- Long").unwrap() as i32;
    let content = view.iter_location(&editor.buffer.iter_at_offset(long + 2));
    let mut wrapped = editor.buffer.iter_at_offset(long);
    assert!(view.forward_display_line(&mut wrapped), "Long bullet must wrap");
    assert_eq!(
        view.iter_location(&wrapped).x(),
        content.x(),
        "Wrapped bullet text must align with its first line"
    );
    let controllers = view.observe_controllers();
    let motion = (0..controllers.n_items())
        .filter_map(|i| {
            controllers
                .item(i)?
                .downcast::<gtk::EventControllerMotion>()
                .ok()
        })
        .next()
        .unwrap();
    for line in 0..editor.buffer.line_count() {
        let rect = view.iter_location(&editor.buffer.iter_at_line(line).unwrap());
        for dx in (0..view.width()).step_by(4) {
            let (x, y) = view.buffer_to_window_coords(
                gtk::TextWindowType::Widget,
                dx,
                rect.y() + rect.height() / 2,
            );
            motion.emit_by_name::<()>("motion", &[&(x as f64), &(y as f64)]);
        }
    }
    editor
        .buffer
        .place_cursor(&editor.buffer.iter_at_line(2).unwrap());
    glib::timeout_future(std::time::Duration::from_millis(200)).await;
    println!("Formatted line hover and cursor placement verified.");
    editor.window.close();
}
