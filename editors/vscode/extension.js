// vnengine VS Code extension — CommonJS, no build step.
//
// Diagnostics come from the engine itself: either the WASM linter
// embedded in media/vnengine-lint.wasm (instant, no install), or the
// `vnengine-pack --check-only --json` binary (authoritative, requires
// the engine on PATH or vnengine.packPath configured).

const vscode = require('vscode');
const cp = require('child_process');
const fs = require('fs');
const path = require('path');

/** @type {WebAssembly.Exports | null} */
let wasm = null;
/** @type {vscode.DiagnosticCollection} */
let collection;
/** @type {vscode.OutputChannel} */
let out;
/** Per-document debounce timers, keyed by URI string. */
const timers = new Map();
/** Asset-list cache: projectDir -> {time, list}. */
const assetCache = new Map();
const ASSET_CACHE_MS = 5000;
/** Directories never scanned for assets. */
const SKIP_DIRS = new Set(['.git', '.vscode', 'node_modules', 'saves', 'build', 'target']);

async function activate(context) {
  collection = vscode.languages.createDiagnosticCollection('vnengine');
  out = vscode.window.createOutputChannel('vnengine');
  context.subscriptions.push(collection, out);

  try {
    const bytes = fs.readFileSync(
      context.asAbsolutePath(path.join('media', 'vnengine-lint.wasm'))
    );
    const { instance } = await WebAssembly.instantiate(bytes, {});
    wasm = instance.exports;
    out.appendLine('vnengine: WASM linter loaded');
  } catch (e) {
    out.appendLine(`vnengine: WASM linter failed to load: ${e}`);
  }

  context.subscriptions.push(
    vscode.workspace.onDidOpenTextDocument(scheduleLint),
    vscode.workspace.onDidChangeTextDocument((e) => scheduleLint(e.document)),
    vscode.workspace.onDidSaveTextDocument((d) => {
      if (d.languageId === 'vn') runDiagnostics(d);
    }),
    vscode.workspace.onDidCloseTextDocument((d) => collection.delete(d.uri)),
    vscode.commands.registerCommand('vnengine.run', cmdRun),
    vscode.commands.registerCommand('vnengine.validate', cmdValidate),
    vscode.commands.registerCommand('vnengine.package', cmdPackage)
  );

  for (const doc of vscode.workspace.textDocuments) {
    if (doc.languageId === 'vn') runDiagnostics(doc);
  }
}

function deactivate() {}

// ---------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------

function scheduleLint(doc) {
  if (doc.languageId !== 'vn') return;
  const key = doc.uri.toString();
  clearTimeout(timers.get(key));
  const ms = vscode.workspace
    .getConfiguration('vnengine')
    .get('diagnosticsDebounceMs', 300);
  timers.set(key, setTimeout(() => runDiagnostics(doc), ms));
}

async function runDiagnostics(doc) {
  if (doc.languageId !== 'vn' || doc.uri.scheme !== 'file') return;
  const mode = vscode.workspace
    .getConfiguration('vnengine')
    .get('diagnostics', 'wasm');
  if (mode === 'off') {
    collection.delete(doc.uri);
    return;
  }
  if (mode === 'binary' && !doc.isDirty) {
    const ok = await binaryDiagnostics(doc);
    if (ok) return;
    // fall through to WASM when the binary is unavailable
  }
  wasmDiagnostics(doc);
}

/**
 * Diagnostics via the embedded WASM linter. Synchronous over the
 * document text; `project.toml` and the asset list are supplied so the
 * engine can apply its full project-mode checks.
 */
async function wasmDiagnostics(doc) {
  if (!wasm) return;
  const version = doc.version;
  const projectDir = findProjectDir(path.dirname(doc.uri.fsPath));
  const request = {
    source: doc.getText(),
    project_toml: null,
    assets: [],
  };
  if (projectDir) {
    const tomlPath = path.join(projectDir, 'project.toml');
    try {
      request.project_toml = fs.readFileSync(tomlPath, 'utf8');
    } catch {
      /* treated as loose file */
    }
    request.assets = await listAssets(projectDir);
  }
  if (doc.isClosed || doc.version !== version) return;
  const response = callLint(JSON.stringify(request));
  if (!response) return;
  collection.set(doc.uri, response.diagnostics.map((d) => toVsDiagnostic(d)));
}

/**
 * Diagnostics via `vnengine-pack --check-only --json`. Returns false
 * when the binary is unavailable so the caller can fall back to WASM.
 */
