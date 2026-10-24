#!/usr/bin/env python3
import json,pathlib,sys
site=pathlib.Path(sys.argv[1])
def leb(b,i):
 x=0;shift=0
 while True:
  y=b[i];i+=1;x|=(y&127)<<shift
  if y<128:return x,i
  shift+=7
def inspect(p):
 b=p.read_bytes()
 assert b[:8]==bytes.fromhex('0061736d01000000')
 i=8;secs=[];stripped=bytearray(b[:8]);functions=None;exports=[]
 while i<len(b):
  start=i;kind=b[i];i+=1;n,i=leb(b,i);end=i+n
  label={0:'custom',1:'type',2:'import',3:'function',4:'table',5:'memory',6:'global',7:'export',9:'element',10:'code',11:'data'}.get(kind,str(kind))
  if kind==0:
   m,j=leb(b,i);label+=':'+b[j:j+m].decode('utf8','replace')
  else:stripped.extend(b[start:end])
  if kind==3:functions,_=leb(b,i)
  if kind==7:
   ct,j=leb(b,i)
   for _ in range(ct):
    m,j=leb(b,j);exports.append(b[j:j+m].decode());j+=m+1;_,j=leb(b,j)
  secs.append({'name':label,'bytes':end-start});i=end
 return {'raw':len(b),'stripped':len(stripped),'functions':functions,'exports':exports,'sections':secs,'code':sum(z['bytes'] for z in secs if z['name']=='code'),'custom':sum(z['bytes'] for z in secs if z['name'].startswith('custom'))},bytes(stripped)
data={}
for p in sorted(site.glob('*/kernel.wasm')):
 v=p.parent.name;item,clean=inspect(p)
 for symbol in ('memory','output_cap','output_ptr','render_html'):
  assert symbol in item['exports'],(v,symbol,item['exports'])
 data[v]=item
 if v in ('rust-small','zig-fast','rust-nostd-small'):
  dest=site/(v+'-stripped');dest.mkdir(exist_ok=True)
  (dest/'kernel.wasm').write_bytes(clean)
  for name in ('index.html','app.mjs'):(dest/name).write_bytes((p.parent/name).read_bytes())
(site.parent/'size-audit.json').write_text(json.dumps(data,indent=2)+'\n')
print(json.dumps({k:{'raw':v['raw'],'stripped':v['stripped'],'code':v['code'],'custom':v['custom'],'functions':v['functions']} for k,v in data.items()},indent=2))
