//! Fixture-driven tests.
//!
//! A fixture is a text file of cases. Each case names a language, a column and
//! occasionally a tab width, then gives the input lines and the expected output
//! lines. The format is ours and deliberately dumb: a case that fails prints the
//! input, the expected output and what actually came out, side by side.
//!
//! Every language with a data file must have at least one fixture, and that is
//! enforced by `every_language_has_fixtures` below. A language shipped without
//! one is a claim nobody has tested, which is worse than not supporting it.

use std::path::{Path, PathBuf};

use rewrap_core::{File, Settings};

#[derive(Debug)]
struct Case {
    name: String,
    language: String,
    column: i32,
    tab_width: usize,
    input: Vec<String>,
    expected: Vec<String>,
    file: PathBuf,
    line: usize,
}

/// Fixtures live at the repository root, beside `languages/`, because they are
/// a claim about the product rather than a detail of one crate.
fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

/// Parse one fixture file. Kept intentionally strict: a malformed fixture is a
/// test that silently does nothing, which is the worst kind.
fn parse_cases(file: &Path) -> Vec<Case> {
    let text =
        std::fs::read_to_string(file).unwrap_or_else(|e| panic!("read {}: {e}", file.display()));
    let mut cases: Vec<Case> = Vec::new();
    let mut current: Option<(Case, Stage)> = None;
    let mut line_no = 0;
    for raw in text.lines() {
        line_no += 1;
        if raw.trim_start().starts_with('#') && current.is_none() {
            continue;
        }
        if raw.trim() == "[case]" {
            if let Some((case, stage)) = current.take() {
                cases.push(check_complete(case, stage, file));
            }
            current = Some((
                Case {
                    name: String::new(),
                    language: String::new(),
                    column: 0,
                    tab_width: 4,
                    input: Vec::new(),
                    expected: Vec::new(),
                    file: file.to_path_buf(),
                    line: line_no,
                },
                Stage::Settings,
            ));
            continue;
        }
        let Some((case, stage)) = current.as_mut() else {
            if raw.trim().is_empty() {
                continue;
            }
            panic!(
                "{}:{line_no}: content before the first [case]",
                file.display()
            );
        };
        match raw.trim() {
            "-- input" => *stage = Stage::Input,
            "-- output" => *stage = Stage::Output,
            _ => match stage {
                Stage::Settings => {
                    let Some((key, value)) = raw.split_once('=') else {
                        panic!("{}:{line_no}: expected `key = value`", file.display());
                    };
                    let value = value.trim();
                    match key.trim() {
                        "name" => case.name = value.to_string(),
                        "language" => case.language = value.to_string(),
                        "column" => {
                            case.column = value.parse().unwrap_or_else(|_| {
                                panic!("{}:{line_no}: column must be a number", file.display())
                            });
                        }
                        "tab" => {
                            case.tab_width = value.parse().unwrap_or_else(|_| {
                                panic!("{}:{line_no}: tab must be a number", file.display())
                            });
                        }
                        other => panic!("{}:{line_no}: unknown key {other}", file.display()),
                    }
                }
                Stage::Input => case.input.push(raw.to_string()),
                Stage::Output => case.expected.push(raw.to_string()),
            },
        }
    }
    if let Some((case, stage)) = current.take() {
        cases.push(check_complete(case, stage, file));
    }
    cases
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Stage {
    Settings,
    Input,
    Output,
}

fn check_complete(case: Case, stage: Stage, file: &Path) -> Case {
    assert!(
        !case.language.is_empty(),
        "{}:{}: case has no language",
        file.display(),
        case.line
    );
    assert!(
        case.column > 0,
        "{}:{}: case has no column",
        file.display(),
        case.line
    );
    assert_ne!(
        stage,
        Stage::Settings,
        "{}:{}: case has no input",
        file.display(),
        case.line
    );
    assert_ne!(
        stage,
        Stage::Input,
        "{}:{}: case has no expected output",
        file.display(),
        case.line
    );
    case
}

fn run(case: &Case) -> Vec<String> {
    let file = File {
        language: case.language.clone(),
        path: String::new(),
    };
    let settings = Settings {
        column: case.column,
        tab_width: case.tab_width,
    };
    let input: Vec<&str> = case.input.iter().map(String::as_str).collect();
    let edit = rewrap_core::rewrap(&file, &settings, &[], &input);
    if edit.is_empty() {
        case.input.clone()
    } else {
        edit.lines
    }
}

fn all_cases() -> Vec<Case> {
    let mut files: Vec<PathBuf> = Vec::new();
    collect(&fixtures_dir(), &mut files);
    files.sort();
    files.iter().flat_map(|file| parse_cases(file)).collect()
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "case") {
            out.push(path);
        }
    }
}

