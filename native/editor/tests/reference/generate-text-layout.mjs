// Development-only oracle: execute the original Token/tokenize/shape functions.
import { readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { stripTypeScriptTypes } from 'node:module';
import { createHash } from 'node:crypto';
const root=resolve(import.meta.dirname,'../../../..');
const source=readFileSync(resolve(root,'packages/runtime/src/utils/text.ts'),'utf8').replace(/\r\n/g,'\n');
const extract=(start,end)=>source.slice(source.indexOf(start),source.indexOf(end));
const code=`
const Computed='computed',TextStyle='style',TextCache='cache',Size='size';
const TextAlign={LEFT:0,CENTER:1,RIGHT:2},TextBaseline={TOP:0,MIDDLE:1,BOTTOM:2,ALPHABETIC:3};
const store=(world,trait)=>world[trait];
let state;
const getMeasureCtx=()=>({measureText:()=>state.metrics[state.index++]});
const applyFont=()=>{}; const getTextCase=()=>0; const transformText=text=>text;
const createRenderSplits=()=>[{words:tokenize(state.text),ranges:[]}];
${extract('export class Token {','function applyFont(')}
${extract('function shapeTokens(','/** Renders text tokens')}
${extract('function tokenize(input:','export function transformText(')}
export function run(input){
  state={...input,index:0};
  const world={computed:{width:[input.size[0]],height:[input.size[1]]},style:{leading:[input.leading],textAlign:[input.align],textBaseline:[input.baseline]},cache:{tokens:[]},size:{width:[input.max_width??undefined]}};
  const entity={id:()=>0,has:trait=>trait===Size?input.authored:true,add:()=>{}};
  tokenizeText(world,entity);shapeTokens(world,entity);
  const tokens=world.cache.tokens[0].flatMap((line,index)=>line.map(token=>({line:index,x:token.x,y:token.y,bounds:[token.left,token.top,token.right,token.bottom]})));
  return {size:[world.computed.width[0],world.computed.height[0]],tokens};
}
export {tokenize};
`;
const original=await import('data:text/javascript;base64,'+Buffer.from(stripTypeScriptTypes(code)).toString('base64'));
let seed=0xAD3A026;
const random=()=>{seed=(Math.imul(seed,1664525)+1013904223)>>>0;return seed/2**32;};
const pick=values=>values[Math.floor(random()*values.length)];
const cases=[];
for(let index=0;index<192;index++){
  const text=pick(['Hello world again','A\nB\n','\n\n','a  b   c','', 'Longword x y z']);
  const parts=original.tokenize(text).flatMap(word=>word.split('\n').map((text,i,all)=>({text,newline_after:i<all.length-1})));
  const metrics=parts.map(part=>({width:part.text?pick([2.25,8,24.1,-2,0,11.333]):0,fontBoundingBoxAscent:pick([6,15,1.125,-1]),fontBoundingBoxDescent:pick([4,7.25,12]),actualBoundingBoxLeft:pick([0,1.2,-3]),actualBoundingBoxRight:pick([2.3,17,0]),actualBoundingBoxAscent:pick([2,9.125,-1]),actualBoundingBoxDescent:pick([0,3.3,7])}));
  const input={text,metrics,max_width:pick([null,0,10,25,99.3,-2]),authored:pick([false,true]),size:[pick([0,10,20.2,80,-10,4294967297]),pick([0,20,80,-2])],leading:pick([.75,1,1.2,2]),align:index%3,baseline:index%4};
  const expected=original.run(input);
  cases.push({input:{...input,metrics:undefined,text:undefined,pieces:parts.map((part,i)=>({newline_after:part.newline_after,metrics:{width:metrics[i].width,ascent:metrics[i].fontBoundingBoxAscent,descent:metrics[i].fontBoundingBoxDescent,left:metrics[i].actualBoundingBoxLeft,right:metrics[i].actualBoundingBoxRight,ink_ascent:metrics[i].actualBoundingBoxAscent,ink_descent:metrics[i].actualBoundingBoxDescent}}))},expected});
}
writeFileSync(resolve(root,'native/editor/tests/fixtures/text-layout.json'),JSON.stringify({source_sha256:createHash('sha256').update(source).digest('hex'),cases},null,2)+'\n');
