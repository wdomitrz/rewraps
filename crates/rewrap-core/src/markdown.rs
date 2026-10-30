//! The Markdown (and plain-text) block classifier.
//!
//! This is not a Markdown parser, and deliberately so. Wrapping only needs to
//! answer one question per line: *may this line's text be re-flowed?* Inline
//! syntax is irrelevant, because a soft line break inside `**bold**` renders as
//! a space and breaking there is harmless. What must not be re-flowed is
//! anything whose line structure is load-bearing: code, tables, front matter,
//! headings, HTML, link definitions.
//!
//! So every construct here is either recognised and frozen, or falls through to
//! "paragraph". An unrecognised construct is therefore never damaged -- the
//! failure mode is that it gets wrapped as prose, which is visible and
//! undoable, not silent corruption.

use crate::{Chunk, Line, Para, Settings};

/// Parse lines into chunks.
pub(crate) fn parse(settings: &Settings, lines: &[Line]) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let mut i = 0usize;
    while i < lines.len() {
        let line = &lines[i];
        if line.is_blank() || line.text.trim().is_empty() {
            chunks.push(Chunk::Verbatim(vec![line.full()]));
            i += 1;
            continue;
        }

        // Front matter is only front matter on the first line. Anywhere else
        // `---` is a thematic break or a setext underline, and treating it as a
        // header would freeze the rest of the file.
        if i == 0 && is_front_matter_delimiter(&line.text) {
            let end = lines
                .iter()
                .enumerate()
                .skip(1)
                .find(|(_, l)| is_front_matter_delimiter(&l.text))
                .map_or(lines.len(), |(index, _)| index + 1);
            chunks.push(verbatim(lines, i, end));
            i = end;
            continue;
        }

        if let Some(fence) = fence(&line.text) {
            let end = closing_fence(lines, i, fence);
            chunks.push(verbatim(lines, i, end));
            i = end;
            continue;
        }

        if is_atx_heading(&line.text) || is_thematic_break(&line.text) {
            chunks.push(Chunk::Verbatim(vec![line.full()]));
            i += 1;
            continue;
        }

        if let Some(kind) = html_block(&line.text) {
            let end = html_end(lines, i, kind);
            chunks.push(verbatim(lines, i, end));
            i = end;
            continue;
        }

        if is_link_reference(&line.text) && !is_previous_paragraph(&chunks) {
            chunks.push(Chunk::Verbatim(vec![line.full()]));
            i += 1;
            continue;
        }

        // Indented code is four spaces past the container. It cannot start
        // mid-paragraph, which is why the check comes after the paragraph case
        // has already had a chance to claim the line.
        if is_indented_code(&line.text) && !is_previous_paragraph(&chunks) {
            chunks.push(Chunk::Verbatim(vec![line.full()]));
            i += 1;
            continue;
        }

        if let Some(end) = table_end(lines, i) {
            chunks.push(verbatim(lines, i, end));
            i = end;
            continue;
        }

        let (chunks_added, next) = paragraph(settings, lines, i);
        chunks.extend(chunks_added);
        i = next;
    }
    chunks
}

fn verbatim(lines: &[Line], start: usize, end: usize) -> Chunk {
    Chunk::Verbatim(lines[start..end].iter().map(Line::full).collect())
}

fn is_previous_paragraph(chunks: &[Chunk]) -> bool {
    // A link definition or an indented block directly after prose belongs to
    // that prose: `[text]` then `[text]: url` is a paragraph in Markdown, and
    // four spaces after a paragraph line is a continuation, not code.
    matches!(chunks.last(), Some(Chunk::Para(_)))
}