async function binaryDiagnostics(doc) {
  const bin = packBinary();
  if (!bin) return false;
  const projectDir = findProjectDir(path.dirname(doc.uri.fsPath));
  if (!projectDir) return false;
  const version = doc.version;
  let report;
  try {
    const { stdout } = await exec(bin, [
      '--project', projectDir, '--check-only', '--json',
    ]);
    report = JSON.parse(stdout);
  } catch (e) {
    if (e.stdout) {
      // Non-zero exit still carries the JSON report on stdout.
      try {
        report = JSON.parse(e.stdout);
      } catch {
        out.appendLine(`vnengine: unparseable vnengine-pack output: ${e.stdout}`);
        return false;
      }
    } else {
      out.appendLine(`vnengine: vnengine-pack failed: ${e.message || e}`);
      return false;
    }
  }
  if (!doc.isClosed && !doc.isDirty && doc.version === version) {
    applyReport(projectDir, report);
  }
  return true;
}

/** Spread a CheckReport's file diagnostics across open documents. */
function applyReport(projectDir, report) {
  const byFile = new Map();
  for (const fd of report.diagnostics || []) {
    const uri = vscode.Uri.file(path.join(projectDir, fd.file));
    if (!byFile.has(uri.toString())) byFile.set(uri.toString(), { uri, diags: [] });
    byFile.get(uri.toString()).diags.push(toVsDiagnostic(fd));
  }
  for (const doc of vscode.workspace.textDocuments) {
    if (doc.uri.scheme === 'file' && !doc.isDirty && findProjectDir(path.dirname(doc.uri.fsPath)) === projectDir) {
      collection.set(doc.uri, []);
    }
  }
  const dirty = new Set(vscode.workspace.textDocuments.filter((doc) => doc.isDirty).map((doc) => doc.uri.toString()));
  for (const { uri, diags } of byFile.values()) {
    if (!dirty.has(uri.toString())) collection.set(uri, diags);
  }
}

function toVsDiagnostic(d) {
  const line = Math.max((d.line || 1) - 1, 0);
  const col = d.col || 0;
  const span = d.span || 1;
  const range = new vscode.Range(line, col, line, col + span);
  const severity =
    d.severity === 'error'
      ? vscode.DiagnosticSeverity.Error
      : vscode.DiagnosticSeverity.Warning;
  const diag = new vscode.Diagnostic(range, d.message, severity);
  diag.source = 'vnengine';
  return diag;
}

// ---------------------------------------------------------------------------
// WASM glue
// ---------------------------------------------------------------------------

function callLint(json) {
  const bytes = new TextEncoder().encode(json);
  const ptr = wasm.vn_alloc(bytes.length);
  try {
    new Uint8Array(wasm.memory.buffer, ptr, bytes.length).set(bytes);
    wasm.vn_lint(ptr, bytes.length);
    const outPtr = wasm.vn_result_ptr();
    const outLen = wasm.vn_result_len();
    const text = new TextDecoder().decode(
      new Uint8Array(wasm.memory.buffer, outPtr, outLen)
    );
    return JSON.parse(text);
  } catch (e) {
    out.appendLine(`vnengine: linter call failed: ${e}`);
    return null;
  } finally {
    wasm.vn_free(ptr, bytes.length);
  }
}

// ---------------------------------------------------------------------------
// Project layout
// ---------------------------------------------------------------------------

/**
 * Walk up from `dir` looking for `project.toml`, stopping at the
 * workspace root or filesystem root. Returns the project dir or null.
 */
function findProjectDir(dir) {
  let stop = null;
  const folder = vscode.workspace.getWorkspaceFolder(vscode.Uri.file(dir));
  if (folder) stop = path.resolve(folder.uri.fsPath);
  let cur = path.resolve(dir);
  while (true) {
    if (fs.existsSync(path.join(cur, 'project.toml'))) return cur;
    if (stop && cur === stop) return null;
    const parent = path.dirname(cur);
    if (parent === cur) return null;
    cur = parent;
  }
}

