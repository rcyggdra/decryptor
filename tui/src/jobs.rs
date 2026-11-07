//! the worker engine: probe → (skip or decrypt) → repack, sequentially.
//! every event lands back in the ui/headless via `Event` (never panics out loud).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Sender;

use log::{debug, info, LevelFilter};

use crate::app::{Event, Status, WorkerSnapshot};
use crate::tools;

/// global ring log so lib `log!` output reaches the ui/headless loop
static RING: std::sync::OnceLock<crate::app::RingLog> = std::sync::OnceLock::new();

pub fn ring() -> &'static crate::app::RingLog {
    RING.get_or_init(crate::app::RingLog::new)
}

struct RingLogger;

impl log::Log for RingLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Debug
    }
    fn log(&self, record: &log::Record) {
        crate::app::RingLog::push(ring(), format!("{}", record.args()));
    }
    fn flush(&self) {}
}

pub fn install_logger() {
    let _ = log::set_logger(&RingLogger);
    log::set_max_level(LevelFilter::Debug);
}

pub fn process_file(
    idx: usize,
    path: PathBuf,
    snap: WorkerSnapshot,
    tx: Sender<Event>,
    cancel: &AtomicBool,
) -> anyhow::Result<()> {
    let send = |e: Event| {
        let _ = tx.send(e);
    };

    send(Event::Status { idx, status: Status::Probing, msg: None });

    let probe = match ctrdecrypt::probe(&path) {
        Ok(p) => p,
        Err(e) => {
            let msg = format!("probe failed: {}", e);
            send(Event::Status { idx, status: Status::NotSupported, msg: Some(msg.clone()) });
            send(Event::Finished { idx, ok: false, repacked: None, msg: Some(msg) });
            anyhow::bail!("probe failed");
        }
    };
    send(Event::Probed { idx, probe: probe.clone() });

    let tid = probe.title_id.clone().unwrap_or_else(|| "?".into());
    let ver = probe
        .title_version
        .map(|v| format!(" · v{}.{}", v.0, v.1))
        .unwrap_or_default();
    let enc = match probe.encrypted {
        Some(true) => " · encrypted",
        Some(false) => " · already decrypted",
        None => " · unknown crypto",
    };
    let label = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| tools::tilde(path));
    info!("{} — tid {}{}{}", label, tid, ver, enc);

    match probe.encrypted {
        Some(false) => {
            send(Event::Status {
                idx,
                status: Status::AlreadyDecrypted,
                msg: Some("already decrypted, nothing to do".into()),
            });
            send(Event::Finished { idx, ok: true, repacked: None, msg: None });
            return Ok(());
        }
        Some(true) => {
            // full pipeline continues below
        }
        None => {
            let msg = "couldn't read the file's encryption state — refusing to guess".to_string();
            send(Event::Status { idx, status: Status::Failed, msg: Some(msg.clone()) });
            send(Event::Finished { idx, ok: false, repacked: None, msg: Some(msg.clone()) });
            anyhow::bail!(msg);
        }
    }

    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
        anyhow::bail!("cancelled");
    }

    // ---- decrypt (in-process, with progress) ----
    send(Event::Status { idx, status: Status::Decrypting, msg: None });
    let ptx = tx.clone();
    let options = ctrdecrypt::DecryptOptions {
        verbose: false,
        seeddb: snap.settings.seeddb.clone(),
        allow_online: snap.settings.allow_online,
        progress: Some(Box::new(move |done, total| {
            let _ = ptx.send(Event::Progress { idx, done, total });
        })),
        ..Default::default()
    };
    let outputs = match ctrdecrypt::decrypt(&path, &options) {
        Ok(o) if !o.is_empty() => {
            send(Event::Decrypted { idx, outputs: o.clone() });
            o
        }
        Ok(_) => {
            let msg = "the dumper produced no output (bad or unsupported file)".to_string();
            send(Event::Status { idx, status: Status::Failed, msg: Some(msg.clone()) });
            send(Event::Finished { idx, ok: false, repacked: None, msg: Some(msg.clone()) });
            anyhow::bail!(msg);
        }
        Err(e) => {
            let msg = format!("decrypt failed: {}", e);
            send(Event::Status { idx, status: Status::Failed, msg: Some(msg.clone()) });
            send(Event::Finished { idx, ok: false, repacked: None, msg: Some(msg.clone()) });
            anyhow::bail!(msg);
        }
    };

    // ---- repack with makerom ----
    let Some(makerom) = tools::resolve_makerom(&snap.settings.makerom) else {
        let msg = "decrypted, but makerom is missing so no repack happened — run install.sh"
            .to_string();
        send(Event::Status { idx, status: Status::Done, msg: Some(msg.clone()) });
        send(Event::Finished { idx, ok: true, repacked: None, msg: Some(msg) });
        return Ok(());
    };

    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
        anyhow::bail!("cancelled");
    }

    send(Event::Status { idx, status: Status::Repacking, msg: None });
    let plan = repack_plan(&path, probe.title_kind, probe.title_version);
    let out_file = plan.output_name(&path);
    let args = plan.args(&outputs, &out_file);
    debug!("makerom plan: {} {}", makerom.display(), args.join(" "));

    let out = Command::new(&makerom)
        .args(&args)
        .current_dir(path.parent().unwrap_or_else(|| Path::new(".")))
        .output();

    match out {
        Ok(o) => {
            // let the log see the tail of makerom chatter
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            );
            let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
            let start = lines.len().saturating_sub(4);
            for l in &lines[start..] {
                send(Event::FileLog { idx, line: (*l).to_string() });
            }

            if o.status.success() && out_file.exists() {
                if !snap.settings.keep_intermediates {
                    for o2 in &outputs {
                        debug!("cleanup intermediate {}", o2.display());
                        let _ = std::fs::remove_file(o2);
                    }
                }
                info!("wrote {}", tools::tilde(&out_file));
                send(Event::Status { idx, status: Status::Done, msg: None });
                send(Event::Finished { idx, ok: true, repacked: Some(out_file), msg: None });
                Ok(())
            } else {
                let code = o.status.code().unwrap_or(-1);
                let msg = format!("makerom exited {} — {}", code, lines.last().copied().unwrap_or("no output"));
                send(Event::Status { idx, status: Status::Failed, msg: Some(msg.clone()) });
                send(Event::Finished { idx, ok: false, repacked: None, msg: Some(msg.clone()) });
                anyhow::bail!(msg);
            }
        }
        Err(e) => {
            let msg = format!("couldn't run {}: {}", makerom.display(), e);
            send(Event::Status { idx, status: Status::Failed, msg: Some(msg.clone()) });
            send(Event::Finished { idx, ok: false, repacked: None, msg: Some(msg.clone()) });
            anyhow::bail!(msg);
        }
    }
}