/// Gather one paragraph, or a frozen block that only a paragraph context can
/// classify (a setext heading).
fn paragraph(settings: &Settings, lines: &[Line], start: usize) -> (Vec<Chunk>, usize) {
    let (quotes, rest) = split_quotes(&lines[start].text);
    let base = lines[start].prefix.clone();
    let quotes_prefix = "> ".repeat(quotes);

    // A list item's marker and its following spaces belong to the prefix of the
    // item's text, so that continuation lines line up under the text rather
    // than under the marker.
    let (indent, marker) =
        list_marker(rest).unwrap_or_else(|| (leading_whitespace(rest), String::new()));
    let content = rest.get(indent.len() + marker.len()..).unwrap_or("");
    let marker_width = settings.width(0, &format!("{indent}{marker}"));
    let first = format!("{base}{quotes_prefix}{indent}{marker}");
    // Continuation keeps the quote markers -- dropping them would end the
    // block -- and pads to where the text starts.
    let cont = format!("{base}{quotes_prefix}{}", " ".repeat(marker_width));

    let mut para = Para::new(first.clone(), cont.clone());
    para.push(content);
    let mut cont_override: Option<String> = None;

    let mut source = vec![lines[start].full()];
    let mut i = start + 1;
    let mut heading = false;
    while i < lines.len() {
        let line = &lines[i];
        if is_blank(line) || line.text.trim().is_empty() {
            break;
        }
        if is_setext_underline(&line.text) {
            heading = true;
            source.push(line.full());
            i += 1;
            break;
        }
        if interrupts_paragraph(&line.text) {
            break;
        }
        let (line_quotes, line_rest) = split_quotes(&line.text);
        if line_quotes != quotes && line_quotes != quotes + 1 {
            // A different quote depth is a different block. `> a` then `b` is a
            // lazy continuation in Markdown, but treating it as the same
            // paragraph would pull a following code line into prose.
            break;
        }
        source.push(line.full());
        if cont_override.is_none() && para.parts.len() == 1 && line.prefix != lines[start].prefix {
            // The lines of this paragraph do not share a prefix, so the block
            // supplies its own continuation (`/* first` then ` * rest`). Taking
            // it from the second line is the only way to reproduce that shape:
            // deriving it from the first would emit `/*` again.
            cont_override = Some(line.prefix.clone());
        }
        para.push(line_rest.trim());
        i += 1;
    }

    if heading {
        // A setext heading's text is not prose: its line break is part of the
        // heading, so the whole thing is frozen.
        return (vec![Chunk::Verbatim(source)], i);
    }
    if let Some(cont) = cont_override {
        para.cont = cont;
    }
    (vec![Chunk::Para(para)], i)
}

fn is_blank(line: &Line) -> bool {
    line.text.trim().is_empty()
}

/// Whether a construct ends the paragraph above it.
fn interrupts_paragraph(text: &str) -> bool {
    let (_, rest) = split_quotes(text);
    let rest = rest.trim_start();
    fence(text).is_some()
        || is_atx_heading(rest)
        || is_thematic_break(rest)
        || html_block(text).is_some()
        || list_marker(rest).is_some()
        || is_indented_code(text)
        || rest.starts_with('>')
}

/// Strip `>` quote markers, returning how many there were.
pub(crate) fn split_quotes(text: &str) -> (usize, &str) {
    let mut count = 0;
    let mut rest = text;
    loop {
        let trimmed = rest.trim_start();
        let Some(after) = trimmed.strip_prefix('>') else {
            break;
        };
        count += 1;
        rest = after.strip_prefix(' ').unwrap_or(after);
    }
    (count, rest)
}

pub(crate) fn leading_whitespace(text: &str) -> String {
    text.chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect()
}

/// A list marker at the start of `text`, as (indent, marker).
fn list_marker(text: &str) -> Option<(String, String)> {
    let indent = leading_whitespace(text);
    let rest = &text[indent.len()..];
    let mut marker_len = 0;
    let mut chars = rest.char_indices();
    match chars.next() {
        Some((_, '-' | '*' | '+')) => marker_len = 1,
        Some((index, c)) if c.is_ascii_digit() => {
            let mut digits = index + 1;
            for (offset, next) in rest.char_indices().skip(1) {
                if next.is_ascii_digit() {
                    digits = offset + 1;
                } else if matches!(next, '.' | ')') && offset == digits {
                    marker_len = offset + 1;
                    break;
                } else {
                    return None;
                }
            }
        }
        _ => return None,
    }
    if marker_len == 0 {
        return None;
    }
    let after = &rest[marker_len..];
    let spaces = leading_whitespace(after);
    // A marker with no following space is not a list item (`-1` is a number)
    // unless it is the whole line.
    if spaces.is_empty() && !after.is_empty() {
        return None;
    }
    Some((indent, rest[..marker_len + spaces.len()].to_string()))
}

