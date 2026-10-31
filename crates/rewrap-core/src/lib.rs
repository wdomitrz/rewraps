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
        // A selection inside a block comment or a docstring starts on a line
        // with no marker of its own, so on its own it is indistinguishable from
        // code and would be left alone. Grow it to the comment that contains it;
        // a selection of complete comment lines is already wrap-able and is left
        // exactly as chosen.
        widen_inside_comment(file, lines, start, end)
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

/// Grow a selection that begins inside a comment to that comment's boundaries.
///
/// Only when the first selected line does not itself begin a comment: a
/// selection of complete `//` lines, or one that starts at a `/*` or `"""`, is
/// already something the parser can read, and widening it would wrap lines the
/// user did not select.
fn widen_inside_comment(file: &File, lines: &[&str], start: usize, end: usize) -> (usize, usize) {
    let Some(language) = resolve(file) else {
        return (start, end);
    };
    if language.is_document() || comment::opener_of(language, lines[start]).is_some() {
        return (start, end);
    }
    match comment_block(language, lines, start) {
        // The block must actually contain the selection; a line outside any
        // comment stays where it is.
        Some((first, last)) if first <= start && end - 1 <= last => (first, last + 1),
        _ => (start, end),
    }
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
///
/// A comment is a *block*, and the block is what gets wrapped. Wrapping the line
/// under the cursor instead loses everything the block encodes: a hanging indent
/// (`#    text`), the star column of a block comment, the alignment of a
/// run-on comment. So the run is found by shape, not by "is this line a comment
/// line":
///
/// * a run of the **same marker** -- `//`, `#`, `///` -- at the same indent is
///   one comment, whatever its indents below the marker;
/// * a block comment is the lines from its opening marker to its closing one,
///   found by counting, because they nest in Rust and OCaml.
fn comment_block(language: &'static Language, lines: &[&str], at: usize) -> Option<(usize, usize)> {
    let line = lines[at];
    let indent = crate::markdown::leading_whitespace(line);
    let rest = &line[indent.len()..];

    // A line that opens a comment: the run starts here.
    if let Some(comment) = opener_marker(language, rest) {
        return Some(match comment.kind {
            Kind::Line => line_run(lines, at, &indent, comment.start),
            _ => delimited_block(lines, at, comment),
        });
    }

    // A continuation line: ` * text` inside a block comment, or the closing
    // line. Find the block that is still open here.
    if let Some(comment) = comment::open_at(language, lines, at) {
        return Some(delimited_block(lines, at, comment));
    }

    // A run-on comment: a line with no marker of its own, indented no further
    // than the marker, continues the run above it. That is the shape a comment
    // takes once a wrap has moved a word onto a line without a marker.
    let previous = lines.get(at.checked_sub(1)?)?;
    let previous_indent = crate::markdown::leading_whitespace(previous);
    let marker = opener_marker(language, &previous[previous_indent.len()..])?;
    if marker.kind == Kind::Line && indent.len() <= previous_indent.len() {
        return Some(line_run(lines, at, &previous_indent, marker.start));
    }
    None
}

fn opener_marker(language: &Language, rest: &str) -> Option<&'static Comment> {
    let mut best: Option<&'static Comment> = None;
    for comment in language.comments {
        if comment.kind == Kind::String || !rest.starts_with(comment.start) {
            continue;
        }
        if let Some(next) = comment.not_followed_by.chars().next() {
            if rest[comment.start.len()..].starts_with(next) {
                continue;
            }
        }
        if best.is_none_or(|current| comment.start.len() > current.start.len()) {
            best = Some(comment);
        }
    }
    best
}

/// Every line of a run of the same marker at the same indent, including run-on
/// lines that carry no marker but are indented to the marker's own level.
fn line_run(lines: &[&str], at: usize, indent: &str, marker: &str) -> (usize, usize) {
    let starts = |line: &str| {
        line.strip_prefix(indent)
            .is_some_and(|rest| rest.starts_with(marker))
    };
    // A line with no marker that is not indented past the marker's own level
    // continues the paragraph above it.
    let continues =
        |line: &str| starts(line) || (!line.trim().is_empty() && !line.starts_with(' '));

    let mut first = at;
    while first > 0 && continues(lines[first - 1]) {
        first -= 1;
    }
    let mut last = at;
    while last + 1 < lines.len() && continues(lines[last + 1]) {
        last += 1;
    }
    // Never swallow code above: a run may not start on a line that is not part
    // of it.
    while first < at && !starts(lines[first]) {
        first += 1;
    }
    (first, last)
}

/// Every line from a block comment's opening marker to its closing one.
fn delimited_block(lines: &[&str], at: usize, comment: &'static Comment) -> (usize, usize) {
    let mut first = at;
    let mut depth = 0usize;
    // Walk up until a line leaves the comment open before it.
    while first > 0 {
        let previous = lines[first - 1];
        let (opens, closes) = count_markers(previous, comment, comment.nesting);
        depth = depth.saturating_add(opens).saturating_sub(closes);
        first -= 1;
        if depth > 0 {
            break;
        }
    }
    let mut last = at;
    while last + 1 < lines.len() && !lines[last].contains(comment.end) {
        last += 1;
    }
    (first, last)
}

