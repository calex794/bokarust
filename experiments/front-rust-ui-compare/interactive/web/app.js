// Controlled study: JS domain model versus the same operations in Rust/WASM.
// The renderer and simulated browser event trace are IDENTICAL in both arms.
const BOARD_N = 10_000;
const BLOCK_N = 1_000; // Fifty synthetic pages; 20 paragraphs per page.
const FNV_SEED = 2166136261;
const FNV_PRIME = 16777619;
const encoder = new TextEncoder();
const decoder = new TextDecoder();
const $ = (id) => document.getElementById(id);
const canvas = $('board');
const ctx = canvas.getContext('2d');
const docs = $('docs');
const input = $('doc-input');
const viewport = { x: 0, y: 0, scale: 1 };
let current = null;
let drag = null;
let browserEvents = { pointer: 0, input: 0, composition: 0 };

function mix(h, value) { return Math.imul((h ^ (value >>> 0)) >>> 0, FNV_PRIME) >>> 0; }
function seedText(index) { return `Paragraph ${String(index).padStart(4, '0')} — Quick brown fox. Русский текст. 漢字.`; }
function codepoints(text) { return Array.from(text); }

class JSCore {
  constructor() { this.reset(); this.mode = 'js'; }
  reset() {
    this.xy = new Int32Array(BOARD_N * 2);
    for (let i = 0; i < BOARD_N; i++) {
      this.xy[i * 2] = 10 + (i % 100) * 40;
      this.xy[i * 2 + 1] = 20 + Math.floor(i / 100) * 40;
    }
    this.boardRev = 0; this.boardPast = []; this.boardFuture = [];
    this.blocks = Array.from({length:BLOCK_N}, (_,i) => ({text:seedText(i), bold:false}));
    this.docRev = 0; this.docPast = []; this.docFuture = [];
  }
  boardMove(id,dx,dy) {
    if (id < 1 || id > BOARD_N || (dx === 0 && dy === 0)) return false;
    const at=(id-1)*2, ox=this.xy[at], oy=this.xy[at+1];
    const nx=ox+dx,ny=oy+dy;
    if (!Number.isSafeInteger(nx) || !Number.isSafeInteger(ny) || nx > 2147483647 || nx < -2147483648 || ny > 2147483647 || ny < -2147483648) return false;
    this.xy[at]=nx;this.xy[at+1]=ny;
    this.boardPast.push({at,ox,oy,nx,ny});this.boardFuture=[];this.boardRev++;return true;
  }
  boardUndo(){const x=this.boardPast.pop();if(!x)return false;this.xy[x.at]=x.ox;this.xy[x.at+1]=x.oy;this.boardFuture.push(x);this.boardRev++;return true;}
  boardRedo(){const x=this.boardFuture.pop();if(!x)return false;this.xy[x.at]=x.nx;this.xy[x.at+1]=x.ny;this.boardPast.push(x);this.boardRev++;return true;}
  boardChecksum(){let h=mix(FNV_SEED,this.boardRev);for(let i=0;i<this.xy.length;i++)h=mix(h,this.xy[i]);return h;}
  docInsert(index,offset,point){if(index<0||index>=BLOCK_N||!Number.isInteger(point)||point<0||point>0x10FFFF||(point>=0xD800&&point<=0xDFFF))return false;
    const b=this.blocks[index],chars=codepoints(b.text);if(offset<0||offset>chars.length)return false;
    const ch=String.fromCodePoint(point);chars.splice(offset,0,ch);b.text=chars.join('');
    this.docPast.push({kind:'insert',index,offset,ch});this.docFuture=[];this.docRev++;return true;}
  docBold(index){if(index<0||index>=BLOCK_N)return false;this.blocks[index].bold=!this.blocks[index].bold;
    this.docPast.push({kind:'bold',index});this.docFuture=[];this.docRev++;return true;}
  docApply(op,undo){let b=this.blocks[op.index];if(op.kind==='bold')b.bold=!b.bold;
    else {let a=codepoints(b.text);if(undo){if(a[op.offset]!==op.ch)throw Error('history mismatch');a.splice(op.offset,1);}else a.splice(op.offset,0,op.ch);b.text=a.join('');}}
  docUndo(){const op=this.docPast.pop();if(!op)return false;this.docApply(op,true);this.docFuture.push(op);this.docRev++;return true;}
  docRedo(){const op=this.docFuture.pop();if(!op)return false;this.docApply(op,false);this.docPast.push(op);this.docRev++;return true;}
  docChecksum(){let h=mix(FNV_SEED,this.docRev);for(const p of this.blocks){for(const b of encoder.encode(p.text))h=mix(h,b);h=mix(h,p.bold?1:0);}return h;}
  view(start,count){return this.blocks.slice(start,start+count);}
  getPositions(){return this.xy;}
  getBoardRevision(){return this.boardRev;}
  getDocRevision(){return this.docRev;}
}

