//! SSR comparison fixture. This is not the actual FrontRust implementation.
#[cfg(not(target_arch = "wasm32"))]
use std::hint::black_box;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rect { pub id: usize, pub x: i32, pub y: i32 }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scene { pub revision: u64, pub rects: Vec<Rect> }
pub struct Editor { pub current: Scene, past: Vec<Scene>, future: Vec<Scene> }

impl Editor {
    pub fn new(count: usize) -> Self {
        assert!((3..=10_000).contains(&count));
        Self {
            current: Scene {
                revision: 0,
                rects: (1..=count).map(|id| Rect {
                    id,
                    x: 10 + (((id - 1) % 25) as i32) * 130,
                    y: 20 + (((id - 1) / 25) as i32) * 90,
                }).collect(),
            },
            past: Vec::new(), future: Vec::new(),
        }
    }
    pub fn move_by(&mut self, id: usize, dx: i32, dy: i32) -> bool {
        if dx == 0 && dy == 0 { return false; }
        let Some(index) = self.current.rects.iter().position(|r| r.id == id) else { return false; };
        let Some(x) = self.current.rects[index].x.checked_add(dx) else { return false; };
        let Some(y) = self.current.rects[index].y.checked_add(dy) else { return false; };
        self.past.push(self.current.clone());
        self.future.clear();
        self.current.rects[index].x = x;
        self.current.rects[index].y = y;
        self.current.revision += 1;
        true
    }
    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.past.pop() else { return false; };
        self.future.push(std::mem::replace(&mut self.current, previous));
        true
    }
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.future.pop() else { return false; };
        self.past.push(std::mem::replace(&mut self.current, next));
        true
    }
}
pub fn fixture(count: usize) -> Scene {
    let mut editor = Editor::new(count);
    assert!(editor.move_by(2, 24, 8));
    let moved = editor.current.clone();
    assert!(editor.undo());
    assert_eq!(editor.current.revision, 0);
    assert!(editor.redo());
    assert_eq!(editor.current, moved);
    moved
}
#[cfg(not(target_arch = "wasm32"))]
pub fn run(label: &str, render: fn(&Scene) -> String) {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(String::as_str).unwrap_or("--check");
    let n = args.get(2).and_then(|s| s.parse::<usize>().ok()).unwrap_or(3);
    if !(3..=10_000).contains(&n) { panic!("rectangles out of range: {n}"); }
    let scene = fixture(n);
    match cmd {
        "--html" => print!("{}", render(&scene)),
        "--check" => {
            let html = render(&scene);
            assert!(html.contains("data-benchmark=\"ui-compare\""));
            for id in 1..=n { assert!(html.contains(&format!("data-id=\"{id}\"")), "missing ID {id}"); }
            println!("OK {label}: {n} rectangles, revision {}", scene.revision);
        },
        "--bench" => {
            for _ in 0..50 { black_box(render(black_box(&scene))); }
            let samples = if n == 3 { 1000 } else { 300 };
            let mut durations = Vec::with_capacity(samples);
            for _ in 0..samples {
                let start = Instant::now();
                black_box(render(black_box(&scene)));
                durations.push(start.elapsed().as_secs_f64() * 1_000_000.0);
            }
            durations.sort_by(|a,b| a.total_cmp(b));
            println!(
                "{{\"variant\":\"{}\",\"rectangles\":{},\"samples\":{},\"p50_us\":{:.3},\"p95_us\":{:.3},\"html_bytes\":{}}}",
                label,n,samples,durations[samples/2],durations[samples*95/100],render(&scene).len()
            );
        },
        _ => panic!("Usage: binary [--check|--html|--bench] [3|300]"),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn undo_redo() {
        let mut e = Editor::new(3);
        let original = e.current.clone();
        assert!(e.move_by(2,24,8));
        assert_eq!(e.current.rects[1], Rect { id:2,x:164,y:28 });
        assert!(e.undo());
        assert_eq!(e.current,original);
        assert!(e.redo());
        assert_eq!(e.current.revision,1);
    }
    #[test] fn noop() {
        let mut e=Editor::new(3);
        assert!(!e.move_by(2,0,0));
        assert!(!e.move_by(999,1,1));
        assert!(!e.undo());
    }
    #[test] fn large_scene() { assert_eq!(fixture(300).rects.len(),300); }
}


/// FNV-style checksum of the canonical command result.
/// This is deliberately *not* an ABI for FrontRust: a temporary parity probe.
pub fn scene_checksum(scene: &Scene) -> u32 {
    let mut hash = 2_166_136_261_u32;
    let values = std::iter::once(scene.revision as u32).chain(
        scene.rects.iter().flat_map(|r| [r.id as u32, r.x as u32, r.y as u32])
    );
    for value in values {
        hash ^= value;
        hash = hash.wrapping_mul(16_777_619_u32);
    }
    hash
}

/// Operates on the same Rust fixture in native and a real wasm32 build.
/// A result of 0 indicates invalid input, not a valid document hash.
#[no_mangle]
pub extern "C" fn front_rust_probe_checksum(count: u32) -> u32 {
    if !(3..=10_000).contains(&count) {
        return 0;
    }
    scene_checksum(&fixture(count as usize))
}

#[cfg(test)]
mod checksum_tests {
    use super::*;
    #[test]
    fn native_checksum_matches_export() {
        for n in [3, 300, 10_000] {
            assert_ne!(front_rust_probe_checksum(n), 0);
            assert_eq!(scene_checksum(&fixture(n as usize)), front_rust_probe_checksum(n));
        }
        assert_eq!(front_rust_probe_checksum(2), 0);
        assert_eq!(front_rust_probe_checksum(10_001), 0);
    }
}
