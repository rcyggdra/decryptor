//! seeddb.bin downloader (with live progress)

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::app::Event;
use crate::tools;

pub const SEEDDB_URL: &str =
    "https://raw.githubusercontent.com/ihaveamac/3DS-rom-tools/master/seeddb.bin";

pub fn download_seeddb(tx: std::sync::mpsc::Sender<Event>, cancel: Arc<AtomicBool>) {
    std::thread::spawn(move || run(tx, cancel));
}

fn run(tx: std::sync::mpsc::Sender<Event>, cancel: Arc<AtomicBool>) {
    let dest = tools::seeddb_install_dir();
    let tmp = dest.with_extension("part");
    let res = (|| -> Result<u64, String> {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let resp = attohttpc::get(SEEDDB_URL)
            .send()
            .map_err(|e| format!("request failed: {}", e))?;
        if !resp.is_success() {
            return Err(format!("http {}", resp.status()));
        }
        let bytes = resp.bytes().map_err(|e| e.to_string())?;
        let mut file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        file.write_all(&bytes).map_err(|e| e.to_string())?;
        let _ = tx.send(Event::SeedProgress { done: bytes.len() as u64, total: bytes.len() as u64 });
        std::fs::rename(&tmp, &dest).map_err(|e| format!("move failed: {}", e))?;
        Ok(bytes.len() as u64)
    })();
    match res {
        Ok(bytes) => {
            let _ = tx.send(Event::SeedSaved { path: dest, bytes });
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            let _ = tx.send(Event::SeedFailed { err: e });
        }
    }
}
