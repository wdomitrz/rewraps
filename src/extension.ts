// The extension: commands, settings and the editor plumbing.
//
// There is deliberately very little here. Everything that decides where a line
// breaks lives in the engine; this file only answers questions the editor is the
// only one that can answer -- which column, which language, whether this edit
// was typing -- and applies the result.

import * as fs from 'fs'
import * as path from 'path'
import * as vscode from 'vscode'

import {initSync, maybe_auto_wrap, rewrap} from './pkg/rewrap_wasm.js'

/** What the engine hands back. */
interface Edit {
  startLine: number
  endLine: number
  lines: string[]
  isEmpty: boolean
  selections: number[] | null
}

let wasmRoot: string | undefined
function loadEngine (context: vscode.ExtensionContext): void {
  if (wasmRoot) return
  wasmRoot = context.extensionPath
  const bytes = fs.readFileSync(path.join(context.extensionPath, 'dist', 'rewrap_wasm_bg.wasm'))
  initSync({module: bytes})
}

/** The column to wrap at, or undefined when nothing usable is configured.
 *
 * The ruler list is read per document, so a language-specific `editor.rulers`
 * wins over the global one without this code knowing that languages exist. Only
 * the first ruler is used: a column is a property of the file, and cycling
 * through several would make the same command produce different results on
 * consecutive invocations.
 */
function columnFor (document: vscode.TextDocument): number | undefined {
  const config = vscode.workspace.getConfiguration('', document)
  const rulers = config.get<unknown[]>('editor.rulers', [])
  for (const ruler of rulers) {
    const column = typeof ruler === 'number' ? ruler
      : (ruler as {column?: number}).column
    if (typeof column === 'number' && column > 0) return column
  }
  return undefined
}

function coreLines (document: vscode.TextDocument): string[] {
  const lines: string[] = []
  for (let i = 0; i < document.lineCount; i++) lines.push(document.lineAt(i).text)
  return lines
}

function selectionArray (selections: readonly vscode.Selection[]): number[] {
  const flat: number[] = []
  for (const s of selections) {
    flat.push(s.anchor.line, s.anchor.character, s.active.line, s.active.character)
  }
  return flat
}

/** Turn the engine's answer into editor edits. */
function apply (document: vscode.TextDocument, edit: Edit): vscode.TextEdit[] {
  if (edit.isEmpty) return []
  const range = new vscode.Range(
    edit.startLine, 0,
    edit.endLine, document.lineAt(Math.min(edit.endLine, document.lineCount - 1)).text.length)
  return [vscode.TextEdit.replace(range, edit.lines.join('\n'))]
}

/** Ask for a column, remembering the answer for this language.
 *
 * Only ever reached from an explicit command. Auto-wrap never prompts: a dialog
 * opening itself while someone types would be worse than not wrapping.
 */
async function askColumn (context: vscode.ExtensionContext, document: vscode.TextDocument)
  : Promise<number | undefined>
{
  const key = `column:${document.languageId}`
  const remembered = context.workspaceState.get<number>(key)
  const entered = await vscode.window.showInputBox({
    prompt: 'Wrap at which column?',
    value: remembered ? String(remembered) : '',
    placeHolder: '80',
    validateInput: (value) => {
      const parsed = Number(value)
      return Number.isInteger(parsed) && parsed > 0 ? undefined : 'Enter a whole number above 0'
    },
  })
  if (entered === undefined) return undefined
  const column = Number(entered)
  await context.workspaceState.update(key, column)
  return column
}

/** The wrap command.
 *
 * `alwaysAsk` is what distinguishes the two commands: "Wrap at Column..." exists
 * precisely to override the configured column for one operation, so it must ask
 * even when a ruler is set. "Wrap Selection or Comment" asks only when there is
 * nothing to go on.
 */
function wrapCommand (context: vscode.ExtensionContext, alwaysAsk = false) {
  return async () => {
    const editor = vscode.window.activeTextEditor
    if (!editor) return
    loadEngine(context)
    const document = editor.document
    const configured = alwaysAsk ? undefined : columnFor(document)
    const column = configured ?? await askColumn(context, document)
    if (column === undefined) {
      // Dismissed: do nothing, and say so rather than wrapping at a column the
      // user did not choose.
      vscode.window.setStatusBarMessage('Rewraps: no column chosen, nothing wrapped', 4000)
      return
    }
    const edit = rewrap(
      document.languageId, document.uri.fsPath, column,
      Number(editor.options.tabSize) || 4,
      coreLines(document), selectionArray(editor.selections)) as unknown as Edit
    const edits = apply(document, edit)
    if (edits.length) {
      const applied = await editor.edit(builder => edits.forEach(e => builder.replace(e.range, e.newText)))
      if (applied && edit.selections) {
        const s = edit.selections
        editor.selections = [new vscode.Selection(s[0], s[1], s[2], s[3])]
      }
    }
  }
}

