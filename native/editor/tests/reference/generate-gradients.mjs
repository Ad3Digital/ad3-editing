// Development-only oracle for the next native paint increment.
import { readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { createHash } from 'node:crypto';
import { stripTypeScriptTypes } from 'node:module';
const root = resolve(import.meta.dirname, '../../../..');
const source = readFileSync(root + '/packages/runtime/src/systems/gradients.ts', 'utf8').replaceAll('\r\n','\n');
const colors = readFileSync(root + '/packages/runtime/src/utils/color.ts', 'utf8').replaceAll('\r\n','\n');
const colorCode = colors.slice(colors.indexOf('export function colorToHex('),colors.indexOf('export function rgbToColor(')).replaceAll('export function','function');
const colorToCss = new Function('clamp', stripTypeScriptTypes(colorCode)+';return colorToCss;')((n,lo,hi)=>Math.min(hi,Math.max(lo,n)));
const code = source.slice(source.indexOf('function addStopsTo(')).replaceAll('export function','function');
const Position = Symbol('Position'), Computed = Symbol('Computed'), ColorStop = Symbol('ColorStop');
const funcs = new Function('store','ChildOf','ColorStop','Position','Computed','colorToCss',stripTypeScriptTypes(code)+';return {createLinearGradient,createRadialGradient};')((world,trait)=>world.computed,()=>null,ColorStop,Position,Computed,colorToCss);
const stopSets = [[],[{offset:.3,color:0xFE3209,opacity:.4}], [{offset:0,color:0xff0000,opacity:1},{offset:1,color:0x0000ff,opacity:1}], [{offset:1.2,color:0x00ff00,opacity:1.5},{offset:-.7,color:0xff0000,opacity:-.5},{offset:.2,color:0xffffff,opacity:.6},{offset:2,color:0x0000ff,opacity:0},{offset:1,color:0x000000,opacity:1}]];
const geometries = [
  {}, {rotation:90}, {rotation:45}, {rotation:-33,scaleX:2,scaleY:.5},
  {x:.2}, {y:.7}, {x:-.2,y:1.4,rotation:83,scaleX:-.4,scaleY:2},
  {scaleX:0,scaleY:0}, {scaleX:1e-7,scaleY:-1},
];
const cases=[];
for (const kind of ['linear','radial']) for (const size of [[320,180],[19.5,11.25],[-12,20],[0,0]]) for (const props of geometries) for(const stops of stopSets){
  const hasPosition = Object.hasOwn(props,'x') || Object.hasOwn(props,'y');
  const computed={positionX:[props.x??0],positionY:[props.y??0],scaleX:[props.scaleX??1],scaleY:[props.scaleY??1],rotation:[props.rotation??0],stopOffset:[0],color:[0],opacity:[1]};
  stops.forEach((stop,i)=>{computed.stopOffset[i+1]=stop.offset;computed.color[i+1]=stop.color;computed.opacity[i+1]=stop.opacity;});
  const world={computed,query:()=>stops.map((_,i)=>({id:()=>i+1}))};
  const fill={id:()=>0,has:trait=>trait===Position&&hasPosition};
  let result;
  const capture = (...points)=>{result={points,stops:[]};return{addColorStop:(offset,color)=>result.stops.push({offset,color})};};
  const ctx={createLinearGradient:capture,createRadialGradient:capture};
  funcs[kind==='linear'?'createLinearGradient':'createRadialGradient'](world,fill,ctx,...size);
  cases.push({kind,size,props,stops,result});
}
const hash = text=>createHash('sha256').update(text).digest('hex');
const output=resolve(root,'native/editor/tests/fixtures/gradients.json');
const json=JSON.stringify({sources:{gradients:hash(source),colors:hash(colors)},cases},null,2)+'\n';
if(process.argv.includes('--check')) {if(readFileSync(output,'utf8').replaceAll('\r\n','\n')!==json)throw Error('Original gradients changed; regenerate and review fixture');}
else writeFileSync(output,json);
console.log(`${cases.length} original gradient geometry/stop cases`);
