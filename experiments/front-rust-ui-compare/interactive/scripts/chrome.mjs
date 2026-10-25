// Dependency-free Chrome DevTools driver. Real browser frames, synthetic events.
import http from 'node:http';
import { readFile, writeFile, mkdtemp, rm } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { spawn } from 'node:child_process';
import { tmpdir } from 'node:os';
import { join, resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
const root=resolve(dirname(fileURLToPath(import.meta.url)),'..');
const args=process.argv.slice(2);const get=(name)=>args[args.indexOf(name)+1];
const wasmPath=args.includes('--wasm')?resolve(get('--wasm')):null;
const output=args.includes('--out')?resolve(get('--out')):resolve(root,'results/interactive.json');
const jsOnly=args.includes('--js-only');
if(!jsOnly&&!wasmPath)throw Error('Missing --wasm compiled module');
const sleep=(ms)=>new Promise(r=>setTimeout(r,ms));
const mime={'.html':'text/html; charset=utf-8','.js':'text/javascript; charset=utf-8','.wasm':'application/wasm'};
const files={'/':join(root,'web/index.html'),'/app.js':join(root,'web/app.js'),'/core.wasm':wasmPath};
const server=http.createServer(async(req,res)=>{
  const path=new URL(req.url,'http://localhost').pathname;const file=files[path];
  if(!file){res.writeHead(404);res.end('not found');return;}
  try {const data=await readFile(file);res.setHeader('content-type',mime[file.slice(file.lastIndexOf('.'))]||'application/octet-stream');res.setHeader('cache-control','no-store');res.end(data);}
  catch(e){res.writeHead(404);res.end(String(e));}
});
await new Promise((resolve,reject)=>server.listen(0,'127.0.0.1',err=>err?reject(err):resolve()));
const port=server.address().port;
const chromeBin=process.env.CHROME||['/usr/bin/google-chrome','/usr/bin/google-chrome-stable','/usr/bin/chromium'].find(existsSync);
if(!chromeBin)throw Error('Install Chrome/Chromium or set CHROME');
const dir=await mkdtemp(join(tmpdir(),'fr-chrome-'));
let chromeStderr='';
const chrome=spawn(chromeBin,['--headless=new','--no-sandbox','--disable-dev-shm-usage','--disable-background-networking','--disable-extensions','--no-proxy-server','--proxy-bypass-list=<-loopback>','--no-first-run','--no-default-browser-check',`--user-data-dir=${dir}`,'--allow-file-access-from-files','--remote-debugging-port=0','about:blank'],{stdio:['ignore','ignore','pipe']});
chrome.stderr.on('data',b=>chromeStderr+=b.toString().slice(-10000));
let ws;
async function connect(){
  let dport;
  for(let i=0;i<100;i++){if(chrome.exitCode!==null)throw Error(`Chrome exited ${chrome.exitCode}`);
    try {const f=await readFile(join(dir,'DevToolsActivePort'),'utf8');dport=Number(f.split('\n')[0]);if(dport)break;}catch{}await sleep(100);}
  if(!dport)throw Error(`Chrome DevTools did not start; bin=${chromeBin}; exit=${chrome.exitCode}; stderr=${chromeStderr.slice(-6000)}`);
  let target;
  for(let i=0;i<50;i++){
    try{const r=await fetch(`http://127.0.0.1:${dport}/json/list`);target=(await r.json()).find(t=>t.type==='page');if(target)break;}catch{}await sleep(100);
  }
  if(!target)throw Error(`no page target; stderr=${chromeStderr.slice(-3000)}`);
  ws=new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve,reject)=>{ws.addEventListener('open',resolve,{once:true});ws.addEventListener('error',reject,{once:true});});
}
let messageId=0;const pending=new Map();
function rpc(method,params={}){return new Promise((resolve,reject)=>{
  const id=++messageId;const timer=setTimeout(()=>{pending.delete(id);reject(Error(`CDP timeout: ${method}`));},90000);
  pending.set(id,{resolve,reject,timer});ws.send(JSON.stringify({id,method,params}));
});}
function onMessage(event){const msg=JSON.parse(event.data);if(!msg.id)return;const p=pending.get(msg.id);if(!p)return;clearTimeout(p.timer);pending.delete(msg.id);
  if(msg.error)p.reject(Error(JSON.stringify(msg.error)));else p.resolve(msg.result);}
