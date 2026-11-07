'use strict';
const vscode = require('vscode');
const binary = require('./lib/binary');

const RELEASES = 'https://github.com/inamdarmihir/ostler/releases';

function settings() {
  const c = vscode.workspace.getConfiguration('ostler');
  return {
    path: c.get('path', '').trim(),
    headed: c.get('headed', false),
    cautious: c.get('cautious', false),
    profileDir: c.get('profileDir', '').trim(),
  };
}

/** Setting, then PATH, then a one-time verified download. Throws with a readable message. */
async function resolveBinary(context) {
  const s = settings();
  if (s.path) return s.path;
  const found = binary.onPath();
  if (found) return found;
  const version = context.extension.packageJSON.version;
  const target = binary.targetFor(process.platform, process.arch);
  if (!target) {
    throw new Error(`There is no prebuilt Ostler for ${process.platform}/${process.arch}. Build it with "cargo install ostler" and set ostler.path.`);
  }
  return vscode.window.withProgress(
    { location: vscode.ProgressLocation.Notification, title: `Ostler: downloading v${version} (one time)` },
    () => binary.download({ version, target, storageDir: context.globalStorageUri.fsPath, base: RELEASES }),
  );
}

function activate(context) {
  const changed = new vscode.EventEmitter();
  context.subscriptions.push(
    changed,
    vscode.workspace.onDidChangeConfiguration((e) => {
      if (e.affectsConfiguration('ostler')) changed.fire();
    }),
    vscode.lm.registerMcpServerDefinitionProvider('ostler.server', {
      onDidChangeMcpServerDefinitions: changed.event,
      provideMcpServerDefinitions() {
        // The command is filled in by resolveMcpServerDefinition, which may need to download the binary.
        return [new vscode.McpStdioServerDefinition('Ostler', 'ostler', binary.serveArgs(settings()), {}, context.extension.packageJSON.version)];
      },
      async resolveMcpServerDefinition(server) {
        try {
          server.command = await resolveBinary(context);
          return server;
        } catch (e) {
          vscode.window.showErrorMessage(`Ostler could not start: ${e.message}`);
          return undefined;
        }
      },
    }),
    vscode.commands.registerCommand('ostler.checkSetup', async () => {
      try {
        const bin = await resolveBinary(context);
        const t = vscode.window.createTerminal('Ostler: check setup');
        t.show();
        t.sendText(`${JSON.stringify(bin)} doctor`);
      } catch (e) {
        vscode.window.showErrorMessage(e.message);
      }
    }),
    vscode.commands.registerCommand('ostler.signIn', async () => {
      const url = await vscode.window.showInputBox({
        prompt: 'Website to sign in to (a Chrome window opens; sign in, then press Enter in the terminal)',
        placeHolder: 'https://example.com/login',
        validateInput: (v) => (/^https?:\/\/\S+$/.test(v) ? undefined : 'Enter a full http(s) address'),
      });
      if (!url) return;
      try {
        const bin = await resolveBinary(context);
        const t = vscode.window.createTerminal('Ostler: sign in');
        t.show();
        t.sendText(`${JSON.stringify(bin)} login ${JSON.stringify(url)}`);
        vscode.window.showInformationMessage('After signing in, set "ostler.profileDir" to ~/.ostler/profile so the assistant can use the session.');
      } catch (e) {
        vscode.window.showErrorMessage(e.message);
      }
    }),
    vscode.commands.registerCommand('ostler.openGuide', () =>
      vscode.env.openExternal(vscode.Uri.parse('https://github.com/inamdarmihir/ostler/blob/main/docs/GETTING-STARTED.md'))),
  );
}

function deactivate() {}

module.exports = { activate, deactivate };
