mod phase2;
use std::hint::black_box;
use std::time::Instant;
use std::fs;
use std::path::PathBuf;

const N: usize = 10_000;
const BATCH: usize = 100;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Id { index: u32, generation: u32 }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Object { id: Id, x: i32, y: i32, w: i32, h: i32, kind: u32 }
fn objects() -> Vec<Object> {
    (0..N).map(|i| Object {
        id: Id { index: i as u32, generation: 1 },
        x: (i % 100) as i32 * 20,
        y: (i / 100) as i32 * 20, w: 13, h: 13, kind: (i % 7) as u32,
    }).collect()
}
#[derive(Clone)]
struct Soa {
    ids: Vec<Id>, x: Vec<i32>, y: Vec<i32>,
    w: Vec<i32>, h: Vec<i32>, kind: Vec<u32>,
}
impl Soa {
    fn new(input: &[Object]) -> Self {
        Self {
            ids: input.iter().map(|o|o.id).collect(),
            x: input.iter().map(|o|o.x).collect(),
            y: input.iter().map(|o|o.y).collect(),
            w: input.iter().map(|o|o.w).collect(),
            h: input.iter().map(|o|o.h).collect(),
            kind: input.iter().map(|o|o.kind).collect(),
        }
    }
    fn get(&self, id: Id) -> Option<(i32,i32)> {
        let i=id.index as usize;
        if self.ids.get(i).copied()? != id { return None; }
        Some((self.x[i], self.y[i]))
    }
}
fn hit_aos(data:&[Object], px:i32, py:i32)->Option<Id> {
    data.iter().rev().find(|o| px>=o.x && px<o.x+o.w && py>=o.y && py<o.y+o.h).map(|o|o.id)
}
fn hit_soa(s:&Soa, px:i32, py:i32)->Option<Id> {
    (0..s.x.len()).rev().find(|&i| px>=s.x[i] && px<s.x[i]+s.w[i] && py>=s.y[i] && py<s.y[i]+s.h[i]).map(|i|s.ids[i])
}
fn move_aos(data:&mut[Object], group:&[Id],dx:i32,dy:i32)->bool {
    if dx==0 && dy==0{return false}
    if !group.iter().all(|id|data.get(id.index as usize).map(|x|x.id)==Some(*id)){return false}
    if !group.iter().all(|id|{let o=&data[id.index as usize];o.x.checked_add(dx).is_some() && o.y.checked_add(dy).is_some()}){return false}
    for id in group {let o=&mut data[id.index as usize];o.x+=dx;o.y+=dy;}
    true
}
fn move_soa(s:&mut Soa,group:&[Id],dx:i32,dy:i32)->bool{
    if dx==0 && dy==0{return false}
    if !group.iter().all(|id|s.get(*id).is_some()){return false}
    if !group.iter().all(|id|{let i=id.index as usize;s.x[i].checked_add(dx).is_some()&&s.y[i].checked_add(dy).is_some()}){return false}
    for id in group {let i=id.index as usize;s.x[i]+=dx;s.y[i]+=dy;}
    true
}
fn group()->Vec<Id>{ (0..BATCH).map(|i|Id{index:(i*97) as u32,generation:1}).collect() }
fn verify(){
    let initial=objects();let mut aos=initial.clone();let mut soa=Soa::new(&initial);let ids=group();
    for i in 0..2000 {
        let px=(i*47%2300) as i32;let py=(i*61%2300) as i32;
        assert_eq!(hit_aos(&aos,px,py),hit_soa(&soa,px,py));
    }
    assert!(move_aos(&mut aos,&ids,7,-3));
    assert!(move_soa(&mut soa,&ids,7,-3));
    for (i,o) in aos.iter().enumerate(){assert_eq!(soa.get(o.id),Some((o.x,o.y)));assert_eq!(soa.kind[i],o.kind)}
    assert!(!move_soa(&mut soa,&[Id{index:97,generation:2}],3,4));
    assert!(!move_aos(&mut aos,&[Id{index:97,generation:2}],3,4));
    assert!(!move_aos(&mut aos,&ids,0,0));
    assert!(!move_soa(&mut soa,&ids,0,0));
    assert!(move_aos(&mut aos,&ids,-7,3));
    assert!(move_soa(&mut soa,&ids,-7,3));
    assert_eq!(aos,initial);
    assert_eq!(soa.x[97],initial[97].x);
}
struct Drag { base:Vec<(Id,i32,i32)>, dx:i32,dy:i32 }
fn drag_start(a:&[Object],ids:&[Id])->Drag {
    Drag{base:ids.iter().map(|id|{let o=&a[id.index as usize];assert_eq!(o.id,*id);(*id,o.x,o.y)}).collect(),dx:0,dy:0}
}
impl Drag {
    fn preview(&mut self,step:i32){self.dx=step;self.dy=-step;}
    fn commit(self,scene:&mut Vec<Object>,revision:&mut u64,history:&mut Vec<Vec<Object>>)->bool {
        if self.dx==0 && self.dy==0{return false}
        let ids:Vec<_>=self.base.iter().map(|r|r.0).collect();
        let before=scene.clone();
        if !move_aos(scene,&ids,self.dx,self.dy){return false}
        history.push(before);*revision+=1;true
    }
}
fn drag_verify(){
 let mut scene=objects();let original=scene.clone();let ids=group();let mut history=Vec::new();let mut revision=0;
 let mut drag=drag_start(&scene,&ids);
 for step in 0..100 {drag.preview(step);}
 assert_eq!(scene,original);assert_eq!(revision,0);
 assert!(drag.commit(&mut scene,&mut revision,&mut history));assert_eq!(revision,1);assert_eq!(history.len(),1);
 let changed=scene.clone();scene=history.pop().unwrap();assert_eq!(scene,original);scene=changed;
 assert_eq!(scene[ids[0].index as usize].x,original[ids[0].index as usize].x+99);
 let before=scene.clone();
 let mut noop=drag_start(&scene,&ids);noop.preview(0);
 assert!(!noop.commit(&mut scene,&mut revision,&mut history));assert_eq!(scene,before);assert_eq!(revision,1);
}
// Derived-only integer geometry cache, never the authoritative scene.
// Header: magic/version/revision/count/hash followed by 4*i32 per rectangle, LE.
fn derived(scene:&[Object])->Vec<u8> {
 let mut out=Vec::with_capacity(32+scene.len()*16);
 out.extend_from_slice(b"FRRC");out.extend_from_slice(&1_u32.to_le_bytes());
 out.extend_from_slice(&7_u64.to_le_bytes());
 out.extend_from_slice(&(scene.len() as u32).to_le_bytes());
 let sum=scene.iter().fold(2166136261_u32,|mut h,o|{
    for v in [o.x,o.y,o.w,o.h] {h^=v as u32;h=h.wrapping_mul(16777619);}
    h
 });
 out.extend_from_slice(&sum.to_le_bytes());
 for o in scene {for v in [o.x,o.y,o.w,o.h] {out.extend_from_slice(&v.to_le_bytes());}}
 out
}
fn cache_accept(bytes:&[u8],scene:&[Object])->bool {
 if bytes.len()!=24+scene.len()*16||bytes.get(..4)!=Some(b"FRRC") {return false}
 if u32::from_le_bytes(bytes[4..8].try_into().unwrap())!=1{return false}
 if u64::from_le_bytes(bytes[8..16].try_into().unwrap())!=7{return false}
 if u32::from_le_bytes(bytes[16..20].try_into().unwrap()) as usize !=scene.len(){return false}
 // Validate every value, not just a noncryptographic hash. Untrusted files need a
 // separate authenticated/canonical parser before they enter an authoritative model.
 derived(scene)==bytes
}
fn cache_verify(){
 let scene=objects();let bytes=derived(&scene);assert!(cache_accept(&bytes,&scene));
 let mut bad=bytes.clone();bad[25]^=1;assert!(!cache_accept(&bad,&scene));
 assert!(!cache_accept(&bytes[..bytes.len()-1],&scene));
 let mut old=bytes.clone();old[4]=2;assert!(!cache_accept(&old,&scene));
 let mut revised=scene.clone();revised[3].x+=1;assert!(!cache_accept(&bytes,&revised));
 let path=PathBuf::from(std::env::temp_dir()).join(format!("front-roc-lab-{}.bin",std::process::id()));
 fs::write(&path,&bytes).unwrap();let recovered=fs::read(&path).unwrap();fs::remove_file(path).unwrap();
 assert!(cache_accept(&recovered,&scene));
}
fn quantiles(samples:&mut [f64])->(f64,f64){
 samples.sort_by(|a,b|a.total_cmp(b));(samples[samples.len()/2],samples[(samples.len()*95/100).min(samples.len()-1)])
}
fn time_it(mut f:impl FnMut(), rounds:usize)->(f64,f64) {
 for _ in 0..10{f();}
 let mut times=Vec::with_capacity(rounds);
 for _ in 0..rounds {let start=Instant::now();f();times.push(start.elapsed().as_secs_f64()*1e6);}
 quantiles(&mut times)
}
fn measure(){
 verify();drag_verify();cache_verify();
 let input=objects();let soa=Soa::new(&input);let ids=group();
 let query:Vec<_>=(0..512).map(|i|((i*47%2300) as i32,(i*61%2300) as i32)).collect();
 let (a50,a95)=time_it(||{let mut sum=0_u64;for &(x,y) in &query {if let Some(id)=hit_aos(black_box(&input),black_box(x),black_box(y)){sum^=id.index as u64}}black_box(sum);},65);
 let (s50,s95)=time_it(||{let mut sum=0_u64;for &(x,y) in &query {if let Some(id)=hit_soa(black_box(&soa),black_box(x),black_box(y)){sum^=id.index as u64}}black_box(sum);},65);
 let (m50,m95)=time_it(||{let mut tmp=input.clone();move_aos(black_box(&mut tmp),black_box(&ids),7,-3);black_box(tmp);},90);
 let (n50,n95)=time_it(||{let mut tmp=soa.clone();move_soa(black_box(&mut tmp),black_box(&ids),7,-3);black_box(tmp);},90);
 let (c50,c95)=time_it(||{black_box(derived(black_box(&input)));},90);
 let cache=derived(&input);
 let (r50,r95)=time_it(||{black_box(cache_accept(black_box(&cache),black_box(&input)));},90);
 let aos_bytes=input.capacity()*std::mem::size_of::<Object>();
 let soa_bytes=soa.ids.capacity()*std::mem::size_of::<Id>()+
 soa.x.capacity()*4+soa.y.capacity()*4+soa.w.capacity()*4+soa.h.capacity()*4+soa.kind.capacity()*4;
 println!("{{\"objects\":{},\"group\":{},\"queries_per_sample\":{},\"aos_bytes\":{},\"soa_bytes\":{},\"hit_aos_p50_us\":{:.3},\"hit_aos_p95_us\":{:.3},\"hit_soa_p50_us\":{:.3},\"hit_soa_p95_us\":{:.3},\"move_aos_p50_us\":{:.3},\"move_aos_p95_us\":{:.3},\"move_soa_p50_us\":{:.3},\"move_soa_p95_us\":{:.3},\"cache_build_p50_us\":{:.3},\"cache_build_p95_us\":{:.3},\"cache_read_p50_us\":{:.3},\"cache_read_p95_us\":{:.3},\"cache_bytes\":{}}}",
 N,BATCH,query.len(),aos_bytes,soa_bytes,a50,a95,s50,s95,m50,m95,n50,n95,c50,c95,r50,r95,cache.len());
}
fn main(){
 match std::env::args().nth(1).as_deref(){
  Some("--bench")=>measure(),
  Some("--bench2")=>phase2::run(),
  _=>{verify();drag_verify();cache_verify();println!("PASS: parity, stale generations, no-op, 100-event drag/one undo, cache corruption and reopening");}
 }
}
#[cfg(test)]
mod tests {
 use super::*;
 #[test]fn two_layouts_identical(){verify()}
 #[test]fn drag_one_revision(){drag_verify()}
 #[test]fn cache_untrusted_input_rejected(){cache_verify()}
}