fn count_markers(text: &str, comment: &Comment, nesting: bool) -> (usize, usize) {
    let mut opens = 0usize;
    let mut closes = 0usize;
    let mut position = 0usize;
    while position < text.len() {
        let rest = &text[position..];
        if nesting && rest.starts_with(comment.start) {
            opens += 1;
            position += comment.start.len();
            continue;
        }
        if rest.starts_with(comment.end) {
            closes += 1;
            position += comment.end.len();
            continue;
        }
        position += rest.chars().next().map_or(1, char::len_utf8);
    }
    (opens, closes)
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
    let typed_end = if newline {
        at.character
    } else {
        at.character + inserted.chars().count()
    };
    let head: String = lines[at.line].chars().take(typed_end).collect();
    if settings.width(0, &head) <= column {
        return Edit::empty();
    }

    // The whole comment, not the line under the cursor. Re-flowing one line
    // invents its continuation prefix from the line itself, which loses a
    // hanging indent (`#    text` becoming `# text`) and silently changes the
    // shape of a block. Wrapping the block lets the content processor keep the
    // shape it was already preserving.
    let (start, end) = expand(file, lines, at.line);
    if start + 1 == end {
        // One line: a line comment, a comment that opens and closes on the line,
        // or a line of code. The single-line path can track the cursor exactly.
        return single_line_auto_wrap(file, settings, lines, at, newline, inserted, typed_end);
    }

    // Count the text characters up to the cursor, so the position can be found
    // again in the output. Markers are skipped: they are not text, and a wrap
    // repeats them.
    let language = resolve(file);
    let offset = |line: &str| match language {
        Some(language) if !language.is_document() => comment::content_offset(language, line),
        _ => line.len() - line.trim_start().len(),
    };
    let mut seen = 0usize;
    for (index, line) in lines.iter().enumerate().take(at.line + 1).skip(start) {
        let from = offset(line);
        let to = if index == at.line {
            typed_end.min(line.len())
        } else {
            line.len()
        };
        seen += line[from.min(to)..to]
            .chars()
            .filter(|c| !c.is_whitespace())
            .count();
    }

    let selection = Selection {
        anchor: Position {
            line: start,
            character: 0,
        },
        active: Position {
            line: end - 1,
            character: lines[end - 1].chars().count(),
        },
    };
    let mut edit = rewrap(file, settings, &[selection], lines);
    if edit.is_empty() {
        return Edit::empty();
    }

    // The space that triggered the wrap is kept when it was typed at the end of
    // the text: it separates the next word, and dropping it welds the next
    // keystroke onto the current one.
    // What matters is that the space was typed at the end of the line's *text*,
    // not that the line is the last of the region: a docstring's region ends at
    // its closing quotes, one line below the text being typed.
    let typed_line = lines[at.line];
    let typed_at_end = !newline
        && typed_end >= typed_line.trim_end().chars().count()
        && typed_line[..typed_end.min(typed_line.len())].ends_with(' ');
    // The space goes on the last line of *text*: for a docstring that is the line
    // above its closing quotes, not the quotes themselves. For a block comment
    // that closed on the typed line it is that line, and there the space would be
    // trailing whitespace on a finished comment, so it is dropped.
    let mut space_at: Option<usize> = None;
    if typed_at_end {
        if let Some(index) = edit
            .lines
            .iter()
            .rposition(|line| !line.trim().is_empty() && !is_closing_only(file, line))
        {
            let line = &mut edit.lines[index];
            if ends_with_closing_marker(file, line) {
                // Nothing to add, and nothing to point at.
            } else {
                if !line.ends_with(' ') {
                    line.push(' ');
                }
                space_at = Some(index);
            }
        }
    }

    let position = if newline {
        Position {
            line: at.line + edit.lines.len(),
            character: inserted.chars().skip(1).take_while(|c| *c == ' ').count(),
        }
    } else if let Some(index) = space_at {
        Position {
            line: start + index,
            character: edit.lines[index].chars().count(),
        }
    } else {
        let found = find_text_position(language, &edit.lines, start, seen);
        Position {
            line: found.0,
            character: found.1,
        }
    };
    edit.selections = Some(vec![Selection {
        anchor: position,
        active: position,
    }]);
    edit
}

