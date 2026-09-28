// Development-only oracle: actual adapter cases, time/cache functions, transforms,
// visibility and renderNode run against small ECS fixtures and Chromium Canvas.
// Native tests consume the committed JSON without JavaScript or Electron.
const {app,BrowserWindow}=require('electron');
process.on('uncaughtException',e=>{console.error(e);app.exit(1)});
const {readFileSync,writeFileSync}=require('node:fs');
const {resolve}=require('node:path');
const {createHash}=require('node:crypto');
const {stripTypeScriptTypes}=require('node:module');
const root=resolve(__dirname,'../../../..'),sources={};
const read=p=>sources[p]=readFileSync(resolve(root,p),'utf8').replace(/\r\n/g,'\n');
const render=read('packages/runtime/src/systems/render.ts');
const timing=read('packages/runtime/src/actions/timing.ts');
const cache=read('packages/runtime/src/actions/cache.ts');
const playback=read('packages/runtime/src/systems/playback.ts');
const adapter=read('packages/reconciler/src/document.ts');
const transform=read('packages/runtime/src/systems/transform.ts');
const common=read('packages/runtime/src/math/common.ts');
const matrix=read('packages/runtime/src/math/matrix2d.ts');
const cut=(s,a,b)=>{const start=s.indexOf(a),end=s.indexOf(b,start+a.length);if(start<0||end<0)throw Error(a);return s.slice(start,end)};
const js=s=>stripTypeScriptTypes(s).replace(/export /g,'');
const kernels=js(matrix.replace("import { degToRad } from './common';",common))+'\n'+js(`
const Computed='computed',LocalTransform='local',WorldTransform='world',Flip='flip',Anchor='anchor',Cache='cache';
const Geometry='geometry',Group='group',AdjustmentLayer='adjustment',GeometryType={RECT:1,TEXT:2};
const Culled='culled',Interactive='interactive',IsMask='mask',Hidden='hidden',BlendMode='blend',Caption='caption',ClipsContent='clip',Transition='transition',MixedCornerRadius='mixed',Dragging='dragging';
const Delay='delay',Trim='trim',PlaybackRate='rate',DEFAULT_DURATION_FRAMES=480;
const store=(w,t)=>w[t],getCtx=w=>w.ctx,getParentNode=e=>e.parent??null,isGroupLike=e=>e.has(Group);
const findAssetDuration=()=>null,buildEffects=()=>null,isStage=()=>false,sortByItemIndex=(a,b)=>a.id()-b.id();
const COMPOSITE_OPERATIONS=['source-over','multiply'];
const Or=(...t)=>({or:t}),ChildOf=p=>({parent:p});
const getViewMatrix=w=>scale2D(w.scale,w.scale);
${cut(timing,'function stored(','/**\n * Pin whatever')}
${cut(cache,'export function refileMask(','function isNodeEntity(')}
${cut(transform,'export function computeLocalMatrix(','/**\n * Compute world-space AABB')}
${cut(playback,'function updateVisibility(','function resetDecoders(')}
${cut(render,'export function drawRectPath(','function getScaledImageProps(')}
${cut(render,'export function renderNode(','/**\n * Render system entry point.')}
function renderShapeNode(w,e){drawRectPath(w,e);if(e.props.fill){w.ctx.fillStyle=e.props.fill;w.ctx.fill()} if(e.props.testStroke){w.ctx.strokeStyle='white';w.ctx.lineWidth=e.props.testStroke;w.ctx.stroke()}}
`);
// Execute the original rect creation and mask setter cases themselves. Their
// absence of default-size/timing changes contradicts the current JSX prose.
const defaults=new Function('return '+js(`(function(){
const Geometry='geometry',Position='position',IsMask='mask',GeometryType={RECT:1};
const createEntity=()=>({add(){},set(){},remove(){}});
let size,entity;const resizeEntity=(w,e,s)=>size=s;
switch('rect'){${cut(adapter,"case 'rect': {","case 'text': {")}}
const node={entity},value=true;
(function(){switch('mask'){${cut(adapter,"case 'mask': {","case 'keepAspectRatio': {")}}}).call({stage:{entity:null}});
return size;
})`))().call({world:{}});
const rect=(props={},children=[])=>({tag:'rect',props,children});
const holder=(masks,props={})=>rect({x:8,y:8,width:96,height:80,end:30,fill:'red',...props},[
 rect({x:40,y:25,width:64,height:60,fill:'blue'}),...masks]);
