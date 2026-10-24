// Dependency-free Chrome DevTools driver: real browser, same JS shell and wasm ABI.
// Three alternating navigations on one runner, byte parity, DOM and button events.
import http from 'node:http';
import {readFile,writeFile,appendFile,mkdtemp,rm,mkdir} from 'node:fs/promises';
import {existsSync} from 'node:fs';
import {spawn} from 'node:child_process';
import {tmpdir} from 'node:os';
import {join,resolve,dirname,extname} from 'node:path';
import {fileURLToPath} from 'node:url';
import {gzipSync,brotliCompressSync,constants} from 'node:zlib';

const root=resolve(dirname(fileURLToPath(import.meta.url)),'..');
const out=join(root,'results'),site=join(out,'site');
await mkdir(out,{recursive:true});
const variants=['rust-small','zig-small','rust-fast','zig-fast'];
const stats={};
for (const v of variants) {
  const assets={},total={raw:0,gzip:0,brotli:0};
  for (const name of ['index.html','app.mjs','kernel.wasm']) {
    const b=await readFile(join(site,v,name));
    const item={raw:b.length,gzip:gzipSync(b,{level:9}).length,
       brotli:brotliCompressSync(b,{params:{[constants.BROTLI_PARAM_QUALITY]:11}}).length};
    assets[name]=item;
    for(const key of ['raw','gzip','brotli'])total[key]+=item[key];
  }
  stats[v]={assets,total};
}
const mime={'.html':'text/html; charset=utf-8','.mjs':'text/javascript; charset=utf-8','.wasm':'application/wasm'};
const server=http.createServer(async(req,res)=>{
  const path=new URL(req.url,'http://localhost').pathname.split('/').filter(Boolean);
  const v=path[0],name=path[1]||'index.html';
  if(path.length>2||!variants.includes(v)||!['index.html','app.mjs','kernel.wasm'].includes(name)){
    res.writeHead(404);res.end('missing');return;
  }
  try {
    const b=await readFile(join(site,v,name));
    res.setHeader('content-type',mime[extname(name)]);
    res.setHeader('cache-control','no-store');
    res.end(b);
  }catch(e){res.writeHead(500);res.end(String(e));}
});
await new Promise((ok,fail)=>server.listen(0,'127.0.0.1',err=>err?fail(err):ok()));
const port=server.address().port;
const chromeBin=process.env.CHROME||['/usr/bin/google-chrome','/usr/bin/google-chrome-stable','/usr/bin/chromium'].find(existsSync);
if(!chromeBin)throw Error('Chrome binary missing');
const dir=await mkdtemp(join(tmpdir(),'fr-zig-rust-'));
const chrome=spawn(chromeBin,[
  '--headless=new','--no-sandbox','--disable-dev-shm-usage','--disable-background-networking',
  '--disable-extensions','--no-first-run','--no-default-browser-check',
  '--no-proxy-server','--proxy-bypass-list=<-loopback>',
  '--enable-precise-memory-info','--remote-debugging-port=0',
  '--user-data-dir='+dir,'about:blank'
],{stdio:['ignore','ignore','pipe']});
let stderr='',ws,id=0;
chrome.stderr.on('data',b=>stderr=(stderr+b.toString()).slice(-5000));
const sleep=ms=>new Promise(ok=>setTimeout(ok,ms)),waiting=new Map();
function rpc(method,params={}){
 return new Promise((ok,fail)=>{
   const key=++id;
   const timer=setTimeout(()=>{waiting.delete(key);fail(Error('CDP timeout '+method));},90000);
   waiting.set(key,{ok,fail,timer});
   ws.send(JSON.stringify({id:key,method,params}));
 });
}
function receive(event){
 const msg=JSON.parse(event.data),rec=waiting.get(msg.id);
 if(!rec)return;
 clearTimeout(rec.timer);waiting.delete(msg.id);
 msg.error?rec.fail(Error(JSON.stringify(msg.error))):rec.ok(msg.result);
}
async function connect(){
 let debugPort=0;
 for(let i=0;i<100;i++){
   if(chrome.exitCode!==null)throw Error('Chrome exit: '+stderr);
   try{
      debugPort=Number((await readFile(join(dir,'DevToolsActivePort'),'utf8')).split('\n')[0]);
      if(debugPort)break;
   }catch{}await sleep(100);
 }
 if(!debugPort)throw Error('DevTools port missing '+stderr);
 let tab;
 for(let i=0;i<100;i++){
   try{
      const response=await fetch('http://127.0.0.1:'+debugPort+'/json/list');
      tab=(await response.json()).find(x=>x.type==='page');
      if(tab)break;
   }catch{}await sleep(100);
 }
 if(!tab)throw Error('Chrome target missing '+stderr);
 ws=new WebSocket(tab.webSocketDebuggerUrl);
 await new Promise((ok,fail)=>{
   ws.addEventListener('open',ok,{once:true});
   ws.addEventListener('error',fail,{once:true});
 });
 ws.addEventListener('message',receive);
}
async function evaluate(expr){
 const r=await rpc('Runtime.evaluate',{expression:expr,awaitPromise:true,returnByValue:true});
 if(r.exceptionDetails)throw Error('Runtime exception: '+JSON.stringify(r.exceptionDetails));
 return r.result?.value;
}
const median=values=>{
 const sorted=[...values].sort((a,b)=>a-b);
 return +sorted[Math.floor(sorted.length/2)].toFixed(4);
};
const runs=[];
try {
 await connect();
 await rpc('Page.enable');await rpc('Runtime.enable');await rpc('Network.enable');
 await rpc('Network.setCacheDisabled',{cacheDisabled:true});
 for(let round=0;round<3;round++){
  const order=round%2===0?variants:[...variants].reverse();
  for(const variant of order){
   await rpc('Page.navigate',{url:'http://127.0.0.1:'+port+'/'+variant+'/index.html'});
   let ready=false;
   for(let i=0;i<160;i++){
     const state=await evaluate('({path:location.pathname,ready:Boolean(window.__probe?.ready),error:window.__probe?.error||null})');
     if(state?.error)throw Error(variant+': '+state.error);
     if(state?.path?.includes('/'+variant+'/')&&state.ready){ready=true;break;}
     await sleep(100);
   }
   if(!ready)throw Error('Browser not ready: '+variant+', stderr='+stderr);
   const initial=await evaluate('({download_ms:window.__probe.download_ms,instantiate_ms:window.__probe.instantiate_ms,initial_ready_ms:window.__probe.initial_ready_ms,wasm_bytes:window.__probe.wasm_bytes})');
   const suite=await evaluate('window.__probe.suite()');
   if(!suite||suite.parity?.length!==3)throw Error('Incomplete suite '+variant);
   if(initial.wasm_bytes!==stats[variant].assets['kernel.wasm'].raw)throw Error('WASM bytes mismatch '+variant);
   runs.push({variant,round:round+1,initial,suite});
   console.log('PASS '+variant+' round '+(round+1)+' initial='+initial.initial_ready_ms+'ms, wasm='+initial.wasm_bytes);
  }
 }
 const base=runs[0].suite.parity;
 for(const row of runs){
  for(let i=0;i<3;i++){
   const value=row.suite.parity[i],expected=base[i];
   if(value.count!==expected.count||value.bytes!==expected.bytes||value.sha256!==expected.sha256)
     throw Error('Byte parity mismatch '+row.variant+', count='+expected.count);
  }
 }
 const summary={};
 for(const variant of variants){
   const subset=runs.filter(x=>x.variant===variant);
   summary[variant]={
     ready_ms_median:median(subset.map(x=>x.initial.initial_ready_ms)),
     instantiate_ms_median:median(subset.map(x=>x.initial.instantiate_ms)),
     wasm_300_p95_us_median:median(subset.map(x=>x.suite.timing['300'].call_us.p95)),
     dom_300_p95_ms_median:median(subset.map(x=>x.suite.timing['300'].dom_ms.p95)),
     click_to_raf_p95_ms_median:median(subset.map(x=>x.suite.click_raf_ms.p95)),
     wasm_linear_memory_bytes:subset[0].suite.wasm_memory_bytes
   };
 }
 const result={
   scope:'Full raw browser HTML+JS+WASM bundle, same JavaScript renderer; not full FrontRust/Leptos/Dioxus',
   method:'one Chromium process, 3 alternating navigations per variant, cache disabled; synthetic button clicks and DOM mutation',
   compression:'gzip/brotli estimates applied individually to each asset; server uses uncompressed localhost HTTP',
   caveats:'headless requestAnimationFrame is not GPU presentation; WebAssembly memory is not process RSS; results not generalizable to all hardware',
   browser_user_agent:await evaluate('navigator.userAgent'),
   package_sizes:stats,byte_parity:base,runs,summary
 };
 await writeFile(join(out,'browser.json'),JSON.stringify(result,null,2)+'\n');
 const lines=['# Browser WASM size and speed comparison','',
   'Same HTML+JS, same byte output, real Rust and Zig WASM. Not a whole frontend framework comparison.','',
   '| Variant | WASM KiB | Bundle raw KiB | Bundle brotli KiB | Ready ms | WASM call p95 us (300) | DOM p95 ms (300) |',
   '|---|---:|---:|---:|---:|---:|---:|'];
 for(const variant of variants){
   const b=stats[variant],x=summary[variant];
   lines.push('| '+variant+' | '+(b.assets['kernel.wasm'].raw/1024).toFixed(1)+' | '+
      (b.total.raw/1024).toFixed(1)+' | '+(b.total.brotli/1024).toFixed(1)+' | '+
      x.ready_ms_median.toFixed(2)+' | '+x.wasm_300_p95_us_median.toFixed(2)+' | '+
      x.dom_300_p95_ms_median.toFixed(3)+' |');
 }
 lines.push('','3 alternating navigations per arm; common Chrome instance; localhost, cache disabled.',
   'Raw/gzip/brotli packages = index.html + app.mjs + kernel.wasm.',
   'Synthetic events and rAF are not actual human input-to-pixel latency. Full user app not tested.');
 const md=lines.join('\n')+'\n';
 await writeFile(join(out,'summary.md'),md);
 console.log(md);
 if(process.env.GITHUB_STEP_SUMMARY)await appendFile(process.env.GITHUB_STEP_SUMMARY,md);
} finally {
 for(const item of waiting.values()){clearTimeout(item.timer);item.fail(Error('closing'));}waiting.clear();
 ws?.close();chrome.kill('SIGTERM');
 await Promise.race([new Promise(ok=>chrome.once('exit',ok)),sleep(1500)]);
 server.closeAllConnections?.();await new Promise(ok=>server.close(ok));
 for(let attempt=0;attempt<5;attempt++) {
   try {await rm(dir,{recursive:true,force:true,maxRetries:3,retryDelay:250});break;}
   catch(err){if(attempt===4)console.warn('Chrome profile cleanup:',String(err));else await sleep(300);}
 }
}