/// Auto-wrap when the typed line is the whole region.
///
/// The text is taken to be everything after the marker and the prefix put back
/// on every line, which lets the cursor be tracked precisely through the break.
fn single_line_auto_wrap(
    file: &File,
    settings: &Settings,
    lines: &[&str],
    at: Position,
    newline: bool,
    inserted: &str,
    typed_end: usize,
) -> Edit {
    let line = lines[at.line];
    let parts = resolve(file).and_then(|language| {
        if language.is_document() {
            Some(comment::LineParts {
                content_start: 0,
                content_end: line.chars().count(),
                first: String::new(),
                cont: String::new(),
                suffix: String::new(),
            })
        } else {
            comment::parts_for(language, lines, at.line)
        }
    });

    let Some(parts) = parts else {
        return fallback_auto_wrap(file, settings, lines, at, newline, inserted);
    };

    let column = settings.effective_column();
    let cursor = typed_end.saturating_sub(line[..parts.content_start].chars().count());
    let (mut out, position) = settings.fill_tracked(
        &line[parts.content_start..parts.content_end],
        &parts.first,
        &parts.cont,
        column,
        Some(cursor),
    );
    // The space that triggered the wrap is kept when it was typed at the end of
    // the text: dropping it welds the next keystroke onto the last word.
    let keep_space = !newline
        && typed_end >= parts.content_end
        && parts.suffix.is_empty()
        && line[..parts.content_end].ends_with(' ');
    if keep_space {
        if let Some(last) = out.last_mut() {
            last.push(' ');
        }
    }
    if !parts.suffix.is_empty() {
        if let Some(last) = out.last_mut() {
            last.push_str(&parts.suffix);
        }
    }
    if out.len() == 1 && out[0] == line {
        return Edit::empty();
    }
    let position = if keep_space {
        Position {
            line: at.line + out.len().saturating_sub(1),
            character: out.last().map_or(0, |line| line.chars().count()),
        }
    } else {
        cursor_after(&out, at.line, position, newline, inserted)
    };
    let mut edit = Edit::replace(at.line, at.line + 1, out);
    edit.selections = Some(vec![Selection {
        anchor: position,
        active: position,
    }]);
    edit
}

/// The line and column of the `wanted`-th text character of `out`.
///
/// Same counting as the input side: skip each line's marker, count non-space
/// characters, stop just after the one the cursor was behind.
fn find_text_position(
    language: Option<&'static Language>,
    out: &[String],
    start: usize,
    wanted: usize,
) -> (usize, usize) {
    if wanted == 0 {
        return (start, 0);
    }
    let mut seen = 0usize;
    for (index, line) in out.iter().enumerate() {
        let from = match language {
            Some(language) if !language.is_document() => comment::content_offset(language, line),
            _ => line.len() - line.trim_start().len(),
        };
        for (at, character) in line.char_indices() {
            if at < from || character.is_whitespace() {
                continue;
            }
            seen += 1;
            if seen == wanted {
                return (start + index, at + character.len_utf8());
            }
        }
    }
    let last = out.len().saturating_sub(1);
    (
        start + last,
        out.get(last).map_or(0, |line| line.chars().count()),
    )
}

/// Whether this output line is nothing but a comment's closing marker.
fn is_closing_only(file: &File, line: &str) -> bool {
    let trimmed = line.trim();
    resolve(file).is_some_and(|language| {
        language
            .comments
            .iter()
            .any(|comment| !comment.end.is_empty() && trimmed == comment.end)
    })
}

/// Whether this output line already ends with a comment's closing marker.
fn ends_with_closing_marker(file: &File, line: &str) -> bool {
    let trimmed = line.trim_end();
    resolve(file).is_some_and(|language| {
        language
            .comments
            .iter()
            .any(|comment| !comment.end.is_empty() && trimmed.ends_with(comment.end))
    })
}

/// Where the cursor goes once the line has been wrapped.
///
/// For a typed space the answer is the position the fill tracked, on whichever
/// line the word now ends. For a newline the cursor was already on the next
/// line before the wrap ran, so it moves down by however many lines the wrap
/// added -- ending up below the text it just pushed.
fn cursor_after(
    out: &[String],
    line: usize,
    position: Option<(usize, usize)>,
    newline: bool,
    inserted: &str,
) -> Position {
    if newline {
        let indent = inserted.chars().skip(1).take_while(|c| *c == ' ').count();
        return Position {
            line: line + out.len(),
            character: indent,
        };
    }
    let (offset, column) = position.unwrap_or((out.len().saturating_sub(1), 0));
    Position {
        line: line + offset,
        character: column,
    }
}

/// Auto-wrap for a line the tracked path cannot handle.
///
/// Wrapping the whole line as a region still fixes the overflow, which is the
/// point of auto-wrap; the cursor is put at the end of the wrapped text rather
/// than tracked, because the prefixes there are the block comment's decoration
/// and not a plain marker.
fn fallback_auto_wrap(
    file: &File,
    settings: &Settings,
    lines: &[&str],
    at: Position,
    newline: bool,
    inserted: &str,
) -> Edit {
    let selection = Selection {
        anchor: Position {
            line: at.line,
            character: 0,
        },
        active: Position {
            line: at.line,
            character: lines[at.line].chars().count(),
        },
    };
    let mut edit = rewrap(file, settings, &[selection], lines);
    if !edit.is_empty() {
        let position = cursor_after(&edit.lines, at.line, None, newline, inserted);
        edit.selections = Some(vec![Selection {
            anchor: position,
            active: position,
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
