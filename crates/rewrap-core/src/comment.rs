//! Finding comments in a code file, using only the language table.
//!
//! The scanner answers one question per line: does a comment start here, and if
//! so which one? Everything else in the file is code and is copied through. Two
//! rules matter more than the rest:
//!
//! * The **longest** matching marker wins, so `///` beats `//` and `/**` beats
//!   `/*` without the data file having to be ordered. That is why a doc comment
//!   gets its own dialect and a plain one does not.
//! * Only a comment that **begins** its line is wrapped. A trailing
//!   `x = 1; // note` needs a parser for the language's literals to find safely,
//!   and guessing wrong there rewrites code. Every dialect here leaves those
//!   alone, which is the conservative half of the trade.

use crate::language::{string_opener, Comment, Kind, Language};
use crate::{Chunk, Line, Settings};

pub(crate) fn parse(language: &Language, settings: &Settings, lines: &[&str]) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let mut i = 0usize;
    while i < lines.len() {
        let text = lines[i];
        match opener(language, text) {
            Some(comment) => match comment.kind {
                Kind::Line => {
                    let (block, next) = line_run(settings, lines, i, comment);
                    chunks.extend(block);
                    i = next;
                }
                Kind::Delimited => {
                    let (block, next) = delimited(settings, lines, i, comment);
                    chunks.extend(block);
                    i = next;
                }
                Kind::String => match string_block(settings, lines, i, comment) {
                    Some((block, next)) => {
                        chunks.extend(block);
                        i = next;
                    }
                    None => {
                        chunks.push(verbatim_line(text));
                        i += 1;
                    }
                },
            },
            None => {
                chunks.push(verbatim_line(text));
                i += 1;
            }
        }
    }
    chunks
}

fn verbatim_line(text: &str) -> Chunk {
    Chunk::Verbatim(vec![text.to_string()])
}

