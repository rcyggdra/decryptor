//! non-interactive mode: decrypt a directory's roms, printing plain lines.
//! used by scripts (ihaveacreditcard) and odd terminals.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Arc};

use crate::app::{Event, Settings, WorkerSnapshot};
use crate::jobs;
use crate::tools;

pub fn run(settings: Settings, dir: &Path, files: Vec<std::path::PathBuf>) -> i32 {
    jobs::install_logger();
    let snap = WorkerSnapshot { settings };
    let (tx, rx) = mpsc::channel::<Event>();
    let cancel: Arc<AtomicBool> = Arc::new(false.into());

    let n = files.len();
    let mut ok = 0;
    let mut failed = 0;

    println!("decryptor-tui headless · {} · {} file(s)", tools::tilde(dir), n);
    for (idx, path) in files.iter().enumerate() {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string());
        println!(
            "[{}/{}] {} ({})",
            idx + 1,
            n,
            name,
            tools::human(std::fs::metadata(path).map(|m| m.len()).unwrap_or(0))
        );
        std::io::Write::flush(&mut std::io::stdout()).ok();

        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            jobs::process_file(idx, path.clone(), snap.clone(), tx.clone(), &cancel)
        }));

        // drain everything the worker said, in order
        loop {
            match rx.try_recv() {
                Ok(ev) => print_event(&ev),
                Err(mpsc::TryRecvError::Empty) | Err(mpsc::TryRecvError::Disconnected) => break,
            }
        }
        let mut ring: Vec<String> = vec![];
        crate::app::RingLog::drain(jobs::ring(), &mut ring);
        for line in ring {
            if !line.trim().is_empty() {
                println!("· {}", line);
            }
        }

        match res {
            Ok(Ok(())) => ok += 1,
            _ => failed += 1,
        }
    }

    println!();
    if failed == 0 {
        println!("✓ {} of {} handled", ok, n);
        0
    } else {
        println!("✗ {} of {} failed to complete", failed, n);
        1
    }
}

fn print_event(ev: &Event) {
    match ev {
        Event::Probed { probe, .. } => {
            print!("\r");
            let ver = probe
                .title_version
                .map(|v| format!(" v{}.{}", v.0, v.1))
                .unwrap_or_default();
            let enc = match probe.encrypted {
                Some(true) => "encrypted",
                Some(false) => "already decrypted",
                None => "unknown crypto",
            };
            println!(
                "  tid {} · {}{} · {}",
                probe.title_id.as_deref().unwrap_or("?"),
                probe.title_kind.map(|k| k.label()).unwrap_or("other"),
                ver,
                enc
            );
        }
        Event::Progress { done, total, .. } if *total > 0 => {
            print!("\r  ▰{:>3}%", done * 100 / total);
            std::io::Write::flush(&mut std::io::stdout()).ok();
        }
        Event::Status { status: crate::app::Status::AlreadyDecrypted, msg: Some(m), .. } => {
            print!("\r");
            println!("  {}", m);
        }
        Event::Status { msg: Some(m), .. } => {
            print!("\r");
            println!("  {}", m);
        }
        Event::Decrypted { outputs, .. } => {
            print!("\r");
            println!("  dumped {} ncch part(s)", outputs.len());
        }
        Event::FileLog { line, .. } => {
            print!("\r");
            println!("  {}", line);
        }
        Event::Finished { ok, repacked, msg, .. } => match (ok, repacked) {
            (true, Some(r)) => {
                print!("\r");
                println!("  → {}", tools::tilde(r));
            }
            (true, None) => {}
            (false, _) => {
                print!("\r");
                if let Some(m) = msg {
                    println!("  ✗ {}", m);
                } else {
                    println!("  ✗ failed");
                }
            }
        },
        _ => {}
    }
}
