//! Hard word wrapping for comments and other text, at a given column.
//!
//! The whole engine is one function over strings: text in, an [`Edit`] out. It
//! holds no state, reads no files and knows no editor, which is what lets the
//! same code serve the extension (as wasm), the CLI and the tests.
//!
//! Wrapping is done by *structure*, not by syntax. A line is either something
//! that must be reproduced exactly -- code, a fenced block, a table, a doctest
//! -- or it is prose with a prefix. Prose is joined, re-flowed and re-prefixed;
//! everything else is copied through untouched. When the classifier does not
//! recognise a construct, it is copied through, so an unknown construct
//! degrades to "left alone" rather than to "mangled".

mod comment;
mod dialects;
mod edit;
mod language;
mod markdown;
mod width;
mod wrap;

pub use edit::{Edit, File, Position, Selection, Settings};
pub use language::{Comment, Dialect, Kind, Language, LANGUAGES};

/// One line of a block: what is emitted before the text, and the text.
#[derive(Debug, Clone)]
pub(crate) struct Line {
    pub prefix: String,
    pub text: String,
}

impl Line {
    fn new(prefix: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
            text: text.into(),
        }
    }

    fn full(&self) -> String {
        format!("{}{}", self.prefix, self.text)
    }

    fn is_blank(&self) -> bool {
        self.text.trim().is_empty() && self.prefix.trim().is_empty()
    }
}

/// A run of output. Either reproduced as it was, or reflowed.
#[derive(Debug)]
pub(crate) enum Chunk {
    Verbatim(Vec<String>),
    Para(Para),
    /// Text appended to the end of the previous output line: a closing marker
    /// that shared its line with the last line of prose.
    Suffix(String),
}

/// A paragraph: the text of one or more source lines, plus how to prefix new
/// lines. `first` is used for the first output line, `cont` for the rest.
#[derive(Debug)]
pub(crate) struct Para {
    pub first: String,
    pub cont: String,
    pub parts: Vec<Part>,
}

#[derive(Debug)]
pub(crate) struct Part {
    pub text: String,
    /// The source line ended with two spaces, which Markdown reads as "keep
    /// this line break". The break is preserved and so are the spaces.
    pub hard_break: bool,
}

impl Para {
    fn new(first: impl Into<String>, cont: impl Into<String>) -> Self {
        Self {
            first: first.into(),
            cont: cont.into(),
            parts: Vec::new(),
        }
    }

    fn push(&mut self, text: &str) {
        let hard_break = text.ends_with("  ");
        self.parts.push(Part {
            text: text.trim().to_string(),
            hard_break,
        });
    }

    /// Reflow into output lines.
    fn render(&self, settings: &Settings, column: usize) -> Vec<String> {
        let mut out = Vec::new();
        let mut segment: Vec<&str> = Vec::new();
        let mut first = self.first.clone();
        for part in &self.parts {
            if part.hard_break {
                segment.push(&part.text);
                let text = segment.join(" ");
                let mut lines = wrap::fill(settings, &text, &first, &self.cont, column);
                if let Some(last) = lines.last_mut() {
                    last.push_str("  ");
                }
                out.append(&mut lines);
                segment.clear();
                first = self.cont.clone();
            } else {
                segment.push(&part.text);
            }
        }
        if !segment.is_empty() || out.is_empty() {
            let text = segment.join(" ");
            out.extend(wrap::fill(settings, &text, &first, &self.cont, column));
        }
        out
    }
}

impl Settings {
    /// The column actually used: 0 or less means "never wrap".
    fn effective_column(&self) -> usize {
        usize::try_from(self.column).unwrap_or(0)
    }
}

/// Turn chunks into output lines.
fn render_chunks(chunks: &[Chunk], settings: &Settings, column: usize) -> Vec<String> {
    let mut out = Vec::new();
    for chunk in chunks {
        match chunk {
            Chunk::Verbatim(lines) => out.extend(lines.iter().cloned()),
            Chunk::Para(para) => out.extend(para.render(settings, column)),
            Chunk::Suffix(suffix) => {
                if let Some(last) = out.last_mut() {
                    last.push_str(suffix);
                }
            }
        }
    }
    out
}

/// The language for this file, or a plain-text fallback.
///
/// A fallback rather than an error: an unknown language is a file the user
/// opened, and refusing to touch it is worse than wrapping its paragraphs. The
/// fallback never claims a marker, so a code file of an unknown language is
/// reproduced unchanged rather than having its code reflowed.
fn resolve(file: &File) -> Option<&'static Language> {
    Language::for_file(&file.language, &file.path)
}