/** Auto-wrap state: a per-file override on top of the setting. */
class AutoWrap {
  private static readonly key = (document: vscode.TextDocument) =>
    `autoWrap:${document.uri.toString()}`

  constructor (
    private readonly context: vscode.ExtensionContext,
    private readonly status: vscode.StatusBarItem) {}

  enabled (document: vscode.TextDocument): boolean {
    const override = this.context.workspaceState.get<boolean>(AutoWrap.key(document))
    if (override !== undefined) return override
    return vscode.workspace.getConfiguration('', document)
      .get<boolean>('rewraps.autoWrap.enabled', true)
  }

  async toggle (): Promise<void> {
    const editor = vscode.window.activeTextEditor
    if (!editor) return
    const document = editor.document
    await this.context.workspaceState.update(AutoWrap.key(document), !this.enabled(document))
    this.refresh(document)
  }

  refresh (document?: vscode.TextDocument): void {
    const target = document ?? vscode.window.activeTextEditor?.document
    if (!target) return this.status.hide()
    const notification = vscode.workspace.getConfiguration('', target)
      .get<string>('rewraps.autoWrap.notification', 'icon')
    if (notification === 'none') return this.status.hide()
    const on = this.enabled(target)
    if (notification === 'icon') {
      this.status.text = '$(word-wrap) Rewraps'
      this.status.color = on ? undefined : new vscode.ThemeColor('disabledForeground')
      this.status.tooltip = `Auto-wrap is ${on ? 'on' : 'off'} for this file`
      this.status.show()
    }
  }
}

export function activate (context: vscode.ExtensionContext): void {
  const status = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Right, 100)
  context.subscriptions.push(status)
  const autoWrap = new AutoWrap(context, status)
  autoWrap.refresh()
  context.subscriptions.push(
    vscode.window.onDidChangeActiveTextEditor(() => autoWrap.refresh()),
    vscode.workspace.onDidChangeConfiguration(() => autoWrap.refresh()))

  context.subscriptions.push(
    vscode.commands.registerCommand('rewraps.wrap', wrapCommand(context)),
    vscode.commands.registerCommand('rewraps.wrapAt', wrapCommand(context, true)),
    vscode.commands.registerCommand('rewraps.toggleAutoWrap', () => autoWrap.toggle()))

  context.subscriptions.push(
    vscode.workspace.onDidChangeTextDocument(async (event) => {
      const editor = vscode.window.activeTextEditor
      if (!editor || editor.document !== event.document) return
      if (!autoWrap.enabled(event.document)) return
      // Only a single insertion into a single empty selection is typing.
      // Anything else -- a paste, an undo, a multi-cursor edit -- must not
      // trigger an automatic rewrite: it would fight the user's undo history.
      if (editor.selections.length !== 1 || !editor.selection.isEmpty) return
      if (event.contentChanges.length !== 1) return
      const change = event.contentChanges[0]
      if (change.rangeLength > 0) return
      loadEngine(context)
      // No ruler means no column, and auto-wrap does nothing: a prompt cannot
      // appear while someone is typing, and guessing a column is worse than
      // leaving the line alone. An explicit wrap asks instead.
      const column = columnFor(event.document)
      if (column === undefined) return
      const edit = maybe_auto_wrap(
        event.document.languageId, event.document.uri.fsPath, column,
        Number(editor.options.tabSize) || 4,
        change.text, change.range.start.line, change.range.start.character,
        coreLines(event.document)) as unknown as Edit
      if (edit.isEmpty) return
      const applied = await editor.edit(builder =>
        apply(event.document, edit).forEach(e => builder.replace(e.range, e.newText)))
      if (applied && edit.selections) {
        // Applied after the edit resolves, so the position the engine computed
        // is not shifted by the replacement itself. This is what puts the cursor
        // after the word the wrap pushed down, instead of leaving it on the line
        // that just got shorter.
        const s = edit.selections
        editor.selections = [new vscode.Selection(s[0], s[1], s[2], s[3])]
      }
    }))
}

export function deactivate (): void {}
