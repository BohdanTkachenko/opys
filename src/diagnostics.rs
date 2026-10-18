//! Source locations for verification, kept separate from semantic messages so
//! moving a line never makes the edit gate mistake an old problem for a new one.

use std::{collections::HashMap, ops::Range, path::PathBuf};

use serde::{
    de::{self, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor},
    Serialize,
};

use crate::{backend::Backend, doc::Doc, frontmatter, project::Project};

/// One-based lines and Unicode scalar columns. Ranges are end-exclusive;
/// equal endpoints identify an insertion point for missing content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Position {
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceRange {
    pub start: Position,
    pub end: Position,
}

#[derive(Debug, Clone, Serialize)]
pub struct Excerpt {
    pub first_line: usize,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    /// The original, stable message, including the document ID when present.
    pub message: String,
    pub doc_id: Option<String>,
    pub path: Option<PathBuf>,
    pub range: Option<SourceRange>,
    pub excerpt: Option<Excerpt>,
    /// Body-relative coordinates for highlighting the rendered markdown.
    pub body_range: Option<SourceRange>,
    pub body_version: Option<String>,
    #[serde(skip)]
    pub(crate) target: Target,
}

impl Finding {
    pub fn display(&self) -> String {
        match (&self.path, &self.range) {
            (Some(path), Some(range)) => format!(
                "{}:{}:{}: {}",
                path.display(),
                range.start.line,
                range.start.column,
                self.message
            ),
            _ => self.message.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum YamlPart {
    Key(String),
    Index(usize),
}

#[derive(Debug, Clone, Default)]
pub(crate) enum Target {
    #[default]
    File,
    Yaml(Vec<YamlPart>),
    Body(Range<usize>),
    BodyEnd,
    Title,
    Parse,
}

#[derive(Default)]
pub(crate) struct Problems {
    pub items: Vec<Finding>,
    id: Option<String>,
    path: Option<PathBuf>,
    pub target: Target,
}

impl Problems {
    pub fn unlocated(&mut self) {
        self.id = None;
        self.path = None;
        self.target = Target::File;
    }
    pub fn document(&mut self, doc: &Doc) {
        self.id = doc.id().map(str::to_owned);
        self.path = Some(doc.path.clone());
        self.field("id");
    }

    pub fn file(&mut self, path: PathBuf) {
        self.id = None;
        self.path = Some(path);
        self.target = Target::File;
    }

    pub fn field(&mut self, field: &str) {
        self.target = Target::Yaml(vec![YamlPart::Key(field.to_owned())]);
    }

    pub fn entry(&mut self, field: &str, key: &str) {
        self.target = Target::Yaml(vec![
            YamlPart::Key(field.to_owned()),
            YamlPart::Key(key.to_owned()),
        ]);
    }

    pub fn element(&mut self, field: &str, index: usize) {
        self.target = Target::Yaml(vec![
            YamlPart::Key(field.to_owned()),
            YamlPart::Index(index),
        ]);
    }

    pub fn push(&mut self, message: String) {
        self.items.push(Finding {
            message,
            doc_id: self.id.clone(),
            path: self.path.clone(),
            range: None,
            excerpt: None,
            body_range: None,
            body_version: None,
            target: self.target.clone(),
        });
    }
}

/// Non-security snapshot identity, shared by doc views and findings. Never
/// used as an integrity or authentication check.
pub fn body_version(body: &str) -> String {
    let hash = body.bytes().fold(0xcbf29ce484222325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
    });
    format!("{hash:016x}")
}

pub(crate) fn section_target(doc: &Doc, heading: &str) -> Target {
    let pattern = format!(r"(?m)^## {}\s*$", regex::escape(heading));
    regex::Regex::new(&pattern)
        .ok()
        .and_then(|re| re.find(&doc.body))
        .map_or(Target::BodyEnd, |m| {
            Target::Body(m.start()..m.start() + m.as_str().trim_end().len())
        })
}

/// Attach locations only to the stored snapshot that produced the finding.
/// No canonical serialization is used: hand-written YAML retains its lines.
pub fn resolve(prj: &Project, docs: &[Doc], findings: &mut [Finding], backend: &dyn Backend) {
    let mut sources: HashMap<PathBuf, Option<Source>> = HashMap::new();
    for finding in findings {
        let Some(path) = finding.path.clone() else {
            continue;
        };
        let source = sources
            .entry(path.clone())
            .or_insert_with(|| backend.stored_text(&path).map(Source::new));
        finding.path = Some(path.strip_prefix(&prj.root).unwrap_or(&path).to_path_buf());
        let Some(source) = source else { continue };
        let doc = docs.iter().find(|doc| doc.path == path);
        if let Some(doc) = doc {
            let Ok((fm, body)) = frontmatter::parse(&source.text, "source") else {
                continue;
            };
            if body != doc.body || fm.map != doc.frontmatter.map {
                continue;
            }
        }
        let body_offset = doc.map(|doc| source.text.len() - doc.body.len());
        let bytes = match &finding.target {
            Target::File => None,
            Target::Parse => parse_range(&source.text),
            Target::Yaml(parts) => yaml_range(&source.text, parts),
            Target::Body(range) => {
                body_offset.map(|offset| offset + range.start..offset + range.end)
            }
            Target::BodyEnd => Some(source.text.len()..source.text.len()),
            Target::Title => body_offset.map(|offset| {
                let body = &source.text[offset..];
                let start = offset + body.len() - body.trim_start().len();
                start..start
            }),
        };
        let Some(bytes) = bytes.filter(|r| source.valid(r)) else {
            continue;
        };
        let range = source.range(bytes.clone());
        finding.excerpt = Some(source.excerpt(&range));
        finding.range = Some(range);
        if let (Some(offset), Some(doc)) = (body_offset, doc) {
            if bytes.start >= offset {
                let body = Source::new(doc.body.clone());
                finding.body_range = Some(body.range(bytes.start - offset..bytes.end - offset));
                finding.body_version = Some(body_version(&doc.body));
            }
        }
    }
}

struct Source {
    text: String,
    lines: Vec<usize>,
}

impl Source {
    fn new(text: String) -> Self {
        let lines = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        Self { text, lines }
    }

    fn valid(&self, range: &Range<usize>) -> bool {
        range.start <= range.end
            && range.end <= self.text.len()
            && self.text.is_char_boundary(range.start)
            && self.text.is_char_boundary(range.end)
    }

    fn position(&self, offset: usize) -> Position {
        let line = self
            .lines
            .partition_point(|start| *start <= offset)
            .saturating_sub(1);
        Position {
            line: line + 1,
            column: self.text[self.lines[line]..offset].chars().count() + 1,
        }
    }

    fn range(&self, bytes: Range<usize>) -> SourceRange {
        SourceRange {
            start: self.position(bytes.start),
            end: self.position(bytes.end),
        }
    }

    fn excerpt(&self, range: &SourceRange) -> Excerpt {
        let first = range.start.line.saturating_sub(3);
        let last = (range.end.line + 2).min(self.lines.len());
        let start = self.lines[first];
        let end = self.lines.get(last).copied().unwrap_or(self.text.len());
        let mut text = self.text[start..end].to_owned();
        // A truncated excerpt must not invent an empty row for the following
        // (unshown) source line. At EOF the trailing empty line is real.
        if end < self.text.len() && text.ends_with('\n') {
            text.pop();
        }
        Excerpt {
            first_line: first + 1,
            text,
        }
    }
}

fn line_range(text: &str, line: usize, column: usize) -> Option<Range<usize>> {
    let source = Source::new(text.to_owned());
    let line_start = *source.lines.get(line.checked_sub(1)?)?;
    let content = text[line_start..].split('\n').next()?;
    let offset = content
        .char_indices()
        .nth(column.saturating_sub(1))
        .map_or(content.len(), |(i, _)| i);
    Some(line_start + offset..line_start + content.trim_end_matches('\r').len())
}

fn parse_range(text: &str) -> Option<Range<usize>> {
    let Some(yaml) = frontmatter::source(text) else {
        let offset = if text.starts_with("---\n") {
            text.len()
        } else {
            0
        };
        return Some(offset..offset);
    };
    let error = serde_norway::from_str::<serde_norway::Mapping>(yaml).err()?;
    let location = error.location()?;
    line_range(text, location.line() + 1, location.column())
}

fn yaml_range(text: &str, path: &[YamlPart]) -> Option<Range<usize>> {
    let yaml = frontmatter::source(text)?;
    let result = Probe(path).deserialize(serde_norway::Deserializer::from_str(yaml));
    if let Err(error) = &result {
        if error.to_string().contains(LOCATION_MARKER) {
            let location = error.location()?;
            return line_range(text, location.line() + 1, location.column());
        }
    }
    // An absent field has no offending token: the insertion point is before
    // the closing YAML fence. A parser limitation must not invent a location.
    if result.is_ok() {
        let offset = 4 + yaml.len() + 1;
        return Some(offset..offset);
    }
    None
}

const LOCATION_MARKER: &str = "opys source location";

/// Ask the existing YAML parser to mark a selected key or sequence item. A
/// visitor error carries the parser's actual position, including flow maps,
/// quoted keys, aliases and block scalars; no line regex guesses at YAML.
struct Probe<'a>(&'a [YamlPart]);

impl<'de> DeserializeSeed<'de> for Probe<'_> {
    type Value = ();
    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        if self.0.is_empty() {
            return deserializer.deserialize_any(Stop);
        }
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Probe<'_> {
    type Value = ();
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("YAML source")
    }
    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<(), M::Error> {
        let Some(YamlPart::Key(wanted)) = self.0.first() else {
            return Ok(());
        };
        while let Some(matches) = map.next_key_seed(Key {
            wanted,
            stop: self.0.len() == 1,
        })? {
            if matches {
                map.next_value_seed(Probe(&self.0[1..]))?;
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(())
    }
    fn visit_seq<S: SeqAccess<'de>>(self, mut seq: S) -> Result<(), S::Error> {
        let Some(YamlPart::Index(wanted)) = self.0.first() else {
            return Ok(());
        };
        let mut index = 0;
        loop {
            let found = if index == *wanted {
                seq.next_element_seed(Probe(&self.0[1..]))?.is_some()
            } else {
                seq.next_element::<IgnoredAny>()?.is_some()
            };
            if !found {
                break;
            }
            index += 1;
        }
        Ok(())
    }
    fn visit_unit<E: de::Error>(self) -> Result<(), E> {
        Ok(())
    }
    fn visit_str<E: de::Error>(self, _: &str) -> Result<(), E> {
        Ok(())
    }
    fn visit_bool<E: de::Error>(self, _: bool) -> Result<(), E> {
        Ok(())
    }
    fn visit_i64<E: de::Error>(self, _: i64) -> Result<(), E> {
        Ok(())
    }
    fn visit_u64<E: de::Error>(self, _: u64) -> Result<(), E> {
        Ok(())
    }
    fn visit_f64<E: de::Error>(self, _: f64) -> Result<(), E> {
        Ok(())
    }
}

