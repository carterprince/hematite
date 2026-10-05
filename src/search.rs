use std::{
    cmp::Reverse,
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

pub struct Note {
    pub path: PathBuf,
    pub title: String,
    pub relative: String,
    modified: Option<SystemTime>,
    text: String,
    title_lower: String,
    path_lower: String,
    text_lower: String,
}

impl Note {
    fn new(path: PathBuf, root: &Path, text: String) -> Self {
        let title = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        Self {
            modified: path.metadata().and_then(|m| m.modified()).ok(),
            title_lower: title.to_lowercase(),
            path_lower: relative.to_lowercase(),
            text_lower: text.to_lowercase(),
            path,
            title,
            relative,
            text,
        }
    }
}

pub struct Hit {
    pub path: PathBuf,
    pub title: String,
    pub relative: String,
    pub preview: Option<String>,
    score: usize,
    modified: Option<SystemTime>,
}

pub fn index(entries: &[crate::vault::Entry], root: &Path) -> (Vec<Note>, usize) {
    let mut unreadable = 0;
    let notes = entries
        .iter()
        .filter(|entry| !entry.directory)
        .map(|entry| {
            let text = fs::read_to_string(&entry.path).unwrap_or_else(|_| {
                unreadable += 1;
                String::new()
            });
            Note::new(entry.path.clone(), root, text)
        })
        .collect();
    (notes, unreadable)
}

fn preview(text: &str, tokens: &[&str]) -> Option<String> {
    let line = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .max_by_key(|line| {
            let lower = line.to_lowercase();
            tokens
                .iter()
                .filter(|token| lower.contains(**token))
                .count()
        })?;
    let lower = line.to_lowercase();
    let first_byte = tokens.iter().filter_map(|token| lower.find(token)).min()?;
    // Work in characters so Unicode snippets never split a UTF-8 code point.
    let chars: Vec<char> = line.chars().collect();
    let first = chars
        .iter()
        .scan(0usize, |offset, character| {
            *offset += character.to_lowercase().map(char::len_utf8).sum::<usize>();
            Some(*offset)
        })
        .position(|end| end > first_byte)
        .unwrap_or(0);
    let start = first.saturating_sub(35);
    let end = (start + 140).min(chars.len());
    Some(format!(
        "{}{}{}",
        if start > 0 { "…" } else { "" },
        chars[start..end].iter().collect::<String>().trim(),
        if end < chars.len() { "…" } else { "" }
    ))
}

pub fn find(notes: &[Note], query: &str, current: Option<(&Path, &str)>) -> Vec<Hit> {
    let query = query
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    if query.is_empty() {
        return Vec::new();
    }
    let tokens: Vec<_> = query.split_whitespace().collect();
    let mut hits = Vec::new();
    for note in notes {
        let current_text = current
            .filter(|(path, _)| *path == note.path)
            .map(|(_, text)| text);
        let current_lower = current_text.map(str::to_lowercase);
        let body = current_lower.as_deref().unwrap_or(&note.text_lower);
        if !tokens.iter().all(|token| {
            note.title_lower.contains(token)
                || note.path_lower.contains(token)
                || body.contains(token)
        }) {
            continue;
        }
        let mut score = if note.title_lower == query {
            1000
        } else if note.title_lower.starts_with(&query) {
            600
        } else if note.title_lower.contains(&query) {
            400
        } else {
            0
        };
        for token in &tokens {
            score += if note.title_lower.contains(token) {
                80
            } else if note.path_lower.contains(token) {
                20
            } else {
                8
            };
        }
        if body.contains(&query) {
            score += 5;
        }
        hits.push(Hit {
            path: note.path.clone(),
            title: note.title.clone(),
            relative: note.relative.clone(),
            preview: preview(current_text.unwrap_or(&note.text), &tokens),
            score,
            modified: note.modified,
        });
    }
    hits.sort_by_cached_key(|hit| {
        (
            Reverse(hit.score),
            Reverse(hit.modified),
            hit.relative.to_lowercase(),
        )
    });
    hits
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn uses_modification_date_to_break_relevance_ties() {
        use std::time::{Duration, UNIX_EPOCH};
        let root = Path::new("/scratch");
        let mut notes = vec![
            Note::new(root.join("a.md"), root, "same content".into()),
            Note::new(root.join("b.md"), root, "same content".into()),
        ];
        notes[0].modified = Some(UNIX_EPOCH + Duration::from_secs(10));
        notes[1].modified = Some(UNIX_EPOCH + Duration::from_secs(20));
        assert_eq!(
            find(&notes, "content", None)
                .iter()
                .map(|hit| hit.title.as_str())
                .collect::<Vec<_>>(),
            ["b", "a"]
        );
    }
    #[test]
    fn balances_title_path_and_body_and_includes_unsaved_text() {
        let root = Path::new("/scratch");
        let notes = vec![
            Note::new(
                root.join("other.md"),
                root,
                "Planning a launch tomorrow".into(),
            ),
            Note::new(root.join("launch.md"), root, "Schedule".into()),
            Note::new(root.join("launch/tasks.md"), root, "Schedule".into()),
        ];
        let hits = find(&notes, "LAUNCH", None);
        assert_eq!(
            hits.iter()
                .map(|hit| hit.title.as_str())
                .collect::<Vec<_>>(),
            ["launch", "tasks", "other"]
        );
        assert_eq!(
            hits[2].preview.as_deref(),
            Some("Planning a launch tomorrow")
        );
        assert_eq!(find(&notes, "launch tomorrow", None).len(), 1);
        assert!(find(&notes, "launch missing", None).is_empty());
        assert!(find(&notes, "   ", None).is_empty());
        assert_eq!(
            find(
                &notes,
                "UNSAVED café",
                Some((&notes[0].path, "Unsaved CAFÉ notes"))
            )[0]
            .title,
            "other"
        );
    }
    #[test]
    fn content_previews_include_distant_unicode_matches() {
        let text = format!("{} café target {}", "é".repeat(180), "字".repeat(180));
        let excerpt = preview(&text, &["café", "target"]).unwrap();
        assert!(excerpt.contains("café target"));
        assert!(excerpt.starts_with('…') && excerpt.ends_with('…'));
        assert!(excerpt.chars().count() <= 142);
    }
}
