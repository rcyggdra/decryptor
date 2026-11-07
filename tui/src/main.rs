//! decryptor-tui — batch CIA/CCI 3DS decryptor with a tui (linux native).

mod app;
mod headless;
mod jobs;
mod net;
mod tools;
mod ui;

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use app::Settings;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub const USAGE: &str = "\
decryptor-tui — batch CIA/CCI 3DS decryptor for linux

usage: decryptor-tui [options] [DIR]

  DIR                  directory of .cia/.3ds/.cci/.3dz files (default: .)

options:
  --headless           run without the interface (script friendly)
  --seeddb PATH        seeddb.bin path (default: auto — cwd, ~/3ds, ~/.3ds …)
  --makerom PATH       makerom path (default: auto — PATH, ~/.local/bin …)
  --no-online          don't fetch missing seeds from nintendo's servers
  --keep-intermediates keep the dumped .ncch parts after repacking
  -h, --help           show this help
  --version            print version

keys (interactive): press h inside the app for the full list
";

struct Args {
    dir: PathBuf,
    headless: bool,
    seeddb: Option<PathBuf>,
    makerom: Option<PathBuf>,
    online: bool,
    keep: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        dir: PathBuf::from("."),
        headless: false,
        seeddb: None,
        makerom: None,
        online: true,
        keep: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--headless" => a.headless = true,
            "--seeddb" => {
                let v = it.next().ok_or("--seeddb needs a path\n\nUSAGE:\n".to_owned() + USAGE)?;
                a.seeddb = Some(PathBuf::from(v));
            }
            "--makerom" => {
                let v = it.next().ok_or("--makerom needs a path\n\nUSAGE:\n".to_owned() + USAGE)?;
                a.makerom = Some(PathBuf::from(v));
            }
            "--no-online" => a.online = false,
            "--keep-intermediates" => a.keep = true,
            "-h" | "--help" => {
                print!("{}", USAGE);
                std::process::exit(0);
            }
            "--version" => {
                println!("decryptor-tui {}", VERSION);
                std::process::exit(0);
            }
            s if s.starts_with('-') => {
                return Err(format!("unknown option: {}\n\n{}", s, USAGE));
            }
            _ => a.dir = PathBuf::from(arg),
        }
    }
    Ok(a)
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(2);
        }
    };

    if !args.dir.is_dir() {
        eprintln!("error: not a directory: {}", args.dir.display());
        std::process::exit(2);
    }

    let settings = Settings {
        seeddb: args.seeddb,
        makerom: args.makerom,
        allow_online: args.online,
        keep_intermediates: args.keep,
    };

    if args.headless {
        let files = tools::scan_dir(&args.dir);
        if files.is_empty() {
            println!("no .cia/.3ds/.cci/.3dz files in {}", args.dir.display());
            std::process::exit(0);
        }
        let code = headless::run(settings, &args.dir, files);
        std::process::exit(code);
    }

    jobs::install_logger();
    let (tx, rx) = mpsc::channel();
    let res = interactive(args.dir, settings, tx, rx);
    ratatui::restore();
    if let Err(e) = res {
        eprintln!("error: {}", e);
        std::process::exit(1);
    }
    std::process::exit(0);
}

fn interactive(
    dir: PathBuf,
    settings: Settings,
    event_tx: mpsc::Sender<app::Event>,
    event_rx: mpsc::Receiver<app::Event>,
) -> anyhow::Result<()> {
    use ratatui::crossterm::event::{self, Event as CEvent, KeyEventKind};

    let mut terminal = ratatui::init();
    let mut app = ui::TuiApp::new(dir, settings, event_tx, event_rx);
    loop {
        if event::poll(Duration::from_millis(30))? {
            if let CEvent::Key(k) = event::read()? {
                if k.kind == KeyEventKind::Press && !ui::handle_key(&mut app, k)? {
                    break;
                }
            }
        }
        app.drain_events();
        app.tick = app.tick.wrapping_add(1);
        terminal.draw(|f| ui::draw(&mut app, f))?;
    }
    Ok(())
}
