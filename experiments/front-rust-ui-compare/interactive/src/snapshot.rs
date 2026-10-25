//! Versioned fixture snapshots. Not a production document format or cryptographic signature.
//! Snapshot v1: "FRST", version:u8, flags:u8, reserved:u16,
//! board_count:u32, block_count:u32, board_revision:u32, doc_revision:u32,
//! board pairs (x:i32,y:i32)*10000, blocks (bold:u8, utf8_length:u32, utf8[])*1000,
//! fnv1a32 of every preceding byte. Little-endian throughout.
//! History is intentionally not persisted; this captures only durable document state.
use super::{Board, Document, Paragraph, BOARD, DOC, BOARD_SIZE, BLOCKS};
use std::cell::RefCell;

const MAX_SNAPSHOT_BYTES: usize = 2 * 1024 * 1024;
const MAX_TEXT_BYTES: usize = 16 * 1024;
const VERSION: u8 = 1;
const HEADER_SIZE: usize = 24;

thread_local! {
    static SNAPSHOT_EXCHANGE: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

fn push_u32(out: &mut Vec<u8>, value: u32) { out.extend_from_slice(&value.to_le_bytes()); }
fn checksum(bytes: &[u8]) -> u32 {
    bytes.iter().fold(2_166_136_261_u32, |h, b| (h ^ u32::from(*b)).wrapping_mul(16_777_619))
}

pub(super) fn encode(board: &Board, document: &Document) -> Vec<u8> {
    let mut result = Vec::with_capacity(160_000);
    result.extend_from_slice(b"FRST");
    result.extend_from_slice(&[VERSION, 0, 0, 0]);
    push_u32(&mut result, board.xy.len() as u32 / 2);
    push_u32(&mut result, document.blocks.len() as u32);
    push_u32(&mut result, board.revision);
    push_u32(&mut result, document.revision);
    for coord in &board.xy { result.extend_from_slice(&coord.to_le_bytes()); }
    for block in &document.blocks {
        result.push(u8::from(block.bold));
        push_u32(&mut result, block.text.len() as u32);
        result.extend_from_slice(block.text.as_bytes());
    }
    let crc = checksum(&result);
    push_u32(&mut result, crc);
    result
}

struct Reader<'a> { bytes: &'a [u8], index: usize }
impl<'a> Reader<'a> {
    fn read(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.index.checked_add(n)?;
        let r = self.bytes.get(self.index..end)?;
        self.index = end;
        Some(r)
    }
    fn u8(&mut self) -> Option<u8> { Some(self.read(1)?[0]) }
    fn u32(&mut self) -> Option<u32> { Some(u32::from_le_bytes(self.read(4)?.try_into().ok()?)) }
    fn i32(&mut self) -> Option<i32> { Some(i32::from_le_bytes(self.read(4)?.try_into().ok()?)) }
}
pub(super) fn decode(bytes: &[u8]) -> Option<(Board, Document)> {
    if !(HEADER_SIZE + 4..=MAX_SNAPSHOT_BYTES).contains(&bytes.len()) { return None; }
    let split = bytes.len().checked_sub(4)?;
    let declared = u32::from_le_bytes(bytes.get(split..)?.try_into().ok()?);
    if checksum(&bytes[..split]) != declared { return None; }
    let mut reader = Reader { bytes: &bytes[..split], index: 0 };
    if reader.read(4)? != b"FRST" || reader.u8()? != VERSION ||
       reader.u8()? != 0 || reader.read(2)? != [0, 0] { return None; }
    if reader.u32()? as usize != BOARD_SIZE || reader.u32()? as usize != BLOCKS { return None; }
    let board_revision = reader.u32()?;
    let doc_revision = reader.u32()?;
    let mut xy = Vec::with_capacity(BOARD_SIZE * 2);
    for _ in 0..BOARD_SIZE {
        xy.push(reader.i32()?); xy.push(reader.i32()?);
    }
    let mut blocks = Vec::with_capacity(BLOCKS);
    for _ in 0..BLOCKS {
        let bold = reader.u8()?;
        if bold > 1 { return None; }
        let length = reader.u32()? as usize;
        if length > MAX_TEXT_BYTES { return None; }
        let text = String::from_utf8(reader.read(length)?.to_vec()).ok()?;
        blocks.push(Paragraph { text, bold: bold == 1 });
    }
    if reader.index != split { return None; }
    Some((
        Board { xy, revision: board_revision, past: Vec::new(), future: Vec::new() },
        Document { blocks, revision: doc_revision, past: Vec::new(), future: Vec::new() },
    ))
}