class WASMCore {
  constructor(instance) {this.mode='wasm';this.w=instance.exports;this.reset();}
  reset(){this.w.fr_board_reset();this.w.fr_doc_reset();}
  boardMove(id,dx,dy){return !!this.w.fr_board_move(id,dx,dy);}
  boardUndo(){return !!this.w.fr_board_undo();}
  boardRedo(){return !!this.w.fr_board_redo();}
  boardChecksum(){return this.w.fr_board_checksum()>>>0;}
  docInsert(i,o,ch){return !!this.w.fr_doc_insert(i,o,ch);}
  docBold(i){return !!this.w.fr_doc_bold(i);}
  docUndo(){return !!this.w.fr_doc_undo();}
  docRedo(){return !!this.w.fr_doc_redo();}
  docChecksum(){return this.w.fr_doc_checksum()>>>0;}
  getPositions(){return new Int32Array(this.w.memory.buffer,this.w.fr_board_ptr(),2*BOARD_N);}
  getBoardRevision(){return this.w.fr_board_revision();}
  getDocRevision(){return this.w.fr_doc_revision();}
  view(start,count){const addr=this.w.fr_doc_visible(start,count),len=this.w.fr_doc_visible_len();const content=decoder.decode(new Uint8Array(this.w.memory.buffer,addr,len));
    return content.trimEnd().split('\n').filter(Boolean).map(line=>({bold:line[0]==='1',text:line.slice(1)}));}
}
let wasmInstance;
let wasmBytes;
let instantiateMs;
async function loadWasm(){
  if(!wasmInstance){const file=await fetch('/core.wasm',{cache:'no-store'});if(!file.ok)throw Error('WASM unavailable: '+file.status);const binary=await file.arrayBuffer();wasmBytes=binary.byteLength;
    const t=performance.now();const result=await WebAssembly.instantiate(binary,{});instantiateMs=performance.now()-t;wasmInstance=result.instance;}
  return wasmInstance;
}
function renderBoard(){
  const points=current.getPositions();const w=canvas.width,h=canvas.height;
  ctx.clearRect(0,0,w,h);ctx.save();ctx.scale(viewport.scale,viewport.scale);ctx.translate(-viewport.x,-viewport.y);
  const minX=viewport.x-32,minY=viewport.y-32,maxX=viewport.x+w/viewport.scale,maxY=viewport.y+h/viewport.scale;
  let visible=0;
  for(let i=0;i<BOARD_N;i++){const x=points[i*2],y=points[i*2+1];if(x>maxX||x+25<minX||y>maxY||y+25<minY)continue;
    ctx.fillStyle=(i===1?'#27619a':'#9abbd3');ctx.fillRect(x,y,25,25);visible++;}
  if(drag){ctx.strokeStyle='#9b3e28';ctx.lineWidth=2;ctx.strokeRect(drag.startX+drag.dx,drag.startY+drag.dy,25,25);}
  ctx.restore();$('board-summary').textContent=`Objects: ${BOARD_N}, visible: ${visible}, revision: ${current.getBoardRevision()}`;
  return visible;
}
function renderDoc(){
  // Only 24 paragraphs are materialized; do not use document-wide innerHTML.
  const fragment=document.createDocumentFragment();let nodes=0;
  for(const block of current.view(0,24)) {const p=document.createElement('div');p.className='para';p.textContent=block.text;p.style.fontWeight=block.bold?'700':'400';fragment.append(p);nodes++;}
  docs.replaceChildren(fragment);$('doc-summary').textContent=`Paragraphs: ${BLOCK_N}, DOM rows: ${nodes}, revision: ${current.getDocRevision()}`;
  return nodes;
}
function assert(cond,msg){if(!cond)throw Error(msg);}
function frame(){return new Promise(resolve=>requestAnimationFrame(()=>resolve()));}
function quantiles(values){const v=[...values].sort((a,b)=>a-b);return {p50_ms:+v[Math.floor(v.length/2)].toFixed(3),p95_ms:+v[Math.min(v.length-1,Math.floor(v.length*0.95))].toFixed(3),max_ms:+v.at(-1).toFixed(3),samples:v.length};}
function fire(el,event){return el.dispatchEvent(event);}

