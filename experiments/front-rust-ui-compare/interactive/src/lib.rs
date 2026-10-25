//! Rust domain-only probe. Not the FrontRust product or a text layout engine.
use std::cell::RefCell;
mod snapshot;

const BOARD_SIZE: usize = 10_000;
const BLOCKS: usize = 1_000; // 50 pages with 20 synthetic paragraphs each.
const FNV_SEED: u32 = 2_166_136_261;
const FNV_PRIME: u32 = 16_777_619;

fn mix(mut hash: u32, word: u32) -> u32 {
    hash ^= word;
    hash.wrapping_mul(FNV_PRIME)
}

#[derive(Clone)]
struct BoardChange { index: usize, old_x: i32, old_y: i32, new_x: i32, new_y: i32 }
struct Board { xy: Vec<i32>, revision: u32, past: Vec<BoardChange>, future: Vec<BoardChange> }
impl Board {
    fn new() -> Self {
        let mut xy = Vec::with_capacity(2 * BOARD_SIZE);
        for i in 0..BOARD_SIZE {
            xy.push(10 + ((i % 100) as i32) * 40);
            xy.push(20 + ((i / 100) as i32) * 40);
        }
        Self { xy, revision: 0, past: Vec::new(), future: Vec::new() }
    }
    fn move_by(&mut self, id: usize, dx: i32, dy: i32) -> bool {
        if !(1..=BOARD_SIZE).contains(&id) || (dx == 0 && dy == 0) { return false; }
        let index = id - 1;
        let (x, y) = (self.xy[2*index], self.xy[2*index+1]);
        let (Some(nx), Some(ny)) = (x.checked_add(dx), y.checked_add(dy)) else { return false; };
        self.xy[2*index] = nx; self.xy[2*index+1] = ny;
        self.past.push(BoardChange { index, old_x:x, old_y:y, new_x:nx, new_y:ny });
        self.future.clear(); self.revision += 1;
        true
    }
    fn undo(&mut self) -> bool {
        let Some(change) = self.past.pop() else { return false; };
        self.xy[2*change.index] = change.old_x;
        self.xy[2*change.index+1] = change.old_y;
        self.future.push(change); self.revision += 1;
        true
    }
    fn redo(&mut self) -> bool {
        let Some(change) = self.future.pop() else { return false; };
        self.xy[2*change.index] = change.new_x;
        self.xy[2*change.index+1] = change.new_y;
        self.past.push(change); self.revision += 1;
        true
    }
    fn checksum(&self) -> u32 {
        self.xy.iter().fold(mix(FNV_SEED, self.revision), |h, v| mix(h, *v as u32))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Paragraph { text: String, bold: bool }
#[derive(Clone)]
enum DocChange { Insert { block: usize, offset: usize, ch: char }, Bold { block: usize } }
struct Document { blocks: Vec<Paragraph>, revision: u32, past: Vec<DocChange>, future: Vec<DocChange> }
fn paragraph_seed(index: usize) -> String {
    format!("Paragraph {index:04} — Quick brown fox. Русский текст. 漢字.")
}
fn byte_index_at(text: &str, offset: usize) -> Option<usize> {
    if offset == text.chars().count() { Some(text.len()) }
    else { text.char_indices().nth(offset).map(|(i, _)| i) }
}
impl Document {
    fn new() -> Self {
        Self { blocks:(0..BLOCKS).map(|index| Paragraph {text:paragraph_seed(index),bold:false}).collect(), revision:0,past:Vec::new(),future:Vec::new() }
    }
    fn insert(&mut self, index: usize, offset: usize, ch: char) -> bool {
        let Some(block) = self.blocks.get_mut(index) else { return false; };
        let Some(byte_index) = byte_index_at(&block.text, offset) else { return false; };
        block.text.insert(byte_index, ch);
        self.past.push(DocChange::Insert {block:index,offset,ch});
        self.future.clear(); self.revision += 1;
        true
    }
    fn bold(&mut self, index: usize) -> bool {
        let Some(block) = self.blocks.get_mut(index) else {return false;};
        block.bold = !block.bold;
        self.past.push(DocChange::Bold {block:index});
        self.future.clear(); self.revision += 1;
        true
    }
    fn apply(&mut self, action: &DocChange, undo: bool) {
        match *action {
            DocChange::Insert { block,offset,ch } => {
                let text = &mut self.blocks[block].text;
                let at = byte_index_at(text,offset).expect("valid tracked position");
                if undo { assert_eq!(text.remove(at),ch); } else {text.insert(at,ch);}
            },
            DocChange::Bold {block} => self.blocks[block].bold = !self.blocks[block].bold,
        }
    }
    fn undo(&mut self) -> bool {
        let Some(op) = self.past.pop() else {return false;};
        self.apply(&op,true); self.future.push(op); self.revision += 1;
        true
    }
    fn redo(&mut self) -> bool {
        let Some(op) = self.future.pop() else {return false;};
        self.apply(&op,false); self.past.push(op); self.revision += 1;
        true
    }
    fn checksum(&self) -> u32 {
        let mut h = mix(FNV_SEED, self.revision);
        for block in &self.blocks {
            for byte in block.text.as_bytes() {h = mix(h,*byte as u32);}
            h = mix(h,block.bold as u32);
        }
        h
    }
}
thread_local! {
    static BOARD: RefCell<Board> = RefCell::new(Board::new());
    static DOC: RefCell<Document> = RefCell::new(Document::new());
    static BUFFER: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

#[no_mangle] pub extern "C" fn fr_board_reset() { BOARD.with(|s| *s.borrow_mut() = Board::new()); }
#[no_mangle] pub extern "C" fn fr_board_ptr() -> u32 {BOARD.with(|s| s.borrow().xy.as_ptr() as u32)}
#[no_mangle] pub extern "C" fn fr_board_len() -> u32 {BOARD_SIZE as u32}
#[no_mangle] pub extern "C" fn fr_board_revision() -> u32 {BOARD.with(|s| s.borrow().revision)}
#[no_mangle] pub extern "C" fn fr_board_move(id:u32,dx:i32,dy:i32) -> u32 {BOARD.with(|s| s.borrow_mut().move_by(id as usize,dx,dy) as u32)}
#[no_mangle] pub extern "C" fn fr_board_undo() -> u32 {BOARD.with(|s| s.borrow_mut().undo() as u32)}
#[no_mangle] pub extern "C" fn fr_board_redo() -> u32 {BOARD.with(|s| s.borrow_mut().redo() as u32)}
#[no_mangle] pub extern "C" fn fr_board_checksum() -> u32 {BOARD.with(|s| s.borrow().checksum())}

#[no_mangle] pub extern "C" fn fr_doc_reset() {DOC.with(|s| *s.borrow_mut() = Document::new());}
#[no_mangle] pub extern "C" fn fr_doc_insert(index:u32,offset:u32,codepoint:u32)->u32 {
    let Some(ch) = char::from_u32(codepoint) else {return 0;};
    DOC.with(|s| s.borrow_mut().insert(index as usize,offset as usize,ch) as u32)
}
#[no_mangle] pub extern "C" fn fr_doc_bold(index:u32)->u32 {DOC.with(|s| s.borrow_mut().bold(index as usize) as u32)}
#[no_mangle] pub extern "C" fn fr_doc_undo()->u32 {DOC.with(|s| s.borrow_mut().undo() as u32)}
#[no_mangle] pub extern "C" fn fr_doc_redo()->u32 {DOC.with(|s| s.borrow_mut().redo() as u32)}
#[no_mangle] pub extern "C" fn fr_doc_revision()->u32 {DOC.with(|s| s.borrow().revision)}
#[no_mangle] pub extern "C" fn fr_doc_checksum()->u32 {DOC.with(|s| s.borrow().checksum())}
/// Return UTF-8 lines for a bounded viewport; the first byte per line is 0 or 1 (bold).
/// The returned pointer remains valid until the next fr_doc_visible invocation.
#[no_mangle] pub extern "C" fn fr_doc_visible(start:u32,count:u32)->u32 {
    let text = DOC.with(|s| {
        s.borrow().blocks.iter().skip(start as usize).take(count.min(50) as usize)
         .map(|p| format!("{}{}\n", if p.bold {'1'} else {'0'},p.text)).collect::<String>()
    });
    BUFFER.with(|b| {let mut b=b.borrow_mut(); *b=text.into_bytes(); b.as_ptr() as u32})
}
#[no_mangle] pub extern "C" fn fr_doc_visible_len()->u32 {BUFFER.with(|b| b.borrow().len() as u32)}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn board_history_and_noop() {
        let mut b=Board::new();let initial=b.checksum();
        assert_eq!(b.xy.len(),20_000);
        assert!(!b.move_by(2,0,0));assert_eq!(b.revision,0);
        assert!(b.move_by(2,7,-3));assert_ne!(b.checksum(),initial);
        assert!(b.undo()); assert_eq!(b.xy[2],50);assert_eq!(b.xy[3],20);
        assert!(b.redo()); assert_eq!(b.xy[2],57); assert_eq!(b.revision,3);
    }
    #[test] fn doc_unicode_history() {
        let mut d=Document::new();let original=d.blocks[0].text.clone();
        assert!(d.insert(0,10,'界'));assert!(d.bold(0));
        assert!(d.undo()); assert!(!d.blocks[0].bold);assert!(d.undo());assert_eq!(d.blocks[0].text,original);
        assert!(d.redo());assert!(d.redo());assert!(d.blocks[0].text.contains('界'));
        assert!(d.blocks[0].bold);assert_eq!(d.revision,6);
    }
    #[test] fn bad_input() {
        let mut d=Document::new();assert!(!d.insert(1001,0,'a'));
        assert!(!d.insert(0,100000,'b'));assert!(!d.undo());
        assert_eq!(fr_doc_insert(0,0,0xD800),0);
    }
}
