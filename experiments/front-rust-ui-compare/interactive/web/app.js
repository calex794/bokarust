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
    browser_events:browserEvents,wasm_bytes:mode==='wasm'?wasmBytes:null,first_wasm_instantiate_ms:mode==='wasm'?instantiateMs:null};
  $('results').textContent=JSON.stringify(result,null,2);
  return result;
}
window.runSuite=runSuite;