/** Recursively list project-relative file paths (`/` separated), cached. */
async function listAssets(projectDir) {
  const hit = assetCache.get(projectDir);
  if (hit && Date.now() - hit.time < ASSET_CACHE_MS) return hit.list;
  const list = [];
  const walk = (dir, depth) => {
    if (depth > 8) return;
    let entries;
    try {
      entries = fs.readdirSync(dir, { withFileTypes: true });
    } catch {
      return;
    }
    for (const e of entries) {
      if (e.name.startsWith('.')) continue;
      const full = path.join(dir, e.name);
      if (e.isDirectory()) {
        if (!SKIP_DIRS.has(e.name)) walk(full, depth + 1);
      } else {
        list.push(path.relative(projectDir, full).split(path.sep).join('/'));
      }
    }
  };
  walk(projectDir, 0);
  assetCache.set(projectDir, { time: Date.now(), list });
  return list;
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/** Directory to run commands against: active .vn file's project, else first workspace folder. */
function commandDir() {
  const doc = vscode.window.activeTextEditor?.document;
  if (doc && doc.uri.scheme === 'file') {
    const found = findProjectDir(path.dirname(doc.uri.fsPath));
    if (found) return found;
  }
  return vscode.workspace.workspaceFolders?.[0]?.uri.fsPath || null;
}

function cmdRun() {
  const bin = engineBinary();
  const dir = commandDir();
  if (!dir) return vscode.window.showErrorMessage('vnengine: no project folder open');
  if (!bin)
    return vscode.window.showErrorMessage(
      'vnengine: `vnengine` binary not found — install the author kit or set vnengine.enginePath'
    );
  return runProjectCommand(bin, ['--project', dir], dir);
}

async function cmdValidate() {
  const doc = vscode.window.activeTextEditor?.document;
  const bin = packBinary();
  const dir = commandDir();
  if (!dir) return vscode.window.showErrorMessage('vnengine: no project folder open');
  if (bin) {
    out.show(true);
    out.appendLine(`$ ${bin} --project ${dir} --check-only --json`);
    try {
      const { stdout } = await exec(bin, ['--project', dir, '--check-only', '--json']);
      const report = JSON.parse(stdout);
      applyReport(dir, report);
      out.appendLine(report.ok ? 'project valid' : 'validation failed');
      for (const d of report.diagnostics || []) {
        out.appendLine(
          `${d.severity}: ${d.file}${d.line ? ':' + d.line : ''} ${d.message}`
        );
      }
    } catch (e) {
      if (e.stdout) {
        try {
          const report = JSON.parse(e.stdout);
          applyReport(dir, report);
          out.appendLine('validation failed');
          for (const d of report.diagnostics || []) {
            out.appendLine(
              `${d.severity}: ${d.file}${d.line ? ':' + d.line : ''} ${d.message}`
            );
          }
          return;
        } catch { /* fall through */ }
      }
      out.appendLine(`vnengine-pack failed: ${e.message || e}`);
    }
  } else if (doc && doc.languageId === 'vn') {
    await wasmDiagnostics(doc);
    vscode.window.showInformationMessage(
      'vnengine: validated with the embedded linter (set vnengine.packPath for full project checks)'
    );
  } else {
    vscode.window.showErrorMessage(
      'vnengine: `vnengine-pack` not found — install the author kit or set vnengine.packPath'
    );
  }
}

function cmdPackage() {
  const bin = packBinary();
  const dir = commandDir();
  if (!dir) return vscode.window.showErrorMessage('vnengine: no project folder open');
  if (!bin)
    return vscode.window.showErrorMessage(
      'vnengine: `vnengine-pack` binary not found — install the author kit or set vnengine.packPath'
    );
  const output = path.join(dir, 'build', `${path.basename(dir)}.vnpkg`);
  return runProjectCommand(bin, ['--project', dir, '--output', output], dir);
}

// ---------------------------------------------------------------------------
// Binaries, terminals, process helpers
// ---------------------------------------------------------------------------

function configPath(key) {
  const p = vscode.workspace.getConfiguration('vnengine').get(key, '');
  return p || null;
}

/** Resolve `name` on PATH (handles .exe on Windows). */
function which(name) {
  const cmd = process.platform === 'win32' ? 'where' : 'which';
  try {
    const out = cp.execFileSync(cmd, [name], { encoding: 'utf8' });
    return out.split(/\r?\n/)[0].trim() || null;
  } catch {
    return null;
  }
}

function engineBinary() {
  return configPath('enginePath') || which('vnengine');
}

function packBinary() {
  return configPath('packPath') || which('vnengine-pack');
}

function runProjectCommand(bin, args, cwd) {
  out.show(true);
  out.appendLine([bin, ...args].join(' '));
  return new Promise((resolve) => {
    const child = cp.spawn(bin, args, { cwd, windowsHide: true });
    child.stdout.on('data', (chunk) => out.append(chunk.toString()));
    child.stderr.on('data', (chunk) => out.append(chunk.toString()));
    child.on('error', (error) => {
      out.appendLine(error.message);
      vscode.window.showErrorMessage(`vnengine: ${error.message}`);
    });
    child.on('close', (code) => {
      if (code !== null && code !== 0) {
        vscode.window.showErrorMessage(`vnengine: command exited with code ${code}. See the vnengine output.`);
      }
      resolve();
    });
  });
}

function exec(bin, args) {
  return new Promise((resolve, reject) => {
    cp.execFile(bin, args, { maxBuffer: 16 * 1024 * 1024 }, (err, stdout, stderr) => {
      if (err) {
        err.stdout = stdout;
        err.stderr = stderr;
        reject(err);
      } else {
        resolve({ stdout, stderr });
      }
    });
  });
}

module.exports = { activate, deactivate };
