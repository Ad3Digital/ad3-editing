// Development oracle: original runtime functions, a small ECS fixture, Chromium.
// Normal Canvas acceleration is intentional. Native tests need only the JSON.
const {app,BrowserWindow}=require('electron');
const {readFileSync,writeFileSync}=require('node:fs');
const {resolve}=require('node:path');
const {createHash}=require('node:crypto');
const {stripTypeScriptTypes}=require('node:module');
const root=resolve(__dirname,'../../../..'),sources={};
const read=p=>sources[p]=readFileSync(resolve(root,p),'utf8').replace(/\r\n/g,'\n');
const render=read('packages/runtime/src/systems/render.ts');
const stroke=read('packages/runtime/src/utils/stroke.ts');
const text=read('packages/runtime/src/utils/text.ts');
const adapter=read('packages/reconciler/src/document.ts');
const transform=read('packages/runtime/src/systems/transform.ts');
const common=read('packages/runtime/src/math/common.ts');
const matrix=read('packages/runtime/src/math/matrix2d.ts');
const cut=(s,a,b)=>{const start=s.indexOf(a),end=s.indexOf(b,start+a.length);if(start<0||end<0)throw Error(a);return s.slice(start,end)};
const js=s=>stripTypeScriptTypes(s).replace(/export /g,'');
const effectNames=['LAYER_BLUR','BRIGHTNESS','CONTRAST','GRAYSCALE','HUE_ROTATION','INVERT','SATURATE','SEPIA'];
const EffectType=Object.fromEntries(effectNames.map((s,i)=>[s,i]));
const typeMap=new Function('EffectType',js(cut(adapter,'const EFFECT_TYPES:', '\n};')+'\n};')+'return EFFECT_TYPES;')(EffectType);
const kernels=js(matrix.replace("import { degToRad } from './common';",common))+'\n'+js(`
const Computed='computed',Cache='cache',Effect='effect',Color='color',Hidden='hidden',Geometry='geometry',LocalTransform='local',WorldTransform='world',BlendMode='blend',Paint='paint',StrokeStyle='stroke',Offset='offset',Blur='blur',Opacity='opacity',TextCache='textCache';
const Root='root',Camera='camera',RenderSurface='surface',ClipsContent='clip',IsMask='mask',Culled='culled',Interactive='interactive',Caption='caption',Transition='transition',MixedCornerRadius='mixed';
const GeometryType={RECT:1,TEXT:2},PaintType={SOLID:0,LINEAR_GRADIENT:1,RADIAL_GRADIENT:2,SURFACE:3,HTML:4,IMAGE:5,VIDEO:6},EffectType=${JSON.stringify(EffectType)};
const Flip='flip',Anchor='anchor',getViewMatrix=w=>scale2D(w.scale,w.scale);
const StrokeJoin={MITER:0,ROUND:1,BEVEL:2,0:'MITER',1:'ROUND',2:'BEVEL'},StrokeCap={BUTT:0,ROUND:1,SQUARE:2,0:'BUTT',1:'ROUND',2:'SQUARE'};
const COMPOSITE_OPERATIONS=['source-over','multiply','screen'];
const store=(w,t)=>w[t],getCtx=w=>w.ctx,colorToHex=c=>'#'+c.toString(16).padStart(6,'0');
const getIntrinsicPaint=()=>undefined,renderGenerating=()=>{},renderFills=()=>{};
const applyFont=()=>{};
${cut(transform,'export function computeLocalMatrix(','/**\n * Compute world-space AABB')}
${cut(render,'export function drawRectPath(','function getScaledImageProps(')}
${cut(render,'const EPSILON =','/**\n * The geometry')}
${cut(render,'export function renderIntrinsicFill(','/** What the image')}
${cut(stroke,'export function applyStrokeStyle(', '\n/**\n * The visible')}
${stroke.slice(stroke.indexOf('export function findWidestStroke('))}
${cut(render,'function renderShadows(','/**\n * The pulse')}
${cut(render,'function renderShapeNode(','function renderTextNode(')}
${cut(render,'export function renderNode(','/**\n * Render system entry point.')}
${cut(text,'function renderTokens(','export function renderText(')}
${cut(text,'function getIntrinsicColor(','function getTextCase(')}
`);
const node=(tag,props={},children=[])=>({tag,props,children});
const rect=(props={},children=[])=>node('rect',{x:24,y:24,width:64,height:56,fill:'#b34679',...props},children);
const effect=(type,value,props={})=>node('effect',{type,value,...props});
const shadow=(props={})=>node('shadow',{color:'#284baf',offsetX:18,offsetY:12,blur:8,...props});
const cases=[];
for(const type of ['blur','brightness','contrast','grayscale','hueRotate','invert','saturate','sepia','unknown']) {
 for(const value of (type==='blur'||type==='unknown'?[0,.7,2,4,12]:type==='hueRotate'?[-90,0,45,180,540]:[-1,0,.4,1,2])) {
  cases.push({name:type+':'+value,nodes:[rect({},[effect(type,value)])]});
 }
}
for(const sigma of [.1,.5,1,1.5,3,6,20,32,33,48,128,256])cases.push({name:'blur-prop:'+sigma,nodes:[rect({blur:sigma,opacity:.65})]});
for(const type of ['brightness','contrast','grayscale','hueRotate','invert','saturate','sepia'])for(const opacity of [.1,.3,.7])cases.push({name:type+'-alpha:'+opacity,nodes:[rect({opacity},[effect(type,type==='hueRotate'?87:.7)])]});
cases.push(
 {name:'chain',nodes:[rect({blur:1.5},[effect('hueRotate',90),effect('contrast',.7),effect('sepia',.4),effect('invert',.7),effect('brightness',.8),effect('blur',2)])]},
 {name:'filter-inherited',nodes:[node('group',{},[effect('invert',1),rect({},[rect({x:20,y:20,width:48,height:48,fill:'#18b947'})])])]},
 {name:'filter-replaced',nodes:[node('group',{},[effect('invert',1),rect({},[effect('grayscale',1)])])]},
 {name:'filter-noop-inherits',nodes:[node('group',{},[effect('invert',1),rect({},[effect('brightness',1),effect('sepia',1,{hidden:true})])])]},
 {name:'filter-each-draw',nodes:[rect({opacity:.4},[effect('invert',1),rect({x:16,y:16,width:64,height:56,fill:'#18b947',opacity:.6})])]},
 {name:'filter-mask-order',nodes:[rect({blur:8},[rect({x:32,y:-10,width:60,height:100,mask:true})])]},
 {name:'filter-offcanvas',nodes:[rect({x:-24,y:24,width:36,height:56,blur:8})]},
 {name:'filter-resolution',scale:2,nodes:[rect({blur:4,scaleX:1.2,scaleY:.7})]},
 {name:'filter-blend',nodes:[rect({x:0,y:0,width:128,height:128,fill:'#385218'}),rect({opacity:.6,blendMode:'multiply'},[effect('invert',1)])]},
 {name:'shadow-default',nodes:[rect({},[shadow()])]},
 {name:'shadow-no-fill',nodes:[rect({fill:undefined},[shadow()])]},
 {name:'shadow-no-blur',nodes:[rect({fill:undefined},[shadow({blur:0,opacity:.4})])]},
 {name:'shadow-zero',nodes:[rect({fill:undefined},[shadow({blur:0,offsetX:0,offsetY:0,opacity:.4})])]},
 {name:'shadow-zero-blurred',nodes:[rect({fill:undefined},[shadow({offsetX:0,offsetY:0,opacity:.4})])]},
 {name:'shadow-opacity',nodes:[rect({opacity:.3},[shadow({opacity:.6})])]},
 {name:'shadow-fractional',nodes:[rect({fill:undefined},[shadow({blur:3,offsetX:-8.5,offsetY:13.25})])]},
 {name:'shadow-fractional-zero-blur',nodes:[rect({fill:undefined},[shadow({blur:0,offsetX:-8.5,offsetY:13.25})])]},
 {name:'shadow-integer-three-blur',nodes:[rect({fill:undefined},[shadow({blur:3,offsetX:-8,offsetY:13})])]},
 {name:'shadow-resolution',scale:2,nodes:[rect({},[shadow()])]},
 {name:'shadow-transform',nodes:[rect({rotation:20,scaleX:.8,scaleY:1.2},[shadow()])]},
 {name:'shadow-transform-zero',nodes:[rect({rotation:20,scaleX:.8,scaleY:1.2},[shadow({blur:0})])]},
 {name:'shadow-rotation',nodes:[rect({rotation:20},[shadow()])]},
 {name:'shadow-scale',nodes:[rect({scaleX:.8,scaleY:1.2},[shadow()])]},
 {name:'shadow-multiple-invalid',nodes:[rect({fill:undefined},[shadow({opacity:.4}),shadow({blur:-5,opacity:8,color:'#af7428'}),shadow({hidden:true})])]},
 {name:'shadow-filter',nodes:[rect({fill:undefined},[effect('invert',1),shadow()])]},
 {name:'shadow-filter-blur',nodes:[rect({blur:4,fill:undefined},[shadow({blur:12})])]},
 {name:'shadow-mask-order',nodes:[rect({fill:undefined},[shadow({offsetX:-30,blur:8}),rect({x:-20,y:-20,width:40,height:100,mask:true})])]},
 {name:'shadow-stroke',nodes:[rect({},[shadow(),node('stroke',{color:'#11b040',width:6})])]},
 {name:'shadow-blend',nodes:[rect({x:0,y:0,width:128,height:128,fill:'#bd8020'}),rect({opacity:.7,blendMode:'multiply'},[shadow()])]},
);
const jsx=n=>`<${n.tag} ${Object.entries(n.props).filter(([,v])=>v!==undefined).map(([k,v])=>`${k}={${JSON.stringify(v)}}`).join(' ')}>${n.children.map(jsx).join('')}</${n.tag}>`;
const blurred=n=>(n.props.blur??0)>0||(n.tag==='effect'&&(typeMap[n.props.type]??0)===0&&(n.props.value??0)>0)||n.children.some(blurred);
app.whenReady().then(async()=>{
 const window=new BrowserWindow({show:false,webPreferences:{sandbox:true,contextIsolation:true,nodeIntegration:false}});
 await window.loadURL('about:blank');
 const results=await window.webContents.executeJavaScript(`(()=>{try {${kernels}
 const typeMap=${JSON.stringify(typeMap)};
 const put=(s,id,v)=>{for(const[k,x]of Object.entries(v))(s[k]??=[])[id]=x};
 function setup(c){
  const scale=c.scale??1,ctx=new OffscreenCanvas(128*scale,128*scale).getContext('2d');
  const w={ctx,scale,computed:{},cache:{children:[],masks:[],effects:[],shadows:[],strokes:[],fills:[]},effect:{type:[]},geometry:{value:[]},local:{},world:{},flip:{x:[],y:[]},anchor:{x:[],y:[]},blend:{value:[]},paint:{value:[]},stroke:{join:[],cap:[],miterLimit:[]},entities:[]};
  for(const key of ['a','b','c','d','e','f']){w.local[key]=[];w.world[key]=[];}
  w.get=t=>t===Root?{get:()=>({a:1})}:t===RenderSurface?{resolution:scale}:undefined;
  function mount(n,parent,world){
   const id=w.entities.length,p=n.props;
   const e={id:()=>id,has:t=>e.traits.has(t),traits:new Set(),get:()=>undefined};w.entities.push(e);
   if(p.hidden)e.traits.add(Hidden);if(p.fill!==undefined)e.traits.add(Color);if(p.mask)e.traits.add(IsMask);
   if(n.tag==='scene')e.traits.add(ClipsContent);
   const blend=COMPOSITE_OPERATIONS.indexOf(p.blendMode??'source-over');if(blend>0){e.traits.add(BlendMode);w.blend.value[id]=blend;}
   const color=typeof(p.color??p.fill)==='number'?(p.color??p.fill):parseInt((p.color??p.fill??'#000000').slice(1),16);
   put(w.computed,id,{width:p.width??0,height:p.height??0,cornerRadius:p.cornerRadius??0,color,blur:p.blur??0,offsetX:p.offsetX??0,offsetY:p.offsetY??0,opacity:p.opacity??1,value:p.value??0,strokeWidth:p.width??1,visibility:1,positionX:p.x??0,positionY:p.y??0,rotation:p.rotation??0,scaleX:p.scaleX??1,scaleY:p.scaleY??1,skewX:0,skewY:0});
   w.effect.type[id]=typeMap[p.type]??0;
   computeLocalMatrix(w,e);computeWorldTransform(w,e,parent);
   for(const key of ['children','masks','effects','shadows','strokes','fills'])w.cache[key][id]=[];
   if(parent){const key={effect:'effects',shadow:'shadows',stroke:'strokes'}[n.tag]??(p.mask?'masks':'children');w.cache[key][parent.id()].push(e);}
   w.geometry.value[id]=['rect','scene'].includes(n.tag)?GeometryType.RECT:0;
   for(const child of n.children)mount(child,e);return e;
  }
  const scene=mount({tag:'scene',props:{width:128,height:128},children:c.nodes},null,new DOMMatrix().scale(scale));
  ctx.scale(scale,scale);renderNode(w,scene);
  const all=ctx.getImageData(0,0,128*scale,128*scale).data,samples=[];
  for(let y=1;y<128*scale;y+=8)for(let x=1;x<128*scale;x+=4){
   const i=(y*128*scale+x)*4,sample=[x,y,...all.slice(i,i+4)];
   if(['shadow-transform','shadow-transform-zero','shadow-rotation','shadow-scale'].includes(c.name)) {
    const lo=[255,255,255,255],hi=[0,0,0,0];
    for(let dy=-1;dy<=1;dy++)for(let dx=-1;dx<=1;dx++){const k=((y+dy)*128*scale+x+dx)*4,a=all[k+3];for(let j=0;j<4;j++){const v=j===3?a:all[k+j]*a/255;lo[j]=Math.min(lo[j],Math.floor(v));hi[j]=Math.max(hi[j],Math.ceil(v));}}
    sample.push(...lo,...hi);
   }
   samples.push(sample);
  }
  return samples;
 }
 return ${JSON.stringify(cases)}.map(setup);
 }catch(e){return {error:e.stack}}})()`);
 if(!Array.isArray(results))throw Error(results.error);
 const output=resolve(__dirname,'../fixtures/effects.json');
 const data=JSON.stringify({electron:process.versions.electron,chromium:process.versions.chrome,hashes:Object.fromEntries(Object.entries(sources).map(([p,s])=>[p,createHash('sha256').update(s).digest('hex')])),cases:cases.map(({nodes,...c},i)=>({...c,tolerance:nodes.some(blurred)?4:2,source:nodes.map(jsx).join(''),samples:results[i]}))})+'\n';
 if(process.argv.includes('--check')){if(readFileSync(output,'utf8').replace(/\r\n/g,'\n')!==data)throw Error('Effect oracle differs; regenerate and review');}else writeFileSync(output,data);
 console.log(cases.length+' original-runtime Canvas effect/shadow cases');app.quit();
}).catch(e=>{console.error(e);app.exit(1)});