/// The comment a line begins with, if any.
///
/// A line comment must start the line apart from indentation. A delimited or
/// string comment may appear further in, because `x = """` and `foo(/*` are
/// real code shapes, and the marker itself is unambiguous.
fn opener(language: &Language, line: &str) -> Option<&'static Comment> {
    let indent = crate::markdown::leading_whitespace(line);
    let rest = &line[indent.len()..];
    let mut best: Option<&'static Comment> = None;
    for comment in language.comments {
        match comment.kind {
            Kind::Line | Kind::Delimited => {
                if !rest.starts_with(comment.start) {
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
            Kind::String => {}
        }
    }
    if best.is_some() {
        return best;
    }
    language
        .comments
        .iter()
        .find(|comment| comment.kind == Kind::String && string_opener(line, comment).is_some())
}

/// Widest visual indentation, so that tabs and spaces count alike.
fn indent_width(settings: &Settings, line: &str) -> usize {
    settings.width(0, &crate::markdown::leading_whitespace(line))
}

/// A run of consecutive line comments at the same indent.
fn line_run(
    settings: &Settings,
    lines: &[&str],
    start: usize,
    comment: &Comment,
) -> (Vec<Chunk>, usize) {
    let indent = crate::markdown::leading_whitespace(lines[start]);
    let width = indent_width(settings, lines[start]);
    let mut bodies = Vec::new();
    let mut i = start;
    while i < lines.len() {
        let line = lines[i];
        let line_indent = crate::markdown::leading_whitespace(line);
        let rest = &line[line_indent.len()..];
        if !rest.starts_with(comment.start)
            || indent_width(settings, line) != width
            || line_indent != indent
        {
            break;
        }
        // Exactly one space after the marker separates it from the text; any
        // further indentation is content and must survive. That is what keeps a
        // code sample inside a comment a code sample -- godoc treats an indented
        // line as code, and trimming here would reflow it into prose.
        let after = &rest[comment.start.len()..];
        // Remember whether a space separated the marker from the text, because
        // it has to be put back only if it was there. A tab-indented godoc code
        // sample (`//<tab>code`) is indented by that tab: adding a space turns
        // it into `// <tab>code` and shifts the sample by a column. Every wrap
        // did that, so a file that was already correct still came out modified
        // -- and `--check` called it a change.
        let (separated, body) = match after.strip_prefix(' ') {
            Some(rest) => (true, rest),
            None => (false, after),
        };
        let body = body.trim_end();
        // The marker is emitted without its own trailing space (`rem ` in batch
        // files), and exactly one separator goes back: the marker's own space if
        // it had one, or the one that was between it and the text. A marker
        // followed directly by a tab keeps the tab and gains nothing, which is
        // what stops a godoc code sample shifting by a column.
        let marker = comment.start.trim_end();
        let marker_had_space = comment.start.len() != marker.len();
        // An empty comment line keeps the marker and nothing else, so no
        // trailing whitespace is left behind.
        let separator = if body.is_empty() || !(separated || marker_had_space) {
            ""
        } else {
            " "
        };
        bodies.push(Line::new(format!("{indent}{marker}{separator}"), body));
        i += 1;
    }
    let body: Vec<Line> = bodies.into_iter().collect();
    (
        crate::dialects::parse_body(comment.dialect, settings, &body),
        i,
    )
}

/// A delimited comment, with a depth counter when the language nests.
fn delimited(
    settings: &Settings,
    lines: &[&str],
    start: usize,
    comment: &Comment,
) -> (Vec<Chunk>, usize) {
    let indent = crate::markdown::leading_whitespace(lines[start]);
    let open_prefix = format!("{indent}{}", comment.start);
    let body_prefix = if comment.decorate.is_empty() {
        format!("{indent} ")
    } else {
        format!("{indent} {} ", comment.decorate)
    };
    let close_prefix = if comment.decorate.is_empty() {
        format!("{indent}{}", comment.end)
    } else {
        format!("{indent} {}", comment.end)
    };

    let mut bodies: Vec<Line> = Vec::new();
    let mut i = start;
    let mut depth = 0usize;
    let mut first = true;
    let mut closed_on_own_line = false;
    let mut written_close = String::new();
    let mut closed = false;

    while i < lines.len() {
        let text = lines[i];
        let from = if first {
            indent.len() + comment.start.len()
        } else {
            0
        };
        let mut at = from;
        let mut segment_end = text.len();
        // The scan starts after the opening marker on the first line, so every
        // start it meets is a nested one.
        while at < text.len() {
            let rest = &text[at..];
            if comment.nesting && rest.starts_with(comment.start) {
                depth += 1;
                at += comment.start.len();
                continue;
            }
            if rest.starts_with(comment.end) {
                if depth == 0 {
                    segment_end = at;
                    closed = true;
                    closed_on_own_line = text[..at].trim().is_empty();
                    if closed_on_own_line {
                        // The closing marker is re-emitted with the indentation it
                        // was written with: ` */` aligned under ` * ` is the
                        // shape nearly every block comment has.
                        written_close = format!("{}{}", &text[..at], comment.end);
                    }
                    break;
                }
                depth -= 1;
                at += comment.end.len();
                continue;
            }
            at += rest.chars().next().map_or(1, char::len_utf8);
        }

        let segment = &text[from.min(segment_end)..segment_end];
        let body = strip_decoration(segment, comment.decorate);
        let blank = body.trim().is_empty();
        // The line that holds only the closing marker contributes no text; a
        // body line that happens to be empty still keeps its marker.
        if !(blank && closed) {
            let prefix = if first {
                if blank {
                    open_prefix.clone()
                } else {
                    format!("{open_prefix} ")
                }
            } else if blank {
                written_prefix(text, comment).trim_end().to_string()
            } else {
                written_prefix(text, comment)
            };
            bodies.push(Line::new(prefix, body.trim_end()));
        }
        first = false;
        i += 1;
        if closed {
            break;
        }
    }

    if !closed {
        // An unterminated comment runs to the end of the block: that is what an
        // editor shows, and reflowing it is at least visible.
        i = lines.len();
    }

    // Continuation lines take the prefix the body was written with. The first
    // body line is the one after the opening marker; with none, the language's
    // decoration character is the best guess there is.
    let cont_prefix = bodies
        .iter()
        .skip(1)
        .map(|line| line.prefix.clone())
        // An indentation-only prefix is meaningful -- the body is indented under
        // the marker -- so only a truly empty one is skipped.
        .find(|prefix| !prefix.is_empty())
        .unwrap_or_else(|| body_prefix.clone());
    let mut chunks = crate::dialects::parse_body(comment.dialect, settings, &bodies);
    for chunk in &mut chunks {
        if let Chunk::Para(para) = chunk {
            para.cont.clone_from(&cont_prefix);
        }
    }
    if closed {
        if closed_on_own_line {
            let close = if written_close.is_empty() {
                close_prefix
            } else {
                written_close
            };
            chunks.push(Chunk::Verbatim(vec![close]));
        } else {
            chunks.push(Chunk::Suffix(format!(" {}", comment.end)));
        }
    }
    (chunks, i)
}

/// The prefix a body line was written with.
///
/// Taken from the line itself -- its indentation plus a decoration character if
/// it has one -- rather than invented from the language's table. A block comment
/// written `/* ... */` with no stars on its body lines must stay that way, and
/// one written with ` * ` must keep them; the table only says what is *allowed*.
fn written_prefix(text: &str, comment: &Comment) -> String {
    let leading = crate::markdown::leading_whitespace(text);
    let rest = &text[leading.len()..];
    // A single decoration character followed by whitespace, or alone: that is a
    // body prefix. `**bold**` is not -- there the character is content.
    let decorate = if comment.decorate.is_empty() {
        "*".to_string()
    } else {
        comment.decorate.to_string()
    };
    if let Some(after) = rest.strip_prefix(decorate.as_str()) {
        if after.is_empty() || after.starts_with([' ', '\t']) {
            return format!("{leading}{decorate} ");
        }
    }
    leading.to_string()
}

/// Remove a body line's decoration character, if it has one.
///
/// Only when the character stands alone or is followed by a space: ` * text` is
/// decoration, ` **bold**` is content, and stripping the latter would delete a
/// character the author wrote.
fn strip_decoration(segment: &str, decorate: &str) -> String {
    let trimmed = segment.trim_start();
    // The star is the conventional body marker even where the table does not
    // name it: `/*` and `/**` bodies are written ` * text` in nearly every
    // codebase, and leaving the star in the text would double it on every wrap.
    let decorate = if decorate.is_empty() { "*" } else { decorate };
    let Some(rest) = trimmed.strip_prefix(decorate) else {
        return trimmed.to_string();
    };
    // Only when the character stands alone or is followed by a space: `**bold**`
    // is content, and stripping it would delete a character the author wrote.
    if rest.is_empty() {
        return String::new();
    }
    if let Some(after) = rest.strip_prefix(' ') {
        return after.to_string();
    }
    trimmed.to_string()
}

/// A string used as documentation (Python's triple quotes).
///
/// Multi-line only, unless the language says a single-line one may be split.
/// Splitting `"""SELECT 1"""` inserts a newline into the value, which changes
/// what the program does -- so the default is to leave it alone.
///
/// Indentation *inside* the string is kept relative to the string's own indent,
/// because the guards in the docstring dialect (a doctest, a literal block after
/// `::`) are indent-sensitive: trimming the lines would silently turn a literal
/// block into prose and reflow the example.
fn string_block(
    settings: &Settings,
    lines: &[&str],
    start: usize,
    comment: &Comment,
) -> Option<(Vec<Chunk>, usize)> {
    let text = lines[start];
    let open = string_opener(text, comment)?;
    let indent = crate::markdown::leading_whitespace(text);
    let head = &text[..open + comment.start.len()];
    let after_open = &text[open + comment.start.len()..];

    if let Some(close) = after_open.find(comment.end) {
        // A complete string on one line.
        if comment.skip_single_line {
            return None;
        }
        let inner = &after_open[..close];
        let tail = &after_open[close + comment.end.len()..];
        let mut chunks =
            crate::dialects::parse_body(comment.dialect, settings, &[Line::new("", inner)]);
        let mut first = true;
        for chunk in &mut chunks {
            if let Chunk::Para(para) = chunk {
                if first {
                    para.first = format!("{head} ");
                    first = false;
                }
                para.cont = format!("{indent}{}", " ".repeat(comment.end.len()));
            }
        }
        chunks.push(Chunk::Suffix(format!(
            "{} {}",
            comment.end,
            tail.trim_end()
        )));
        return Some((chunks, start + 1));
    }

    // Every body line keeps its own indentation, measured from the string's
    // indent: a docstring's literal block and its doctests are indent-sensitive,
    // and trimming here would turn the first into prose and lose the shared
    // indent the second is written with.
    let first_line = Line::new(String::new(), after_open);
    let mut bodies = vec![first_line];
    let mut i = start + 1;
    let mut closed = false;
    while i < lines.len() {
        let line = lines[i];
        let relative = line.strip_prefix(indent.as_str()).unwrap_or(line);
        if let Some(close) = relative.find(comment.end) {
            if !relative[..close].trim().is_empty() {
                bodies.push(Line::new(String::new(), &relative[..close]));
            }
            closed = true;
            i += 1;
            break;
        }
        bodies.push(Line::new(String::new(), relative));
        i += 1;
    }
    if !closed {
        // An unterminated literal is not a comment: leave the file alone rather
        // than guessing where the author meant it to end.
        return None;
    }

    // Line 0 continues after the opening quotes; the rest are re-emitted under
    // the string's own indent, so the body keeps the column it was written at.
    let mut prefixed: Vec<Line> = Vec::with_capacity(bodies.len());
    for (index, line) in bodies.into_iter().enumerate() {
        // The first body line starts after the opening quotes, so it carries
        // `head`; every later one carries the string's own indent. Either way
        // the body's own leading whitespace is kept, which is what a literal
        // block and a doctest are indented by.
        let base = if index == 0 {
            head.to_string()
        } else {
            indent.clone()
        };
        // The line's own indentation stays in its text, not in its prefix: it is
        // what tells the paragraph where a hanging indent is, and a prefix that
        // carries it cannot be measured against the string's own indent.
        prefixed.push(Line::new(base, line.text));
    }
    let mut chunks = crate::dialects::parse_body(comment.dialect, settings, &prefixed);
    // The opening quotes are the string's delimiter, not a marker that repeats:
    // a continuation prefix derived from that line would put `"""` in the middle
    // of a sentence. Continuation lines line up with the string's own indent.
    if let Some(Chunk::Para(para)) = chunks.iter_mut().find(|chunk| {
        matches!(chunk, Chunk::Para(para) if para.first.trim_end().ends_with(comment.start))
    }) {
        para.cont.clone_from(&indent);
    }
    chunks.push(Chunk::Verbatim(vec![format!("{indent}{}", comment.end)]));
    Some((chunks, i))
}

/// The comment whose marker begins this text, if any.
pub(crate) fn opener_of(language: &Language, line: &str) -> Option<&'static Comment> {
    let indent = crate::markdown::leading_whitespace(line);
    marker_at(language, &line[indent.len()..])
}

