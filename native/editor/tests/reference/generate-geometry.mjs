// Development-only oracle. Execute actual original functions, never duplicated
// transform formulas. Native tests and production do not invoke Node/TypeScript.
import { readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { stripTypeScriptTypes } from 'node:module';
import { createHash } from 'node:crypto';
const root = resolve(import.meta.dirname, '../../../..');
const sources = {};
const read = path => {
  const source=readFileSync(resolve(root, path), 'utf8').replace(/\r\n/g,'\n');
  sources[path]=source;
  return source;
};
const moduleFrom = async source => import('data:text/javascript;base64,' + Buffer.from(stripTypeScriptTypes(source)).toString('base64'));
const matrix = read('packages/runtime/src/math/matrix2d.ts');
const common = read('packages/runtime/src/math/common.ts');
const aabb = read('packages/runtime/src/math/aabb.ts');
const transform = read('packages/runtime/src/systems/transform.ts');
const interaction = read('packages/runtime/src/queries/interaction.ts');
const camera = read('packages/runtime/src/queries/camera.ts');
const math = matrix.replace("import { degToRad } from './common';", common)
  + aabb.replace("import type { Mat2D } from './matrix2d';", '');
const kernels = await moduleFrom(math + `
const Flip='flip', Anchor='anchor', Computed='computed', LocalTransform='local', WorldTransform='world', WorldBounds='bounds';
const Sequential='sequence', Group='group', Geometry='geometry', Hidden='hidden', IsMask='mask', Culled='culled', RenderSurface='surface';
const store=(world,trait)=>world[trait];
const getParentNode=entity=>entity.parent;
const getCamera=world=>world.camera;
const MIN_DETERMINANT=1e-12;
const Or=(...traits)=>({or:traits}), Not=trait=>({not:trait}), ChildOf=entity=>({parent:entity});
${camera.slice(camera.indexOf('export function getCameraInverse('), camera.indexOf('/**\n * Visible stage size'))}
${transform.slice(transform.indexOf('export function computeLocalMatrix('), transform.indexOf('function adjustLayers('))}
export { cullEntity };
${interaction.slice(interaction.indexOf('export function isPointerInEntity('), interaction.indexOf('/** The entities a selection mask spans:'))}
function entityWorldMat(world,entity) { return Object.fromEntries(Object.entries(world.world).map(([key,v])=>[key,v[entity.id()]])); }
`);
const { identity2D, multiply2D, translate2D, rotate2D, scale2D, skew2D, invert2D, decompose2D,
  rectToQuad, quadCenter, pointInQuad, quadsIntersect, quadContainsQuad, computeLocalMatrix,
  computeWorldTransform, computeWorldBounds, computeGroupBounds, cullEntity, getViewMatrix,
  getCameraInverse, isPointerInEntity } = kernels;
const emptyStore = () => Object.fromEntries(['a','b','c','d','e','f'].map(key=>[key,[]]));
const put = (store,id,values) => { for(const [key,v] of Object.entries(values)) (store[key]??=[])[id]=v; };
const row = (store,id) => Object.fromEntries(Object.entries(store).map(([key,v])=>[key,v[id]]));
const entity = (id, traits=[], parent=null) => ({ id:()=>id, parent, traits:new Set(traits),
  has(trait){return this.traits.has(trait)}, add(trait){this.traits.add(trait)}, remove(trait){this.traits.delete(trait)} });
let seed=0xAD32026;
const random=()=>{ seed=(Math.imul(seed,1664525)+1013904223)>>>0;return seed/2**32; };
const pick=a=>a[Math.floor(random()*a.length)];
const range=(min,max)=>min+(max-min)*random();
const cases=[];
for(let i=0;i<180;i++) {
  const input = {
    position:[range(-200,200),range(-100,100)], offset:[range(-20,20),range(-20,20)],
    size:[pick([0,1,32,100,1920,-25]),pick([0,1,16,90,1080,-30])],
    anchor:[pick([null,0,.25,.5,1,-.2]),pick([null,0,.75,.5,1,1.2])],
    flip:[pick([null,-1,1]),pick([null,-1,1])], scale:[pick([0,.25,1,2,-1]),pick([0,.5,1,3,-2])],
    rotation:pick([0,1,30,90,180,-45,360,720.25]), skew:[pick([0,15,-25,45]),pick([0,10,-20,30])],
  };
  const origin=[range(-20,20),range(-20,20)];
  const cameraMatrix=multiply2D(translate2D(19,-27),multiply2D(rotate2D(i%3?0:18),scale2D(.75,1.5)));
  const parent=i%2?multiply2D(translate2D(-11,8),multiply2D(rotate2D(-12),skew2D(11,-8))):null;
  const resolution=pick([1,1.25,2,3]);
  const surface=i%4?[320,180]:null;
  const parentCulled=parent?i%3===0:null;
  const world={ flip:{},anchor:{},computed:{},local:emptyStore(),world:emptyStore(),
    bounds:Object.fromEntries(['minX','minY','maxX','maxY'].map(key=>[key,[]])),
    camera:cameraMatrix,get:()=>surface?{canvas:{width:surface[0],height:surface[1]},resolution}:undefined };
  put(world.flip,0,{x:input.flip[0]??undefined,y:input.flip[1]??undefined});
  put(world.anchor,0,{x:input.anchor[0]??undefined,y:input.anchor[1]??undefined});
  put(world.computed,0,{positionX:input.position[0],positionY:input.position[1],offsetX:input.offset[0],offsetY:input.offset[1],
    width:input.size[0],height:input.size[1],originX:origin[0],originY:origin[1],scaleX:input.scale[0],scaleY:input.scale[1],
    rotation:input.rotation,skewX:input.skew[0],skewY:input.skew[1]});
  const item=entity(0);const parentEntity=parent?entity(1,parentCulled?['culled']:[]):null;
  if(parent) put(world.world,1,parent);
  computeLocalMatrix(world,item);
  computeWorldTransform(world,item,parentEntity);
  computeWorldBounds(world,item);
  cullEntity(world,item,parentEntity);
  const local=row(world.local,0), worldMatrix=row(world.world,0), bounds=row(world.bounds,0);
  const quad=rectToQuad(multiply2D(worldMatrix,translate2D(...origin)),...input.size);
  // libm transcendental results and JSON readers can differ by an ULP. Keep
  // end-to-end boolean probes off random transformed edges; exact boundary
  // semantics are checked separately with integral matrices and quads.
  const determinant=worldMatrix.a*worldMatrix.d-worldMatrix.b*worldMatrix.c;
  const points=[{x:bounds.maxX+1,y:bounds.maxY+1},{x:bounds.minX-1,y:bounds.minY-1}];
  if(Math.abs(determinant)>1e-9 && input.size.every(n=>Math.abs(n)>0)) {
    points.push(quadCenter(quad));
    for(const [u,v] of [[.1,.1],[.9,.1],[.1,.9],[.9,.9],[-.1,.5],[1.1,.5]]) {
      points.push(kernels.transformPoint(worldMatrix,origin[0]+u*input.size[0],origin[1]+v*input.size[1]));
    }
  }
  cases.push({input,origin,camera:cameraMatrix,resolution:surface?resolution:1,parent,surface,parentCulled,
    local,world:worldMatrix,bounds,quad,inverse:invert2D(worldMatrix),decomposed:decompose2D(worldMatrix),
    view:getViewMatrix(world),cameraInverse:getCameraInverse(world),culled:item.has('culled'),
    points:points.map(point=>({point,inside:isPointerInEntity(world,item,point),inQuad:pointInQuad(point.x,point.y,quad)}))});
}
const groups=[];
for(let i=0;i<45;i++) {
  const children=Array.from({length:i%6},(_,j)=>({
    local:j%3?multiply2D(translate2D(j*20-37,i-15),multiply2D(rotate2D(i*3),scale2D(j%2?-1:1,.75))):undefined,
    size:[30+j,20+j],hidden:(i+j)%4===0,mask:(i+j)%7===0,
  }));
  const previousOrigin=[17,-23];
  const world={computed:{},local:emptyStore()};
  const group=entity(0,['group']);
  const entities=children.map((child,j)=>entity(j+1,['geometry',...(child.hidden?['hidden']:[]),...(child.mask?['mask']:[])],group));
  put(world.computed,0,{width:123,height:321,originX:previousOrigin[0],originY:previousOrigin[1]});
  children.forEach((child,j)=>{put(world.computed,j+1,{width:child.size[0],height:child.size[1]});if(child.local)put(world.local,j+1,child.local)});
  world.query=(...filters)=>entities.filter(e=>filters.every(f=>f.or?f.or.some(t=>e.has(t)):f.not?!e.has(f.not):f.parent?e.parent===f.parent:e.has(f)));
  computeGroupBounds(world,group);
  const r=row(world.computed,0);
  groups.push({children,previousOrigin,frame:{origin:[r.originX,r.originY],size:[r.width,r.height]}});
}
// Include the historical vertex-only crossing behavior and half-open raycast
// boundaries so a future "improvement" cannot silently change selection.
const quads=[rectToQuad(identity2D(),10,10),rectToQuad(translate2D(10,0),10,10),
  rectToQuad(translate2D(2,2),4,4),rectToQuad(translate2D(-10,-1),20,2),
  rectToQuad(translate2D(-1,-10),2,20),rectToQuad(multiply2D(translate2D(7,-4),rotate2D(37)),15,6)];
const pairs=quads.flatMap(a=>quads.map(b=>({a,b,intersects:quadsIntersect(a,b),contains:quadContainsQuad(a,b)})));
const hashes=Object.fromEntries(Object.entries(sources).map(([path,source])=>[path,createHash('sha256').update(source).digest('hex')]));
const encode=value=>JSON.stringify(value,(_key,value)=>typeof value==='number'&&!Number.isFinite(value)?String(value):value);
// One reference case per line keeps the checked-in fixture readable in diffs.
const content=`{\n"sources":${JSON.stringify(hashes,null,2)},\n`
  + Object.entries({cases,groups,pairs}).map(([name,values])=>`"${name}":[\n${values.map(encode).join(',\n')}\n]`).join(',\n')+'\n}\n';
const path=resolve(import.meta.dirname,'../fixtures/geometry.json');
if(process.argv.includes('--check')) {
  if(readFileSync(path,'utf8').replace(/\r\n/g,'\n')!==content) throw new Error('Geometry reference fixture is stale; regenerate it with this script and review the diff.');
  console.log('Geometry references and normalized source hashes match the original functions.');
} else {
  writeFileSync(path,content);
  console.log(`Generated ${cases.length} transform, ${groups.length} group and ${pairs.length} quad reference cases.`);
}
