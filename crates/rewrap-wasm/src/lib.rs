//! The engine as a wasm module.
//!
//! The boundary is deliberately small and allocation-light: arrays of strings in,
//! one plain object out. No JSON, because the module runs on every keystroke and
//! a serializer would be both slower and a dependency.
//!
//! Everything the editor needs is already in the core: this file only translates
//! between JavaScript values and the engine's types.

use wasm_bindgen::prelude::*;

use rewrap_core::{Edit, File, Position, Selection, Settings};

fn settings(column: i32, tab_width: usize) -> Settings {
    Settings { column, tab_width }
}

fn to_strings(lines: js_sys::Array) -> Vec<String> {
    lines
        .iter()
        .map(|line| line.as_string().unwrap_or_default())
        .collect()
}

fn to_selections(raw: js_sys::Array) -> Vec<Selection> {
    // Flat quadruples: anchorLine, anchorCharacter, activeLine, activeCharacter.
    let numbers: Vec<usize> = raw
        .iter()
        .map(|value| value.as_f64().map_or(0, |n| n.max(0.0) as usize))
        .collect();
    numbers
        .as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| Selection {
            anchor: Position {
                line: chunk[0],
                character: chunk[1],
            },
            active: Position {
                line: chunk[2],
                character: chunk[3],
            },
        })
        .collect()
}

fn edit_to_js(edit: &Edit) -> js_sys::Object {
    let object = js_sys::Object::new();
    let _ = js_sys::Reflect::set(
        &object,
        &"startLine".into(),
        &JsValue::from_f64(edit.start_line as f64),
    );
    let _ = js_sys::Reflect::set(
        &object,
        &"endLine".into(),
        &JsValue::from_f64(edit.end_line as f64),
    );
    let lines = js_sys::Array::new();
    for line in &edit.lines {
        lines.push(&JsValue::from_str(line));
    }
    let _ = js_sys::Reflect::set(&object, &"lines".into(), &lines);
    let _ = js_sys::Reflect::set(
        &object,
        &"isEmpty".into(),
        &JsValue::from_bool(edit.is_empty()),
    );
    let selections = js_sys::Array::new();
    if let Some(mapped) = &edit.selections {
        for selection in mapped {
            for value in [
                selection.anchor.line,
                selection.anchor.character,
                selection.active.line,
                selection.active.character,
            ] {
                selections.push(&JsValue::from_f64(value as f64));
            }
        }
        let _ = js_sys::Reflect::set(&object, &"selections".into(), &selections);
    } else {
        let _ = js_sys::Reflect::set(&object, &"selections".into(), &JsValue::NULL);
    }
    object
}

/// Wrap `lines`. Empty `selections` means the whole document.
#[wasm_bindgen]
#[must_use]
pub fn rewrap(
    language: &str,
    path: &str,
    column: i32,
    tab_width: usize,
    lines: js_sys::Array,
    selections: js_sys::Array,
) -> js_sys::Object {
    let lines = to_strings(lines);
    let borrowed: Vec<&str> = lines.iter().map(String::as_str).collect();
    let selections = to_selections(selections);
    let file = File {
        language: language.to_string(),
        path: path.to_string(),
    };
    let edit = rewrap_core::rewrap(&file, &settings(column, tab_width), &selections, &borrowed);
    edit_to_js(&edit)
}

/// The edit for typing `inserted` at a position, if one is warranted.
///
/// Eight arguments because that is exactly what the editor has at the moment a
/// character arrives, and bundling them into an object would mean allocating
/// one on every keystroke.
#[wasm_bindgen]
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn maybe_auto_wrap(
    language: &str,
    path: &str,
    column: i32,
    tab_width: usize,
    inserted: &str,
    line: usize,
    character: usize,
    lines: js_sys::Array,
) -> js_sys::Object {
    let lines = to_strings(lines);
    let borrowed: Vec<&str> = lines.iter().map(String::as_str).collect();
    let file = File {
        language: language.to_string(),
        path: path.to_string(),
    };
    let edit = rewrap_core::maybe_auto_wrap(
        &file,
        &settings(column, tab_width),
        inserted,
        Position { line, character },
        &borrowed,
    );
    edit_to_js(&edit)
}

/// Every language name, so the editor and the README can agree on the list.
#[wasm_bindgen]
#[must_use]
pub fn languages() -> js_sys::Array {
    let array = js_sys::Array::new();
    for name in rewrap_core::languages() {
        array.push(&JsValue::from_str(name));
    }
    array
}

/// Where to wrap, given the ruler lists the editor has for this file.
///
/// The editor's settings are per language and override global ones, and the
/// engine has no business knowing that: it is handed the already-resolved list
/// and returns the column, or `-1` when there is nothing to wrap at and the
/// caller should ask the user.
#[wasm_bindgen]
#[must_use]
pub fn resolve_column(rulers: js_sys::Array) -> i32 {
    rulers.get(0).as_f64().map_or(-1, |first| first as i32)
}
