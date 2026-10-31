// A stand-in for the `vscode` module, so the extension can be driven without an
// editor.
//
// It behaves like the real host where the extension depends on it: applying an
// edit fires onDidChangeTextDocument *before* the edit promise resolves (the
// window a re-entrancy bug lives in), and `edit` receives and records the
// undo-stop options it is given.
//
// This exists because the extension layer had no tests at all, and an API that
// does not exist (`editor.setSelections`) reached a release because of it.

const noop = () => ({dispose() {}})

class Position {
  constructor (line, character) {
    this.line = line
    this.character = character
  }
}

class Selection {
  constructor (anchorLine, anchorCharacter, activeLine, activeCharacter) {
    this.anchor = new Position(anchorLine, anchorCharacter)
    this.active = new Position(activeLine, activeCharacter)
  }

  // The real Selection computes this; the extension uses it to tell typing from
  // a selection being replaced.
  get isEmpty () {
    return this.anchor.line === this.active.line
      && this.anchor.character === this.active.character
  }

  get isSingleLine () { return this.anchor.line === this.active.line }
}

class Range {
  constructor (startLine, startCharacter, endLine, endCharacter) {
    this.start = new Position(startLine, startCharacter)
    this.end = new Position(endLine, endCharacter)
  }
}

const state = {changeHandlers: [], edits: [], commands: {}}

module.exports = {
  Position,
  Selection,
  Range,
  StatusBarAlignment: {Right: 2},
  ThemeColor: class { constructor (id) { this.id = id } },
  TextEdit: {replace: (range, newText) => ({range, newText})},
  window: {
    createStatusBarItem: () => ({show () {}, hide () {}, text: '', tooltip: ''}),
    showInputBox: async () => undefined,
    setStatusBarMessage: noop,
    onDidChangeActiveTextEditor: noop,
  },
  workspace: {
    getConfiguration: () => ({get: () => undefined}),
    onDidChangeConfiguration: noop,
    onDidChangeTextDocument: handler => {
      state.changeHandlers.push(handler)
      return {dispose () {}}
    },
  },
  commands: {
    registerCommand: (name, fn) => {
      state.commands[name] = fn
      return {dispose () {}}
    },
  },
  __state: state,
}
