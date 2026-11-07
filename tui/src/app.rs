//! application state shared between the ui thread and the job worker.

use std::path::PathBuf;
use std::sync::Mutex;

use ctrdecrypt::Probe;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Pending,
    Probing,
    Queued,
    Decrypting,
    Repacking,
    Done,
    AlreadyDecrypted,
    Failed,
    NotSupported,
}

#[derive(Clone)]
pub struct FileEntry {
    pub path: PathBuf,
    pub name: String,
    pub size: u64,
    pub selected: bool,
    pub status: Status,
    pub probe: Option<Probe>,
    /// (done, total)
    pub progress: (u64, u64),
    pub message: Option<String>,
    pub outputs: Vec<PathBuf>,
    pub repacked: Option<PathBuf>,
}

impl FileEntry {
    pub fn new(path: PathBuf) -> Self {
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string_lossy().to_string());
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        FileEntry {
            path,
            name,
            size,
            selected: true,
            status: Status::Pending,
            probe: None,
            progress: (0, 0),
            message: None,
            outputs: vec![],
            repacked: None,
        }
    }
}

#[derive(Clone)]
pub struct Settings {
    pub seeddb: Option<PathBuf>,
    pub makerom: Option<PathBuf>,
    pub allow_online: bool,
    pub keep_intermediates: bool,
}

/// The chunk of state the worker thread is allowed to see (plain data only).
#[derive(Clone)]
pub struct WorkerSnapshot {
    pub settings: Settings,
}

#[derive(Debug)]
pub enum Event {
    /// probe finished
    Probed { idx: usize, probe: Probe },
    /// status change with optional message
    Status { idx: usize, status: Status, msg: Option<String> },
    /// progress ticks while decrypting
    Progress { idx: usize, done: u64, total: u64 },
    /// a worker log line for the file (makerom output etc.)
    FileLog { idx: usize, line: String },
    /// decrypt done, ncch produced
    Decrypted { idx: usize, outputs: Vec<PathBuf> },
    /// repack finished (ok or not)
    Finished { idx: usize, ok: bool, repacked: Option<PathBuf>, msg: Option<String> },
    /// worker done
    Idle,
    /// seeddb download progress (done, total)
    SeedProgress { done: u64, total: u64 },
    SeedSaved { path: PathBuf, bytes: u64 },
    SeedFailed { err: String },
}

/// shared ring log; logger threads write here, the ui drains it
pub struct RingLog(pub Mutex<VecDeque<String>>);

use std::collections::VecDeque;

impl RingLog {
    pub fn new() -> Self {
        RingLog(Mutex::new(VecDeque::with_capacity(2048)))
    }

    pub fn push(line: &RingLog, s: String) {
        let mut q = line.0.lock().unwrap();
        if q.len() > 4096 {
            q.pop_front();
        }
        q.push_back(s);
    }

    pub fn drain(line: &RingLog, out: &mut Vec<String>) {
        let mut q = line.0.lock().unwrap();
        for _ in 0..256 {
            match q.pop_front() {
                Some(s) => out.push(s),
                None => break,
            }
        }
    }
}

pub mod modal {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[allow(dead_code)]
    pub enum Modal {
        None,
        MainMenu,
        Settings { field: usize },
        Downloads,
        Help,
        About,
        DirInput,
        ConfirmQuit,
    }
}
