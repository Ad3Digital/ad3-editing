// Development-only Canvas oracle. Run with the repository Electron executable.
// The native editor and its tests read the fixture without Electron/Node.
const { app, BrowserWindow } = require('electron');
const { readFileSync, writeFileSync } = require('node:fs');
const { resolve } = require('node:path');
const { createHash } = require('node:crypto');
const root = resolve(__dirname, '../..');
const output = resolve(process.argv[2] || resolve(root, 'tests/fixtures/text-metrics.json'));
process.on('uncaughtException', error => { console.error(error); app.exit(1); });
app.whenReady().then(async () => {
  const window = new BrowserWindow({ show: false, webPreferences: { sandbox: true, contextIsolation: true, nodeIntegration: false } });
  await window.loadURL('data:text/html,<html></html>');
  const fonts = ['Inter.ttf', 'Inter-Italic.ttf'].map(name => ({ name, bytes: readFileSync(resolve(root, 'assets/fonts', name)) }));
  const cases = await window.webContents.executeJavaScript(`(async () => {
    for (const [data, style] of ${JSON.stringify(fonts.map((font, i) => [font.bytes.toString('base64'), i ? 'italic' : 'normal']))}) {
      const face = await new FontFace('Inter', 'url(data:font/ttf;base64,' + data + ')', {weight:'100 900', style}).load();
      document.fonts.add(face);
    }
    const ctx = new OffscreenCanvas(1, 1).getContext('2d');
    const cases = [];
    for (const size of [8, 16, 32, 80]) for (const style of ['normal', 'italic'])
      for (const weight of [400, 700]) for (const spacing of [0, 2, -1.5])
      for (const baseline of ['top', 'middle', 'bottom', 'alphabetic'])
      for (const text of ${JSON.stringify(['', ' ', 'Hello', 'AV', 'office', 'A\u0301gj', '\u039f\u03a3', 'a\t\nb'])}) {
        ctx.font = style + ' ' + weight + ' ' + size + 'px Inter';
        ctx.textBaseline = baseline; ctx.letterSpacing = spacing + 'px';
        const m = ctx.measureText(text);
        cases.push({size, style, weight, spacing, baseline, text, metrics:{width:m.width,ascent:m.fontBoundingBoxAscent,descent:m.fontBoundingBoxDescent,left:m.actualBoundingBoxLeft,right:m.actualBoundingBoxRight,ink_ascent:m.actualBoundingBoxAscent,ink_descent:m.actualBoundingBoxDescent}});
      }
    return cases;
  })()`);
  writeFileSync(output, JSON.stringify({electron:process.versions.electron, chromium:process.versions.chrome,
    fonts:Object.fromEntries(fonts.map(font => [font.name, createHash('sha256').update(font.bytes).digest('hex')])), cases}, null, 2) + '\n');
  app.quit();
}).catch(error => { console.error(error); app.exit(1); });