struct Key<'a> {
    wanted: &'a str,
    stop: bool,
}
impl<'de> DeserializeSeed<'de> for Key<'_> {
    type Value = bool;
    fn deserialize<D: de::Deserializer<'de>>(self, de: D) -> Result<bool, D::Error> {
        de.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for Key<'_> {
    type Value = bool;
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("YAML key")
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<bool, E> {
        let matches = value == self.wanted;
        if matches && self.stop {
            Err(E::custom(LOCATION_MARKER))
        } else {
            Ok(matches)
        }
    }
    fn visit_unit<E: de::Error>(self) -> Result<bool, E> {
        Ok(false)
    }
    fn visit_bool<E: de::Error>(self, _: bool) -> Result<bool, E> {
        Ok(false)
    }
    fn visit_i64<E: de::Error>(self, _: i64) -> Result<bool, E> {
        Ok(false)
    }
    fn visit_u64<E: de::Error>(self, _: u64) -> Result<bool, E> {
        Ok(false)
    }
    fn visit_f64<E: de::Error>(self, _: f64) -> Result<bool, E> {
        Ok(false)
    }
    fn visit_seq<S: SeqAccess<'de>>(self, mut seq: S) -> Result<bool, S::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(false)
    }
    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<bool, M::Error> {
        while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
        Ok(false)
    }
}

