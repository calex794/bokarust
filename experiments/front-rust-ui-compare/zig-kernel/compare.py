"""Exact output parity and matched, rotated benchmarks for two native kernels."""
import ctypes, hashlib, json, os, random, statistics, subprocess
from pathlib import Path
from html.parser import HTMLParser

P=Path(__file__).resolve().parent
OUT=P/"results"
OUT.mkdir(exist_ok=True)
libs={"rust":(P/"rust.so","render_html_rust"),"zig":(P/"zig.so","render_html_zig")}
CAP=2*1024*1024
class Tags(HTMLParser):
    def __init__(self):
        super().__init__()
        self.ids=[];self.main=0
    def handle_starttag(self,tag,attrs):
        x=dict(attrs)
        if tag=="main" and x.get("data-benchmark")=="ui-compare":self.main+=1
        if "data-id" in x:
            assert tag=="div" and x.get("role")=="img" and x.get("aria-label")
            self.ids.append((int(x["data-id"]),int(x["data-x"]),int(x["data-y"])))
def output(path,symbol,n):
    lib=ctypes.CDLL(str(path))
    f=getattr(lib,symbol)
    f.argtypes=[ctypes.POINTER(ctypes.c_ubyte),ctypes.c_size_t,ctypes.c_size_t]
    f.restype=ctypes.c_size_t
    dest=(ctypes.c_ubyte*CAP)()
    size=f(dest,CAP,n)
    assert 0<size<=CAP,(symbol,n,size)
    assert f(dest,0,n)==0
    assert f(dest,CAP,2)==0
    return bytes(dest[:size])
parity={}
for n in (3,300,10000):
    a=output(*libs["rust"],n)
    b=output(*libs["zig"],n)
    assert a==b, f"byte mismatch for {n}: {len(a)} vs {len(b)}"
    t=Tags();t.feed(a.decode())
    assert t.main==1 and len(t.ids)==n
    assert (2,164,28) in t.ids
    assert sorted(k for k,_,_ in t.ids)==list(range(1,n+1))
    parity[str(n)]={"bytes":len(a),"sha256":hashlib.sha256(a).hexdigest()}
(OUT/"parity.json").write_text(json.dumps(parity,indent=2)+"\n")
rng=random.Random(20261025)
raw={lang:{"3":[],"300":[]} for lang in libs}
for roundno in range(9):
    for n in (3,300):
        order=list(libs);rng.shuffle(order)
        for lang in order:
            path,symbol=libs[lang]
            result=json.loads(subprocess.check_output([str(P/"bench"),str(path),symbol,str(n)],text=True))
            assert result["count"]==n and result["bytes"]==parity[str(n)]["bytes"]
            result["round"]=roundno+1
            raw[lang][str(n)].append(result)
def median(vals):return round(float(statistics.median(vals)),5)
rows={}
for lang,(path,_) in libs.items():
    rows[lang]={
        "file_bytes":path.stat().st_size,
        "compile_seconds":float((OUT/(lang+"-build-seconds.txt")).read_text()),
        "p95_3_median_us":median([x["p95_us"] for x in raw[lang]["3"]]),
        "p95_300_median_us":median([x["p95_us"] for x in raw[lang]["300"]]),
        "rss_300_median_mib":round(median([x["rss_kib"] for x in raw[lang]["300"]])/1024,3),
        "samples":raw[lang]
    }
summary={
 "method":"same C ABI, hand-written buffer formatter, exact byte parity, 9 alternating rounds, single GitHub runner",
 "non_claims":"not full framework, no browser/WASM, shared-library bytes not application size",
 "toolchain":{"rust":subprocess.check_output(["rustc","--version"],text=True).strip(),"zig":os.environ.get("ZIG_VERSION")},
 "parity":parity,"results":rows
}
(OUT/"results.json").write_text(json.dumps(summary,indent=2)+"\n")
lines=["## Native Zig vs Rust buffer-only HTML experiment","","| Language | .so KiB | Compile s | p95 3 objs (us) | p95 300 objs (us) | RSS MiB |","|---|---:|---:|---:|---:|---:|"]
for lang in ("rust","zig"):
    x=rows[lang]
    lines.append(f"| {lang} | {x['file_bytes']/1024:.1f} | {x['compile_seconds']:.2f} | {x['p95_3_median_us']:.4f} | {x['p95_300_median_us']:.3f} | {x['rss_300_median_mib']:.3f} |")
lines+=["","All bytes identical for 3/300/10000 objects. C ABI and same buffer/decimal algorithm.",
        "Comparing .so sizes is not the same as comparing final application download size.",
        "No Leptos/Dioxus overhead in this kernel, and no real FrontRust app or WebAssembly."]
report="\n".join(lines)+"\n"
(OUT/"summary.md").write_text(report)
print(report)
if os.getenv("GITHUB_STEP_SUMMARY"):
    with open(os.environ["GITHUB_STEP_SUMMARY"],"a") as f:f.write(report)