input.addEventListener('compositionstart',()=>{browserEvents.composition++;});
input.addEventListener('compositionend',()=>{browserEvents.composition++;});
input.addEventListener('beforeinput',event=>{
  if(!current||event.inputType!=='insertText'||!event.data)return;
  event.preventDefault();const letters=codepoints(event.data);for(const ch of letters){assert(current.docInsert(0,10,ch.codePointAt(0)),'doc insert refused');}
  browserEvents.input++;renderDoc();
});
$('bold').addEventListener('click',()=>{if(current){current.docBold(0);renderDoc();}});
canvas.addEventListener('pointerdown',event=>{if(!current)return;drag={startX:50,startY:20,downX:event.clientX,downY:event.clientY,dx:0,dy:0};browserEvents.pointer++;});
canvas.addEventListener('pointermove',event=>{if(!drag)return;drag.dx=Math.round(event.clientX-drag.downX);drag.dy=Math.round(event.clientY-drag.downY);browserEvents.pointer++;renderBoard();});
canvas.addEventListener('pointerup',event=>{if(!drag)return;drag.dx=Math.round(event.clientX-drag.downX);drag.dy=Math.round(event.clientY-drag.downY);assert(current.boardMove(2,drag.dx,drag.dy),'move refused');drag=null;browserEvents.pointer++;renderBoard();});
canvas.addEventListener('wheel',event=>{event.preventDefault();viewport.scale=Math.max(0.25,Math.min(4,viewport.scale*(event.deltaY<0?1.2:1/1.2)));renderBoard();},{passive:false});

