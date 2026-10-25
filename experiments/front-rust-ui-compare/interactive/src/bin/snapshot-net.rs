//! Local-only TCP experiment for optimistic concurrency on FRST v1 snapshots.
//! Not a federated transport, authentication service or multi-node consensus.
//! Server serializes commits, holds an exclusive advisory file lock, and persists
//! before acknowledging. Transport: FNRT request, FNRS response, 17-byte headers.
use front_rust_interactive_probe::persistence;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Error, ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

const REQUEST_MAGIC: &[u8; 4] = b"FNRT";
const RESPONSE_MAGIC: &[u8; 4] = b"FNRS";
const GET: u8 = 1;
const CAS: u8 = 2;
const CAS_DROP_ACK: u8 = 3;
const OK: u8 = 0;
const APPLIED: u8 = 1;
const CONFLICT: u8 = 2;
const REPLAY: u8 = 3;
const INVALID: u8 = 4;
const MAX_BYTES: usize = 2 * 1024 * 1024;
const HEAD_LEN: usize = 17;

fn io_error(message: &'static str) -> Error { Error::new(ErrorKind::InvalidInput, message) }
fn timeout(stream: &TcpStream) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    Ok(())
}
#[derive(Debug)]
struct Request {
    op: u8,
    board_revision: u32,
    document_revision: u32,
    body: Vec<u8>,
}
#[derive(Debug)]
struct Response {
    status: u8,
    board_revision: u32,
    document_revision: u32,
    body: Vec<u8>,
}
fn encode_request(req: &Request) -> Vec<u8> {
    let mut v = Vec::with_capacity(HEAD_LEN + req.body.len());
    v.extend_from_slice(REQUEST_MAGIC);
    v.push(req.op);
    v.extend_from_slice(&req.board_revision.to_le_bytes());
    v.extend_from_slice(&req.document_revision.to_le_bytes());
    v.extend_from_slice(&(req.body.len() as u32).to_le_bytes());
    v.extend_from_slice(&req.body);
    v
}
fn encode_response(resp: &Response) -> Vec<u8> {
    let mut v = Vec::with_capacity(HEAD_LEN + resp.body.len());
    v.extend_from_slice(RESPONSE_MAGIC);
    v.push(resp.status);
    v.extend_from_slice(&resp.board_revision.to_le_bytes());
    v.extend_from_slice(&resp.document_revision.to_le_bytes());
    v.extend_from_slice(&(resp.body.len() as u32).to_le_bytes());
    v.extend_from_slice(&resp.body);
    v
}
fn decode_request(stream: &mut TcpStream) -> io::Result<Request> {
    let mut header = [0; HEAD_LEN];
    stream.read_exact(&mut header)?;
    if &header[..4] != REQUEST_MAGIC { return Err(io_error("bad request magic")); }
    let op = header[4];
    let board_revision = u32::from_le_bytes(header[5..9].try_into().unwrap());
    let document_revision = u32::from_le_bytes(header[9..13].try_into().unwrap());
    let length = u32::from_le_bytes(header[13..17].try_into().unwrap()) as usize;
    if length > MAX_BYTES || !(op == GET || op == CAS || op == CAS_DROP_ACK) ||
       (op == GET && length != 0) {
        return Err(io_error("invalid opcode or oversized request"));
    }
    let mut body = vec![0; length];
    stream.read_exact(&mut body)?;
    Ok(Request { op, board_revision, document_revision, body })
}
fn decode_response(stream: &mut TcpStream) -> io::Result<Response> {
    let mut header = [0; HEAD_LEN];
    stream.read_exact(&mut header)?;
    if &header[..4] != RESPONSE_MAGIC { return Err(io_error("bad response magic")); }
    let status = header[4];
    let board_revision = u32::from_le_bytes(header[5..9].try_into().unwrap());
    let document_revision = u32::from_le_bytes(header[9..13].try_into().unwrap());
    let length = u32::from_le_bytes(header[13..17].try_into().unwrap()) as usize;
    if length > MAX_BYTES || status > INVALID { return Err(io_error("bad response")); }
    let mut body = vec![0; length];
    stream.read_exact(&mut body)?;
    Ok(Response { status, board_revision, document_revision, body })
}
fn serve_one(stream: &mut TcpStream, path: &Path) -> io::Result<()> {
    timeout(stream)?;
    let req = match decode_request(stream) { Ok(x) => x, Err(_) => return Ok(()) };
    let current = persistence::recover(path)?;
    let state = persistence::receipt(&current)?;
    let mut response = Response {
        status: OK,
        board_revision: state.board_revision,
        document_revision: state.document_revision,
        body: Vec::new(),
    };
    if req.op == GET {
        response.body = current;
    } else if req.body == current {
        // Exact replay is idempotent even if the first reply was lost.
        response.status = REPLAY;
    } else if req.board_revision != state.board_revision ||
              req.document_revision != state.document_revision {
        response.status = CONFLICT;
    } else {
        match persistence::receipt(&req.body) {
            Ok(candidate) if
                candidate.board_revision >= state.board_revision &&
                candidate.document_revision >= state.document_revision &&
                (candidate.board_revision > state.board_revision ||
                 candidate.document_revision > state.document_revision) => {
                // Only one server writer; this commit cannot race another TCP
                // request. Advisory file lock prevents another *participating*
                // server process on the same local filesystem.
                persistence::persist(path, &req.body, false)?;
                response.status = APPLIED;
                response.board_revision = candidate.board_revision;
                response.document_revision = candidate.document_revision;
                if req.op == CAS_DROP_ACK {
                    // A write was committed, but the client never got an ACK.
                    // A later request with identical body must yield REPLAY.
                    return Ok(());
                }
            }
            _ => response.status = INVALID,
        }
    }
    stream.write_all(&encode_response(&response))?;
    stream.flush()
}
fn server(data: &Path, ready: &Path) -> io::Result<()> {
    if ready.exists() { return Err(io_error("ready file already exists")); }
    // Hold a stable sidecar inode across atomic replacement of the data file.
    let lock_path = data.with_extension("frst.lock");
    let lock_file = OpenOptions::new().read(true).write(true).create(true).open(lock_path)?;
    lock_file.try_lock().map_err(|_| Error::new(ErrorKind::WouldBlock, "another server already owns data"))?;
    persistence::recover(data)?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    fs::write(ready, format!("{}\n", address))?;
    println!("READY {}", address);
    io::stdout().flush()?;
    for incoming in listener.incoming() {
        match incoming {
            Ok(mut stream) => { if let Err(e) = serve_one(&mut stream, data) {
                eprintln!("CLIENT ERROR: {e}");
            } },
            Err(e) => eprintln!("ACCEPT ERROR: {e}"),
        }
    }
    drop(lock_file);
    Ok(())
}
fn connect(addr: &str, req: Request) -> io::Result<Response> {
    let address: SocketAddr = addr.parse().map_err(|_| io_error("invalid address"))?;
    if !address.ip().is_loopback() { return Err(io_error("local-only experiment")); }
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(5))?;
    timeout(&stream)?;
    stream.write_all(&encode_request(&req))?;
    decode_response(&mut stream)
}
fn fetch(addr: &str, target: &Path) -> io::Result<()> {
    let response = connect(addr, Request{op:GET,board_revision:0,document_revision:0,body:Vec::new()})?;
    if response.status != OK { return Err(io_error("GET rejected")); }
    let proof = persistence::receipt(&response.body)?;
    if proof.board_revision != response.board_revision ||
       proof.document_revision != response.document_revision { return Err(io_error("GET revision mismatch")); }
    // Save the downloaded file; unlike server publication, local client files
    // in this experiment have no concurrent writers.
    persistence::persist(target, &response.body, false)?;
    println!("GET {}", proof.json());
    Ok(())
}
fn cas(addr: &str, base: &Path, proposal: &Path, drop_ack: bool) -> io::Result<()> {
    let old = persistence::recover(base)?;
    let expected = persistence::receipt(&old)?;
    let body = persistence::recover(proposal)?;
    let response = connect(addr, Request {
        op: if drop_ack {CAS_DROP_ACK} else {CAS},
        board_revision: expected.board_revision,
        document_revision: expected.document_revision,
        body,
    });
    match response {
        Ok(r) => {
            let status = match r.status { APPLIED=>"APPLIED",CONFLICT=>"CONFLICT",REPLAY=>"REPLAY",INVALID=>"INVALID",_=>"UNEXPECTED" };
            println!("{} board_rev={} doc_rev={}",status,r.board_revision,r.document_revision);
            if r.status == APPLIED || r.status == REPLAY { Ok(()) }
            else if r.status == CONFLICT { Err(Error::new(ErrorKind::AlreadyExists,"STALE_REVISION")) }
            else { Err(io_error("invalid write or response")) }
        }
        Err(e) if drop_ack && e.kind() == ErrorKind::UnexpectedEof => {
            println!("ACK_LOST");
            Err(Error::new(ErrorKind::ConnectionAborted,"ACK_LOST: publication result unknown"))
        }
        Err(e) => Err(e),
    }
}
fn main() -> io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        return Err(io_error("usage: snapshot-net <serve DATA READY | get ADDR LOCAL | cas ADDR BASE PROPOSAL | drop-ack ADDR BASE PROPOSAL>"));
    }
    match args[1].as_str() {
        "serve" if args.len() == 4 => server(Path::new(&args[2]),Path::new(&args[3])),
        "get" if args.len() == 4 => fetch(&args[2],Path::new(&args[3])),
        "cas" if args.len() == 5 => cas(&args[2],Path::new(&args[3]),Path::new(&args[4]),false),
        "drop-ack" if args.len() == 5 => cas(&args[2],Path::new(&args[3]),Path::new(&args[4]),true),
        _ => Err(io_error("invalid command or argument count")),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn framing_roundtrip_headers_are_bounded() {
        let req=Request{op:CAS,board_revision:1,document_revision:2,body:vec![1,2,3]};
        let bytes=encode_request(&req);
        assert_eq!(bytes.len(),20);
        assert_eq!(&bytes[..4],REQUEST_MAGIC);
        assert_eq!(u32::from_le_bytes(bytes[13..17].try_into().unwrap()),3);
        let resp=Response{status:CONFLICT,board_revision:3,document_revision:4,body:vec![]};
        let encoded=encode_response(&resp);
        assert_eq!(encoded.len(),HEAD_LEN);
        assert_eq!(encoded[4],CONFLICT);
    }
}
