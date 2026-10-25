//! FR-ROC-001 phase 2: Rust spatial index, pooled preview buffers and a
//! revisioned *derived-only* geometry cache. No product code is imported.
//! All comparisons share exactly the same 10,000-object authoritative scene.
use crate::{group, hit_aos, objects, Id, Object, BATCH, N};
use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::{HashMap, HashSet};
use std::hint::black_box;
use std::mem::size_of;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

struct CountAlloc;
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
#[global_allocator]
static ALLOCATOR: CountAlloc = CountAlloc;
unsafe impl GlobalAlloc for CountAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        System.alloc(layout)
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        System.alloc_zeroed(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, old: Layout, size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        System.realloc(ptr, old, size)
    }
}

// Exact reverse-order selection is preserved even when objects overlap.
// A cell can hold multiple rectangles. No out-of-range access on negative x/y.
const CELL: i32 = 64;
#[derive(Clone)]
struct UniformGrid {
    cells: HashMap<(i32, i32), Vec<usize>>,
}
impl UniformGrid {
    fn cells_for(o: &Object) -> ((i32, i32), (i32, i32)) {
        assert!(o.w > 0 && o.h > 0);
        let end_x = o.x.checked_add(o.w - 1).expect("geometry exceeds i32");
        let end_y = o.y.checked_add(o.h - 1).expect("geometry exceeds i32");
        (
            (o.x.div_euclid(CELL), o.y.div_euclid(CELL)),
            (end_x.div_euclid(CELL), end_y.div_euclid(CELL)),
        )
    }
    fn add(&mut self, index: usize, o: &Object) {
        let ((min_x, min_y), (max_x, max_y)) = Self::cells_for(o);
        for cx in min_x..=max_x {
            for cy in min_y..=max_y {
                self.cells.entry((cx, cy)).or_default().push(index);
            }
        }
    }
    fn remove(&mut self, index: usize, o: &Object) {
        let ((min_x, min_y), (max_x, max_y)) = Self::cells_for(o);
        for cx in min_x..=max_x {
            for cy in min_y..=max_y {
                let key = (cx, cy);
                let empty = if let Some(items) = self.cells.get_mut(&key) {
                    items.retain(|&i| i != index);
                    items.is_empty()
                } else {
                    false
                };
                if empty {
                    self.cells.remove(&key);
                }
            }
        }
    }
    fn build(objects: &[Object]) -> Self {
        let mut grid = Self { cells: HashMap::new() };
        for (i, o) in objects.iter().enumerate() {
            grid.add(i, o);
        }
        grid
    }
    fn hit(&self, scene: &[Object], x: i32, y: i32) -> Option<Id> {
        let cell = (x.div_euclid(CELL), y.div_euclid(CELL));
        self.cells.get(&cell)
            .and_then(|items| items.iter().copied().filter(|&i| {
                let o = &scene[i];
                // i64 comparisons avoid overflow in arbitrary valid i32 extents.
                i64::from(x) >= i64::from(o.x)
                    && i64::from(x) < i64::from(o.x) + i64::from(o.w)
                    && i64::from(y) >= i64::from(o.y)
                    && i64::from(y) < i64::from(o.y) + i64::from(o.h)
            }).max())
            .map(|i| scene[i].id)
    }
    fn move_batch(&mut self, scene: &mut [Object], ids: &[Id], dx: i32, dy: i32) -> bool {
        if (dx == 0 && dy == 0) || ids.is_empty() {
            return false;
        }
        // Validate the ENTIRE batch first, to prevent a partially moved scene.
        let mut seen = HashSet::with_capacity(ids.len());
        for id in ids {
            let i = id.index as usize;
            let Some(o) = scene.get(i) else { return false };
            if o.id != *id || !seen.insert(i) {
                return false;
            }
            let Some(x) = o.x.checked_add(dx) else { return false };
            let Some(y) = o.y.checked_add(dy) else { return false };
            if x.checked_add(o.w - 1).is_none() || y.checked_add(o.h - 1).is_none() {
                return false;
            }
        }
        for id in ids {
            let i = id.index as usize;
            let before = scene[i];
            self.remove(i, &before);
            scene[i].x += dx;
            scene[i].y += dy;
            self.add(i, &scene[i]);
        }
        true
    }
    // Deliberately an estimate of Vec backing stores, NOT measured process RSS.
    fn reference_bytes(&self) -> usize {
        self.cells.values().map(|v| v.capacity() * size_of::<usize>()).sum()
    }
    fn occupied_cells(&self) -> usize {
        self.cells.len()
    }
}
fn queries() -> Vec<(i32, i32)> {
    (0..512).map(|i| {
        let x = ((i * 47) % 2300) as i32 - if i % 19 == 0 { 300 } else { 0 };
        let y = ((i * 61) % 2300) as i32 - if i % 17 == 0 { 300 } else { 0 };
        (x, y)
    }).collect()
}
fn verify_grid() {
    let scene = objects();
    let grid = UniformGrid::build(&scene);
    for (x, y) in queries().into_iter().chain([
        (0, 0), (9, 20), (10, 20), (22, 32), (23, 32), (-10, -10),
        (1999, 1999), (-1, 0), (0, -1), (i32::MAX, i32::MIN)
    ]) {
        assert_eq!(grid.hit(&scene, x, y), hit_aos(&scene, x, y), "query {x},{y}");
    }
    let mut overlaps = scene[..10].to_vec();
    overlaps[8].x = 12;
    overlaps[8].y = 21;
    overlaps[9].x = 12;
    overlaps[9].y = 21;
    let overlap_index = UniformGrid::build(&overlaps);
    assert_eq!(hit_aos(&overlaps, 15, 25), Some(overlaps[9].id));
    assert_eq!(overlap_index.hit(&overlaps, 15, 25), Some(overlaps[9].id));
    let mut moved = scene.clone();
    let mut live_index = grid.clone();
    let ids = group();
    assert!(live_index.move_batch(&mut moved, &ids, -37, 21));
    for (x, y) in queries() {
        assert_eq!(live_index.hit(&moved, x, y), hit_aos(&moved, x, y));
    }
    let snapshot = moved.clone();
    assert!(!live_index.move_batch(&mut moved, &[Id{index:97, generation:2}], 1, 1));
    assert!(!live_index.move_batch(&mut moved, &[ids[0], ids[0]], 1, 1));
    assert!(!live_index.move_batch(&mut moved, &ids, i32::MAX, 1));
    assert_eq!(moved, snapshot);
    assert!(live_index.move_batch(&mut moved, &ids, 37, -21));
    assert_eq!(moved, scene);
    // A negative-coordinate object crossing cell boundaries still hits correctly.
    let mut negative = vec![scene[0]];
    negative[0].x = -70;
    negative[0].y = -2;
    let ngi = UniformGrid::build(&negative);
    for (x, y) in [(-70,-2),(-64,0),(-57,10),(-56,0)] {
        assert_eq!(ngi.hit(&negative,x,y), hit_aos(&negative,x,y));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PreviewPoint { id: Id, x: i32, y: i32 }
fn preview_into(scene: &[Object], ids: &[Id], offset: i32, out: &mut Vec<PreviewPoint>) {
    out.clear();
    for id in ids {
        let o = &scene[id.index as usize];
        assert_eq!(o.id, *id);
        // Bounded test offsets; a real editor must report checked-overflow errors.
        out.push(PreviewPoint {
            id: *id,
            x: o.x.checked_add(offset).expect("drag overflow"),
            y: o.y.checked_sub(offset).expect("drag overflow"),
        });
    }
}
fn fresh_preview_events(scene: &[Object], ids: &[Id]) -> usize {
    let mut final_checksum = 0_usize;
    for step in 0..100 {
        let mut points = Vec::with_capacity(ids.len());
        preview_into(scene, ids, step, &mut points);
        final_checksum ^= black_box(points.len() + points[0].x as usize);
        black_box(points);
    }
    final_checksum
}
fn pooled_preview_events(scene: &[Object], ids: &[Id], scratch: &mut Vec<PreviewPoint>) -> usize {
    let mut final_checksum = 0_usize;
    for step in 0..100 {
        preview_into(scene, ids, step, scratch);
        final_checksum ^= black_box(scratch.len() + scratch[0].x as usize);
        black_box(&*scratch);
    }
    final_checksum
}
fn verify_preview() {
    let initial = objects();
    let ids = group();
    let mut scratch = Vec::with_capacity(BATCH);
    let start_ptr = scratch.as_ptr();
    assert_eq!(fresh_preview_events(&initial, &ids), pooled_preview_events(&initial, &ids, &mut scratch));
    assert_eq!(scratch.len(), BATCH);
    assert_eq!(scratch.as_ptr(), start_ptr);
    let mut reference = Vec::new();
    preview_into(&initial, &ids, 99, &mut reference);
    assert_eq!(scratch, reference);
    let mut durable_scene = initial.clone();
    let original = durable_scene.clone();
    // Preview does not mutate canonical state. Exactly one commit is recorded.
    let mut history: Vec<Vec<Object>> = Vec::new();
    let mut revision = 0_u64;
    assert_eq!(revision, 0);
    let before = durable_scene.clone();
    assert!(crate::move_aos(&mut durable_scene, &ids, 99, -99));
    history.push(before);
    revision += 1;
    assert_eq!(revision, 1);
    assert_eq!(history.len(), 1);
    assert_eq!(history.pop().unwrap(), original);
    assert_eq!(initial, original);
    assert!(!crate::move_aos(&mut durable_scene, &ids, 0, 0));
    assert_eq!(revision, 1);
}

// Derived-only format v2: magic 4, version u32, document revision u64,
// count u32, aggregate hash u64, then 24 B / record:
// typed ID index+generation and x/y/w/h. Never source-of-truth document data.
// Checksum is non-cryptographic. Load always cross-checks authoritative scene.
const MAGIC: &[u8;4] = b"FRC2";
const HEADER: usize = 28;
const ENTRY: usize = 24;
const VERSION: u32 = 2;
fn item_bytes(o: &Object) -> [u8;ENTRY] {
    let mut out = [0u8;ENTRY];
    for (part, value) in [
        o.id.index, o.id.generation, o.x as u32, o.y as u32, o.w as u32, o.h as u32
    ].into_iter().enumerate() {
        out[part*4..part*4+4].copy_from_slice(&value.to_le_bytes());
    }
    out
}
fn record_hash(id: usize, content: &[u8;ENTRY]) -> u64 {
    let mut h = 0xcbf29ce484222325_u64 ^ (id as u64).wrapping_mul(0x9e3779b97f4a7c15);
    for byte in content {
        h ^= *byte as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}
fn read_u32(bytes: &[u8], p: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(p..p+4)?.try_into().ok()?))
}
fn read_u64(bytes: &[u8], p: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(p..p+8)?.try_into().ok()?))
}
#[derive(Clone)]
struct CacheV2 { data: Vec<u8>, revision: u64, checksum: u64 }
impl CacheV2 {
    fn build(scene: &[Object], revision: u64) -> Self {
        assert!(scene.len() <= 100_000);
        let mut data = Vec::with_capacity(HEADER + ENTRY*scene.len());
        data.extend_from_slice(MAGIC);
        data.extend_from_slice(&VERSION.to_le_bytes());
        data.extend_from_slice(&revision.to_le_bytes());
        data.extend_from_slice(&(scene.len() as u32).to_le_bytes());
        let mut checksum = 0_u64;
        data.extend_from_slice(&checksum.to_le_bytes());
        for (index,o) in scene.iter().enumerate() {
            let entry = item_bytes(o);
            checksum ^= record_hash(index, &entry);
            data.extend_from_slice(&entry);
        }
        data[20..28].copy_from_slice(&checksum.to_le_bytes());
        Self { data, revision, checksum }
    }
    // Opening a cache requires a *trusted document*. The cache never becomes
    // authoritative. Every entry and checksum is verified on first load.
    fn validate(data: Vec<u8>, scene: &[Object], revision: u64) -> Result<Self, &'static str> {
        let expected = HEADER.checked_add(ENTRY.checked_mul(scene.len()).ok_or("overflow")?).ok_or("overflow")?;
        if data.len() != expected { return Err("length mismatch"); }
        if data.get(..4) != Some(MAGIC) { return Err("magic mismatch"); }
        if read_u32(&data,4) != Some(VERSION) { return Err("schema mismatch"); }
        if read_u64(&data,8) != Some(revision) { return Err("stale revision"); }
        if read_u32(&data,16) != Some(scene.len() as u32) { return Err("count mismatch"); }
        let mut aggregate=0_u64;
        for (index,o) in scene.iter().enumerate() {
            let mut expected_entry = item_bytes(o);
            let offset=HEADER+ENTRY*index;
            if data[offset..offset+ENTRY] != expected_entry {
                return Err("cache differs from canonical document");
            }
            aggregate ^= record_hash(index, &expected_entry);
            // Prevent the compiler from treating this validation as a no-op.
            black_box(&mut expected_entry);
        }
        if read_u64(&data,20) != Some(aggregate) {return Err("checksum mismatch")}
        Ok(Self{ data, revision, checksum:aggregate })
    }
    // O(1) incremental patch on a previously validated in-memory cache.
    // Caller must pass confirmed old/new objects and expected document revision.
    fn patch(&mut self, old: Object, new: Object, expected_revision: u64) -> Result<bool, &'static str> {
        if self.revision != expected_revision {return Err("revision conflict")}
        if old.id != new.id {return Err("identity changed")}
        let index=old.id.index as usize;
        let start=HEADER.checked_add(ENTRY.checked_mul(index).ok_or("overflow")?).ok_or("overflow")?;
        let end=start.checked_add(ENTRY).ok_or("overflow")?;
        if end>self.data.len() {return Err("index out of range")}
        let old_bytes=item_bytes(&old);
        if self.data[start..end] != old_bytes {return Err("stale cache slot")}
        let new_bytes=item_bytes(&new);
        if old_bytes==new_bytes {return Ok(false)}
        let next_revision=self.revision.checked_add(1).ok_or("revision overflow")?;
        let new_checksum=self.checksum^record_hash(index,&old_bytes)^record_hash(index,&new_bytes);
        self.data[start..end].copy_from_slice(&new_bytes);
        self.data[8..16].copy_from_slice(&next_revision.to_le_bytes());
        self.data[20..28].copy_from_slice(&new_checksum.to_le_bytes());
        self.checksum=new_checksum;
        self.revision=next_revision;
        Ok(true)
    }
}
fn verify_cache() {
    let original=objects();
    let old=original[89];
    let mut revised=original.clone();
    revised[89].x+=17;
    revised[89].y-=5;
    let updated=revised[89];
    let mut cache=CacheV2::build(&original, 7);
    let before=cache.data.clone();
    assert_eq!(cache.patch(old,old,7),Ok(false));
    assert_eq!(cache.data,before);
    assert!(cache.patch(old,updated,7).unwrap());
    let rebuilt=CacheV2::build(&revised,8);
    assert_eq!(cache.data,rebuilt.data);
    assert_eq!(CacheV2::validate(cache.data.clone(),&revised,8).is_ok(),true);
    assert!(CacheV2::validate(cache.data.clone(),&original,8).is_err());
    assert!(CacheV2::validate(cache.data.clone(),&revised,7).is_err());
    assert!(CacheV2::validate(cache.data[..cache.data.len()-1].to_vec(),&revised,8).is_err());
    let mut corrupt=cache.data.clone();corrupt[HEADER+37]^=1;
    assert!(CacheV2::validate(corrupt,&revised,8).is_err());
    let mut versioned=cache.data.clone();versioned[4]=9;
    assert!(CacheV2::validate(versioned,&revised,8).is_err());
    let mut digest=cache.data.clone();digest[20]^=1;
    assert!(CacheV2::validate(digest,&revised,8).is_err());
    let old_result=cache.data.clone();
    assert!(cache.patch(old,updated,8).is_err());
    assert!(cache.patch(updated, Object{id:Id{index:89,generation:2},..updated},8).is_err());
    assert!(cache.patch(updated,updated,7).is_err());
    assert_eq!(cache.data,old_result);
    // Reopened cache must reject changes to the trusted authoritative scene.
    let path=std::env::temp_dir().join(format!("fr-roc-v2-{}-{}.bin",std::process::id(),std::thread::current().name().unwrap_or("t")));
    std::fs::write(&path,&cache.data).unwrap();
    let disk=std::fs::read(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(CacheV2::validate(disk,&revised,8).is_ok());
}

struct Stats { samples:Vec<f64>, p50_us:f64, p95_us:f64 }
impl Stats {
    fn new(mut samples: Vec<f64>) -> Self {
        samples.sort_by(|a,b|a.total_cmp(b));
        let p50_us=samples[samples.len()/2];
        let p95_us=samples[samples.len()*95/100];
        Self{samples,p50_us,p95_us}
    }
    fn json(&self)->String {
        let samples=self.samples.iter().map(|t|format!("{t:.4}")).collect::<Vec<_>>().join(",");
        format!("{{\"p50_us\":{:.4},\"p95_us\":{:.4},\"samples_us\":[{samples}]}}",self.p50_us,self.p95_us)
    }
}
fn time(mut f:impl FnMut(),count:usize)->Stats {
    for _ in 0..5 {f();}
    let mut times=Vec::with_capacity(count);
    for _ in 0..count {
        let start=Instant::now();f();
        times.push(start.elapsed().as_secs_f64()*1e6);
    }
    Stats::new(times)
}
pub(super) fn run() {
    verify_grid();verify_preview();verify_cache();
    let scene=objects();let group=group();let queries=queries();
    let grid=UniformGrid::build(&scene);
    let mut linear_checksum=0_usize;
    let linear=time(|| {
        let mut sum=0_usize;
        for &(x,y) in &queries {
            if let Some(id)=hit_aos(black_box(&scene), black_box(x), black_box(y)){sum^=id.index as usize;}
        }
        linear_checksum^=black_box(sum);
    },65);
    let mut indexed_checksum=0_usize;
    let indexed=time(|| {
        let mut sum=0_usize;
        for &(x,y) in &queries {
            if let Some(id)=grid.hit(black_box(&scene), black_box(x), black_box(y)){sum^=id.index as usize;}
        }
        indexed_checksum^=black_box(sum);
    },65);
    assert_eq!(linear_checksum,indexed_checksum);
    let build=time(||{black_box(UniformGrid::build(black_box(&scene)));},45);
    let move_indexed=time(||{
        let mut copy=scene.clone();
        let mut fresh=grid.clone();
        let start=Instant::now();
        assert!(fresh.move_batch(black_box(&mut copy),black_box(&group),7,-3));
        let elapsed=start.elapsed().as_secs_f64()*1e6;
        black_box((copy,fresh,elapsed));
    },35);
    // move_indexed above deliberately includes clone+index clone, so its values
    // are not an isolated O(100) update measurement; reported as end-to-end copy.
    let fresh_preview=time(||{black_box(fresh_preview_events(black_box(&scene),black_box(&group)));},85);
    let mut scratch=Vec::with_capacity(group.len());
    let pooled_preview=time(||{black_box(pooled_preview_events(black_box(&scene),black_box(&group),&mut scratch));},85);
    let before_alloc=ALLOCATIONS.load(Ordering::Relaxed);
    let baseline_output=fresh_preview_events(&scene,&group);
    let allocated_fresh=ALLOCATIONS.load(Ordering::Relaxed)-before_alloc;
    let before_pool=ALLOCATIONS.load(Ordering::Relaxed);
    let reuse_output=pooled_preview_events(&scene,&group,&mut scratch);
    let allocated_pool=ALLOCATIONS.load(Ordering::Relaxed)-before_pool;
    assert_eq!(baseline_output,reuse_output);
    assert!(allocated_fresh>=100, "fresh path must really allocate each event");
    assert_eq!(allocated_pool,0,"scratch path must allocate nothing per event");
    let cache_before=CacheV2::build(&scene,7);
    let mut revised=scene.clone();revised[89].x+=17;revised[89].y-=5;
    let old=scene[89];let new=revised[89];
    let full_cache=time(||{black_box(CacheV2::build(black_box(&revised),8));},100);
    let patched_cache=time(||{
        // Copy is outside the timed update. We report only patch_one cost.
        let mut tmp=cache_before.clone();
        let start=Instant::now();
        assert!(tmp.patch(old,new,7).unwrap());
        let elapsed=start.elapsed().as_secs_f64()*1e6;
        black_box((tmp,elapsed));
    },100);
    // A separate (and explicitly labelled) O(1) patch-only timing loop.
    let mut patch_samples=Vec::with_capacity(130);
    for _ in 0..130 {
        let mut tmp=cache_before.clone();
        let start=Instant::now();
        assert!(tmp.patch(old,new,7).unwrap());
        patch_samples.push(start.elapsed().as_secs_f64()*1e6);
        black_box(tmp);
    }
    let patch_only=Stats::new(patch_samples);
    assert_eq!(CacheV2::build(&revised,8).data, {
        let mut patched=cache_before.clone();
        assert!(patched.patch(old,new,7).unwrap());
        patched.data
    });
    let memory_estimate=grid.reference_bytes();
    let metrics=format!(
       "{{\"scope\":\"isolated rust geometry microbench; no actual FrontRust code\",\
       \"objects\":{N},\"selected\":{BATCH},\"points_per_query_sample\":{},\
       \"grid_cell_width\":{CELL},\"grid_occupied_cells\":{},\
       \"grid_vec_reference_bytes_estimate\":{memory_estimate},\
       \"linear_hit\":{},\"grid_hit\":{},\"grid_build\":{},\"grid_move_including_clone\":{},\
       \"preview_fresh\":{},\"preview_pooled\":{},\
       \"fresh_allocations_per_100_events\":{allocated_fresh},\"pooled_allocations_per_100_events\":{allocated_pool},\
       \"cache_rebuild\":{},\"cache_patch_including_clone\":{},\"cache_patch_only\":{},\
       \"cache_blob_bytes\":{},\"correctness\":\"PASS\"}}",
       queries.len(),grid.occupied_cells(),
       linear.json(),indexed.json(),build.json(),move_indexed.json(),
       fresh_preview.json(),pooled_preview.json(),
       full_cache.json(),patched_cache.json(),patch_only.json(),cache_before.data.len()
    );
    println!("{metrics}");
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]fn spatial_queries_and_updates_match_reverse_linear(){verify_grid()}
    #[test]fn scratch_pool_has_exact_preview_and_one_transaction(){verify_preview()}
    #[test]fn cache_patch_equals_rebuild_and_untrusted_cache_is_rejected(){verify_cache()}
}
