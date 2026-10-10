#!/usr/bin/env python3
"""List egret `GET` route handlers that contain a write signal, for a human to read.

Usage (from the repo root): python3 scripts/audit-get-routes.py
The output of the 2026-10-02 run is interpreted in docs/en/plans/2026-10-02-data-apps-p4-get-audit.md.
"""
import re,glob,sys
files=[f for f in glob.glob('crates/*/src/**/*.rs',recursive=True)]
route_re=re.compile(r'\.route\(\s*"([^"]+)"\s*,(.*?)\)\s*(?=\.route\(|\.layer|\.with_state|;|\n\s*\))',re.S)
sig=re.compile(r'INSERT\s+INTO|UPDATE\s+[a-z_."]+\s+SET|DELETE\s+FROM|\.execute\(|\.emit\(|Command::new|tokio::spawn|\.send\(|\.post\(|set_cookie|\.add\(|\.revoke|revoke_|\.create\(|\.mint\(|\.issue|\.insert\(|\.delete\(|\.remove\(|\.save|\.update\(|\.start\(|\.stop\(|\.enqueue|\.trigger|\.run\(|\.refresh', re.I)
out=[]
for f in files:
    src=open(f).read()
    for m in re.finditer(r'\.route\(\s*"([^"]+)"\s*,',src):
        path=m.group(1)
        # take text until the matching close of this route call
        depth=1;i=m.end()
        while i<len(src) and depth>0:
            c=src[i]
            if c=='(':depth+=1
            elif c==')':depth-=1
            i+=1
        body=src[m.end():i-1]
        # handlers registered with get( ... ) or .get(
        for g in re.finditer(r'(?<![A-Za-z_])get\(\s*([A-Za-z_][A-Za-z0-9_:]*)\s*\)|\.get\(\s*([A-Za-z_][A-Za-z0-9_:]*)\s*\)',body):
            h=(g.group(1) or g.group(2)).split('::')[-1]
            fm=re.search(r'(?:pub\s+)?(?:pub\(crate\)\s+)?async\s+fn\s+'+re.escape(h)+r'\b',src)
            if not fm:
                out.append((f,path,h,'HANDLER NOT IN SAME FILE',[]));continue
            start=fm.start()
            end=src.find('\n}\n',start)
            fn=src[start:end]
            hits=sorted(set(x.group(0).lower() for x in sig.finditer(fn)))
            out.append((f,path,h,'FLAG' if hits else 'ok',hits))
import collections
c=collections.Counter(o[3] for o in out)
print(c)
for o in out:
    if o[3]!='ok': print(o[3],o[0].replace('crates/',''),o[1],o[2],o[4][:6])