/// How one line of a comment splits into marker, text and closing marker.
///
/// Auto-wrap wraps a single line while typing, so it needs to know which part of
/// that line is the comment and which is the text -- the cursor offset it is
/// given counts from the start of the line, marker included. Returning all four
/// pieces here keeps that knowledge in one place instead of duplicating the
/// marker rules a second time.
pub(crate) struct LineParts {
    /// Where the text starts in the source line.
    pub content_start: usize,
    /// Where the text ends -- before the closing marker, when the line had one.
    pub content_end: usize,
    /// What goes before the first output line.
    pub first: String,
    /// What goes before every later output line.
    pub cont: String,
    /// The closing marker, when the source line ended the comment on that line.
    pub suffix: String,
}

/// Split the line at `at` into the parts auto-wrap needs.
///
/// Three shapes, because a comment being typed in is not always complete on one
/// line:
///
/// * the line **begins** a comment -- `// text`, `/** text */`, or `/** text`
///   whose body continues below;
/// * the line is a **continuation** inside a block comment -- ` * text`, which
///   carries no opening marker of its own and used to match nothing at all, so
///   typing in it did nothing;
/// * anything else, which the caller handles.
///
/// `None` means "not a comment line": a docstring or a line of code.
pub(crate) fn parts_for(language: &Language, lines: &[&str], at: usize) -> Option<LineParts> {
    let line = *lines.get(at)?;
    let indent = crate::markdown::leading_whitespace(line);
    let rest = &line[indent.len()..];

    if let Some(comment) = marker_at(language, rest) {
        return Some(opening_parts(line, &indent, rest, comment));
    }

    let comment = open_at(language, lines, at)?;
    let body = &rest;
    // A continuation line normally carries the decoration character
    // (` * text`). When it does not, its own indentation is kept as the prefix
    // and nothing is invented -- adding stars to a comment that has none would
    // be a change nobody asked for.
    let (content_start, cont) =
        if !comment.decorate.is_empty() && body.starts_with(comment.decorate) {
            let after = &body[comment.decorate.len()..];
            let after = after.strip_prefix(' ').unwrap_or(after);
            (
                line.len() - after.len(),
                format!("{indent}{} ", comment.decorate),
            )
        } else {
            (indent.len(), format!("{indent} "))
        };
    let (content_end, suffix) = match line[content_start..].find(comment.end) {
        Some(close) => (content_start + close, format!(" {}", comment.end)),
        None => (line.len(), String::new()),
    };
    Some(LineParts {
        content_start,
        content_end,
        first: line[..content_start].to_string(),
        cont,
        suffix,
    })
}

