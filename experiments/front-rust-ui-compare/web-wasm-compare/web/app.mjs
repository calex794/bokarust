// Shared across all variants. Only kernel.wasm differs between Rust and Zig.
const capacity = 2 * 1024 * 1024;
const output = document.getElementById('status');
const view = document.getElementById('view');
const decoder = new TextDecoder('utf-8', { fatal:true });
const encoder = new TextEncoder();
const now = () => performance.now();
function stats(numbers) {
  const s = [...numbers].sort((a,b)=>a-b);
  return {p50:+s[Math.floor(s.length/2)].toFixed(4),
          p95:+s[Math.floor(s.length*0.95)].toFixed(4)};
}
async function bootstrap() {
  const downloadStart=now();
  const response=await fetch('./kernel.wasm',{cache:'no-store'});
  if (!response.ok) throw Error('WASM download HTTP '+response.status);
  const bytes=await response.arrayBuffer();
  const downloadEnd=now();
  const start=now();
  const {instance}=await WebAssembly.instantiate(bytes,{});
  const instantiateMs=now()-start;
  const wasm=instance.exports;
  if (!(wasm.memory instanceof WebAssembly.Memory) ||
      typeof wasm.output_ptr!=='function' ||
      typeof wasm.output_cap!=='function' ||
      typeof wasm.render_html!=='function') {
    throw Error('WASM ABI mismatch: '+Object.keys(wasm));
  }
  const ptr=wasm.output_ptr()>>>0;
  if (wasm.output_cap()!==capacity || ptr+capacity>wasm.memory.buffer.byteLength) {
    throw Error('Invalid WASM memory or output buffer');
  }
  function run(n, updateDom) {
    const t0=now();
    const len=wasm.render_html(ptr,capacity,n)>>>0;
    const t1=now();
    if(!len || len>capacity)throw Error('Invalid generated HTML length: '+len);
    const html=decoder.decode(new Uint8Array(wasm.memory.buffer,ptr,len));
    const t2=now();
    if (updateDom) view.innerHTML=html;
    const t3=now();
    return {html,bytes:len,call_us:(t1-t0)*1000,decode_us:(t2-t1)*1000,
            dom_ms:t3-t2,total_ms:t3-t0};
  }
  function draw(n) {
    const r=run(n,true);
    const node=view.querySelector('[data-id="2"]');
    if (!node || node.dataset.x!=='164' || node.dataset.y!=='28' ||
        view.querySelectorAll('[data-id]').length!==n) throw Error('DOM parity '+n);
    output.textContent='Rendered '+n+' rectangles';
    return r;
  }
  document.getElementById('count3').addEventListener('click',()=>draw(3));
  document.getElementById('count300').addEventListener('click',()=>draw(300));
  draw(3);
  document.body.dataset.testStatus='ready';
  async function digest(n) {
    const r=run(n,false);
    const hash=new Uint8Array(await crypto.subtle.digest('SHA-256',encoder.encode(r.html)));
    return {count:n,bytes:r.bytes,sha256:Array.from(hash,x=>x.toString(16).padStart(2,'0')).join('')};
  }
  async function suite() {
    const parity=[];
    for (const n of [3,300,10000]) parity.push(await digest(n));
    const timing={};
    for (const n of [3,300]) {
      for(let i=0;i<8;i++)draw(n);
      const calls=[],decodes=[],doms=[],totals=[];
      for(let i=0;i<55;i++) {
        const r=draw(n);
        calls.push(r.call_us);decodes.push(r.decode_us);doms.push(r.dom_ms);totals.push(r.total_ms);
      }
      timing[n]={call_us:stats(calls),decode_us:stats(decodes),
                 dom_ms:stats(doms),total_ms:stats(totals)};
    }
    const clickRaf=[];
    for(let i=0;i<12;i++) {
      const btn=document.getElementById(i%2?'count3':'count300');
      const t0=now();
      btn.click();
      await new Promise(resolve=>requestAnimationFrame(resolve));
      clickRaf.push(now()-t0);
    }
    draw(3);
    return {parity,timing,click_raf_ms:stats(clickRaf),
            wasm_memory_bytes:wasm.memory.buffer.byteLength,
            browser_js_heap_bytes:performance.memory?.usedJSHeapSize??null};
  }
  window.__probe={ready:true, variant:location.pathname.split('/')[1],
     wasm_bytes:bytes.byteLength,download_ms:+(downloadEnd-downloadStart).toFixed(4),
     instantiate_ms:+instantiateMs.toFixed(4),
     initial_ready_ms:+now().toFixed(4),suite};
}
bootstrap().catch(error=>{
  output.textContent=String(error?.stack || error);
  document.body.dataset.testStatus='error';
  window.__probe={ready:false,error:String(error?.stack || error)};
});
