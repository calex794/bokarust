#!/usr/bin/env python3
"""Run the *native SSR-only* comparison and store inspectable evidence."""
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import resource
from html.parser import HTMLParser

ROOT=Path(__file__).resolve().parents[1]
VARIANT=sys.argv[1]
CONFIG={
  "rust-baseline": ("rust-baseline", []),
  "leptos-ssr": ("leptos-ssr-compare", ["--features", "leptos-view"]),
  "dioxus-ssr": ("dioxus-ssr-compare", ["--features", "dioxus-view"]),
}
if VARIANT not in CONFIG: raise SystemExit("unknown variant")
binary, extra=CONFIG[VARIANT]
out=ROOT/"results"/VARIANT
out.mkdir(parents=True,exist_ok=True)
env=os.environ.copy()
env["CARGO_TARGET_DIR"]=str(Path(os.environ.get("RUNNER_TEMP","/tmp"))/("ui-compare-"+VARIANT))
manifest=str(ROOT/"Cargo.toml")
def execute(args, target, check=True):
    with (out/target).open("w") as f:
        t=time.perf_counter()
        p=subprocess.run(args,env=env,stdout=f,stderr=subprocess.STDOUT,check=False)
        seconds=time.perf_counter()-t
    if check and p.returncode: raise RuntimeError(f"{args} failed ({p.returncode}); see {target}")
    return seconds
execute(["cargo","fetch","--manifest-path",manifest],"fetch.log")
build_seconds=execute(["cargo","build","--manifest-path",manifest,"--release","--bin",binary,*extra],"build.log")
execute(["cargo","test","--manifest-path",manifest,"--lib"],"tests.log")
exe=Path(env["CARGO_TARGET_DIR"])/"release"/binary
execute([str(exe),"--check","3"],"check.log")
class Parse(HTMLParser):
    def __init__(self):
        super().__init__();self.rectangles=[];self.mains=0;self.headings=0
    def handle_starttag(self,tag,attrs):
        a=dict(attrs)
        if tag=="main" and a.get("data-benchmark")=="ui-compare": self.mains+=1
        if tag=="h1": self.headings+=1
        if "data-id" in a:
            assert a.get("role")=="img" and a.get("aria-label"),a
            self.rectangles.append((int(a["data-id"]),int(a["data-x"]),int(a["data-y"])))
for n in (3,300):
    h=subprocess.check_output([str(exe),"--html",str(n)],env=env)
    (out/f"page_{n}.html").write_bytes(h)
    p=Parse();p.feed(h.decode())
    assert p.mains==1 and p.headings==1,(p.mains,p.headings)
    assert len(p.rectangles)==n,len(p.rectangles)
    assert sorted(r[0] for r in p.rectangles)==list(range(1,n+1))
    assert (2,164,28) in p.rectangles
    print("Validated",VARIANT,n)
    execute([str(exe),"--bench",str(n)],f"bench_{n}.json")
    result=json.loads((out/f"bench_{n}.json").read_text())
    assert result["rectangles"]==n and result["samples"]>=300
    globals()[f"bench_{n}"]=result
metrics={
  "variant":VARIANT,
  "scope":"native SSR only; not FrontRust, browser, WebAssembly or user input",
  "binary_bytes":exe.stat().st_size,
  "build_seconds":round(build_seconds,3),
  "bench_3":bench_3,"bench_300":bench_300,
  "toolchain":subprocess.check_output(["rustc","--version"]).decode().strip(),
}
(out/"metrics.json").write_text(json.dumps(metrics,indent=2)+"\n")
print(json.dumps(metrics,indent=2))