fn fence(text: &str) -> Option<(char, usize)> {
    let trimmed = text.trim_start();
    for marker in ['`', '~'] {
        let count = trimmed.chars().take_while(|c| *c == marker).count();
        if count >= 3 {
            return Some((marker, count));
        }
    }
    None
}

fn closing_fence(lines: &[Line], start: usize, (marker, count): (char, usize)) -> usize {
    for (offset, line) in lines.iter().enumerate().skip(start + 1) {
        let trimmed = line.text.trim();
        if trimmed.len() >= count && trimmed.chars().all(|c| c == marker) {
            return offset + 1;
        }
    }
    // An unclosed fence runs to the end of the block: Markdown says so, and it
    // is also the safe answer, because everything after it is inside code.
    lines.len()
}

fn is_front_matter_delimiter(text: &str) -> bool {
    let trimmed = text.trim_end();
    trimmed == "---" || trimmed == "..."
}

fn is_atx_heading(text: &str) -> bool {
    let trimmed = text.trim_start();
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    (1..=6).contains(&hashes) && trimmed[hashes..].starts_with([' ', '\t'])
}

fn is_thematic_break(text: &str) -> bool {
    let trimmed = text.trim();
    for marker in ['*', '-', '_'] {
        if trimmed.len() >= 3 && trimmed.chars().all(|c| c == marker) {
            return true;
        }
    }
    false
}

fn is_setext_underline(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }
    let first = trimmed.chars().next().unwrap_or(' ');
    matches!(first, '=' | '-') && trimmed.chars().all(|c| c == first)
}

fn is_indented_code(text: &str) -> bool {
    let (_, rest) = split_quotes(text);
    rest.starts_with("    ") || rest.starts_with('\t')
}

fn is_link_reference(text: &str) -> bool {
    let trimmed = text.trim_start();
    let Some(rest) = trimmed.strip_prefix('[') else {
        return false;
    };
    // `[label]: destination`
    rest.split_once("]:")
        .is_some_and(|(label, _)| !label.is_empty() && !label.contains('['))
}

/// Whether a line holds a pipe that is not escaped.
fn has_pipe(text: &str) -> bool {
    let mut escaped = false;
    for c in text.chars() {
        match c {
            '\\' if !escaped => escaped = true,
            '|' if !escaped => return true,
            _ => escaped = false,
        }
    }
    false
}

fn is_table_delimiter_row(text: &str) -> bool {
    let trimmed = text.trim();
    !trimmed.is_empty()
        && trimmed.contains('|')
        && trimmed.contains('-')
        && trimmed
            .chars()
            .all(|c| matches!(c, '|' | ':' | '-' | ' ' | '\t'))
}

