use pulldown_cmark::{Alignment, Event, Options, Parser, Tag, TagEnd};
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Style {
    Bold,
    Italic,
    Strikethrough,
    Heading(u8),
}

#[derive(Debug)]
pub struct Styled {
    pub range: Range<i32>,
    pub style: Style,
}

#[derive(Debug)]
pub struct Link {
    pub label: Range<i32>,
    pub source: Range<i32>,
    pub uri: String,
}

#[derive(Default, Debug)]
pub struct Table {
    pub range: Range<i32>,
    pub alignments: Vec<Alignment>,
    pub rows: Vec<TableRow>,
}

#[derive(Debug)]
pub struct TableRow {
    pub range: Range<i32>,
    pub cells: Vec<String>,
    pub header: bool,
}

#[derive(Default, Debug)]
pub struct Document {
    pub styles: Vec<Styled>,
    pub markers: Vec<Range<i32>>,
    pub bullets: Vec<i32>,
    pub tasks: Vec<(i32, bool)>,
    pub links: Vec<Link>,
    pub images: Vec<(Range<i32>, String)>,
    pub tables: Vec<Table>,
}

pub fn supported_uri(uri: &str) -> bool {
    uri.split_once(':').is_some_and(|(scheme, rest)| {
        !rest.is_empty()
            && matches!(
                scheme.to_ascii_lowercase().as_str(),
                "http" | "https" | "mailto"
            )
    })
}