const letters=['Ж','z','界','a'];
// Isolate domain operations from animation frames and DOM painting. Includes ABI
// overhead and state reset; does not measure text layout or browser GPU.
function measureDomainOnly(core) {
  const samples=[]; let finalBoard=0, finalDoc=0;
  for(let repetition=0;repetition<11;repetition++) {
    core.reset();
    const start=performance.now();
    for(let i=0;i<200;i++) assert(core.boardMove(2,7,-3),'domain board move');
    for(let i=0;i<200;i++) assert(core.docBold(i % 100),'domain doc toggle');
    for(let i=0;i<20;i++) {assert(core.boardUndo(),'domain board undo');assert(core.docUndo(),'domain doc undo');}
    for(let i=0;i<20;i++) {assert(core.boardRedo(),'domain board redo');assert(core.docRedo(),'domain doc redo');}
    const elapsed=performance.now()-start;
    finalBoard=core.boardChecksum();finalDoc=core.docChecksum();
    samples.push(elapsed);
  }
  return {operations_per_repeat:480,rounds:11,elapsed:quantiles(samples),
    board_checksum:finalBoard,document_checksum:finalDoc};
}
async function runSuite(mode){
  current=mode==='wasm'?new WASMCore(await loadWasm()):new JSCore();
  current.reset();viewport.x=0;viewport.y=0;viewport.scale=1;drag=null;browserEvents={pointer:0,input:0,composition:0};
  assert(current.getPositions().length===20_000,'wrong scene size');
  assert(current.view(0,24).length===24,'wrong document viewport');
  const initialBoard=current.boardChecksum(),initialDoc=current.docChecksum();
  const firstVisible=renderBoard(),firstRows=renderDoc();await frame();
  // A reproducible Docs-like trace: real InputEvent dispatch + bold + undo/redo.
  const docTimings=[];
  fire(input,new CompositionEvent('compositionstart',{bubbles:true,data:'Ж'}));
  for(let i=0;i<40;i++){
    const t=performance.now();fire(input,new InputEvent('beforeinput',{bubbles:true,cancelable:true,inputType:'insertText',data:letters[i%4]}));
    await frame();docTimings.push(performance.now()-t);
  }
  fire(input,new CompositionEvent('compositionend',{bubbles:true,data:'Ж'}));
  for(let i=0;i<13;i++) fire($('bold'),new MouseEvent('click',{bubbles:true}));
  for(let i=0;i<8;i++)assert(current.docUndo(),'doc undo refused');
  for(let i=0;i<8;i++)assert(current.docRedo(),'doc redo refused');
  renderDoc();await frame();
  assert(current.getDocRevision()===40+13+8+8,'document revision mismatched');
  assert(current.docChecksum()!==initialDoc,'document unchanged');
  const boardTimings=[];
  for(let i=0;i<30;i++){
    const t=performance.now();
    fire(canvas,new PointerEvent('pointerdown',{bubbles:true,clientX:100,clientY:100,pointerId:1}));
    fire(canvas,new PointerEvent('pointermove',{bubbles:true,clientX:105,clientY:103,pointerId:1}));
    fire(canvas,new PointerEvent('pointerup',{bubbles:true,clientX:107,clientY:97,pointerId:1}));
    await frame();boardTimings.push(performance.now()-t);
  }
  for(let i=0;i<5;i++)assert(current.boardUndo(),'board undo refused');
  for(let i=0;i<5;i++)assert(current.boardRedo(),'board redo refused');
  renderBoard();await frame();
  assert(current.getBoardRevision()===30+5+5,'one revision per committed action expected');
  assert(current.boardChecksum()!==initialBoard,'board unchanged');
  fire(canvas,new WheelEvent('wheel',{bubbles:true,cancelable:true,deltaY:-100}));
  assert(viewport.scale>1,'zoom not applied');
  const visibleAfterZoom=renderBoard();await frame();
  assert(browserEvents.input===40&&browserEvents.pointer===90&&browserEvents.composition===2,'browser event trace mismatch');
  assert(docs.children.length===24,'document virtualization missing');
  const result={mode,board_objects:BOARD_N,document_pages:50,document_paragraphs:BLOCK_N,
    visible_board_objects:firstVisible,visible_after_zoom:visibleAfterZoom,dom_rows:firstRows,
    board_checksum:current.boardChecksum(),document_checksum:current.docChecksum(),
    board_revision:current.getBoardRevision(),document_revision:current.getDocRevision(),
    board_drag_input_to_frame:quantiles(boardTimings),document_input_to_frame:quantiles(docTimings),
    browser_events:browserEvents,wasm_bytes:mode==='wasm'?wasmBytes:null,first_wasm_instantiate_ms:mode==='wasm'?instantiateMs:null,
    domain_only:measureDomainOnly(current)};
  $('results').textContent=JSON.stringify(result,null,2);
  return result;
}
window.runSuite=runSuite;