/// The comment whose marker begins `rest`, longest marker first.
fn marker_at(language: &Language, rest: &str) -> Option<&'static Comment> {
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

fn decoration(indent: &str, comment: &Comment) -> String {
    if comment.decorate.is_empty() {
        format!("{indent} ")
    } else {
        format!("{indent} {} ", comment.decorate)
    }
}

/// Parts for a line whose own marker opens the comment.
fn opening_parts(line: &str, indent: &str, rest: &str, comment: &'static Comment) -> LineParts {
    let after_start = &rest[comment.start.len()..];
    let (separated, body) = match after_start.strip_prefix(' ') {
        Some(rest) => (true, rest),
        None => (false, after_start),
    };
    let marker = comment.start.trim_end();
    // The separating space goes back only if one was there; a marker followed
    // directly by a tab keeps the tab and gains nothing.
    let marker_had_space = comment.start.len() != marker.len();
    let separator = if separated || marker_had_space {
        " "
    } else {
        ""
    };
    let first = format!("{indent}{marker}{separator}");
    let content_start = line.len() - body.len();

    match comment.kind {
        Kind::Line => LineParts {
            content_start,
            content_end: line.len(),
            first: first.clone(),
            cont: first,
            suffix: String::new(),
        },
        // A block comment that closes on this line, or opens on it and runs on.
        // In the second case there is no closing marker to emit, and the lines
        // below keep their own prefixes -- only this line is re-flowed.
        _ => match body.find(comment.end) {
            Some(close) => LineParts {
                content_start,
                content_end: content_start + close,
                first,
                cont: decoration(indent, comment),
                suffix: format!(" {}", comment.end),
            },
            None => LineParts {
                content_start,
                content_end: line.len(),
                first,
                cont: decoration(indent, comment),
                suffix: String::new(),
            },
        },
    }
}

