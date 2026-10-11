// Original bindAsset/findAssetDuration/forwardDecoders and fill consumer oracle.
// The native tests need only the committed data; no JavaScript runs in Rust.
const {readFileSync,writeFileSync}=require('node:fs');
const {resolve}=require('node:path');
const {createHash}=require('node:crypto');
const {stripTypeScriptTypes}=require('node:module');
const root=resolve(__dirname,'../../../..'),sources={};
const read=p=>sources[p]=readFileSync(resolve(root,p),'utf8').replace(/\r\n/g,'\n');
const assets=read('packages/runtime/src/actions/assets.ts'),time=read('packages/runtime/src/utils/time.ts'),playback=read('packages/runtime/src/systems/playback.ts');
const render=read('packages/runtime/src/systems/render.ts'),text=read('packages/runtime/src/utils/text.ts');
const cut=(s,a,b)=>{const start=s.indexOf(a),end=s.indexOf(b,start+a.length);if(start<0||end<0)throw Error(a);return s.slice(start,end)};
const js=s=>stripTypeScriptTypes(s).replace(/export /g,'');
const kernel=js(`
const Paint='paint',Geometry='geometry',Audio='audio',Hidden='hidden',Culled='culled',Mode='mode',Caption='caption',Group='group',AdjustmentLayer='adjustment',Library='library',FrameRate='fps',AssetId='asset',SourceFrameRate='sourceFps',Cache='cache';
const PaintType={IMAGE:1,VIDEO:2,HTML:3,SHADER:4,SURFACE:5,SOLID:6,LINEAR_GRADIENT:7,RADIAL_GRADIENT:8,WAVEFORM:9};
const ChildOf=p=>({parent:p}),Not=t=>({not:t}),Or=(...t)=>({or:t});
const store=(w,t)=>w[t],secondsToFrames=(s,fps)=>Math.round(s*fps);
const forwardVideoDecoder=(w,s,e,f)=>w.visual.push(f.id()),forwardImageDecoder=forwardVideoDecoder;
const forwardAudioDecoder=(w,s,e,f)=>w.audio=(f??e).id();
${cut(assets,'export function bindAsset(','/**\n * The rate')}
${cut(assets,'export function getSequenceFrameRate(','/** The asset with')}
${cut(time,'export function getIntrinsicPaint(','/** Whether')}
${cut(time,'export function findAssetDuration(','/**\n * The modifiers')}
${cut(playback,'function forwardDecoders(','function updateVisibility(')}
`);
const evaluate=new Function('c',kernel+`
 const all=[],assetMap=new Map();const w={paint:{value:[]},visual:[],audio:null};
 w.get=t=>t===Library?{get:id=>assetMap.get(id)}:t===FrameRate?{value:30}:t===Mode?{value:'offline'}:undefined;
 function mount(id,tag,kind,duration,hidden,parent){
  const traits=new Map();if(tag==='audio')traits.set(Audio,{});else if(parent===null)traits.set(Geometry,{});
  if(tag!=='audio'&&kind)traits.set(Paint,{value:tag.startsWith('image')?PaintType.IMAGE:PaintType.VIDEO});
  if(hidden)traits.set(Hidden,{});
  const e={id:()=>id,parent,has:t=>traits.has(t),get:t=>traits.get(t),add:t=>{if(!traits.has(t))traits.set(t,{})},set:(t,v)=>{traits.set(t,v);if(t===Paint)w.paint.value[id]=v.value;}};
  if(kind){assetMap.set(id,{id,type:kind,duration,frameRate:24});bindAsset(e,assetMap.get(id));w.paint.value[id]=e.get(Paint)?.value;}
  all.push(e);return e;
 }
 const holder=mount('holder',c.tag,c.own,c.ownDuration,false,null);
 const fills=c.fills.map((f,i)=>mount('fill'+i,f.tag,f.kind,f.duration,f.hidden,holder));holder.set(Cache,{fills});
 w.query=(...filters)=>all.filter(e=>filters.every(f=>typeof f==='string'?e.has(f):f.parent?e.parent===f.parent:f.not?!e.has(f.not):f.or.some(t=>e.has(t))));
 const duration=findAssetDuration(w,holder);forwardDecoders(w,holder,holder);
 return {duration,audio:w.audio,visual:w.visual,boundPaints:all.map(e=>e.get(Paint)?.value??null)};
`);
const cases=[];
for(const [tag,own] of [['rect',null],['image','VIDEO'],['video','IMAGE'],['video','VIDEO'],['audio','AUDIO']])
 for(const alias of ['imagePaint','videoPaint'])
 for(const first of [null,'IMAGE','VIDEO','SEQUENCE'])for(const last of [null,'IMAGE','VIDEO'])for(const hidden of [false,true]) {
  const fills=[];if(first)fills.push({tag:alias,kind:first,duration:2,hidden});if(last)fills.push({tag:alias,kind:last,duration:3,hidden:false});
  const c={tag,own,ownDuration:1.25,fills};cases.push({...c,...evaluate(c)});
 }
