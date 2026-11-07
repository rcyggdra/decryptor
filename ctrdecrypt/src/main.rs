//! ctrdecrypt CLI — a thin wrapper around the `ctrdecrypt` library.
//!
//! All the actual work lives in lib.rs so the TUI can drive it in-process.

use std::io::Write;
use std::path::Path;

use ctrdecrypt::{decrypt, DecryptOptions};

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let mut partition: Option<u8> = None;
    let mut verbose = true;

    if args.len() < 2 {
        println!("Usage: ctrdecrypt <ROMFILE> [OPTIONS]\nOptions:\n\t--ncch <partition-number>\n\t--no-verbose");
        return;
    }

    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--ncch" => {
                if i + 1 >= args.len() {
                    println!("Missing partition number");
                    return;
                }
                match args[i + 1].parse::<u8>() {
                    Ok(num) => partition = Some(num),
                    Err(_) => {
                        println!("Invalid partition number: {}", args[i + 1]);
                        return;
                    }
                }
                i += 1; // skip the number
            }
            "--no-verbose" => verbose = false,
            _ => {
                println!("Invalid argument: {}", args[i]);
                return;
            }
        }
        i += 1;
    }

    if !Path::exists(Path::new(&args[1])) {
        println!("ROM does not exist");
        return;
    }

    // seeddb.bin from the working dir, exactly like upstream; the lib falls
    // back to ~/3ds and ~/.3ds when it's missing
    if !Path::new("seeddb.bin").exists() {
        let home = std::env::var("HOME").unwrap_or_default();
        if !Path::new(&home).join("3ds/seeddb.bin").exists()
            && !Path::new(&home).join(".3ds/seeddb.bin").exists()
        {
            eprintln!("note: no seeddb.bin found (cwd, ~/3ds, ~/.3ds) — seed-encrypted titles need one or a working internet connection");
        }
    }

    env_logger::Builder::new()
        .format(|buf, record| writeln!(buf, "{}", record.args()))
        .filter(None, if verbose { log::LevelFilter::Debug } else { log::LevelFilter::Info })
        .init();

    let opts = DecryptOptions {
        verbose,
        partition,
        ..Default::default()
    };

    let path = std::path::PathBuf::from(&args[1]);
    if let Err(e) = decrypt(&path, &opts) {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}