struct Stop;
macro_rules! stop_visits {
    ($($name:ident($type:ty)),* $(,)?) => {$(
        fn $name<E: de::Error>(self, _: $type) -> Result<(), E> { Err(E::custom(LOCATION_MARKER)) }
    )*};
}
impl<'de> Visitor<'de> for Stop {
    type Value = ();
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("source location")
    }
    stop_visits!(
        visit_str(&str),
        visit_bool(bool),
        visit_i64(i64),
        visit_u64(u64),
        visit_f64(f64)
    );
    fn visit_unit<E: de::Error>(self) -> Result<(), E> {
        Err(E::custom(LOCATION_MARKER))
    }
    fn visit_seq<S: SeqAccess<'de>>(self, _: S) -> Result<(), S::Error> {
        Err(de::Error::custom(LOCATION_MARKER))
    }
    fn visit_map<M: MapAccess<'de>>(self, _: M) -> Result<(), M::Error> {
        Err(de::Error::custom(LOCATION_MARKER))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_locations_use_original_quoted_flow_and_multiline_source() {
        let text = "---\n{id: FEAT-0001, 'strange key': [one, BAD]}\n---\n\n# λ\n";
        let range = yaml_range(
            text,
            &[YamlPart::Key("strange key".into()), YamlPart::Index(1)],
        )
        .unwrap();
        assert!(text[range.clone()].starts_with("BAD"));
        assert_eq!(Source::new(text.into()).range(range).start.line, 2);
        let text = "---\nnotes: |\n  status: misleading\n\"status\": wrong\ntags:\n  - okay\n  - BAD\n---\n# Title\n";
        let range = yaml_range(text, &[YamlPart::Key("status".into())]).unwrap();
        assert_eq!(&text[range], "\"status\": wrong");
        let range = yaml_range(text, &[YamlPart::Key("tags".into()), YamlPart::Index(1)]).unwrap();
        assert_eq!(&text[range], "BAD");
        let range = yaml_range(text, &[YamlPart::Key("missing".into())]).unwrap();
        assert!(range.is_empty());
        assert!(text[range.start..].starts_with("---"));
    }

    #[test]
    fn columns_are_unicode_scalars_and_ranges_are_exclusive() {
        let text = "first\nλ 🦀 broken\n";
        let start = text.find("broken").unwrap();
        let source = Source::new(text.into());
        let range = source.range(start..start + 6);
        assert_eq!(range.start, Position { line: 2, column: 5 });
        assert_eq!(
            range.end,
            Position {
                line: 2,
                column: 11
            }
        );
    }

    #[test]
    fn excerpts_do_not_invent_a_blank_row_at_a_truncated_boundary() {
        let source = Source::new("a\nb\nbad\nd\ne\nf\n".into());
        let excerpt = source.excerpt(&source.range(4..7));
        assert_eq!(excerpt.text, "a\nb\nbad\nd\ne");
        let eof = source.text.len();
        let excerpt = source.excerpt(&source.range(eof..eof));
        assert!(
            excerpt.text.ends_with('\n'),
            "the real EOF insertion line stays visible"
        );
    }
}