/// Wrap `lines` as the file's language, honouring `selections`.
///
/// Selections decide *what* is wrapped: an empty selection list means the whole
/// document, which is what a formatter wants; a selection means only those
/// lines, which is what "wrap this comment" wants.
#[must_use]
pub fn rewrap(file: &File, settings: &Settings, selections: &[Selection], lines: &[&str]) -> Edit {
    let column = settings.effective_column();
    let (start, end) = region(selections, lines.len());
    if start >= end {
        return Edit::empty();
    }
    // A cursor is a request to wrap the thing at the cursor, not the one line
    // under it. Reflowing a single line out of a three-line comment produces
    // the worst possible result -- one long line above two short ones -- so the
    // region grows to the enclosing comment or paragraph first.
    let (start, end) = if start + 1 == end {
        expand(file, lines, start)
    } else {
        (start, end)
    };
    let region_lines = &lines[start..end];
    let chunks = match resolve(file) {
        Some(language) => process_language(language, settings, region_lines),
        None => markdown::parse(settings, &plain(region_lines)),
    };
    let out = render_chunks(&chunks, settings, column);
    edit_for(&out, region_lines, start, end, settings, selections)
}

/// Lines that carry no prefix.
fn plain(lines: &[&str]) -> Vec<Line> {
    lines.iter().map(|line| Line::new("", *line)).collect()
}

/// Grow a single-line region to the block that line belongs to.
///
/// Three shapes, in the order they can be decided: a comment that begins its
/// line grows to its whole run (or to the block-comment markers around it); a
/// line of prose grows to its blank-line-delimited paragraph; anything else --
/// code -- stays exactly where it is, because wrapping code is not a thing this
/// tool does.
fn expand(file: &File, lines: &[&str], at: usize) -> (usize, usize) {
    let line = lines[at];
    if line.trim().is_empty() {
        return (at, at + 1);
    }
    if let Some(language) = resolve(file) {
        if !language.is_document() {
            if let Some((first, last)) = comment_block(language, lines, at) {
                return (first, last + 1);
            }
            // Not inside a comment: nothing to do to a line of code.
            return (at, at + 1);
        }
    }
    paragraph(lines, at)
}

/// The comment containing line `at`, if any.
fn comment_block(language: &'static Language, lines: &[&str], at: usize) -> Option<(usize, usize)> {
    let indent = crate::markdown::leading_whitespace(lines[at]);
    let rest = &lines[at][indent.len()..];
    let mut best: Option<&'static Comment> = None;
    for comment in language.comments {
        if comment.kind == Kind::String || !rest.starts_with(comment.start) {
            continue;
        }
        if best.is_none_or(|current| comment.start.len() > current.start.len()) {
            best = Some(comment);
        }
    }
    let comment = best?;
    let mut first = at;
    let mut last = at;
    match comment.kind {
        Kind::Line => {
            // A run of the same marker at the same indent is one comment.
            while first > 0 && begins_comment(lines[first - 1], &indent, comment.start) {
                first -= 1;
            }
            while last + 1 < lines.len() && begins_comment(lines[last + 1], &indent, comment.start)
            {
                last += 1;
            }
        }
        _ => {
            // A block comment: grow outwards to the lines holding its markers.
            while first > 0 && !lines[first].contains(comment.start) {
                first -= 1;
            }
            while last + 1 < lines.len() && !lines[last].contains(comment.end) {
                last += 1;
            }
        }
    }
    Some((first, last))
}

fn begins_comment(line: &str, indent: &str, marker: &str) -> bool {
    line.strip_prefix(indent)
        .is_some_and(|rest| rest.starts_with(marker))
}

/// The blank-line-delimited paragraph containing `at`.
fn paragraph(lines: &[&str], at: usize) -> (usize, usize) {
    let mut first = at;
    let mut last = at;
    while first > 0 && !lines[first - 1].trim().is_empty() {
        first -= 1;
    }
    while last + 1 < lines.len() && !lines[last + 1].trim().is_empty() {
        last += 1;
    }
    (first, last + 1)
}

/// The half-open line range an operation covers.
fn region(selections: &[Selection], count: usize) -> (usize, usize) {
    if count == 0 {
        return (0, 0);
    }
    let mut start = usize::MAX;
    let mut end = 0;
    for selection in selections {
        let (a, b) = (selection.anchor.line, selection.active.line);
        start = start.min(a.min(b));
        // A selection whose active end sits at column 0 of a line does not
        // include that line -- that is how editors report "selected to the end
        // of the previous line".
        let last = if b.max(a) > a.min(b) && selection.active.character == 0 && b > a {
            b - 1
        } else {
            b.max(a)
        };
        end = end.max(last);
    }
    if start == usize::MAX {
        return (0, count);
    }
    (start.min(count.saturating_sub(1)), (end + 1).min(count))
}

/// Build the edit, or an empty one when nothing would change.
///
/// Returning an empty edit rather than a replace-everything edit matters: a
/// no-op edit still marks the document modified in most editors, and a
/// formatter that does that on every save is unusable.
fn edit_for(
    out: &[String],
    input: &[&str],
    start: usize,
    end: usize,
    settings: &Settings,
    selections: &[Selection],
) -> Edit {
    let unchanged = out.len() == input.len() && out.iter().zip(input).all(|(a, b)| a == *b);
    if unchanged {
        return Edit::empty();
    }
    let mut edit = Edit::replace(start, end, out.to_vec());
    edit.selections = Some(map_selections(selections, start, out.len(), settings));
    edit
}