/// The end of a table starting at `start`, if there is one.
///
/// The rule is deliberately permissive -- two or more lines with a pipe and one
/// delimiter row -- because table syntax differs between Markdown flavours and
/// the cost of treating a table as prose (it gets destroyed) is much higher than
/// the cost of treating two lines of prose as a table (it does not wrap).
fn table_end(lines: &[Line], start: usize) -> Option<usize> {
    if !has_pipe(&lines[start].text) {
        return None;
    }
    let mut end = start + 1;
    while end < lines.len() && !lines[end].text.trim().is_empty() && has_pipe(&lines[end].text) {
        end += 1;
    }
    if end - start < 2 {
        return None;
    }
    let delimiter = lines[start..end]
        .iter()
        .any(|line| is_table_delimiter_row(&line.text));
    delimiter.then_some(end)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HtmlBlock {
    /// Ends at a blank line.
    UntilBlank,
    /// Ends at a specific closing tag.
    UntilTag(&'static str),
}

fn html_block(text: &str) -> Option<HtmlBlock> {
    let (_, rest) = split_quotes(text);
    let trimmed = rest.trim_start();
    if !trimmed.starts_with('<') {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("<!--") {
        return Some(HtmlBlock::UntilTag("-->"));
    }
    if lower.starts_with("<![cdata[") {
        return Some(HtmlBlock::UntilTag("]]>"));
    }
    for tag in ["script", "pre", "style", "textarea"] {
        if lower.starts_with(&format!("<{tag}")) {
            return Some(HtmlBlock::UntilTag("</"));
        }
    }
    if lower.starts_with("<?") || lower.starts_with("<!") {
        return Some(HtmlBlock::UntilBlank);
    }
    // A block-level tag alone on its line, or an opening tag that is not
    // closed on the same line, starts an HTML block.
    const BLOCK: &[&str] = &[
        "address",
        "article",
        "aside",
        "blockquote",
        "details",
        "div",
        "dl",
        "fieldset",
        "figcaption",
        "figure",
        "footer",
        "form",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "header",
        "hr",
        "main",
        "nav",
        "ol",
        "p",
        "section",
        "table",
        "ul",
    ];
    let name: String = lower
        .trim_start_matches("</")
        .trim_start_matches('<')
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect();
    BLOCK
        .contains(&name.as_str())
        .then_some(HtmlBlock::UntilBlank)
}

fn html_end(lines: &[Line], start: usize, kind: HtmlBlock) -> usize {
    match kind {
        HtmlBlock::UntilTag(tag) => {
            for (offset, line) in lines.iter().enumerate().skip(start + 1) {
                if line.text.to_ascii_lowercase().contains(tag) {
                    return offset + 1;
                }
            }
            lines.len()
        }
        HtmlBlock::UntilBlank => {
            for (offset, line) in lines.iter().enumerate().skip(start + 1) {
                if line.text.trim().is_empty() {
                    return offset;
                }
            }
            lines.len()
        }
    }
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

    fn parse_text(input: &[&str]) -> Vec<String> {
        let lines: Vec<Line> = input.iter().map(|text| Line::new("", *text)).collect();
        crate::render_chunks(&parse(&settings(), &lines), &settings(), 20)
    }

    #[test]
    fn quotes_and_lists_become_prefixes() {
        assert_eq!(
            parse_text(&["> - alpha beta gamma delta"]),
            vec!["> - alpha beta gamma", ">   delta"]
        );
    }

    #[test]
    fn a_setext_heading_is_frozen() {
        let input = ["alpha beta gamma delta", "==="];
        assert_eq!(parse_text(&input), input.to_vec());
    }

    #[test]
    fn a_table_is_frozen() {
        let input = ["| alpha | beta gamma delta epsilon |", "| --- | --- |"];
        assert_eq!(parse_text(&input), input.to_vec());
    }

    #[test]
    fn an_unclosed_fence_freezes_the_rest() {
        let input = ["```", "alpha beta gamma delta epsilon"];
        assert_eq!(parse_text(&input), input.to_vec());
    }

    #[test]
    fn front_matter_only_counts_on_the_first_line() {
        let input = ["text", "", "---", "alpha beta gamma delta epsilon"];
        assert_ne!(
            parse_text(&input),
            input.to_vec(),
            "a later --- is not front matter"
        );
    }

    #[test]
    fn a_list_continuation_lines_up_under_the_text() {
        assert_eq!(
            parse_text(&["- alpha beta gamma delta"]),
            vec!["- alpha beta gamma", "  delta"]
        );
    }

    #[test]
    fn an_ordered_marker_widens_the_continuation_indent() {
        assert_eq!(
            parse_text(&["10. alpha beta gamma delta"]),
            vec!["10. alpha beta gamma", "    delta"]
        );
    }

    #[test]
    fn a_link_definition_after_prose_stays_with_the_paragraph() {
        // `[text]` then `[text]: url` is one paragraph, so it wraps as prose.
        let out = parse_text(&["alpha beta", "[x]: /url"]);
        assert_eq!(out.join(" "), "alpha beta [x]: /url");
    }
}
