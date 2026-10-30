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
        let after = after.strip_prefix(' ').unwrap_or(after).trim_end();
        // An empty comment line keeps the marker and nothing else: emitting the
        // separating space would leave trailing whitespace all over the file.
        // The marker is emitted with exactly one space after it, even when the
        // marker itself ends in one (`rem ` in batch files): repeating the
        // marker's own trailing space would widen the comment on every wrap.
        let marker = comment.start.trim_end();
        let prefix = if after.is_empty() {
            format!("{indent}{marker}")
        } else {
            format!("{indent}{marker} ")
        };
        bodies.push(Line::new(prefix, after));
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
                body_prefix.trim_end().to_string()
            } else {
                body_prefix.clone()
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

    let mut chunks = crate::dialects::parse_body(comment.dialect, settings, &bodies);
    for chunk in &mut chunks {
        if let Chunk::Para(para) = chunk {
            para.cont.clone_from(&body_prefix);
        }
    }
    if closed {
        if closed_on_own_line {
            chunks.push(Chunk::Verbatim(vec![close_prefix]));
        } else {
            chunks.push(Chunk::Suffix(format!(" {}", comment.end)));
        }
    }
    (chunks, i)
}

/// Remove a body line's decoration character, if it has one.
///
/// Only when the character stands alone or is followed by a space: ` * text` is
/// decoration, ` **bold**` is content, and stripping the latter would delete a
/// character the author wrote.
fn strip_decoration(segment: &str, decorate: &str) -> String {
    let trimmed = segment.trim_start();
    if decorate.is_empty() {
        return trimmed.to_string();
    }
    let Some(rest) = trimmed.strip_prefix(decorate) else {
        return trimmed.to_string();
    };
    if rest.is_empty() {
        return String::new();
    }
    rest.strip_prefix(' ').unwrap_or(rest).to_string()
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

    let mut bodies = vec![Line::new(head.to_string(), after_open.trim_end())];
    let mut i = start + 1;
    let mut closed = false;
    while i < lines.len() {
        let line = lines[i];
        let relative = line.strip_prefix(indent.as_str()).unwrap_or(line);
        if let Some(close) = relative.find(comment.end) {
            // A closing quote alone on its line ends the body; pushing the
            // empty line it left behind would put a blank line inside the
            // string, which changes its value and its documentation output.
            if !relative[..close].trim().is_empty() {
                bodies.push(Line::new(String::new(), &relative[..close]));
            }
            closed = true;
            i += 1;
            break;
        }
        bodies.push(Line::new(String::new(), relative.trim_end()));
        i += 1;
    }
    if !closed {
        // An unterminated literal is not a comment: leave the file alone rather
        // than guessing where the author meant it to end.
        return None;
    }
    let mut chunks = crate::dialects::parse_body(comment.dialect, settings, &bodies);
    // The first block continues after the opening quotes on the same line; every
    // later line lines up under the string's own indentation.
    let mut seen_para = false;
    for chunk in &mut chunks {
        match chunk {
            Chunk::Para(para) => {
                if seen_para {
                    para.first.clone_from(&indent);
                }
                seen_para = true;
                para.cont.clone_from(&indent);
            }
            Chunk::Verbatim(lines) => {
                for line in lines.iter_mut() {
                    if !line.trim().is_empty() {
                        line.insert_str(0, &indent);
                    }
                }
            }
            Chunk::Suffix(_) => {}
        }
    }
    chunks.push(Chunk::Verbatim(vec![format!("{indent}{}", comment.end)]));
    Some((chunks, i))
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
