// Development-only oracle: execute the original fitting function, not a copy.
import {readFileSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {stripTypeScriptTypes} from 'node:module';
const root=resolve(import.meta.dirname,'../../../..');
const file=resolve(root,'packages/runtime/src/systems/render.ts');
const code=readFileSync(file,'utf8');
const fn=code.slice(code.indexOf('function getScaledImageProps('),code.indexOf('const EPSILON'));
const original=new Function('ScaleModeType',stripTypeScriptTypes(fn)+';return getScaledImageProps;')({COVER:0,FILL:1,FIT:2,NONE:3});
const cases=[];
for(const [mode,value] of [['cover',0],['fill',1],['contain',2]]){
  for(const [source,target] of [ [[16,9],[32,32]],[[9,16],[32,32]],[[640,480],[19.5,11.5]],[[1,1],[0,10]],[[16,9],[-32,-18]],[[16,9],[-32,18]],[[16,9],[32,-18]],[[17,31],[0,0]] ]){
    cases.push({mode,source,target,result:original(value,...source,...target)});
  }
}
const json=JSON.stringify(cases,null,2)+'\n';
const output=resolve(root,'native/editor/tests/fixtures/media-fit.json');
if(process.argv.includes('--check')){
  if(readFileSync(output,'utf8').replaceAll('\r\n','\n')!==json)throw Error('Original media fit changed; regenerate/review fixture');
  console.log('Original media fitting matches fixture');
}else{writeFileSync(output,json);console.log(`Wrote ${cases.length} original fit cases`);}