const SNAP_HEADER=24, SNAP_LIMIT=2*1024*1024;
function fnvBytes(bytes){let h=FNV_SEED;for(const x of bytes)h=mix(h,x);return h>>>0;}
function encodeSnapshot(core){
  const positions=core.getPositions(), blocks=core.view(0,BLOCK_N);
  const encoded=blocks.map(b=>encoder.encode(b.text));
  const size=SNAP_HEADER+positions.length*4+encoded.reduce((s,b)=>s+5+b.length,0)+4;
  if(size>SNAP_LIMIT)throw Error('snapshot oversized');
  const buf=new ArrayBuffer(size),v=new DataView(buf),raw=new Uint8Array(buf);let at=0;
  for(const b of [70,82,83,84,1,0,0,0])raw[at++]=b;
  for(const n of [BOARD_N,BLOCK_N,core.getBoardRevision(),core.getDocRevision()]){v.setUint32(at,n,true);at+=4;}
  for(const n of positions){v.setInt32(at,n,true);at+=4;}
  for(let i=0;i<blocks.length;i++){raw[at++]=blocks[i].bold?1:0;v.setUint32(at,encoded[i].length,true);at+=4;raw.set(encoded[i],at);at+=encoded[i].length;}
  if(at!==size-4)throw Error('bad snapshot writer length');
  v.setUint32(at,fnvBytes(raw.subarray(0,at)),true);return raw;
}
function decodeSnapshot(bytes){
  if(bytes.length<SNAP_HEADER+4||bytes.length>SNAP_LIMIT)return null;
  const v=new DataView(bytes.buffer,bytes.byteOffset,bytes.byteLength);let at=0;
  const take=n=>{if(n<0||at+n>bytes.length-4)throw Error('truncated');const old=at;at+=n;return old;};
  try{
    if(String.fromCharCode(...bytes.subarray(0,4))!=='FRST'||bytes[4]!==1||bytes[5]||bytes[6]||bytes[7])return null;
    at=8;
    const count=v.getUint32(take(4),true),blockCount=v.getUint32(take(4),true);
    if(count!==BOARD_N||blockCount!==BLOCK_N)return null;
    const boardRev=v.getUint32(take(4),true),docRev=v.getUint32(take(4),true);
    const xy=new Int32Array(count*2);
    for(let i=0;i<xy.length;i++)xy[i]=v.getInt32(take(4),true);
    const blocks=[];
    for(let i=0;i<blockCount;i++){
      const flag=bytes[take(1)];if(flag>1)return null;
      const len=v.getUint32(take(4),true);if(len>16384)return null;
      const text=new TextDecoder('utf-8',{fatal:true}).decode(bytes.subarray(take(len),at));
      blocks.push({bold:!!flag,text});
    }
    if(at!==bytes.length-4||fnvBytes(bytes.subarray(0,at))!==v.getUint32(at,true))return null;
    return {xy,blocks,boardRev,docRev};
  }catch{return null;}
}
JSCore.prototype.exportSnapshot=function(){return encodeSnapshot(this);};
JSCore.prototype.restoreSnapshot=function(bytes){
  const decoded=decodeSnapshot(bytes);if(!decoded)return false;
  this.xy=decoded.xy;this.blocks=decoded.blocks;
  this.boardRev=decoded.boardRev;this.docRev=decoded.docRev;
  this.boardPast=[];this.boardFuture=[];this.docPast=[];this.docFuture=[];
  return true;
};
WASMCore.prototype.exportSnapshot=function(){
  const ptr=this.w.fr_snapshot_ptr(),len=this.w.fr_snapshot_len();
  return new Uint8Array(this.w.memory.buffer,ptr,len).slice();
};
WASMCore.prototype.restoreSnapshot=function(bytes){
  const ptr=this.w.fr_import_ptr(bytes.length);if(!ptr)return false;
  new Uint8Array(this.w.memory.buffer,ptr,bytes.length).set(bytes);
  return !!this.w.fr_restore(bytes.length);
};
async function verifySnapshotInterop(){
  const js=new JSCore(),wasm=new WASMCore(await loadWasm());
  js.boardMove(2,11,-5);js.docInsert(0,10,'界'.codePointAt(0));js.docBold(0);
  const jsBytes=js.exportSnapshot();
  assert(wasm.restoreSnapshot(jsBytes),'Rust rejected JS snapshot');
  assert(wasm.boardChecksum()===js.boardChecksum()&&wasm.docChecksum()===js.docChecksum(),'JS to WASM state mismatch');
  const wasmBytes=wasm.exportSnapshot();
  assert(jsBytes.length===wasmBytes.length&&jsBytes.every((b,i)=>wasmBytes[i]===b),'noncanonical snapshot');
  js.reset();
  assert(js.restoreSnapshot(wasmBytes),'JS rejected WASM snapshot');
  assert(js.boardChecksum()===wasm.boardChecksum()&&js.docChecksum()===wasm.docChecksum(),'WASM to JS state mismatch');
  const stable=[js.boardChecksum(),js.docChecksum(),wasm.boardChecksum(),wasm.docChecksum()];
  for(const broken of [new Uint8Array(wasmBytes.subarray(0,wasmBytes.length-1)),(()=>{const x=wasmBytes.slice();x[35]^=8;return x;})(),(()=>{const x=wasmBytes.slice();x[4]=2;return x;})()]){
    assert(!js.restoreSnapshot(broken),'JS accepted invalid snapshot');
    assert(!wasm.restoreSnapshot(broken),'WASM accepted invalid snapshot');
    assert([js.boardChecksum(),js.docChecksum(),wasm.boardChecksum(),wasm.docChecksum()].every((n,i)=>n===stable[i]),'invalid snapshot mutated state');
  }
  assert(!js.boardUndo()&&!js.docUndo()&&!wasm.boardUndo()&&!wasm.docUndo(),'history leaked after restore');
  return {snapshot_bytes:wasmBytes.length,board_checksum:stable[0],document_checksum:stable[1],corrupt_samples_rejected:3,version:1};
}
window.verifySnapshotInterop=verifySnapshotInterop;
