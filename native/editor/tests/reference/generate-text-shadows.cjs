// Original renderTokens/getShadows/getStrokes/findWidestStroke command oracle.
// A Canvas-state recorder isolates styling semantics from font rasterization.
const {readFileSync,writeFileSync}=require('node:fs');
const {resolve}=require('node:path');
const {createHash}=require('node:crypto');
const {stripTypeScriptTypes}=require('node:module');
const root=resolve(__dirname,'../../../..'),sources={};
const read=p=>sources[p]=readFileSync(resolve(root,p),'utf8').replace(/\r\n/g,'\n');
const text=read('packages/runtime/src/utils/text.ts'),stroke=read('packages/runtime/src/utils/stroke.ts');
const cut=(s,a,b)=>{const start=s.indexOf(a),end=s.indexOf(b,start+a.length);if(start<0||end<0)throw Error(a);return s.slice(start,end)};
const code=stripTypeScriptTypes(`
const Computed='computed',Cache='cache',Color='color',Hidden='hidden',BlendMode='blend',Paint='paint',StrokeStyle='stroke',Offset='offset',Blur='blur',Opacity='opacity',TextCache='textCache',Root='root',Camera='camera',RenderSurface='surface';
const PaintType={LINEAR_GRADIENT:1,RADIAL_GRADIENT:2},COMPOSITE_OPERATIONS=['source-over'];
const StrokeJoin={MITER:0,0:'MITER'},StrokeCap={BUTT:0,0:'BUTT'};
const store=(w,t)=>w[t],colorToHex=c=>'#'+c.toString(16).padStart(6,'0'),applyFont=()=>{};
${stroke.slice(stroke.indexOf('export function applyStrokeStyle('))}
${cut(text,'function renderTokens(','export function renderText(')}
${cut(text,'function getIntrinsicColor(','function getTextCase(')}
`).replace(/export /g,'');
const renderTokens=new Function(code+'return renderTokens;')();
const n=(tag,props={},children=[])=>({tag,props,children});
const shadow=props=>n('shadow',{color:0xff0000,offsetX:10,blur:4,...props});
const strokeNode=props=>n('stroke',{color:0x0000ff,width:4,...props});
const cases=[
 {name:'plain',children:[shadow({})]},
 {name:'invalid-state',children:[shadow({opacity:.4}),shadow({color:0x00ff00,blur:-1,opacity:5})]},
 {name:'widest-visible',children:[shadow({}),strokeNode({width:9,opacity:0}),strokeNode({width:20,hidden:true}),strokeNode({width:3})]},
 {name:'range-replacement',children:[shadow({}),n('textRange',{start:2,end:3},[shadow({color:0x00ff00,offsetY:8})])]},
 {name:'hidden-range-list',children:[shadow({}),n('textRange',{start:2,end:3,hidden:true},[shadow({hidden:true})])]},
 {name:'range-invalid-width',children:[shadow({}),strokeNode({width:9}),n('textRange',{start:2,end:3},[strokeNode({width:-1})])]},
 {name:'range-retained-blur',children:[shadow({blur:12}),n('textRange',{start:2,end:3},[shadow({blur:-1})])]},
 {name:'authored-shadow-computed-stroke',children:[shadow({id:'s1',color:0x284baf,opacity:.6}),strokeNode({id:'t1',width:1,opacity:.2})],overrides:{s1:{color:0x00ff00,blur:20,offsetX:30,opacity:.1},t1:{width:10}}},
];
let next=0;for(const c of cases){const assign=n=>{n.props.id??='style'+next++;n.children.forEach(assign)};c.children.forEach(assign);}
const jsx=n=>`<${n.tag} ${Object.entries(n.props).map(([k,v])=>`${k}={${JSON.stringify(v)}}`).join(' ')}>${n.children.map(jsx).join('')}</${n.tag}>`;
for(const c of cases){
 const w={computed:{},cache:{shadows:[],strokes:[],fills:[]},color:{value:[]},blend:{value:[]},paint:{value:[]},stroke:{join:[],cap:[],miterLimit:[]},offset:{x:[],y:[]},blur:{value:[]},opacity:{value:[]},textCache:{tokens:[]}};
 const all=[];
 const put=(s,id,v)=>{for(const[k,x]of Object.entries(v))(s[k]??=[])[id]=x};
 function mount(n){const id=all.length,p=n.props,traits=new Set();const e={id:()=>id,has:t=>traits.has(t),node:n};all.push(e);
  if(p.hidden)traits.add('hidden');if(p.color!==undefined)traits.add('color');if(p.blur!==undefined)traits.add('blur');if(p.opacity!==undefined)traits.add('opacity');if(p.offsetX!==undefined||p.offsetY!==undefined)traits.add('offset');
  const sampled={...p,...c.overrides?.[p.id]};
  put(w.computed,id,{color:sampled.color,width:200,height:80,strokeWidth:sampled.width});
  put(w.color,id,{value:p.color??0});put(w.blur,id,{value:p.blur??0});put(w.opacity,id,{value:p.opacity??1});put(w.offset,id,{x:p.offsetX??0,y:p.offsetY??0});
  const children=n.children.map(mount);for(const [tag,key]of [['shadow','shadows'],['stroke','strokes'],['solidPaint','fills']])w.cache[key][id]=children.filter(e=>e.node.tag===tag);return e;
 }
 const entity=mount(n('text',{color:0xabcdef,opacity:.6},c.children));
 w.textCache.tokens[0]=[[{chars:'A ',x:0,y:0,ranges:[]},{chars:'B',x:20,y:0,ranges:all.filter(e=>e.node.tag==='textRange')}]];
 w.get=t=>t==='root'?{get:()=>({a:1})}:t==='surface'?{resolution:1}:undefined;
 let state={globalAlpha:.6,shadowBlur:0,shadowOffsetX:0,shadowOffsetY:0,shadowColor:'transparent',fillStyle:'#000000',strokeStyle:'#33aa55',lineWidth:2,globalCompositeOperation:'source-over'};
 const stack=[],draws=[],ctx={save(){stack.push({...state})},restore(){state=stack.pop()}};
 for(const key of Object.keys(state))Object.defineProperty(ctx,key,{get:()=>state[key],set:v=>{if(key==='globalAlpha'&&!(v>=0&&v<=1)||key==='shadowBlur'&&v<0||key==='lineWidth'&&v<=0)return;state[key]=v}});
 for(const op of ['fill','stroke'])ctx[op+'Text']=()=>draws.push([op==='stroke',state[op+'Style'],state.globalAlpha,state.shadowColor,state.shadowBlur,state.shadowOffsetX,state.shadowOffsetY,op==='stroke'?state.lineWidth:null]);
 renderTokens(ctx,w,entity);
 c.draws=draws;c.source=`<rect width={256} height={128}><stroke color={0x33aa55} width={2}/><text width={220} height={80} fontSize={36} opacity={0.6} color={0xabcdef}>A B${c.children.map(jsx).join('')}</text></rect>`;
 delete c.children;
}
const output=resolve(__dirname,'../fixtures/text-shadows.json');
const data=JSON.stringify({hashes:Object.fromEntries(Object.entries(sources).map(([p,s])=>[p,createHash('sha256').update(s).digest('hex')])),columns:['stroke','ink','alpha','shadowColor','shadowBlur','shadowOffsetX','shadowOffsetY','strokeWidth'],cases},null,2)+'\n';
if(process.argv.includes('--check')){if(readFileSync(output,'utf8').replace(/\r\n/g,'\n')!==data)throw Error('Text shadow oracle differs');}else writeFileSync(output,data);
console.log(cases.length+' original text shadow/style command traces');