/// Put the cursor back where it was, as closely as the new text allows.
///
/// A wrap can add or remove lines, so a position is kept by preserving the
/// offset from the start of the wrapped region and clamping it to the new
/// length. Losing the cursor to the end of the document after formatting is the
/// single most irritating thing a formatter can do.
fn map_selections(
    selections: &[Selection],
    start: usize,
    len: usize,
    _settings: &Settings,
) -> Vec<Selection> {
    let last = len.saturating_sub(1);
    selections
        .iter()
        .map(|selection| {
            let map = |position: Position| Position {
                line: start + (position.line.saturating_sub(start)).min(last),
                character: position.character,
            };
            Selection {
                anchor: map(selection.anchor),
                active: map(selection.active),
            }
        })
        .collect()
}

/// The processor for a language: whole-document prose, or code with comments.
fn process_language(language: &Language, settings: &Settings, lines: &[&str]) -> Vec<Chunk> {
    if let Some(dialect) = language.document {
        return dialects::parse_document(dialect, settings, &plain(lines));
    }
    comment::parse(language, settings, lines)
}

/// Auto-wrap: the edit that should happen when `inserted` was typed at `at`.
///
/// Deliberately conservative. It fires only for whitespace inserted into a line
/// that is already past the column, which is the "I kept typing and the line
/// grew" case. Everything else -- a paste, a deletion, a multi-cursor edit --
/// returns an empty edit, because an automatic edit that guesses wrong fights
/// the user's own undo history.
#[must_use]
pub fn maybe_auto_wrap(
    file: &File,
    settings: &Settings,
    inserted: &str,
    at: Position,
    lines: &[&str],
) -> Edit {
    let column = settings.effective_column();
    if column == 0 || inserted.is_empty() || !inserted.chars().all(char::is_whitespace) {
        return Edit::empty();
    }
    // Only a single space, or a newline with its indentation, is a typing
    // event. Anything longer is a paste.
    let newline = inserted.starts_with('\n') || inserted.starts_with("\r\n");
    if !newline && inserted.chars().count() > 1 {
        return Edit::empty();
    }
    if at.line >= lines.len() {
        return Edit::empty();
    }
    let line = lines[at.line];
    let up_to = if newline {
        at.character
    } else {
        at.character + inserted.len()
    };
    let head: String = line.chars().take(up_to).collect();
    if settings.width(0, &head) <= column {
        return Edit::empty();
    }
    // Wrap just this line, by selecting it.
    let selection = Selection {
        anchor: Position {
            line: at.line,
            character: 0,
        },
        active: Position {
            line: at.line,
            character: line.chars().count(),
        },
    };
    let mut edit = rewrap(file, settings, &[selection], lines);
    if !edit.is_empty() {
        let after = if newline {
            Position {
                line: at.line + 1,
                character: inserted.chars().skip(1).take_while(|c| *c == ' ').count(),
            }
        } else {
            Position {
                line: at.line,
                character: at.character + inserted.len(),
            }
        };
        edit.selections = Some(vec![Selection {
            anchor: after,
            active: after,
        }]);
    }
    edit
}

/// The names of every language the engine knows, for the editor to offer and
/// for the CLI's `--list-languages`.
#[must_use]
pub fn languages() -> Vec<&'static str> {
    LANGUAGES.iter().map(|language| language.name).collect()
}

/// The editor ids the table answers to, which is what a fixture names.
#[must_use]
pub fn language_ids() -> Vec<&'static str> {
    LANGUAGES.iter().map(|language| language.id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> Settings {
        Settings {
            column: 20,
            tab_width: 4,
        }
    }

    fn markdown() -> File {
        File {
            language: "markdown".into(),
            path: "x.md".into(),
        }
    }

    fn run(input: &[&str]) -> Vec<String> {
        let edit = rewrap(&markdown(), &settings(), &[], input);
        if edit.is_empty() {
            input.iter().map(|line| (*line).to_string()).collect()
        } else {
            edit.lines
        }
    }

    #[test]
    fn a_blank_line_ends_a_paragraph() {
        assert_eq!(
            run(&["alpha beta gamma delta", "", "one two three four five"]),
            vec![
                "alpha beta gamma",
                "delta",
                "",
                "one two three four",
                "five"
            ]
        );
    }

    #[test]
    fn a_fenced_block_is_never_touched() {
        let input = ["```", "alpha beta gamma delta epsilon", "```"];
        assert_eq!(run(&input), input.to_vec());
    }

    #[test]
    fn wrapping_is_idempotent() {
        let once = run(&["alpha beta gamma delta epsilon zeta"]);
        let twice = run(&once.iter().map(String::as_str).collect::<Vec<_>>());
        assert_eq!(once, twice);
    }

    #[test]
    fn an_unchanged_document_produces_no_edit() {
        let edit = rewrap(&markdown(), &settings(), &[], &["short"]);
        assert!(edit.is_empty());
    }

    #[test]
    fn two_trailing_spaces_keep_their_line_break() {
        let out = run(&["alpha beta  ", "gamma delta epsilon zeta"]);
        assert!(out[0].ends_with("  "), "{out:?}");
    }
}