const variants=[
 ['default',holder([rect({mask:true})])],
 ['single',holder([rect({mask:true,x:15,y:10,width:42,height:48})])],
 ['hidden',holder([rect({mask:true,hidden:true,opacity:0,fill:'lime',blendMode:'multiply',x:15,y:10,width:42,height:48},[rect({width:128,height:128,fill:'lime'})])])],
 ['multiple',holder([rect({mask:true,x:10,y:5,width:60,height:50}),rect({mask:true,x:30,y:20,width:60,height:60})])],
 ['rounded',holder([rect({mask:true,x:10,y:5,width:70,height:70,cornerRadius:30})])],
 ['rotated',holder([rect({mask:true,x:20,y:10,width:55,height:50,rotation:32,scaleX:.8,scaleY:1.2})],{rotation:-12})],
 ['zero',holder([rect({mask:true,width:0,height:30})])],
 ['negative',holder([rect({mask:true,x:60,y:60,width:-40,height:-40})])],
 ['singular',holder([rect({mask:true,width:50,height:50,scaleX:0})])],
 ['timed',holder([rect({mask:true,x:15,width:42,height:60,start:1,end:2})])],
 ['nested',rect({width:112,height:100,fill:'red'},[rect({mask:true,x:20,y:8,width:80,height:80}),holder([rect({mask:true,width:60,height:60})])])],
 ['mask-on-mask',holder([rect({mask:true,x:15,width:70,height:60},[rect({mask:true,width:0,height:0})])])],
 ['stroke',holder([rect({mask:true,x:-5,y:-5,width:58,height:80})],{testStroke:12})],
 ['sibling',rect({width:100,height:100},[holder([rect({mask:true,width:40,height:40})]),rect({x:80,y:90,width:32,height:20,fill:'lime'})])],
];
const jsx=n=>`<${n.tag} ${Object.entries(n.props).filter(([k])=>k!=='testStroke').map(([k,v])=>`${k}={${JSON.stringify(v)}}`).join(' ')}>${n.props.testStroke?`<stroke color="white" width={${n.props.testStroke}}/>`:''}${n.children.map(jsx).join('')}</${n.tag}>`;
const cases=variants.flatMap(([name,node])=>[0,1,2,17].map(time=>({name:name+'@'+time,time,scale:name==='rotated'?1.5:1,node,source:jsx(node)})));
app.whenReady().then(async()=>{
 const window=new BrowserWindow({show:false,webPreferences:{sandbox:true,contextIsolation:true,nodeIntegration:false}});
 window.webContents.on('console-message',event=>console.error(event.message));
 await window.loadURL('about:blank');
 const results=await window.webContents.executeJavaScript(`(()=>{try {${kernels}
 const put=(s,id,v)=>{for(const[k,x]of Object.entries(v))(s[k]??=[])[id]=x};
 function setup(c){
  const w={scale:c.scale,ctx:new OffscreenCanvas(128*c.scale,128*c.scale).getContext('2d'),computed:{},local:{},world:{},flip:{x:[],y:[]},anchor:{x:[],y:[]},cache:{children:[],masks:[]},geometry:{value:[]},blend:{value:[]},entities:[]};
  for(const key of ['a','b','c','d','e','f']){w.local[key]=[];w.world[key]=[]}
  for(const key of ['start','end','origin','duration','playbackRate','localTime','visibility'])w.computed[key]=[];
  w.query=(...filters)=>w.entities.filter(e=>filters.every(f=>f.or?f.or.some(t=>e.has(t)):e.parent===f.parent));
  function mount(n,parent){const id=w.entities.length,p=n.props;
   const e={id:()=>id,parent,props:p,traits:new Set([Geometry,...(n.tag==='scene'?[ClipsContent,Group]:[]),...(p.hidden?[Hidden]:[])]),has(t){return this.traits.has(t)},add(t){this.traits.add(t)},remove(t){this.traits.delete(t)},get(t){if(t===Cache)return {children:w.cache.children[id],masks:w.cache.masks[id]};return this[t]}};
   w.entities.push(e);w.cache.children[id]=[];w.cache.masks[id]=[];
   if(parent)w.cache.children[parent.id()].push(e);
   e.delay=(p.start??0)*30;e.trim={start:0,end:p.end===undefined?null:((p.end??0)-(p.start??0))*30};e.get=function(t){if(t===Cache)return {children:w.cache.children[id],masks:w.cache.masks[id]};if(t===Delay)return {value:this.delay};if(t===Trim)return this.trim;return undefined};
   if(p.mask){e.add(IsMask);refileMask(w,e,parent,true)}
   put(w.computed,id,{width:p.width??100,height:p.height??100,positionX:p.x??0,positionY:p.y??0,offsetX:0,offsetY:0,rotation:p.rotation??0,scaleX:p.scaleX??1,scaleY:p.scaleY??1,skewX:0,skewY:0,cornerRadius:p.cornerRadius??0,opacity:p.opacity??1});
   w.geometry.value[id]=GeometryType.RECT;computeLocalMatrix(w,e);computeWorldTransform(w,e,parent);
   for(const child of n.children)mount(child,e);return e;
  }
  const scene=mount({tag:'scene',props:{width:128,height:128,end:30},children:[c.node]},null);
  propagateTimeRangeDown(w,scene);w.computed.localTime[0]=c.time*30;updateVisibility(w,scene,scene);w.ctx.setTransform(c.scale,0,0,c.scale,0,0);renderNode(w,scene);
  const all=w.ctx.getImageData(0,0,128*c.scale,128*c.scale).data;
  const samples=[];for(let y=3;y<128*c.scale;y+=16)for(let x=3;x<128*c.scale;x+=16){
   const offset=(y*128*c.scale+x)*4,lo=[255,255,255,255],hi=[0,0,0,0];
   // Compare interiors exactly; permit only the local one-pixel raster boundary,
   // rather than a large global channel tolerance for different AA algorithms.
   for(let dy=-1;dy<=1;dy++)for(let dx=-1;dx<=1;dx++){
    const k=((y+dy)*128*c.scale+x+dx)*4,alpha=all[k+3];
    for(let j=0;j<4;j++){const v=j===3?alpha:all[k+j]*alpha/255;lo[j]=Math.min(lo[j],Math.floor(v));hi[j]=Math.max(hi[j],Math.ceil(v))}
   }
   samples.push({x,y,rgba:Array.from(all.slice(offset,offset+4)),nearbyMin:lo,nearbyMax:hi});
  }
  const alphas=new Set();for(let i=3;i<all.length;i+=4)alphas.add(all[i]);
  return {samples,alphaValues:[...alphas].sort((a,b)=>a-b),matrices:w.entities.map(e=>Object.fromEntries(Object.entries(w.world).map(([k,v])=>[k,v[e.id()]]))),spans:w.entities.map(e=>({mask:e.has(IsMask),start:w.computed.start[e.id()],end:w.computed.end[e.id()]}))};
 }
 return ${JSON.stringify(cases)}.map(setup);
}catch(e){return {error:e.stack}}})()`);
 if(!Array.isArray(results))throw Error(results.error);
 const outputIndex=process.argv.indexOf('--output');
 const output=outputIndex<0?resolve(__dirname,'../fixtures/masks.json'):resolve(process.argv[outputIndex+1]);
 const data=JSON.stringify({electron:process.versions.electron,chromium:process.versions.chrome,hashes:Object.fromEntries(Object.entries(sources).map(([p,s])=>[p,createHash('sha256').update(s).digest('hex')])),defaults,cases:cases.map(({node,...c},i)=>({...c,...results[i]}))},null,2)+'\n';
 if(process.argv.includes('--check')){if(readFileSync(output,'utf8').replace(/\r\n/g,'\n')!==data)throw Error('Mask oracle differs; regenerate and review')}
 else writeFileSync(output,data);
 console.log(cases.length+' original-runtime Canvas mask cases; default '+JSON.stringify(defaults));app.quit();
}).catch(e=>{console.error(e);app.exit(1)});
