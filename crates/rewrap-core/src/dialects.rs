//! The content dialects: how the text *inside* a comment is read.
//!
//! Every one of these is a pre-pass over the lines plus one of two real
//! processors (Markdown or plain text). That is the whole point of keeping the
//! dialect set closed: a new language picks an existing dialect for free, and
//! only a genuinely new *kind* of text costs a module.
//!
//! The pre-passes are all "freeze this" rules. A doctest, a literal block after
//! `::`, a directive body, an XML tag: none of them are prose, and reflowing
//! them is not cosmetic damage but a change to what the file means.

use crate::language::Dialect;
use crate::markdown::split_quotes;
use crate::{Chunk, Line, Para, Settings};

/// Parse a comment body.
pub(crate) fn parse_body(dialect: Dialect, settings: &Settings, lines: &[Line]) -> Vec<Chunk> {
    match dialect {
        Dialect::Verbatim => verbatim(lines),
        Dialect::Markdown => crate::markdown::parse(settings, lines),
        Dialect::PlainText => plaintext(lines),
        Dialect::Godoc => grouped(settings, lines, is_indented),
        Dialect::Docstring => docstring(settings, lines),
        Dialect::XmlDoc => grouped(settings, lines, |line| {
            line.text.trim_start().starts_with('<')
        }),
        Dialect::JsDoc
        | Dialect::Javadoc
        | Dialect::Doxygen
        | Dialect::RustDoc
        | Dialect::Dartdoc => tags(settings, lines, dialect),
    }
}

/// Parse a whole document (Markdown file, plain-text file).
pub(crate) fn parse_document(dialect: Dialect, settings: &Settings, lines: &[Line]) -> Vec<Chunk> {
    parse_body(dialect, settings, lines)
}

fn verbatim(lines: &[Line]) -> Vec<Chunk> {
    vec![Chunk::Verbatim(lines.iter().map(Line::full).collect())]
}

/// Paragraphs separated by blank lines only.
fn plaintext(lines: &[Line]) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let mut i = 0usize;
    while i < lines.len() {
        if lines[i].text.trim().is_empty() {
            chunks.push(Chunk::Verbatim(vec![lines[i].full()]));
            i += 1;
            continue;
        }
        let prefix = lines[i].prefix.clone();
        let mut para = Para::new(prefix.clone(), prefix);
        while i < lines.len() && !lines[i].text.trim().is_empty() {
            para.push(&lines[i].text);
            i += 1;
        }
        chunks.push(Chunk::Para(para));
    }
    chunks
}

/// Split into runs of "frozen" and "prose" lines, parsing the prose normally.
///
/// This is the shape of every simple dialect: a rule says "this line is not
/// prose", and the rest is ordinary Markdown.
fn grouped(settings: &Settings, lines: &[Line], frozen: impl Fn(&Line) -> bool) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let mut i = 0usize;
    while i < lines.len() {
        if frozen(&lines[i]) {
            let start = i;
            while i < lines.len() && frozen(&lines[i]) {
                i += 1;
            }
            chunks.push(Chunk::Verbatim(
                lines[start..i].iter().map(Line::full).collect(),
            ));
        } else {
            let start = i;
            while i < lines.len() && !frozen(&lines[i]) {
                i += 1;
            }
            chunks.extend(crate::markdown::parse(settings, &lines[start..i]));
        }
    }
    chunks
}

fn is_indented(line: &Line) -> bool {
    line.text.starts_with(' ') || line.text.starts_with('\t')
}

/// A doc-comment tag (`@param`, `\param`) starts its own block.
///
/// Without this, `@param x the thing` would be joined to the paragraph above it
/// and the tag would end up in the middle of a line, where javadoc and jsdoc
/// readers stop recognising it.
fn tags(settings: &Settings, lines: &[Line], dialect: Dialect) -> Vec<Chunk> {
    let marker = match dialect {
        Dialect::Doxygen => '\\',
        _ => '@',
    };
    let mut chunks = Vec::new();
    let mut start = 0usize;
    for i in 1..=lines.len() {
        let breaks = i == lines.len()
            || is_tag(&lines[i].text, marker)
            || (is_tag(&lines[start].text, marker)
                && i > start + 1
                && is_tag(&lines[i].text, marker));
        if breaks {
            if start < i {
                chunks.extend(crate::markdown::parse(settings, &lines[start..i]));
            }
            start = i;
        }
    }
    chunks
}

fn is_tag(text: &str, marker: char) -> bool {
    let (_, rest) = split_quotes(text);
    let trimmed = rest.trim_start();
    let mut chars = trimmed.chars();
    chars.next() == Some(marker) && chars.next().is_some_and(|c| c.is_ascii_alphabetic())
}

