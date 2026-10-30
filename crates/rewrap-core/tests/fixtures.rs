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