/// The block comment still open at line `at`, if the line is inside one.
///
/// Found by counting markers over the lines above: the last unmatched opening
/// marker wins, and the longest marker wins a tie, so `/**` is not mistaken for
/// `/*` in a language that has both.
pub(crate) fn open_at(language: &Language, lines: &[&str], at: usize) -> Option<&'static Comment> {
    let mut best: Option<(usize, &'static Comment)> = None;
    for comment in language.comments {
        // A delimited comment and a docstring are both blocks: they open once,
        // they can span lines, and everything between the markers is theirs. The
        // only difference is that a string's body is a literal, which matters to
        // the wrapping and not to finding it. Strings used to be skipped here,
        // which is why typing inside a docstring did nothing at all.
        if !matches!(comment.kind, Kind::Delimited | Kind::String) {
            continue;
        }
        let mut depth = 0usize;
        let mut last_open = None;
        for (index, line) in lines[..at].iter().enumerate() {
            let mut position = 0usize;
            while position < line.len() {
                let rest = &line[position..];
                if rest.starts_with(comment.start) {
                    // A docstring's opening quotes and its closing quotes are the
                    // same characters, and a one-line string opens and closes on
                    // the same line. Counting in order handles both: the opener
                    // is seen first, and its own closer later on the same line.
                    depth += 1;
                    last_open = Some(index * 10_000 + position);
                    position += comment.start.len();
                    continue;
                }
                if depth > 0 && rest.starts_with(comment.end) {
                    depth = depth.saturating_sub(1);
                    position += comment.end.len();
                    continue;
                }
                position += rest.chars().next().map_or(1, char::len_utf8);
            }
        }
        // A line that *ends* the comment has no unmatched opening marker above
        // it -- the line's own `*/` balances the `/**`. But a comment must have
        // been open at the start of that line, or the marker would close
        // nothing. Without this the last line of a block comment was the one
        // line typing did nothing in.
        if depth == 0 && !lines[at].contains(comment.end) {
            continue;
        }
        let Some(open) = last_open else { continue };
        let better = best.is_none_or(|(best_open, best_comment)| {
            open > best_open
                || (open == best_open && comment.start.len() > best_comment.start.len())
        });
        if better {
            best = Some((open, comment));
        }
    }
    best.map(|(_, comment)| comment)
}

