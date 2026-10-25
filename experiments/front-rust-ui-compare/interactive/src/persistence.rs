//! Linux-first persistence experiment. Not the Akula File Gate.
//! One trusted local writer: write/sync a unique adjacent file, rename, sync parent.
//! Never update the existing snapshot in place. Existing files must be readable,
//! version-valid and within bounds before a new operation is committed.
use crate::{snapshot, Board, Document};
use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

pub struct Receipt {
    pub board_checksum: u32,
    pub document_checksum: u32,
    pub board_revision: u32,
    pub document_revision: u32,
    pub bytes: usize,
}
impl Receipt {
    pub fn json(&self) -> String {
        format!(
            "{{\"board_checksum\":{},\"document_checksum\":{},\"board_revision\":{},\"document_revision\":{},\"bytes\":{}}}",
            self.board_checksum, self.document_checksum, self.board_revision, self.document_revision, self.bytes
        )
    }
}
fn invalid() -> io::Error { io::Error::new(ErrorKind::InvalidData, "invalid FRST snapshot") }

pub fn receipt(bytes: &[u8]) -> io::Result<Receipt> {
    let (b, d) = snapshot::decode(bytes).ok_or_else(invalid)?;
    Ok(Receipt {
        board_checksum: b.checksum(), document_checksum: d.checksum(),
        board_revision: b.revision, document_revision: d.revision, bytes: bytes.len()
    })
}
pub fn seed() -> Vec<u8> {
    let mut b = Board::new();
    let mut d = Document::new();
    assert!(b.move_by(2, 11, -5));
    assert!(d.insert(0, 10, '界'));
    assert!(d.bold(0));
    snapshot::encode(&b, &d)
}
pub fn mutate(bytes: &[u8]) -> io::Result<Vec<u8>> {
    let (mut b, mut d) = snapshot::decode(bytes).ok_or_else(invalid)?;
    if !b.move_by(4, 9, 12) || !d.insert(42, 15, '💾') {
        return Err(invalid());
    }
    Ok(snapshot::encode(&b, &d))
}
pub fn recover(path: &Path) -> io::Result<Vec<u8>> {
    if fs::metadata(path)?.len() > MAX_FILE_BYTES {
        return Err(io::Error::new(ErrorKind::InvalidData, "snapshot too large"));
    }
    let bytes = fs::read(path)?;
    receipt(&bytes)?;
    Ok(bytes)
}
/// Injected interruption before rename simulates failed publication, not a real
/// power-loss test. Only successful fsync and directory sync count as durable ACK.
pub fn persist(path: &Path, bytes: &[u8], interrupt_before_rename: bool) -> io::Result<Receipt> {
    let proof = receipt(bytes)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(invalid());
    }
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    if !parent.is_dir() { return Err(io::Error::new(ErrorKind::NotFound, "destination directory missing")); }
    let name = path.file_name().ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, "filename missing"))?;
    let start = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    let mut temp = None;
    for _ in 0..8 {
        let serial = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let unique = format!(".{}.{}.{}.{}.pending", name.to_string_lossy(), std::process::id(), start, serial);
        let temp_path = parent.join(unique);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
        match options.open(&temp_path) {
            Ok(file) => { temp = Some((temp_path, file)); break; }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    let (temp_path, mut file) = temp.ok_or_else(|| io::Error::new(ErrorKind::AlreadyExists, "could not reserve staging name"))?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        if interrupt_before_rename {
            return Err(io::Error::new(ErrorKind::Interrupted, "injected before-rename crash"));
        }
        fs::rename(&temp_path, path)?;
        #[cfg(unix)]
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result?;
    Ok(proof)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir() -> std::path::PathBuf {
        let folder = std::env::temp_dir().join(format!(
            "front-rust-persist-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&folder).expect("test directory");
        folder
    }
    #[test]
    fn disk_roundtrip_and_reprocess() {
        let dir = test_dir(); let path=dir.join("data.frst");
        let initial=seed();
        persist(&path, &initial, false).unwrap();
        assert_eq!(recover(&path).unwrap(), initial);
        let modified=mutate(&recover(&path).unwrap()).unwrap();
        persist(&path, &modified, false).unwrap();
        assert_eq!(recover(&path).unwrap(), modified);
        assert_eq!(receipt(&modified).unwrap().board_revision, 2);
        assert_eq!(receipt(&modified).unwrap().document_revision, 3);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn failed_publish_does_not_change_previous_document() {
        let dir = test_dir(); let path=dir.join("document.frst");
        let initial=seed();
        persist(&path, &initial, false).unwrap();
        let changed=mutate(&initial).unwrap();
        assert_eq!(persist(&path,&changed,true).unwrap_err().kind(),ErrorKind::Interrupted);
        assert_eq!(recover(&path).unwrap(),initial);
        let pending=fs::read_dir(&dir).unwrap().filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".pending")).count();
        assert_eq!(pending,0);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn rejects_truncation_and_corruption_before_replacing() {
        let dir=test_dir();let path=dir.join("file.frst");
        let original=seed();persist(&path,&original,false).unwrap();
        let mut corrupted=original.clone();corrupted[35]^=9;
        assert!(persist(&path,&corrupted,false).is_err());
        assert!(persist(&path,&original[..original.len()-1],false).is_err());
        assert_eq!(recover(&path).unwrap(), original);
        fs::remove_dir_all(dir).unwrap();
    }
}
