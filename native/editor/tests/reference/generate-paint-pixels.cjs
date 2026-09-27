// Development-only Canvas oracle. Native tests consume the committed JSON.
const {app,BrowserWindow}=require('electron');
const {writeFileSync}=require('node:fs');
const {resolve}=require('node:path');
const modes=['sourceOver','multiply','screen','overlay','darken','lighten','colorDodge','colorBurn','hardLight','softLight','difference','exclusion','hue','saturation','color','luminosity'];
const colors=[['#93582b',1,'#3471d9',1],['#93582b',.4,'#3471d9',.65]];
const cases=[];
for(const mode of modes)for(const [bg,ba,fg,fa] of colors)cases.push({kind:'blend',mode,bg,ba,fg,fa});
for(const kind of ['linear','radial'])for(const stops of [[],[[.3,'#f32e01',.4]],[[0,'#ff0000',1],[1,'#0000ff',1]],[[0,'#ff0000',0],[1,'#0000ff',1]],[[0,'#ff0000',1],[.5,'#ff0000',1],[.5,'#0000ff',1],[1,'#0000ff',1]]])for(const rotation of [0,45,90])cases.push({kind,stops,rotation});
cases.push({kind:'linear',stops:[[0,'#ff0000',1],[1,'#0000ff',1]],rotation:0,scaleX:0});
const source=c=>{
  if(c.kind==='blend')return `<rect width={16} height={8} fill="${c.bg}" opacity={${c.ba}}/><rect width={16} height={8} fill="${c.fg}" opacity={${c.fa}} blendMode="${c.mode}"/>`;
  const tag=c.kind+'GradientPaint';
  return `<rect width={16} height={8}><${tag} rotation={${c.rotation}}${c.scaleX===undefined?'':` scaleX={${c.scaleX}}`}>${c.stops.map(([offset,color,opacity])=>`<colorStop offset={${offset}} color="${color}" opacity={${opacity}}/>`).join('')}</${tag}></rect>`;
};
app.whenReady().then(async()=>{
 const window=new BrowserWindow({show:false,webPreferences:{sandbox:true,contextIsolation:true,nodeIntegration:false}});
 await window.loadURL('data:text/html,<html></html>');
 const results=await window.webContents.executeJavaScript(`(()=>{
  const samples=[[0,0],[4,2],[7,3],[8,4],[11,5],[15,7]];
  return ${JSON.stringify(cases)}.map(c=>{
   const ctx=new OffscreenCanvas(16,8).getContext('2d');
   if(c.kind==='blend'){
    ctx.fillStyle=c.bg;ctx.globalAlpha=c.ba;ctx.fillRect(0,0,16,8);
    ctx.globalCompositeOperation=c.mode.replace(/[A-Z]/g,x=>'-'+x.toLowerCase());
    ctx.fillStyle=c.fg;ctx.globalAlpha=c.fa;ctx.fillRect(0,0,16,8);
   }else{
    const a=c.rotation*Math.PI/180,cos=Math.cos(a),sin=Math.sin(a),sx=c.scaleX??1;
    const point=(x,y)=>[(.5+x*sx*cos-y*sin)*16,(.5+x*sx*sin+y*cos)*8];
    let g;
    if(c.kind==='linear')g=ctx.createLinearGradient(...point(-.5,0),...point(.5,0));
    else {const x=point(.5,0),y=point(0,.5);g=ctx.createRadialGradient(8,4,0,8,4,Math.max(.0001,Math.hypot(x[0]-8,x[1]-4),Math.hypot(y[0]-8,y[1]-4)));}
    for(const [offset,color,opacity] of c.stops) {
     const rgb=parseInt(color.slice(1),16);g.addColorStop(offset,'rgba('+[(rgb>>16)&255,(rgb>>8)&255,rgb&255,opacity].join(',')+')');
    }
    ctx.fillStyle=g;ctx.fillRect(0,0,16,8);
   }
   return samples.map(([x,y])=>({x,y,rgba:Array.from(ctx.getImageData(x,y,1,1).data)}));
  });
 })()`);
 const output=resolve(__dirname,'../fixtures/paint-pixels.json');
 writeFileSync(output,JSON.stringify({electron:process.versions.electron,chromium:process.versions.chrome,cases:cases.map((c,i)=>({name:JSON.stringify(c),source:source(c),samples:results[i]}))},null,2)+'\n');
 console.log(`${cases.length} Canvas paint/blend cases`);app.quit();
}).catch(e=>{console.error(e);app.exit(1)});
