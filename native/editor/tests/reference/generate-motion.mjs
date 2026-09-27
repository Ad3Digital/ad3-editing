// Development-only reference fixture generation. Native applications/tests do
// not invoke Node or the TypeScript runtime. Run from any working directory.
import { readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { stripTypeScriptTypes } from 'node:module';
const root = resolve(import.meta.dirname, '../../../..');
const fixtures = resolve(import.meta.dirname, '../fixtures');
const original = path => pathToFileURL(resolve(root, path)).href;
const save = (name, value) => writeFileSync(resolve(fixtures, name), JSON.stringify(value, null, 2));
const { cubicBezier, steps, spring } = await import(original('node_modules/animejs/dist/modules/index.js'));
const curves = [
  ['linear', t => t],
  ['easeIn', cubicBezier(.42, 0, 1, 1)],
  ['easeOut', cubicBezier(0, 0, .58, 1)],
  ['easeInOut', cubicBezier(.42, 0, .58, 1)],
  ...[['gentle', .5, 628], ['snappy', .15, 300], ['bouncy', .4, 500], ['strong', .65, 400]]
    .map(([name, bounce, duration]) => [name, spring({ bounce, duration }).ease]),
  ...[[-1, 10], [-.5, 500], [0, 600], [1, 10000]]
    .map(([bounce, duration]) => [`spring(${bounce},${duration})`, spring({ bounce, duration }).ease]),
  ...[[.1, .7, .5, 1], [.4, .095, .546, .875], [.44, .02, .252, .992], [.33, 0, .2, 1], [.4, 0, 1, 1]]
    .map(points => [`cubicBezier(${points.join(',')})`, cubicBezier(...points)]),
  ['steps(9)', steps(9)], ['steps(9,true)', steps(9, true)],
  ['cubicBezier(-0.2,0.3,1.2,1)', cubicBezier(-.2, .3, 1.2, 1)],
];
const progress = [0, .0001, .01, 1 / 30, .125, .25, 1 / 3, .5, .75, .99, 1];
save('easing.json', curves.map(([descriptor, fn]) => ({ descriptor, samples: progress.map(t => [t, fn(t)]) })));

const { revealChars, revealWords, scrambleChars } = await import(original('packages/runtime/src/utils/text-motion.ts'));
const texts = ['', 'a', 'AD3 editor', '  Olá\t mundo  ', '🙂 AD3 🚀', 'a\u0085b\u00a0c\ufeffd', 'one\n two\r\nthree'];
const ratios = [-.1, 0, .01, .1, .25, .3, 1 / 3, .5, .7, .9, .999, 1, 1.1];
save('text-motion.json', texts.flatMap(text => ratios.map(ratio => ({
  text, ratio, chars: revealChars(text, ratio).toWellFormed(),
  words: revealWords(text, ratio).toWellFormed(), scramble: scrambleChars(text, ratio).toWellFormed(),
}))));

const source = readFileSync(resolve(root, 'packages/runtime/src/utils/color.ts'), 'utf8')
  .replace("from 'colord'", `from '${original('node_modules/colord/index.mjs')}'`)
  .replace("import { clamp } from '../math/common';", 'const clamp=(v,min,max)=>Math.min(Math.max(v,min),max);');
const { parseColor } = await import('data:text/javascript;base64,' + Buffer.from(stripTypeScriptTypes(source)).toString('base64'));
const colors = ['', ' ', '#f', '#fc', '#fca', '#fcab', '#ff0080', '#ff008080', '#12345', '#1234567', 'ffa', '11',
  'red', 'ReD', 'mint', 'transparent', 'rebeccapurple', 'rgb(30, 80, 240)', 'rgb(10% 50% 80% / .25)',
  'rgba(0,1,2,.5)', 'rgb(0, 1%, 2)', 'rgb(0 1 2 .5)', 'rgb(0 1 2 / .5)', 'rgb(0. 1 2)', 'rgb(-5,+10,999)',
  'hsl(0 100% 50%)', 'hsl(.5turn,100%,50%)', 'hsl(2rad 33% 61% / 20%)', 'hsl(2RAD 33% 61% / 20%)',
  'hsl(-100grad,100%,40%)', 'hsla(+90deg, 10%, 50%, .5)', 'hsl(359,0%,0%)', 'hsl(0,99%,100%)',
  '\ufeff green \u00a0', 'a\u0085b', 'rgb(.5,.5,.5)', 'rgb(1 2 3 / .5)',
  'hsl(.5turn,.5%,.5%)', 'hsl(90 .5% .5% / .5)'];
for (const hue of [-720, -30, 0, 30, 60, 120, 180, 240, 300, 360, 450, 1e10])
  for (const saturation of [0, 25, 50, 100, 150])
    for (const lightness of [0, 25, 50, 75, 100]) colors.push(`hsl(${hue},${saturation}%,${lightness}%)`);
save('colors.json', colors.map(text => ({ text, color: parseColor(text) })));
const motionSource = readFileSync(resolve(root, 'packages/runtime/src/systems/motion.ts'), 'utf8');
const lerpSource = 'export ' + motionSource.slice(motionSource.indexOf('function lerpColor('), motionSource.indexOf('function clamp01('));
const { lerpColor } = await import('data:text/javascript;base64,' + Buffer.from(stripTypeScriptTypes(lerpSource)).toString('base64'));
const colorPairs = [[0xff0000, 0x0000ff], [0x123456, 0xaafc01], [0x000000, 0xffffff]];
const colorProgress = [-Number.MAX_VALUE, -1e300, -1e12, -1e8, -1.5, -.1, 0, .5, 1, 1.1, 2, 1e8, 1e12, 1e300, Number.MAX_VALUE, 2 ** 52, 2 ** 52 + 1];
save('color-lerp.json', colorPairs.flatMap(([from, to]) => colorProgress.map(progress => ({ from, to, progress, color: lerpColor(from, to, progress) & 0xffffff }))));
console.log('Generated easing, text motion and color fixtures from the original editor sources.');
