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
        // Note: *not* `markdown::parse`. That is the whole-document classifier,
        // and inside a comment it is wrong in a way that damages files: it
        // re-reads the marker-stripped text as block Markdown, so a `# - item`
        // list collapses into the paragraph above it, and the `#` of the next
        // line ends up in the middle of a sentence. A comment's block structure
        // is its markers, not its punctuation, so a comment body is paragraphs
        // separated by blank lines -- with the decorator prefixes that are
        // still meaningful (a list marker, a quote marker, an ordered item)
        // carried onto continuation lines.
        Dialect::Markdown => comment_text(settings, lines),
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
    // A whole file *is* Markdown, so it gets the block classifier: headings,
    // tables, fenced code, front matter. A comment body does not, and the
    // difference matters -- routing a document through `parse_body` made every
    // fenced block in a `.md` file get re-flowed.
    match dialect {
        Dialect::Markdown => crate::markdown::parse(settings, lines),
        other => parse_body(other, settings, lines),
    }
}

/// A comment body: paragraphs of text with their own prefixes kept.
///
/// Two prefixes per paragraph. `first` is the prefix of the paragraph's first
/// line, so a `- item` keeps its marker and a `> quote` keeps its quote; `cont`
/// is that prefix with the marker's own characters replaced by spaces, so a
/// continuation line lines up under the text instead of under the marker.
///
/// A line must not *begin* a block, though: a fenced code sample or a table
/// inside a comment is code the author wrote, and re-flowing it is not cosmetic
/// damage. Those lines are emitted unchanged and are never part of a paragraph.
fn comment_text(settings: &Settings, lines: &[Line]) -> Vec<Chunk> {
    comment_text_opt(settings, lines, true)
}

/// As [`comment_text`], but with a say over whether indentation means code.
///
/// In most comments it does: four spaces past the marker is a sample, and a
/// sample must not be re-flowed. Inside a docstring it does not -- an indented
/// line there is usually a hanging indent continuing the sentence above -- and
/// the literal blocks that *are* code are the ones the guards already found
/// (after `::`, under a directive).
fn comment_text_opt(settings: &Settings, lines: &[Line], indent_is_code: bool) -> Vec<Chunk> {
    let _ = settings;
    let mut chunks = Vec::new();
    let mut i = 0usize;
    while i < lines.len() {
        if lines[i].text.trim().is_empty() {
            chunks.push(Chunk::Verbatim(vec![lines[i]
                .prefix
                .trim_end()
                .to_string()]));
            i += 1;
            continue;
        }
        if crate::markdown::opens_a_block(&lines[i].text)
            && (indent_is_code || !crate::markdown::is_indented_code(&lines[i].text))
        {
            // A fenced sample runs to its closing fence; any other block is just
            // the lines it is made of. Either way it is emitted as written.
            let start = i;
            let fence = crate::markdown::fence_of(&lines[i].text);
            i += 1;
            while i < lines.len() && !lines[i].text.trim().is_empty() {
                if let Some(closer) = fence {
                    let trimmed = lines[i].text.trim();
                    if trimmed.len() >= closer.1 && trimmed.chars().all(|c| c == closer.0) {
                        i += 1;
                        break;
                    }
                } else if crate::markdown::starts_a_block(&lines[i].text) {
                    break;
                }
                i += 1;
            }
            chunks.push(Chunk::Verbatim(
                lines[start..i].iter().map(Line::full).collect(),
            ));
            continue;
        }
        let prefix = lines[i].prefix.clone();
        let text = lines[i].text.clone();
        // The leading whitespace of the first line is part of the paragraph: it
        // is what an indented docstring line is indented by, and what the
        // continuation lines line up with.
        let leading = text.len() - text.trim_start().len();
        let marker = leading_marker(&text[leading..]);
        let cont_indent = leading + marker.chars().count();
        let first = format!("{prefix}{}", " ".repeat(leading));
        let cont = format!("{prefix}{}", " ".repeat(cont_indent));
        let mut para = Para::new(first, cont);
        // The marker travels with the text: a `- ` or a `> ` is the paragraph's
        // own, and repeating only the indent would delete it from the output.
        para.push(text.get(leading..).unwrap_or(""));
        // Continuation lines line up at the deepest indent this paragraph uses:
        // the first line's text offset, or further in when a following line is
        // indented under it (a hanging indent).
        let mut cont_indent = cont_indent;
        i += 1;
        while i < lines.len() && !lines[i].text.trim().is_empty() {
            if crate::markdown::opens_a_block(&lines[i].text)
                || !leading_marker(&lines[i].text).trim().is_empty()
            {
                break;
            }
            // A hanging indent is the shape a definition or an explanation takes:
            // a short first line with the rest indented under the text. New lines
            // belong at the deepest indent the paragraph already uses, not at the
            // first line's.
            let line = &lines[i];
            let leading_here = line.text.len() - line.text.trim_start().len();
            cont_indent = cont_indent.max(leading_here);
            para.push(line.text.trim());
            i += 1;
        }
        para.cont = format!("{prefix}{}", " ".repeat(cont_indent));
        chunks.push(Chunk::Para(para));
    }
    chunks
}