#[test]
fn every_fixture_case_matches() {
    let cases = all_cases();
    assert!(
        !cases.is_empty(),
        "no fixture cases under {}",
        fixtures_dir().display()
    );
    assert!(
        cases.len() > 50,
        "expected a substantial fixture suite, found {}",
        cases.len()
    );
    let mut failures = Vec::new();
    for case in &cases {
        let got = run(case);
        if got != case.expected {
            failures.push(format!(
                "\n{}:{} {} [{}]\n  input:    {:?}\n  expected: {:?}\n  got:      {:?}",
                case.file.display(),
                case.line,
                case.name,
                case.language,
                case.input,
                case.expected,
                got
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} case(s) failed:{}",
        failures.len(),
        failures.join("")
    );
}

/// No language may ship without fixtures.
///
/// The check is by language *name* because that is what a fixture declares, and
/// the table is the only list of what we claim to support.
#[test]
fn every_language_has_fixtures() {
    let cases = all_cases();
    let covered: std::collections::BTreeSet<&str> =
        cases.iter().map(|case| case.language.as_str()).collect();
    let missing: Vec<&str> = rewrap_core::language_ids()
        .into_iter()
        .filter(|id| !covered.contains(id))
        .collect();
    assert!(missing.is_empty(), "languages with no fixture: {missing:?}");
}

/// Wrapping twice must be the same as wrapping once.
///
/// This is the property that makes the feature safe to run on save and on every
/// keystroke: if a second application kept moving text, a formatter would drift
/// and an auto-wrap would loop.
#[test]
fn wrapping_is_idempotent() {
    for case in all_cases() {
        let first = run(&case);
        let file = File {
            language: case.language.clone(),
            path: String::new(),
        };
        let settings = Settings {
            column: case.column,
            tab_width: case.tab_width,
        };
        let input: Vec<&str> = first.iter().map(String::as_str).collect();
        let second = rewrap_core::rewrap(&file, &settings, &[], &input);
        assert!(
            second.is_empty(),
            "{}:{} {} is not idempotent: {:?} -> {:?} -> {:?}",
            case.file.display(),
            case.line,
            case.name,
            case.input,
            first,
            second.lines
        );
    }
}

/// No word may appear, vanish or change.
///
/// The comparison is over *distinct* words, because a wrap legitimately repeats
/// a comment marker: `// a b` on two lines contains `//` twice and the word
/// `rem` twice for a batch file. Repeats are covered by the idempotence check
/// instead.
///
/// A wrap moves line breaks and repeats comment markers, so a character-level
/// comparison would trip over the markers themselves. Words are the unit that
/// matters: if every word survives, the prose was re-flowed rather than edited.
#[test]
fn wrapping_preserves_every_word() {
    for case in all_cases() {
        let got = run(&case);
        assert_eq!(
            words(&case.input),
            words(&got),
            "{}:{} {} changed the text, not just the line breaks\n  input: {:?}\n  got:   {:?}",
            case.file.display(),
            case.line,
            case.name,
            case.input,
            got
        );
    }
}

fn words(lines: &[String]) -> Vec<String> {
    let mut words: Vec<String> = lines
        .iter()
        .flat_map(|line| {
            line.split(|c: char| !c.is_alphanumeric())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .filter(|word| !word.is_empty())
        .collect();
    words.sort();
    words.dedup();
    words
}

/// A cursor in a comment must wrap the whole comment, not the line under it.
///
/// Reflowing one line out of a multi-line comment is the worst possible result:
/// it leaves a long line above two short ones and calls it formatting.
#[test]
fn a_cursor_wraps_the_enclosing_comment() {
    use rewrap_core::{File, Position, Selection, Settings};
    let lines = [
        "fn main() {",
        "    // alpha beta gamma",
        "    // delta epsilon zeta",
        "    let x = 1;",
        "}",
    ];
    let file = File {
        language: "rust".into(),
        path: "x.rs".into(),
    };
    let settings = Settings {
        column: 30,
        tab_width: 4,
    };
    // Cursor on the second comment line only.
    let cursor = Selection {
        anchor: Position {
            line: 2,
            character: 0,
        },
        active: Position {
            line: 2,
            character: 0,
        },
    };
    let edit = rewrap_core::rewrap(&file, &settings, &[cursor], &lines);
    assert!(!edit.is_empty());
    assert_eq!(
        edit.start_line, 1,
        "the comment starts on the first comment line"
    );
    assert_eq!(edit.end_line, 2);
    assert_eq!(
        edit.lines,
        vec!["    // alpha beta gamma delta", "    // epsilon zeta"]
    );
}

/// A cursor in a line of code wraps nothing at all.
#[test]
fn a_cursor_on_code_does_nothing() {
    use rewrap_core::{File, Position, Selection, Settings};
    let lines = ["let alpha = beta_gamma_delta_epsilon_zeta_eta = 1;"];
    let file = File {
        language: "rust".into(),
        path: "x.rs".into(),
    };
    let settings = Settings {
        column: 20,
        tab_width: 4,
    };
    let cursor = Selection {
        anchor: Position {
            line: 0,
            character: 2,
        },
        active: Position {
            line: 0,
            character: 2,
        },
    };
    let edit = rewrap_core::rewrap(&file, &settings, &[cursor], &lines);
    assert!(edit.is_empty(), "code must be left alone: {:?}", edit.lines);
}

/// The auto-wrap path, which runs on every keystroke.
///
/// It had no tests at all until this file: the feature was described, shipped and
/// then reported broken -- the cursor stayed behind on the shortened line instead
/// of following the word it had just pushed down.
mod auto_wrap {
    use rewrap_core::{maybe_auto_wrap, File, Position, Settings};

    fn settings() -> Settings {
        Settings {
            column: 30,
            tab_width: 4,
        }
    }

    fn rust() -> File {
        File {
            language: "rust".into(),
            path: "x.rs".into(),
        }
    }

    /// Type `text` at `column` of `line`, returning the edit and the cursor.
    fn type_at(
        lines: &[&str],
        line: usize,
        column: usize,
        text: &str,
    ) -> (Vec<String>, Option<(usize, usize)>) {
        let file = rust();
        let edit = maybe_auto_wrap(
            &file,
            &settings(),
            text,
            Position {
                line,
                character: column,
            },
            lines,
        );
        let cursor = edit
            .selections
            .as_ref()
            .and_then(|s| s.first())
            .map(|s| (s.active.line, s.active.character));
        let out = if edit.is_empty() {
            lines.iter().map(|l| (*l).to_string()).collect()
        } else {
            edit.lines.clone()
        };
        (out, cursor)
    }

    #[test]
    fn a_space_that_still_fits_does_nothing() {
        // The line is under the column even with the space typed, so there is
        // nothing to do -- auto-wrap is about overflow, not about tidiness.
        let line = "// alpha beta gamma delta ";
        let (out, cursor) = type_at(&[line], 0, line.len() - 1, " ");
        assert_eq!(out, vec![line]);
        assert_eq!(cursor, None);
    }

    #[test]
    fn a_real_overflow_pushes_a_word_down_and_takes_the_cursor_with_it() {
        let line = "// alpha beta gamma delta epsilon ";
        let (out, cursor) = type_at(&[line], 0, line.len() - 1, " ");
        // The typed space is kept: it is the separator before the next word.
        assert_eq!(out, vec!["// alpha beta gamma delta", "// epsilon "]);
        assert_eq!(
            cursor,
            Some((1, "// epsilon ".len())),
            "cursor must be after the word it pushed down, past the space"
        );
    }

    #[test]
    fn typing_inside_the_column_does_nothing() {
        let line = "// short ";
        let (out, cursor) = type_at(&[line], 0, line.len() - 1, " ");
        assert_eq!(out, vec!["// short "]);
        assert_eq!(cursor, None, "no edit, so the editor keeps its own cursor");
    }

    #[test]
    fn a_one_line_block_comment_wraps_with_its_decoration_and_tracks_the_cursor() {
        let line = "/* alpha beta gamma delta epsilon */";
        let (out, cursor) = type_at(&[line], 0, 34, " ");
        assert_eq!(out, vec!["/* alpha beta gamma delta", " * epsilon */"]);
        // The cursor belongs after "epsilon", on the continuation line -- before
        // the closing marker, not past it.
        assert_eq!(cursor, Some((1, " * epsilon".len())));
    }

    #[test]
    fn a_javadoc_one_liner_tracks_the_cursor_the_same_way() {
        let file = File {
            language: "typescript".into(),
            path: "x.ts".into(),
        };
        let settings = Settings {
            column: 30,
            tab_width: 4,
        };
        let line = "/** alpha beta gamma delta epsilon */";
        let edit = maybe_auto_wrap(
            &file,
            &settings,
            " ",
            Position {
                line: 0,
                character: line.len() - 1,
            },
            &[line],
        );
        assert_eq!(
            edit.lines,
            vec!["/** alpha beta gamma delta", " * epsilon */"]
        );
        let cursor = edit.selections.expect("cursor")[0].active;
        assert_eq!((cursor.line, cursor.character), (1, " * epsilon".len()));
    }

    /// The space that triggers a wrap must survive it.
    ///
    /// Reported from use: typing a space rewrapped and then *dropped* the space,
    /// so the next keystroke welded itself onto the last word.
    #[test]
    fn the_space_that_triggered_the_wrap_is_kept() {
        let line = "// alpha beta gamma delta epsilon ";
        let (out, _) = type_at(&[line], 0, line.len() - 1, " ");
        assert!(
            out.last().is_some_and(|last| last.ends_with(' ')),
            "the typed space vanished: {out:?}"
        );
        let last = out.last().expect("a line").clone();
        let cursor = last.chars().count();
        let after_typing = format!("{last}x");
        assert_eq!(after_typing.chars().nth(cursor), Some('x'));
        assert_eq!(
            after_typing.chars().nth(cursor - 1),
            Some(' '),
            "no space before the new word"
        );
    }

    /// A continuation line inside a block comment carries no marker of its own,
    /// so nothing used to match it and typing did nothing at all.
    #[test]
    fn typing_inside_a_multi_line_block_comment_wraps_the_whole_block() {
        let file = File {
            language: "typescript".into(),
            path: "x.ts".into(),
        };
        let settings = Settings {
            column: 30,
            tab_width: 4,
        };
        let lines = ["/**", " * alpha beta gamma delta epsilon zeta ", " */"];
        let edit = maybe_auto_wrap(
            &file,
            &settings,
            " ",
            Position {
                line: 1,
                character: lines[1].len() - 1,
            },
            &lines,
        );
        assert!(!edit.is_empty(), "typing in a block comment did nothing");
        assert_eq!(
            (edit.start_line, edit.end_line),
            (0, 2),
            "the whole block is the unit, not the typed line"
        );
        assert_eq!(edit.lines[0], "/**", "the opening line is kept");
        assert_eq!(
            edit.lines
                .iter()
                .filter(|line| line.contains("/**"))
                .count(),
            1,
            "still one comment"
        );
        assert!(edit.lines.join("\n").trim_end().ends_with("*/"));
        assert!(
            edit.lines.join(" ").contains("zeta"),
            "the typed word survives: {:?}",
            edit.lines
        );
    }

    /// The line that closes the comment keeps the closing marker on its last
    /// line rather than losing it.
    #[test]
    fn typing_on_the_closing_line_keeps_the_closing_marker() {
        let file = File {
            language: "typescript".into(),
            path: "x.ts".into(),
        };
        let settings = Settings {
            column: 30,
            tab_width: 4,
        };
        let lines = ["/**", " * alpha beta gamma delta epsilon zeta */"];
        let edit = maybe_auto_wrap(
            &file,
            &settings,
            " ",
            Position {
                line: 1,
                character: 35,
            },
            &lines,
        );
        assert!(!edit.is_empty(), "typing on the closing line did nothing");
        let text = edit.lines.join("\n");
        assert!(text.trim_end().ends_with("*/"), "still closed: {text}");
        assert_eq!(text.matches("/**").count(), 1, "still one comment: {text}");
    }

    /// The opening line of a block comment that runs on wraps the whole block:
    /// a comment is the unit, and its body keeps whatever shape it had.
    #[test]
    fn typing_on_an_opening_line_wraps_the_whole_block_and_keeps_it_one_comment() {
        let file = File {
            language: "typescript".into(),
            path: "x.ts".into(),
        };
        let settings = Settings {
            column: 30,
            tab_width: 4,
        };
        let lines = [
            "/** alpha beta gamma delta epsilon zeta and more words here ",
            " * second line of the same comment",
            " */",
        ];
        let edit = maybe_auto_wrap(
            &file,
            &settings,
            " ",
            Position {
                line: 0,
                character: lines[0].len() - 1,
            },
            &lines,
        );
        let text = edit.lines.join("\n");
        assert_eq!(
            text.matches("/**").count(),
            1,
            "still one comment: {:?}",
            edit.lines
        );
        assert!(text.trim_end().ends_with("*/"), "still closed: {text}");
        // Every body line keeps the decoration, so the block is still a block
        // comment rather than prose that happens to have a marker above it.
        for line in &edit.lines[1..] {
            assert!(
                line.starts_with(" * ") || line.trim() == "*/",
                "body lines keep their decoration: {line:?} in {:?}",
                edit.lines
            );
        }
        // And no word was lost in the reflow.
        let words = |text: &str| {
            let mut words: Vec<String> = text
                .split(|c: char| !c.is_alphanumeric())
                .filter(|word| !word.is_empty())
                .map(str::to_string)
                .collect();
            words.sort();
            words.dedup();
            words
        };
        let before = words("/** alpha beta gamma delta epsilon zeta and more words here second line of the same comment */");
        let after = words(&text);
        for word in before {
            assert!(after.contains(&word), "lost the word {word:?} in {text}");
        }
    }

    #[test]
    fn a_paste_is_not_auto_wrapped() {
        let line = "// alpha beta gamma delta epsilon zeta eta theta";
        let (out, cursor) = type_at(&[line], 0, 3, "alpha beta gamma delta epsilon zeta ");
        assert_eq!(out, vec![line], "a paste must not trigger a rewrite");
        assert_eq!(cursor, None);
    }

    #[test]
    fn code_is_never_auto_wrapped() {
        let line = "let alpha = beta_gamma_delta_epsilon_zeta_eta = 1;";
        let (out, cursor) = type_at(&[line], 0, 20, " ");
        assert_eq!(out, vec![line]);
        assert_eq!(cursor, None);
    }

    #[test]
    fn a_typed_newline_keeps_the_cursor_below_the_text_it_pushed_down() {
        // The editor has already split the line: line 0 is the old text, line 1
        // is what follows the newline.
        let lines = ["// alpha beta gamma delta epsilon", ""];
        let file = rust();
        let settings = Settings {
            column: 30,
            tab_width: 4,
        };
        let edit = maybe_auto_wrap(
            &file,
            &settings,
            "\n",
            Position {
                line: 0,
                character: lines[0].len(),
            },
            &lines,
        );
        assert!(!edit.is_empty(), "the first line was past the column");
        let cursor = edit.selections.expect("auto-wrap sets the cursor")[0].active;
        assert_eq!(
            cursor.line,
            edit.lines.len(),
            "cursor belongs below the wrapped lines"
        );
        assert_eq!(cursor.character, 0);
    }
}

/// A block comment must stay *one* block comment.
///
/// Wrapping `/** ... */` must produce a decorated continuation (` * ... */`),
/// never a second `/**`: repeating the opening marker closes the first comment
/// and starts another, which changes what the code means.
mod block_comments_stay_one_block {
    use rewrap_core::{maybe_auto_wrap, File, Position, Settings};

    fn settings(column: i32) -> Settings {
        Settings {
            column,
            tab_width: 4,
        }
    }

    fn typescript() -> File {
        File {
            language: "typescript".into(),
            path: "x.ts".into(),
        }
    }

    #[test]
    fn a_one_line_block_comment_wraps_into_a_decorated_continuation() {
        let lines = ["/** alpha beta gamma delta epsilon zeta */"];
        let edit = maybe_auto_wrap(
            &typescript(),
            &settings(30),
            " ",
            Position {
                line: 0,
                character: 40,
            },
            &lines,
        );
        assert_eq!(
            edit.lines,
            vec!["/** alpha beta gamma delta", " * epsilon zeta */"],
            "a second /** would be a second comment"
        );
        assert_eq!(
            edit.lines
                .iter()
                .filter(|line| line.contains("/**"))
                .count(),
            1,
            "exactly one opening marker"
        );
    }

    #[test]
    fn typing_on_after_the_wrap_still_keeps_one_block() {
        // The state after the wrap above, with more words typed at the end.
        let lines = [
            "/** alpha beta gamma delta",
            " * epsilon zeta eta theta iota kappa */",
        ];
        let edit = maybe_auto_wrap(
            &typescript(),
            &settings(30),
            " ",
            Position {
                line: 1,
                character: 45,
            },
            &lines,
        );
        assert!(!edit.is_empty(), "the line was past the column");
        assert_eq!(
            edit.lines
                .iter()
                .filter(|line| line.contains("/**"))
                .count(),
            1,
            "exactly the one opening line: {:?}",
            edit.lines
        );
        assert!(edit
            .lines
            .last()
            .is_some_and(|line| line.trim_end().ends_with("*/")));
    }

    #[test]
    fn a_rust_block_comment_stays_one_block_too() {
        let rust = File {
            language: "rust".into(),
            path: "x.rs".into(),
        };
        let lines = ["/* alpha beta gamma delta epsilon zeta */"];
        let edit = maybe_auto_wrap(
            &rust,
            &settings(30),
            " ",
            Position {
                line: 0,
                character: 42,
            },
            &lines,
        );
        assert_eq!(
            edit.lines,
            vec!["/* alpha beta gamma delta", " * epsilon zeta */"]
        );
    }
}

#[test]
fn dbg_end2() {
    use rewrap_core::{maybe_auto_wrap, File, Position, Settings};
    let ts = File {
        language: "typescript".into(),
        path: "x.ts".into(),
    };
    let s = Settings {
        column: 30,
        tab_width: 4,
    };
    let typed = " * epsilon zeta  */";
    eprintln!(
        "before */: {:?}",
        maybe_auto_wrap(
            &ts,
            &s,
            " ",
            Position {
                line: 2,
                character: 15
            },
            &["/**", " * alpha beta gamma delta", typed]
        )
        .lines
    );
    let rust = File {
        language: "rust".into(),
        path: "x.rs".into(),
    };
    eprintln!(
        "rust doc: {:?}",
        maybe_auto_wrap(
            &rust,
            &s,
            " ",
            Position {
                line: 1,
                character: 22
            },
            &[
                "/// alpha beta gamma delta",
                "/// epsilon zeta epsilon",
                "fn main() {}"
            ]
        )
        .lines
    );
}

/// The last line of a block comment is where people finish a sentence.
///
/// It was the one line typing did nothing in: it carries no opening marker, and
/// its own `*/` balanced the marker above, so the enclosing search decided the
/// comment was already closed.
mod typing_on_the_last_line_of_a_block_comment {
    use rewrap_core::{maybe_auto_wrap, File, Position, Settings};

    fn settings() -> Settings {
        Settings {
            column: 30,
            tab_width: 4,
        }
    }

    fn typescript() -> File {
        File {
            language: "typescript".into(),
            path: "x.ts".into(),
        }
    }

    fn sample() -> [&'static str; 3] {
        [
            "/**",
            " * alpha beta gamma delta",
            " * epsilon zeta theta kappa lambda  */",
        ]
    }

    #[test]
    fn typing_before_the_closing_marker_wraps() {
        let lines = sample();
        let edit = maybe_auto_wrap(
            &typescript(),
            &settings(),
            " ",
            Position {
                line: 2,
                character: lines[2].len() - 3,
            },
            &lines,
        );
        assert!(!edit.is_empty(), "typing on the last line did nothing");
        let text = edit.lines.join("\n");
        assert!(
            text.trim_end().ends_with("*/"),
            "the closing marker stays last: {text}"
        );
        assert_eq!(text.matches("/**").count(), 1, "still one comment: {text}");
        assert!(text.contains("lambda"), "the last word survives: {text}");
    }

    #[test]
    fn typing_after_the_closing_marker_wraps_the_same_way() {
        let lines = sample();
        let edit = maybe_auto_wrap(
            &typescript(),
            &settings(),
            " ",
            Position {
                line: 2,
                character: lines[2].len(),
            },
            &lines,
        );
        assert!(!edit.is_empty(), "typing after */ did nothing");
        let text = edit.lines.join("\n");
        assert!(text.trim_end().ends_with("*/"), "still closed: {text}");
        assert_eq!(text.matches("/**").count(), 1, "still one comment: {text}");
        assert!(text.contains("lambda"), "the word survived: {text}");
    }

    #[test]
    fn the_cursor_follows_onto_the_new_line() {
        let lines = sample();
        let edit = maybe_auto_wrap(
            &typescript(),
            &settings(),
            " ",
            Position {
                line: 2,
                character: lines[2].len() - 3,
            },
            &lines,
        );
        let cursor = edit.selections.expect("cursor")[0].active;
        assert_eq!(cursor.line, 3, "one line below where it was typed");
        assert!(cursor.character > 0);
    }
}

/// A comment is wrapped as a block, not as the line under the cursor.
///
/// Reported from use: a hanging indent (`#    `) collapsed to `# ` when a line
/// in the middle of a block was typed in, because the wrap invented the
/// continuation prefix from that one line.
mod the_comment_is_the_unit {
    use rewrap_core::{maybe_auto_wrap, File, Position, Settings};

    fn settings() -> Settings {
        Settings {
            column: 34,
            tab_width: 4,
        }
    }

    fn python() -> File {
        File {
            language: "python".into(),
            path: "x.py".into(),
        }
    }

    fn block() -> Vec<&'static str> {
        vec![
            "# 2. maybe_auto_wrap has no",
            "#    automated tests at all. I",
            "#    checked: zero tests and",
        ]
    }

    #[test]
    fn typing_at_the_end_of_a_hanging_indent_keeps_the_indent() {
        let mut lines = block();
        lines[2] = "#    checked: zero tests and something";
        let edit = maybe_auto_wrap(
            &python(),
            &settings(),
            " ",
            Position {
                line: 2,
                character: "#    checked: zero tests and something".len() - 1,
            },
            &lines[..2]
                .iter()
                .copied()
                .chain([lines[2]])
                .collect::<Vec<_>>(),
        );
        assert!(!edit.is_empty(), "typing should have wrapped");
        for line in &edit.lines {
            assert!(
                line.starts_with("# ") || line.starts_with("#    "),
                "a continuation line lost the marker: {line:?} in {:?}",
                edit.lines
            );
        }
        let continuation: Vec<&String> = edit
            .lines
            .iter()
            .filter(|line| line.starts_with("#    "))
            .collect();
        assert!(
            continuation.len() >= 2,
            "the hanging indent was not preserved: {:?}",
            edit.lines
        );
    }

    #[test]
    fn the_cursor_lands_after_the_word_it_pushed_down() {
        let lines = [
            "# 2. maybe_auto_wrap has no",
            "#    automated tests at all. I",
            "#    checked: zero tests and something",
        ];
        let edit = maybe_auto_wrap(
            &python(),
            &Settings {
                column: 34,
                tab_width: 4,
            },
            " ",
            Position {
                line: 2,
                character: 38,
            },
            &lines,
        );
        let cursor = edit.selections.expect("cursor")[0].active;
        let line = &edit.lines[cursor.line as usize];
        assert_eq!(
            line.chars().take(cursor.character).collect::<String>(),
            *line,
            "the cursor should be at the end of its line"
        );
        assert!(
            line.contains("something"),
            "cursor moved to the wrong line: {line:?}"
        );
    }

    #[test]
    fn a_single_line_comment_still_wraps_on_its_own() {
        // The document already holds the typed space, as the editor reports it.
        let lines = ["# alpha beta gamma delta epsilon zeta eta theta "];
        let edit = maybe_auto_wrap(
            &python(),
            &settings(),
            " ",
            Position {
                line: 0,
                character: lines[0].len() - 1,
            },
            &lines,
        );
        assert_eq!(
            edit.lines,
            vec!["# alpha beta gamma delta epsilon", "# zeta eta theta "]
        );
        let cursor = edit.selections.expect("cursor")[0].active;
        assert_eq!(cursor.line, 1);
    }
}