/// Export pointer/length must be copied immediately before the next exchange operation.
#[no_mangle]
pub extern "C" fn fr_snapshot_ptr() -> u32 {
    let data = BOARD.with(|b| DOC.with(|d| encode(&b.borrow(), &d.borrow())));
    SNAPSHOT_EXCHANGE.with(|slot| {
        let mut slot = slot.borrow_mut();
        *slot = data;
        slot.as_ptr() as u32
    })
}
#[no_mangle]
pub extern "C" fn fr_snapshot_len() -> u32 {
    SNAPSHOT_EXCHANGE.with(|slot| slot.borrow().len() as u32)
}

/// Borrow a temporary bounded input area, not the committed document.
#[no_mangle]
pub extern "C" fn fr_import_ptr(len: u32) -> u32 {
    let len = len as usize;
    if len < HEADER_SIZE + 4 || len > MAX_SNAPSHOT_BYTES { return 0; }
    SNAPSHOT_EXCHANGE.with(|slot| {
        let mut slot = slot.borrow_mut();
        slot.resize(len, 0);
        slot.as_mut_ptr() as u32
    })
}

/// On error, the existing board and document are unchanged.
#[no_mangle]
pub extern "C" fn fr_restore(len: u32) -> u32 {
    let candidate = SNAPSHOT_EXCHANGE.with(|slot| {
        let slot = slot.borrow();
        if slot.len() != len as usize { return None; }
        decode(&slot)
    });
    let Some((board, doc)) = candidate else { return 0; };
    BOARD.with(|b| *b.borrow_mut() = board);
    DOC.with(|d| *d.borrow_mut() = doc);
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restores_unicode_geometry_revisions_and_forgets_undo_history() {
        let mut board = Board::new();
        let mut doc = Document::new();
        assert!(board.move_by(2, 9, -4));
        assert!(doc.insert(0, 10, '界'));
        assert!(doc.bold(0));
        let snapshot = encode(&board, &doc);
        let (mut restored_board, mut restored_doc) = decode(&snapshot).expect("valid");
        assert_eq!(snapshot, encode(&restored_board, &restored_doc));
        assert_eq!(board.checksum(), restored_board.checksum());
        assert_eq!(doc.checksum(), restored_doc.checksum());
        assert!(!restored_board.undo());
        assert!(!restored_doc.undo());
        assert_eq!(restored_board.revision, 1);
        assert_eq!(restored_doc.revision, 2);
        assert!(snapshot.len() < MAX_SNAPSHOT_BYTES);
    }
    #[test]
    fn rejects_corrupt_unsupported_and_trailing_data() {
        let sample = encode(&Board::new(), &Document::new());
        let mut modified = sample.clone();
        modified[30] ^= 0x11;
        assert!(decode(&modified).is_none());
        modified = sample.clone();
        modified[4] = 2;
        let offset = modified.len() - 4;
        let crc = checksum(&modified[..offset]);
        modified[offset..].copy_from_slice(&crc.to_le_bytes());
        assert!(decode(&modified).is_none());
        modified = sample.clone();
        modified.extend_from_slice(&[0]);
        assert!(decode(&modified).is_none());
        assert!(decode(&sample[..sample.len()-1]).is_none());
    }
    #[test]
    fn rejects_invalid_utf8_boolean_and_wrong_counts() {
        let original = encode(&Board::new(), &Document::new());
        let mut bytes = original.clone();
        // First block flag after header and 10k coordinate pairs.
        let flag_index = HEADER_SIZE + BOARD_SIZE * 8;
        bytes[flag_index] = 2;
        let crc_offset = bytes.len() - 4;
        let crc = checksum(&bytes[..crc_offset]);
        bytes[crc_offset..].copy_from_slice(&crc.to_le_bytes());
        assert!(decode(&bytes).is_none());
        let mut bytes = original.clone();
        bytes[8] = 0;
        let crc = checksum(&bytes[..crc_offset]);
        bytes[crc_offset..].copy_from_slice(&crc.to_le_bytes());
        assert!(decode(&bytes).is_none());
    }
}
