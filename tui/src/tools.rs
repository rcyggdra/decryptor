//! resolve tool + key paths, in plain linux fashion (like ihaveacreditcard)

use std::path::{Path, PathBuf};

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into()))
}

/// first existing candidate
fn first_existing(cands: &[PathBuf]) -> Option<PathBuf> {
    cands.into_iter().find(|p| p.is_file()).cloned()
}

/// seeddb.bin search order (explicit > cwd > ~/3ds > ~/.3ds > ./bin > exe dir)
pub fn resolve_seeddb(explicit: &Option<PathBuf>) -> Option<PathBuf> {
    if let Some(p) = explicit {
        if p.is_file() {
            return Some(p.clone());
        }
    }
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()));
    let h = home();
    let mut cands = vec![PathBuf::from("seeddb.bin")];
    cands.push(h.join("3ds").join("seeddb.bin"));
    cands.push(h.join(".3ds").join("seeddb.bin"));
    cands.push(PathBuf::from("bin/seeddb.bin"));
    if let Some(d) = &exe_dir {
        cands.push(d.join("seeddb.bin"));
        cands.push(d.join("../bin/seeddb.bin"));
    }
    first_existing(&cands)
}

/// the place a downloaded seeddb should land (~/3ds preferred, then ~/.3ds)
pub fn seeddb_install_dir() -> PathBuf {
    let h = home();
    let d = h.join("3ds");
    if d.is_dir() {
        return d.join("seeddb.bin");
    }
    let d2 = h.join(".3ds");
    if d2.is_dir() {
        return d2.join("seeddb.bin");
    }
    d
}

/// makerom search order
pub fn resolve_makerom(explicit: &Option<PathBuf>) -> Option<PathBuf> {
    if let Some(p) = explicit {
        if p.is_file() || is_executable(p) {
            return Some(p.clone());
        }
    }
    // next to this binary (bin/ layout from install.sh) then path-ish spots
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()));
    let mut cands: Vec<PathBuf> = Vec::new();
    if let Some(d) = &exe_dir {
        cands.push(d.join("makerom"));
        cands.push(d.join("../bin/makerom"));
    }
    cands.push(home().join(".local/bin/makerom"));
    cands.push(PathBuf::from("/usr/bin/makerom"));
    cands.push(PathBuf::from("/usr/local/bin/makerom"));
    if let Some(p) = first_existing(&cands) {
        return Some(p);
    }
    if let Ok(pathenv) = std::env::var("PATH") {
        for dir in pathenv.split(':') {
            let p = PathBuf::from(dir).join("makerom");
            if is_executable(&p) {
                return Some(p);
            }
        }
    }
    None
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .ok()
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// pretty path with $HOME collapsed to ~ (accepts &Path, PathBuf, &str …)
pub fn tilde(p: impl AsRef<Path>) -> String {
    tilde_str(p.as_ref().to_string_lossy().as_ref())
}

pub fn tilde_str(s: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    if !home.is_empty() && s == home {
        "~".to_string()
    } else if !home.is_empty() {
        if let Some(rest) = s.strip_prefix(&format!("{}/", home)) {
            return format!("~/{}", rest);
        }
        s.to_string()
    } else {
        s.to_string()
    }
}

/// short label for log prefixes: stem of the name, capped
pub fn guesstimate(name: &str) -> String {
    let mut out = name.to_string();
    for ext in [".3ds", ".cci", ".cia", ".3dz"] {
        if let Some(stripped) = out.strip_suffix(ext) {
            out = stripped.to_string();
        }
    }
    if out.chars().count() > 16 {
        let mut s: String = out.chars().take(16).collect();
        s.push('…');
        return s;
    }
    out
}

/// human readable size
pub fn human(bytes: u64) -> String {
    const K: f64 = 1024.0;
    let b = bytes as f64;
    if b >= K * K * K {
        format!("{:.1} GiB", b / (K * K * K))
    } else if b >= K * K {
        format!("{:.0} MiB", b / (K * K))
    } else if b >= K {
        format!("{:.0} KiB", b / K)
    } else {
        format!("{} B", bytes)
    }
}

/// list rom files in a directory (.cia/.3ds/.cci/.3dz), skipping our own outputs
pub fn scan_dir(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut out: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| {
            if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                matches!(ext.to_ascii_lowercase().as_str(), "3ds" | "cci" | "cia" | "3dz")
            } else {
                false
            }
        })
        .filter(|p| {
            !p.file_name()
                .map(|n| {
                    let n = n.to_string_lossy().to_lowercase();
                    n.contains("decrypted") || n.contains("-decrypted")
                })
                .unwrap_or(false)
        })
        .collect();
    out.sort();
    out
}
