// Drives the built extension the way the editor does, and checks the things that
// only the editor layer can get wrong.
//
// Run against `dist/extension.js` and the bundled engine, with `scripts/vscode-stub.js`
// standing in for the editor. No VS Code, no network, no wasm loader differences:
// the same artefact the VSIX ships.
//
//   node scripts/check-extension.js [path-to-extension-root]
const Module = require('module')
const path = require('path')

const root = process.argv[2] || path.join(__dirname, '..')
const stubPath = require.resolve('./vscode-stub.js')

// Route `require('vscode')` to the stub, exactly as the extension host would
// provide the real module.
const resolve = Module._resolveFilename
Module._resolveFilename = function (request, ...rest) {
  return request === 'vscode' ? stubPath : resolve.call(this, request, ...rest)
}

const vscode = require('vscode')
const state = vscode.__state

const checks = []
function check (name, fn) { checks.push({name, fn}) }
function assert (condition, message) { if (!condition) throw new Error(message) }

function makeEditor (lines, languageId, column) {
  const document = {
    lines: [...lines], languageId, version: 1,
    uri: {fsPath: `/tmp/x.${languageId}`, toString: () => `file:///tmp/x.${languageId}`},
    get lineCount () { return document.lines.length },
    lineAt: i => ({text: document.lines[i]}),
  }
  const editor = {
    document, options: {tabSize: 4}, selectionSetters: 0, _sel: null,
    get selection () { return (editor._sel || [new vscode.Selection(0, 0, 0, 0)])[0] },
    async edit (build, options) {
      const edits = []
      build({replace: (range, newText) => edits.push({range, newText})})
      for (const {range, newText} of edits) {
        document.lines = [...document.lines.slice(0, range.start.line),
                          ...newText.split('\n'),
                          ...document.lines.slice(range.end.line + 1)]
      }
      document.version += 1
      state.edits.push({options, count: edits.length})
      // The real editor fires the change event while the edit is in flight,
      // which is what a re-entrancy bug shows up against.
      for (const handler of state.changeHandlers) {
        await handler({document, contentChanges: [{text: ' ', rangeLength: 1,
          range: {start: {line: 0, character: 0}}}]})
      }
      return true
    },
  }
  Object.defineProperty(editor, 'selections', {
    get () { return editor._sel || [new vscode.Selection(0, 0, 0, 0)] },
    set (value) { editor._sel = value; editor.selectionSetters += 1 },
  })
  Object.defineProperty(editor, 'selection', {get () { return editor.selections[0] }})
  vscode.workspace.getConfiguration = () => ({
    get: (key, fallback) => key === 'editor.rulers' ? [column] : fallback,
  })
  return editor
}

function activate () {
  require(path.join(root, 'dist', 'extension.js'))
    .activate({extensionPath: root, subscriptions: [],
               workspaceState: {get: () => undefined, update: async () => {}}})
}

check('auto-wrap applies one atomic edit', async () => {
  const lines = ['// alpha beta gamma delta epsilon ']
  const editor = makeEditor(lines, 'rust', 30)
  vscode.window.activeTextEditor = editor
  state.edits.length = 0
  await state.changeHandlers[0]({document: editor.document, contentChanges: [
    {text: ' ', rangeLength: 0, range: {start: {line: 0, character: lines[0].length - 1}}}]})
  assert(state.edits.length === 1, `expected one edit, got ${state.edits.length}`)
  assert(state.edits[0].options.undoStopBefore === false,
    'auto-wrap must join the undo step of the keystroke that caused it')
  assert(editor.selectionSetters === 1, 'the cursor was not set exactly once')
  assert(editor.document.lines.length === 2, `expected two lines, got ${JSON.stringify(editor.document.lines)}`)
  const at = editor.selections[0].active
  assert(at.line === 1, `cursor should be on the second line, was ${at.line}`)
})

check('the wrap command is its own undo step', async () => {
  const editor = makeEditor(['// alpha beta gamma delta epsilon zeta'], 'rust', 30)
  vscode.window.activeTextEditor = editor
  state.edits.length = 0
  await state.commands['rewraps.wrap']()
  assert(state.edits.length === 1, `expected one edit, got ${state.edits.length}`)
  assert(state.edits[0].options.undoStopBefore === true, 'an explicit wrap must be its own undo step')
  assert(editor.selectionSetters === 1, 'the command did not set the cursor exactly once')
})

check('a cursor in a comment wraps the whole comment', async () => {
  const lines = ['fn main() {', '    // alpha beta gamma', '    // delta epsilon zeta', '}']
  const editor = makeEditor(lines, 'rust', 30)
  editor._sel = [new vscode.Selection(2, 4, 2, 4)]
  vscode.window.activeTextEditor = editor
  state.edits.length = 0
  await state.commands['rewraps.wrap']()
  assert(state.edits.length === 1, 'the command did nothing')
  const code = editor.document.lines[0]
  const comment = editor.document.lines.slice(1, -1)
  assert(code === 'fn main() {', `code above was touched: ${JSON.stringify(code)}`)
  assert(editor.document.lines.at(-1) === '}', 'code below the comment was touched')
  assert(comment.every(line => line.startsWith('    // ')),
    `the comment lines lost their marker: ${JSON.stringify(comment)}`)
  const words = comment.map(line => line.replace(/^\s*\/\/\s?/, '')).join(' ')
  assert(words === 'alpha beta gamma delta epsilon zeta',
    `words were changed: ${JSON.stringify(words)}`)
})

check('a line of code is never wrapped', async () => {
  const editor = makeEditor(['let alpha = beta_gamma_delta_epsilon_zeta_eta = 1;'], 'rust', 20)
  editor._sel = [new vscode.Selection(0, 3, 0, 3)]
  vscode.window.activeTextEditor = editor
  state.edits.length = 0
  await state.commands['rewraps.wrap']()
  assert(state.edits.length === 0, 'a line of code was rewritten')
})

;(async () => {
  activate()
  let failures = 0
  for (const {name, fn} of checks) {
    try {
      await fn()
      console.log(`  ok    ${name}`)
    } catch (error) {
      failures += 1
      console.log(`  FAIL  ${name}\n        ${error.message}`)
    }
  }
  if (failures) {
    console.log(`\n${failures} of ${checks.length} extension checks failed`)
    process.exit(1)
  }
  console.log(`\n${checks.length} extension checks passed`)
})()