pub fn parse(text: &str) -> Document {
    let boundaries: Vec<_> = text
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(text.len()))
        .collect();
    let offset = |byte: usize| {
        boundaries
            .binary_search(&byte)
            .expect("Parser offsets must be UTF-8 boundaries") as i32
    };
    let chars = |range: Range<usize>| offset(range.start)..offset(range.end);
    let mut document = Document::default();
    let mut protected = Vec::<Range<usize>>::new();
    let mut link: Option<(Range<usize>, Option<Range<usize>>, String)> = None;
    let mut table: Option<Table> = None;
    let mut cell = false;
    for (event, range) in Parser::new_ext(text, Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH).into_offset_iter() {
        match &event {
            Event::Start(Tag::Table(alignments)) => {
                table = Some(Table {
                    range: chars(range.clone()),
                    alignments: alignments.clone(),
                    rows: Vec::new(),
                });
                protected.push(range.clone());
            }
            Event::Start(Tag::TableHead | Tag::TableRow) => {
                if let Some(table) = &mut table {
                    let source = text[range.clone()].trim_end_matches(['\r', '\n']);
                    table.rows.push(TableRow {
                        range: chars(range.start..range.start + source.len()),
                        cells: Vec::new(),
                        header: matches!(event, Event::Start(Tag::TableHead)),
                    });
                }
            }
            Event::Start(Tag::TableCell) => {
                cell = true;
                if let Some(row) = table.as_mut().and_then(|table| table.rows.last_mut()) {
                    row.cells.push(String::new());
                }
            }
            Event::End(TagEnd::TableCell) => cell = false,
            Event::End(TagEnd::Table) => {
                if let Some(table) = table.take() {
                    document.tables.push(table);
                }
            }
            _ if cell => {
                if let Some(value) = table
                    .as_mut()
                    .and_then(|table| table.rows.last_mut())
                    .and_then(|row| row.cells.last_mut())
                {
                    match &event {
                        Event::Text(text) | Event::Code(text) => {
                            value.push_str(&gtk::glib::markup_escape_text(text))
                        }
                        Event::Start(Tag::Strong) => value.push_str("<b>"),
                        Event::End(TagEnd::Strong) => value.push_str("</b>"),
                        Event::Start(Tag::Emphasis) => value.push_str("<i>"),
                        Event::End(TagEnd::Emphasis) => value.push_str("</i>"),
                        Event::Start(Tag::Strikethrough) => value.push_str("<s>"),
                        Event::End(TagEnd::Strikethrough) => value.push_str("</s>"),
                        Event::SoftBreak | Event::HardBreak => value.push('\n'),
                        _ => (),
                    }
                }
            }
            _ => (),
        }
        if matches!(
            &event,
            Event::Text(_)
                | Event::Code(_)
                | Event::Start(Tag::Strong | Tag::Emphasis | Tag::Strikethrough | Tag::Image { .. })
        ) {
            if let Some((_, label, _)) = &mut link {
                if let Some(label) = label {
                    label.start = label.start.min(range.start);
                    label.end = label.end.max(range.end);
                } else {
                    *label = Some(range.clone());
                }
            }
        }
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                let level = level as u8;
                let source = text[range.clone()].trim_end_matches(['\r', '\n']);
                let line = source.trim_start_matches(' ');
                let width = line.bytes().take_while(|byte| *byte == b'#').count();
                if level <= 4 && width == level as usize {
                    let start = range.start + source.len() - line.len();
                    let prefix = width + line[width..].len()
                        - line[width..].trim_start_matches([' ', '\t']).len();
                    document.markers.push(chars(start..start + prefix));
                    document.styles.push(Styled {
                        range: chars(range.start..range.start + source.len()),
                        style: Style::Heading(level),
                    });
                }
            }
            Event::Start(Tag::Image { dest_url, .. }) => {
                document
                    .images
                    .push((chars(range.clone()), dest_url.into_string()));
                protected.push(range);
            }
            Event::Start(Tag::Strong | Tag::Emphasis | Tag::Strikethrough) => {
                let strong = matches!(event, Event::Start(Tag::Strong));
                let strike = matches!(event, Event::Start(Tag::Strikethrough));
                let width = if strong || strike { 2 } else { 1 };
                let source = &text[range.clone()];
                if source.len() >= width * 2 && (source.starts_with('*') || source.starts_with('_') || source.starts_with("~~"))
                {
                    document
                        .markers
                        .push(chars(range.start..range.start + width));
                    document.markers.push(chars(range.end - width..range.end));
                    document.styles.push(Styled {
                        range: chars(range.start + width..range.end - width),
                        style: if strike { Style::Strikethrough } else if strong { Style::Bold } else { Style::Italic },
                    });
                }
            }
            Event::Start(Tag::Item) => {
                let line = text[range.clone()].split('\n').next().unwrap_or_default();
                let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
                let marker = range.start + indent;
                let rest = &line[indent..];
                if ["- ", "-\t", "* ", "*\t"]
                    .iter()
                    .any(|marker| rest.starts_with(marker))
                {
                    let position = offset(marker);
                    let task = rest.as_bytes();
                    if task.len() >= 6
                        && task[2] == b'['
                        && task[4] == b']'
                        && matches!(task[3], b' ' | b'x' | b'X')
                        && matches!(task[5], b' ' | b'\t')
                    {
                        document.markers.push(position..position + 5);
                        document.tasks.push((position, task[3] != b' '));
                    } else {
                        document.markers.push(position..position + 1);
                        document.bullets.push(position);
                    }
                }
            }
            Event::Start(Tag::Link { dest_url, .. }) => {
                protected.push(range.clone());
                link = Some((range, None, dest_url.into_string()));
            }
            Event::End(TagEnd::Link) => {
                if let Some((source, Some(label), uri)) = link.take() {
                    if supported_uri(&uri) {
                        if source.start < label.start {
                            document.markers.push(chars(source.start..label.start));
                        }
                        if label.end < source.end {
                            document.markers.push(chars(label.end..source.end));
                        }
                        document.links.push(Link {
                            label: chars(label),
                            source: chars(source),
                            uri,
                        });
                    }
                }
            }
            Event::Code(_)
            | Event::Start(Tag::CodeBlock(_))
            | Event::Html(_)
            | Event::InlineHtml(_) => protected.push(range),
            _ => {}
        }
    }

    // CommonMark parses explicit links; also recognize familiar bare web URLs.
    let mut consumed = 0;
    for (start, ch) in text.char_indices() {
        if start < consumed || !matches!(ch, 'h' | 'H') {
            continue;
        }
        let rest = &text[start..];
        if !(rest
            .get(..7)
            .is_some_and(|s| s.eq_ignore_ascii_case("http://"))
            || rest
                .get(..8)
                .is_some_and(|s| s.eq_ignore_ascii_case("https://")))
        {
            continue;
        }
        if text[..start]
            .chars()
            .next_back()
            .is_some_and(|ch| ch.is_alphanumeric() || ch == '_')
        {
            continue;
        }
        let length = rest
            .find(|ch: char| ch.is_whitespace() || matches!(ch, '<' | '>' | '"' | '`'))
            .unwrap_or(rest.len());
        let mut uri = rest[..length].trim_end_matches(['.', ',', ';', ':', '!', '?']);
        while uri.ends_with(')') && uri.matches(')').count() > uri.matches('(').count() {
            uri = &uri[..uri.len() - 1];
        }
        for (open, close) in [('[', ']'), ('{', '}')] {
            while uri.ends_with(close) && uri.matches(close).count() > uri.matches(open).count() {
                uri = &uri[..uri.len() - 1];
            }
        }
        let range = start..start + uri.len();
        consumed = range.end;
        if protected
            .iter()
            .any(|span| span.start < range.end && span.end > range.start)
        {
            continue;
        }
        document.links.push(Link {
            label: chars(range.clone()),
            source: chars(range),
            uri: uri.into(),
        });
    }
    document
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rendered(text: &str, doc: &Document) -> String {
        text.chars()
            .enumerate()
            .filter(|(i, _)| !doc.markers.iter().any(|range| range.contains(&(*i as i32))))
            .map(|(_, ch)| ch)
            .collect()
    }
    #[test]
    fn parses_table_cells_alignment_and_source_rows() {
        let text = "| Claim | Prediction | Observation |\n| :--- | :---: | ---: |\n| Café **bold** | Longer text | A \\| B |\n";
        let doc = parse(text);
        assert_eq!(doc.tables.len(), 1);
        let table = &doc.tables[0];
        assert_eq!(table.rows.len(), 2);
        assert!(table.rows[0].header);
        assert_eq!(
            table.rows[1].cells,
            ["Café <b>bold</b>", "Longer text", "A | B"]
        );
        assert_eq!(
            table.alignments,
            [Alignment::Left, Alignment::Center, Alignment::Right]
        );
        let range = &table.rows[0].range;
        assert_eq!(
            text.chars()
                .skip(range.start as usize)
                .take((range.end - range.start) as usize)
                .collect::<String>(),
            "| Claim | Prediction | Observation |"
        );
    }
    #[test]
    fn recognizes_checked_and_unchecked_tasks_without_extra_bullets() {
        let text = "- [ ] item\n- [X] done\n* [x] lowercase\n- normal\n\n```\n- [ ] code\n```\n";
        let document = parse(text);
        assert_eq!(
            document
                .tasks
                .iter()
                .map(|(_, checked)| *checked)
                .collect::<Vec<_>>(),
            vec![false, true, true]
        );
        assert_eq!(document.bullets.len(), 1);
        assert!(rendered(text, &document).starts_with(" item\n done\n lowercase\n normal"));
        assert!(rendered(text, &document).contains("- [ ] code"));
    }
    #[test]
    fn headings_and_star_bullets_preserve_source_offsets() {
        let text = "# Café\n## Second\n### Third\n#### Fourth\n\n* Star item\n- Dash item\n\n```\n# Code\n* Code\n```\n\n#nospace\n";
        let doc = parse(text);
        let headings: Vec<_> = doc
            .styles
            .iter()
            .filter_map(|span| {
                if let Style::Heading(level) = span.style {
                    Some(level)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(headings, vec![1, 2, 3, 4]);
        assert_eq!(doc.bullets.len(), 2);
        let visible = rendered(text, &doc);
        assert!(visible.starts_with("Café\nSecond\nThird\nFourth\n"));
        assert!(visible.contains("```\n# Code\n* Code\n```"));
        assert!(visible.ends_with("#nospace\n"));
    }
    #[test]
    fn parses_styles_bullets_and_links_with_unicode_offsets() {
        let text = "é **bold** and *italic*\n- item\n  - nested\n[GTK](https://gtk.org) and https://example.org.\n";
        let doc = parse(text);
        assert_eq!(doc.styles.len(), 2);
        assert_eq!(doc.styles[0].style, Style::Bold);
        assert_eq!(doc.bullets.len(), 2);
        assert_eq!(doc.links.len(), 2);
        assert_eq!(doc.links[0].uri, "https://gtk.org");
        assert_eq!(doc.links[1].uri, "https://example.org");
        assert_eq!(
            rendered(text, &doc),
            "é bold and italic\n item\n   nested\nGTK and https://example.org.\n"
        );
    }
    #[test]
    fn leaves_code_escapes_and_unfinished_markup_alone() {
        let text = "`**code** https://code.example`\n\n```md\n- **code**\nhttps://code.example\n```\n\n\\*literal\\* and **unfinished";
        let doc = parse(text);
        assert!(doc.styles.is_empty());
        assert!(doc.bullets.is_empty());
        assert!(doc.links.is_empty());
        assert_eq!(rendered(text, &doc), text);
    }
    #[test]
    fn supports_nested_emphasis_and_keeps_unsafe_links_literal() {
        let text = "***both*** and **bold *nested*** [bad](javascript:alert) [mail](mailto:test@example.org)";
        let doc = parse(text);
        assert_eq!(
            rendered(text, &doc),
            "both and bold nested [bad](javascript:alert) mail"
        );
        assert_eq!(doc.styles.len(), 4);
        assert_eq!(doc.links.len(), 1);
    }
    #[test]
    fn preserves_url_parentheses_and_ipv6_addresses() {
        let doc = parse("See (https://example.org/a_(b)). http://[::1]/test <https://example.org>");
        let uris: Vec<_> = doc.links.iter().map(|link| link.uri.as_str()).collect();
        assert!(uris.contains(&"https://example.org/a_(b)"));
        assert!(uris.contains(&"http://[::1]/test"));
        assert!(uris.contains(&"https://example.org"));
        assert_eq!(uris.len(), 3);
    }
}