// --------------------------------------------------------------------------- //
// repack plan — mirrors the upstream .bat routing rules
// --------------------------------------------------------------------------- //

struct RepackPlan {
    head: Vec<String>,
    /// "{BASE}" gets the input stem
    base: String,
    /// true when the input was ncsd (partition-index ids), false for cia content ids
    ncsd: bool,
}

impl RepackPlan {
    fn output_name(&self, input: &Path) -> PathBuf {
        let mut stem = input
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_default();
        for ext in [".3ds", ".cci", ".cia", ".3dz"] {
            if let Some(s) = stem.strip_suffix(ext) {
                stem = s.to_string();
                break;
            }
        }
        input.with_file_name(self.base.replace("{BASE}", &stem))
    }

    fn args(&self, outputs: &[PathBuf], out_file: &Path) -> Vec<String> {
        let mut args = self.head.clone();
        for out in outputs {
            if let Some(spec) = content_spec(out, self.ncsd) {
                args.extend(["-content".into(), spec]);
            }
        }
        args.extend(["-o".into(), out_file.to_string_lossy().to_string()]);
        args
    }
}

/// makerom v0.19 content spec (parser walks to the last-but-one ':', so the
/// 3-field form is required for both cci and cia):
///   cci → "{file}:{partition_index}:{partition_index}"   (id ignored)
///   cia → "{file}:{content_index}:{content_id}"
fn content_spec(out: &Path, ncsd: bool) -> Option<String> {
    let name = out.file_name()?.to_string_lossy().to_string();
    let stem = name.strip_suffix(".ncch")?;
    // layouts: "{base}.{partition-name}.{cid:08X}.ncch" (ncsd) or
    //          "{base}.{content-index-decimal}.{cid:08X}.ncch" (cia)
    let parts: Vec<&str> = stem.rsplitn(3, '.').collect();
    if parts.len() < 3 {
        return None;
    }
    let cid_str = parts[0];
    let mid = parts[1];
    if cid_str.len() != 8 || !cid_str.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let cid = u32::from_str_radix(cid_str, 16).ok()?;
    if ncsd {
        // mid is a partition label (Main, Manual, …); the pid equals the content id
        let index = cid;
        Some(format!("{}:{}:{}", out.display(), index, index))
    } else {
        let index: u32 = mid.parse().ok()?;
        Some(format!("{}:{}:{:08X}", out.display(), index, cid))
    }
}

fn repack_plan(
    path: &Path,
    kind: Option<ctrdecrypt::TitleKind>,
    ver: Option<(u16, u16, u16)>,
) -> RepackPlan {
    let mut stem = path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default();
    for ext in [".3ds", ".cci", ".cia", ".3dz"] {
        if let Some(s) = stem.strip_suffix(ext) {
            stem = s.to_string();
            break;
        }
    }
    let ext_cia = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("cia") || e.eq_ignore_ascii_case("3dz"))
        .unwrap_or(false);
    let sign = vec!["-ignoresign".to_string(), "-target".into(), "p".into()];
    let vers = ver.map(|v| vec!["-ver".to_string(), format_ver(v)]).unwrap_or_default();

    if ext_cia {
        match kind {
            Some(ctrdecrypt::TitleKind::Update) => {
                let mut head = vec!["-f".into(), "cia".into()];
                head.extend(sign);
                head.extend(vers);
                RepackPlan { head, base: format!("{stem} (Update)-decrypted.cia"), ncsd: false }
            }
            Some(ctrdecrypt::TitleKind::Dlc) => {
                let mut head = vec!["-f".into(), "cia".into(), "-dlc".into()];
                head.extend(sign);
                head.extend(vers);
                RepackPlan { head, base: format!("{stem} (DLC)-decrypted.cia"), ncsd: false }
            }
            _ => {
                let mut head = vec!["-f".into(), "cia".into()];
                head.extend(sign);
                head.extend(vers);
                RepackPlan { head, base: format!("{stem}-decrypted.cia"), ncsd: false }
            }
        }
    } else {
        let mut head = vec!["-f".into(), "cci".into()];
        head.extend(sign);
        RepackPlan { head, base: format!("{stem}-decrypted.cci"), ncsd: true }
    }
}

fn format_ver(v: (u16, u16, u16)) -> String {
    if v.2 == 0 {
        format!("{}.{}", v.0, v.1)
    } else {
        format!("{}.{}.{}", v.0, v.1, v.2)
    }
}