async function evaluate(code){const r=await rpc('Runtime.evaluate',{expression:code,awaitPromise:true,returnByValue:true});
  if(r.exceptionDetails)throw Error(`Browser exception ${JSON.stringify(r.exceptionDetails)}`);
  return r.result?.value;}
const percentile=(v,p)=>{const a=[...v].sort((a,b)=>a-b);return Number(a[Math.min(a.length-1,Math.floor(a.length*p))].toFixed(3));};
try{
  await connect();ws.addEventListener('message',onMessage);
  await rpc('Page.enable');await rpc('Runtime.enable');await rpc('Page.navigate',{url:`http://127.0.0.1:${port}/`});
  let ready=false;
  for(let i=0;i<150;i++){try{if(await evaluate('typeof window.runSuite === "function"')){ready=true;break;}}catch{}await sleep(100);}
  if(!ready)throw Error('web app failed to load (inspect network policy or script errors)');
  const rounds=jsOnly?['js']:['js','wasm','wasm','js','js','wasm'];
  const runs=[];
  for(const mode of rounds){const r=await evaluate(`window.runSuite(${JSON.stringify(mode)})`);if(!r?.board_checksum||!r?.document_checksum)throw Error(`Invalid result: ${JSON.stringify(r)}`);runs.push(r);
    console.log(`${mode}: board hash ${r.board_checksum}; docs hash ${r.document_checksum}; board p95 ${r.board_drag_input_to_frame.p95_ms}ms; text p95 ${r.document_input_to_frame.p95_ms}ms`);}
  const snapshots=jsOnly?null:await evaluate('window.verifySnapshotInterop()');
  if(snapshots)console.log(`PASS snapshot interop: ${JSON.stringify(snapshots)}`);
  const baseline=runs.find(r=>r.mode==='js');
  for(const run of runs){for(const key of ['board_checksum','document_checksum','board_revision','document_revision','visible_board_objects','dom_rows','domain_only.board_checksum','domain_only.document_checksum']){
      const read=(obj,path)=>path.split('.').reduce((v,part)=>v?.[part],obj);
      if(read(run,key)!==read(baseline,key))throw Error(`Parity failure ${key}: ${read(baseline,key)} != ${read(run,key)} (${run.mode})`);
    }}
  if(!jsOnly&&runs.filter(x=>x.mode==='wasm').length!==3)throw Error('Missing WASM runs');
  const summary={};for(const mode of ['js','wasm']){const rs=runs.filter(r=>r.mode===mode);if(!rs.length)continue;
    summary[mode]={board_input_to_frame_p95_median_ms:percentile(rs.map(r=>r.board_drag_input_to_frame.p95_ms),0.5),doc_input_to_frame_p95_median_ms:percentile(rs.map(r=>r.document_input_to_frame.p95_ms),0.5),wasm_bytes:rs[0].wasm_bytes,domain_only_p95_median_ms:percentile(rs.map(r=>r.domain_only.elapsed.p95_ms),0.5)};}
  await writeFile(output,JSON.stringify({environment:{userAgent:await evaluate('navigator.userAgent'),node:process.version,chrome:await evaluate('navigator.userAgentData?.brands?.map(x=>x.brand+" "+x.version).join(", ") ?? navigator.userAgent')},fixture:{board_objects:10000,paragraphs:1000,pages:50},snapshot_interop:snapshots,scope:'Synthetic pointer/InputEvent, same JS DOM/Canvas paint, domain-core difference ONLY; not FrontRust, Leptos or Dioxus web comparison',runs,summary},null,2));
  console.log(`PASS browser trace/parity: ${output}`);
}finally{
  for(const p of pending.values()){clearTimeout(p.timer);p.reject(Error('closing CDP'));}pending.clear();
  ws?.close();chrome.kill('SIGTERM');await Promise.race([new Promise(r=>chrome.once('exit',r)),sleep(1500)]);server.closeAllConnections?.();await new Promise(r=>server.close(r));
  for(let i=0;i<3;i++){try{await rm(dir,{recursive:true,force:true});break;}catch(e){if(i===2)console.warn('Chrome cleanup:',String(e));else await sleep(250);}}
}