/// Where the text of a comment line begins, for counting characters past markers.
///
/// Auto-wrap tracks the cursor by counting non-space characters, and a marker is
/// not text: `#` and the space after it must not be counted, or a repeated marker
/// on a wrapped line would shift the count and the cursor would land on the wrong
/// word. A hanging indent (`#    text`) is spaces, so it needs no special case --
/// it is simply not counted.
pub(crate) fn content_offset(language: &Language, line: &str) -> usize {
    let indent = crate::markdown::leading_whitespace(line);
    let rest = &line[indent.len()..];
    if let Some(comment) = marker_at(language, rest) {
        let after = &rest[comment.start.trim_end().len()..];
        let after = after.strip_prefix(' ').unwrap_or(after);
        return line.len() - after.len();
    }
    // A continuation line's decoration character, e.g. ` * text`.
    for comment in language.comments {
        if comment.kind != Kind::Delimited || comment.decorate.is_empty() {
            continue;
        }
        if let Some(after) = rest.strip_prefix(comment.decorate) {
            let after = after.strip_prefix(' ').unwrap_or(after);
            return line.len() - after.len();
        }
    }
    indent.len()
}

#[cfg(test)]
mod tests {
    use crate::{File, Settings};

    fn settings() -> Settings {
        Settings {
            column: 30,
            tab_width: 4,
        }
    }

    fn wrap(language: &str, path: &str, input: &[&str]) -> Vec<String> {
        let file = File {
            language: language.into(),
            path: path.into(),
        };
        let edit = crate::rewrap(&file, &settings(), &[], input);
        if edit.is_empty() {
            input.iter().map(|line| (*line).to_string()).collect()
        } else {
            edit.lines
        }
    }

    #[test]
    fn a_line_comment_run_is_wrapped_and_reprefixed() {
        assert_eq!(
            wrap("rust", "x.rs", &["// alpha beta gamma delta epsilon"]),
            vec!["// alpha beta gamma delta", "// epsilon"]
        );
    }

    #[test]
    fn code_lines_are_never_touched() {
        let input = ["fn main() { let alpha = beta_gamma_delta_epsilon_zeta; }"];
        assert_eq!(wrap("rust", "x.rs", &input), input.to_vec());
    }

    #[test]
    fn a_doc_comment_keeps_its_own_marker() {
        let out = wrap("rust", "x.rs", &["/// alpha beta gamma delta epsilon"]);
        assert_eq!(out, vec!["/// alpha beta gamma delta", "/// epsilon"]);
    }

    #[test]
    fn block_comments_get_the_decoration_character_on_new_lines() {
        let out = wrap("rust", "x.rs", &["/* alpha beta gamma delta epsilon */"]);
        assert_eq!(out, vec!["/* alpha beta gamma delta", " * epsilon */"]);
    }

    #[test]
    fn a_nested_block_comment_is_matched_by_depth() {
        let input = ["/* a /* inner */ b */", "let x = 1;"];
        let out = wrap("rust", "x.rs", &input);
        // The second line is code, not a continuation of the comment.
        assert_eq!(out[out.len() - 1], "let x = 1;");
    }

    #[test]
    fn different_indents_are_different_comment_blocks() {
        let input = [
            "// alpha beta gamma delta epsilon",
            "    // gamma",
            "    x();",
        ];
        let out = wrap("rust", "x.rs", &input);
        // The indented comment is its own block, and the code after it is code.
        assert_eq!(out[out.len() - 1], "    x();");
        assert!(out.iter().any(|line| line == "    // gamma"), "{out:?}");
    }

    #[test]
    fn a_shebang_is_not_a_comment() {
        let input = ["#!/usr/bin/env python3", "# alpha beta gamma delta epsilon"];
        let out = wrap("python", "x.py", &input);
        assert_eq!(out[0], "#!/usr/bin/env python3");
    }

    #[test]
    fn a_python_docstring_is_wrapped_but_a_single_line_string_is_not() {
        let out = wrap(
            "python",
            "x.py",
            &["\"\"\"alpha beta gamma delta epsilon\"\"\""],
        );
        assert_eq!(out.len(), 1, "a one-line string keeps its value: {out:?}");
    }
}