// Record original shape/text fill dispatch with mixed media tags. The geometry,
// source pixels and clocks are tested separately against the native draw path.
const consumers=js(`
const Computed='computed',Cache='cache',Paint='paint',BlendMode='blend',Hidden='hidden',Color='color',Opacity='opacity',Offset='offset',Blur='blur',TextCache='textCache',Root='root',Camera='camera',RenderSurface='surface';
const PaintType={IMAGE:1,VIDEO:2,SOLID:3,LINEAR_GRADIENT:4,RADIAL_GRADIENT:5},COMPOSITE_OPERATIONS=['source-over','multiply'];
const store=(w,t)=>w[t],getCtx=w=>w.ctx,colorToHex=c=>'#'+c.toString(16).padStart(6,'0');
const shaderConsumesFill=()=>false,applyFont=()=>{},findWidestStroke=()=>null;
const renderMedia=(w,e,f,kind)=>w.calls.push(['media',f.id(),kind,w.ctx.globalAlpha,w.ctx.globalCompositeOperation]);
${cut(render,'export function renderFills(','/**\n * Whether')}
${cut(text,'function renderTokens(','export function renderText(')}
${cut(text,'function getIntrinsicColor(','function getTextCase(')}
`);
const traces=new Function(consumers+`
 const w={calls:[],ctx:{globalAlpha:.8,globalCompositeOperation:'source-over',fillStyle:'black',save(){},restore(){},fill(){w.calls.push(['solid',this.fillStyle,this.globalAlpha,this.globalCompositeOperation])},fillText(){w.calls.push(['text',this.fillStyle,this.globalAlpha,this.globalCompositeOperation])}},computed:{opacity:[1,.5,.25],color:[0,0xff0000,0x00ff00],width:[100],height:[100]},paint:{value:[0,1,2]},blend:{value:[0,1,0]},color:{value:[0,0x123456,0xabcdef]},opacity:{value:[1,.5,.25]},offset:{},blur:{},textCache:{tokens:[[[{chars:'A',x:0,y:0,ranges:[]}]]]},cache:{fills:[],strokes:[],shadows:[]}};
 const holder={id:()=>0,has:()=>false},a={id:()=>1,has:t=>t==='opacity'||t==='blend'},b={id:()=>2,has:t=>t==='opacity'};
 w.cache.fills[0]=[a,b];w.get=t=>t==='root'?{get:()=>({a:1})}:t==='surface'?{resolution:1}:undefined;
 renderFills(w,holder);const shapes=w.calls;w.calls=[];renderTokens(w.ctx,w,holder);return {shapes,text:w.calls};
`)();
const output=resolve(__dirname,'../fixtures/media-paints.json');
const data=JSON.stringify({hashes:Object.fromEntries(Object.entries(sources).map(([p,s])=>[p,createHash('sha256').update(s).digest('hex')])),cases,traces},null,2)+'\n';
if(process.argv.includes('--check')){if(readFileSync(output,'utf8').replace(/\r\n/g,'\n')!==data)throw Error('Media paint oracle differs');}else writeFileSync(output,data);
console.log(cases.length+' original media binding/duration/decoder cases and shape/text fill traces');