/// A Python docstring: plain text, with the constructs that must never be
/// reflowed frozen.
///
/// These guards ship *with* docstring support, not after it, because a
/// docstring is a string literal: a doctest that gains a line break stops
/// passing, and a literal block that gets reflowed changes the documented
/// example. So the classification happens up front, per line, and the prose
/// between the frozen runs is reflowed.
fn docstring(settings: &Settings, lines: &[Line]) -> Vec<Chunk> {
    let frozen = classify_docstring(lines);
    let mut chunks = Vec::new();
    let mut i = 0usize;
    while i < lines.len() {
        let start = i;
        let flag = frozen[i];
        while i < lines.len() && frozen[i] == flag {
            i += 1;
        }
        if flag {
            chunks.push(Chunk::Verbatim(
                lines[start..i].iter().map(Line::full).collect(),
            ));
        } else {
            chunks.extend(field_list_blocks(settings, &lines[start..i]));
        }
    }
    chunks
}

/// Which lines of a docstring must not be reflowed.
fn classify_docstring(lines: &[Line]) -> Vec<bool> {
    let mut frozen = vec![false; lines.len()];
    let mut literal = false;
    let mut directive = false;
    let mut last_text = "";
    for (index, line) in lines.iter().enumerate() {
        let text = line.text.as_str();
        let trimmed = text.trim_start();
        let blank = text.trim().is_empty();
        let indented = is_indented(line);

        if blank {
            // A blank line belongs to whichever run it is inside; at the top
            // level it just separates paragraphs, so it is not frozen.
            frozen[index] = literal || directive;
            continue;
        }
        if literal && indented {
            frozen[index] = true;
            continue;
        }
        literal = false;
        if directive {
            if indented {
                frozen[index] = true;
                continue;
            }
            directive = false;
        }

        if trimmed.starts_with(">>>") || trimmed.starts_with("... ") || trimmed == "..." {
            frozen[index] = true;
            continue;
        }
        if trimmed.starts_with(".. ") {
            frozen[index] = true;
            directive = true;
            last_text = trimmed;
            continue;
        }
        // A literal block opens with `::` and continues while indented.
        if last_text.ends_with("::") && indented {
            literal = true;
            frozen[index] = true;
            continue;
        }
        last_text = trimmed;
    }
    frozen
}

/// Split a prose run into one paragraph per field-list entry (`:param x:`).
fn field_list_blocks(_settings: &Settings, lines: &[Line]) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let mut start = 0usize;
    for i in 1..=lines.len() {
        let is_field = i < lines.len() && is_field_list_entry(&lines[i].text);
        if i == lines.len() || is_field {
            if start < i {
                chunks.extend(plaintext(&lines[start..i]));
            }
            start = i;
        }
    }
    chunks
}

fn is_field_list_entry(text: &str) -> bool {
    let trimmed = text.trim_start();
    let Some(rest) = trimmed.strip_prefix(':') else {
        return false;
    };
    rest.split_once(':')
        .is_some_and(|(name, _)| !name.is_empty() && !name.contains(' '))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> Settings {
        Settings {
            column: 28,
            tab_width: 4,
        }
    }

    fn body(dialect: Dialect, input: &[&str]) -> Vec<String> {
        let lines: Vec<Line> = input.iter().map(|text| Line::new("", *text)).collect();
        let chunks = parse_body(dialect, &settings(), &lines);
        crate::render_chunks(&chunks, &settings(), 28)
    }

    #[test]
    fn a_doctest_is_never_reflowed() {
        let input = [">>> total = sum(alpha, beta, gamma)", ">>> print(total)"];
        assert_eq!(body(Dialect::Docstring, &input), input.to_vec());
    }

    #[test]
    fn a_literal_block_after_a_double_colon_is_never_reflowed() {
        let input = [
            "Example::",
            "",
            "    alpha beta gamma delta epsilon zeta eta",
        ];
        assert_eq!(body(Dialect::Docstring, &input), input.to_vec());
    }

    #[test]
    fn a_directive_and_its_body_are_frozen() {
        let input = [".. note::", "   alpha beta gamma delta epsilon zeta"];
        assert_eq!(body(Dialect::Docstring, &input), input.to_vec());
    }

    #[test]
    fn a_javadoc_tag_starts_its_own_block() {
        let out = body(
            Dialect::Javadoc,
            &["some text here", "@param x the value of x"],
        );
        assert_eq!(out[1], "@param x the value of x", "{out:?}");
    }

    #[test]
    fn an_xmldoc_tag_line_is_frozen() {
        let input = [
            "<summary>",
            "alpha beta gamma delta epsilon zeta",
            "</summary>",
        ];
        let out = body(Dialect::XmlDoc, &input);
        assert_eq!(out[0], "<summary>");
        assert_eq!(
            out.last().map(String::as_str),
            Some("</summary>"),
            "{out:?}"
        );
    }

    #[test]
    fn godoc_leaves_indented_code_alone() {
        let input = ["// Example:", "//", "//   alpha beta gamma delta epsilon"];
        let stripped: Vec<Line> = input
            .iter()
            .map(|text| {
                Line::new(
                    "// ",
                    text.trim_start_matches("// ").trim_start_matches("//"),
                )
            })
            .collect();
        let chunks = parse_body(Dialect::Godoc, &settings(), &stripped);
        let out = crate::render_chunks(&chunks, &settings(), 28);
        assert!(out.iter().any(|line| line.contains("   alpha")), "{out:?}");
    }
}