/// The prefix a paragraph's continuation lines align under.
///
/// Only the markers that mean something inside prose: a bullet, an ordered item
/// (`1.`), a block quote. Anything else starts flush with the text, which is what
/// a wrapped comment should look like.
fn leading_marker(text: &str) -> String {
    let trimmed = text.trim_start();
    let indent = &text[..text.len() - trimmed.len()];
    let mut rest = trimmed;
    let mut marker = String::new();
    // Nested quotes: `> > text`.
    while let Some(after) = rest.strip_prefix('>') {
        marker.push_str("> ");
        rest = after.strip_prefix(' ').unwrap_or(after);
    }
    // A bullet. The spaces after it are part of the marker: `-   text` aligns
    // under `text`, and one space would put every continuation a column short.
    for bullet in ['-', '*', '+'] {
        if let Some(after) = rest.strip_prefix(bullet) {
            let spaces = after.len() - after.trim_start().len();
            if spaces > 0 {
                return format!("{indent}{marker}{bullet}{}", " ".repeat(spaces));
            }
        }
    }
    // An ordered item: the digits, the `.` or `)`, and its spaces.
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if !digits.is_empty() {
        let tail = &rest[digits.len()..];
        if let Some(sep) = tail.chars().next().filter(|c| matches!(c, '.' | ')')) {
            let after = &tail[1..];
            let spaces = after.len() - after.trim_start().len();
            if spaces > 0 {
                return format!("{indent}{marker}{digits}{sep}{}", " ".repeat(spaces));
            }
        }
    }
    format!("{indent}{marker}")
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
            chunks.extend(comment_text(settings, &lines[start..i]));
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
    // "Indented" means indented past the body's own baseline, which is the
    // indent of its first non-blank line. Absolute indentation is wrong here:
    // the string's own indent is already in every line's prefix, so a body at
    // column 4 and a literal block at column 8 differ only by their prefixes.
    //
    // Only the whitespace of a prefix counts: the first body line's prefix ends
    // in the opening quotes, which are delimiter, not indentation.
    let depth = |line: &Line| {
        let prefix = line.prefix.len() - line.prefix.trim_start().len();
        let text = line.text.len() - line.text.trim_start().len();
        prefix + text
    };
    let baseline = lines
        .iter()
        .find(|line| !line.text.trim().is_empty())
        .map_or(0, depth);
    let indented = |line: &Line| depth(line) > baseline;
    let mut frozen = vec![false; lines.len()];
    let mut literal = false;
    let mut directive = false;
    let mut last_text = "";
    for (index, line) in lines.iter().enumerate() {
        let text = line.text.as_str();
        let trimmed = text.trim_start();
        let blank = text.trim().is_empty();
        let indented = indented(line);

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
fn field_list_blocks(settings: &Settings, lines: &[Line]) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let mut start = 0usize;
    for i in 1..=lines.len() {
        let is_field = i < lines.len() && is_field_list_entry(&lines[i].text);
        if i == lines.len() || is_field {
            if start < i {
                // A docstring's prose is prose even when indented; the literal
                // blocks and directives were already frozen above.
                chunks.extend(comment_text_opt(settings, &lines[start..i], false));
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
