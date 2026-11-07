//! rendering + key handling for the interactive tui.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Row, Table};
use ratatui::Frame;

use crate::app::{modal::Modal, Event, FileEntry, Settings, Status};
use crate::tools;

const ACCENT: Color = Color::Rgb(0x5c, 0xb9, 0xae);
const ACCENT_DIM: Color = Color::Rgb(0x3f, 0x7e, 0x76);
const LIGHT: Color = Color::Rgb(0xc0, 0xca, 0xf2);
const MUTED: Color = Color::Rgb(0x7d, 0x85, 0xb0);
const GREEN: Color = Color::Rgb(0x9e, 0xce, 0x6a);
const YELLOW: Color = Color::Rgb(0xe0, 0xaf, 0x68);
const RED: Color = Color::Rgb(0xf7, 0x76, 0x77);

const SPIN: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

const MENU: [&str; 7] = [
    "run all pending files",
    "rescan directory",
    "settings",
    "seeddb.bin downloads",
    "help · keys",
    "about",
    "quit",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputTarget {
    Dir,
    SeeddbPath,
    MakeromPath,
}

pub struct TuiApp {
    pub event_tx: mpsc::Sender<Event>,
    event_rx: mpsc::Receiver<Event>,
    pub files: Vec<FileEntry>,
    pub dir: PathBuf,
    pub settings: Settings,
    pub seeddb_found: Option<String>,
    pub makerom_found: Option<String>,
    pub cursor: usize,
    pub log_lines: Vec<String>,
    pub log_focus: bool,
    pub log_follow: bool,
    pub log_scroll: usize,
    pub log_huge: bool,
    pub modal: Modal,
    pub menu_cursor: usize,
    pub input: String,
    pub input_editing: Option<InputTarget>,
    pub busy: bool,
    pub tick: u32,
    cancel: Arc<AtomicBool>,
    seed_flag: Arc<AtomicBool>,
    seed_bytes: (u64, u64),
}

impl TuiApp {
    pub fn new(
        dir: PathBuf,
        settings: Settings,
        event_tx: mpsc::Sender<Event>,
        event_rx: mpsc::Receiver<Event>,
    ) -> Self {
        let seeddb_found = tools::resolve_seeddb(&settings.seeddb).map(|p| tools::tilde(&p));
        let makerom_found = tools::resolve_makerom(&settings.makerom).map(|p| tools::tilde(&p));
        let mut app = TuiApp {
            event_tx,
            event_rx,
            files: vec![],
            dir: dir.clone(),
            settings,
            seeddb_found,
            makerom_found,
            cursor: 0,
            log_lines: vec![],
            log_focus: false,
            log_follow: true,
            log_scroll: 0,
            log_huge: false,
            modal: Modal::None,
            menu_cursor: 0,
            input: String::new(),
            input_editing: None,
            busy: false,
            tick: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            seed_flag: Arc::new(AtomicBool::new(false)),
            seed_bytes: (0, 0),
        };
        app.scan(dir);
        app
    }

    pub fn scan(&mut self, dir: PathBuf) {
        let files = tools::scan_dir(&dir);
        self.cursor = 0;
        self.files = files.into_iter().map(FileEntry::new).collect();
        self.dir = dir.clone();
        self.push_log(format!(
            "scanned {} · {} rom file(s) — nothing processed yet",
            tools::tilde_str(dir.to_string_lossy().as_ref()),
            self.files.len()
        ));
    }

    pub fn push_log(&mut self, s: impl Into<String>) {
        self.log_lines.push(s.into());
        if self.log_lines.len() > 6000 {
            self.log_lines.drain(0..2000);
        }
    }

    /// fold worker/net events into visible state; also drains the global ring log
    pub fn drain_events(&mut self) {
        let mut ring: Vec<String> = vec![];
        crate::app::RingLog::drain(crate::jobs::ring(), &mut ring);
        for l in ring {
            if !l.trim().is_empty() {
                self.push_log(l);
            }
        }
        loop {
            match self.event_rx.try_recv() {
                Ok(ev) => self.on_event(ev),
                Err(_) => break,
            }
        }
    }

    fn on_event(&mut self, ev: Event) {
        match ev {
            Event::Probed { idx, probe } => {
                if let Some(f) = self.files.get_mut(idx) {
                    f.probe = Some(probe);
                }
            }
            Event::Status { idx, status, msg } => {
                let name = self.files.get(idx).map(|f| f.name.clone()).unwrap_or_default();
                if let Some(f) = self.files.get_mut(idx) {
                    f.status = status;
                    f.message = msg.clone();
                }
                if let Some(m) = msg {
                    self.push_log(format!("{}: {}", name, m));
                }
            }
            Event::Progress { idx, done, total } => {
                if let Some(f) = self.files.get_mut(idx) {
                    f.progress = (done, total);
                }
            }
            Event::FileLog { idx, line } => {
                let name = self.files.get(idx).map(|f| f.name.clone()).unwrap_or_default();
                self.push_log(format!("[{}] {}", tools::guesstimate(&name), line));
            }
            Event::Decrypted { idx, outputs } => {
                let name = self.files.get(idx).map(|f| f.name.clone()).unwrap_or_default();
                if let Some(f) = self.files.get_mut(idx) {
                    f.outputs = outputs.clone();
                }
                self.push_log(format!("{}: dumped {} ncch part(s)", name, outputs.len()));
            }
            Event::Finished { idx, ok, repacked, msg } => {
                let name = self.files.get(idx).map(|f| f.name.clone()).unwrap_or_default();
                let size = self.files.get(idx).map(|f| f.size.max(1)).unwrap_or(1);
                if let Some(f) = self.files.get_mut(idx) {
                    if ok {
                        if f.status != Status::AlreadyDecrypted {
                            f.status = Status::Done;
                        }
                        f.progress = (size, size);
                    } else {
                        f.status = Status::Failed;
                    }
                    f.repacked = repacked.clone();
                }
                if let Some(r) = &repacked {
                    self.push_log(format!("wrote {}", tools::tilde(r)));
                }
                if let Some(m) = msg {
                    self.push_log(format!("{}: {}", name, m));
                }
            }
            Event::Idle => self.busy = false,
            Event::SeedProgress { done, total } => self.seed_bytes = (done, total),
            Event::SeedSaved { path, bytes } => {
                self.seed_flag.store(false, Ordering::Relaxed);
                self.seed_bytes = (0, 0);
                self.seeddb_found = Some(tools::tilde(&path));
                self.push_log(format!(
                    "seeddb.bin saved · {} bytes → {}",
                    bytes,
                    tools::tilde(path.to_string_lossy().as_ref())
                ));
            }
            Event::SeedFailed { err } => {
                self.seed_flag.store(false, Ordering::Relaxed);
                self.seed_bytes = (0, 0);
                self.push_log(format!("seeddb download failed: {}", err));
            }
        }
    }

    pub fn pending_indices(&self) -> Vec<usize> {
        self.files
            .iter()
            .enumerate()
            .filter(|(_, f)| matches!(f.status, Status::Pending | Status::Failed | Status::NotSupported))
            .map(|(i, _)| i)
            .collect()
    }

    fn current_runnable(&self) -> bool {
        self.files
            .get(self.cursor)
            .map(|f| matches!(f.status, Status::Pending | Status::Failed | Status::NotSupported))
            .unwrap_or(false)
    }

    pub fn dispatch(&mut self, indices: Vec<usize>) {
        if self.busy {
            self.push_log("worker already busy — one batch at a time");
            return;
        }
        let jobs: Vec<(usize, PathBuf)> = indices
            .into_iter()
            .filter(|&i| i < self.files.len())
            .map(|i| (i, self.files[i].path.clone()))
            .collect();
        if jobs.is_empty() {
            self.push_log("nothing to run");
            return;
        }
        let indices: Vec<usize> = jobs.iter().map(|(i, _)| *i).collect();
        for i in indices {
            if let Some(f) = self.files.get_mut(i) {
                f.status = Status::Queued;
                f.progress = (0, 0);
            }
        }
        self.busy = true;
        let tx = self.event_tx.clone();
        let snap = crate::app::WorkerSnapshot { settings: self.settings.clone() };
        let cancel = self.cancel.clone();
        std::thread::spawn(move || {
            for (idx, path) in jobs {
                let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    crate::jobs::process_file(idx, path, snap.clone(), tx.clone(), &cancel)
                }));
                if let Err(p) = res {
                    let msg = format!("internal error: {}", panic_text(p.as_ref()));
                    let _ = tx.send(Event::Finished { idx, ok: false, repacked: None, msg: Some(msg) });
                }
            }
            let _ = tx.send(Event::Idle);
        });
    }

    pub fn start_seed_download(&mut self) {
        if self.seed_flag.load(Ordering::Relaxed) {
            self.push_log("seeddb download already in progress");
            return;
        }
        self.seed_flag.store(true, Ordering::Relaxed);
        self.push_log(format!(
            "downloading seeddb.bin → {}",
            tools::tilde(tools::seeddb_install_dir().to_string_lossy().as_ref())
        ));
        crate::net::download_seeddb(self.event_tx.clone(), self.seed_flag.clone());
    }

    pub fn seed_working(&self) -> bool {
        self.seed_flag.load(Ordering::Relaxed)
    }

    pub fn seed_progress(&self) -> (u64, u64) {
        self.seed_bytes
    }

    pub fn spinner(&self) -> char {
        SPIN[(self.tick / 3) as usize % SPIN.len()]
    }

    pub fn needs_confirm(&self) -> bool {
        self.busy || self.seed_working()
    }
}

fn panic_text(p: &dyn std::any::Any) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "(unknown)".to_string()
    }
}

fn expand_tilde(s: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    if s == "~" {
        home
    } else if let Some(rest) = s.strip_prefix("~/") {
        format!("{}/{}", home, rest)
    } else {
        s.to_string()
    }
}

fn open_input(app: &mut TuiApp, t: InputTarget) {
    app.input = match t {
        InputTarget::Dir => app.dir.to_string_lossy().to_string(),
        InputTarget::SeeddbPath => app
            .settings
            .seeddb
            .clone()
            .map(|p| p.to_string_lossy().to_string())
            .or_else(|| app.seeddb_found.clone().map(|s| expand_tilde(&s)))
            .unwrap_or_default(),
        InputTarget::MakeromPath => app
            .settings
            .makerom
            .clone()
            .map(|p| p.to_string_lossy().to_string())
            .or_else(|| app.makerom_found.clone().map(|s| expand_tilde(&s)))
            .unwrap_or_default(),
    };
    app.input_editing = Some(t);
}

// --------------------------------------------------------------------------- //
// input                       (Ok(true) = keep running, Ok(false) = quit)
// --------------------------------------------------------------------------- //

pub fn handle_key(app: &mut TuiApp, key: KeyEvent) -> anyhow::Result<bool> {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        if app.needs_confirm() {
            app.modal = Modal::ConfirmQuit;
            return Ok(true);
        }
        return Ok(false);
    }

    if let Some(target) = app.input_editing {
        return Ok(input_key(app, key, target));
    }

    match app.modal {
        Modal::None => plain_key(app, key),
        Modal::MainMenu => menu_key(app, key),
        Modal::Settings { field } => Ok(settings_key(app, key, field)),
        Modal::Downloads => Ok(downloads_key(app, key)),
        Modal::Help | Modal::About => {
            match key.code {
                KeyCode::Esc
                | KeyCode::Enter
                | KeyCode::Char('q')
                | KeyCode::Char('h')
                | KeyCode::Char('m')
                | KeyCode::Char('?')
                | KeyCode::Char('a') => app.modal = Modal::None,
                _ => {}
            }
            Ok(true)
        }
        Modal::ConfirmQuit => match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => Ok(false),
            _ => {
                app.modal = Modal::None;
                Ok(true)
            }
        },
        Modal::DirInput => Ok(true),
    }
}

fn input_key(app: &mut TuiApp, key: KeyEvent, target: InputTarget) -> bool {
    match key.code {
        KeyCode::Esc => {
            app.input_editing = None;
        }
        KeyCode::Enter => {
            let val = expand_tilde(app.input.trim());
            let p = PathBuf::from(&val);
            match target {
                InputTarget::Dir => {
                    if p.is_dir() {
                        app.scan(p);
                        app.input_editing = None;
                        app.modal = Modal::None;
                    } else {
                        app.push_log(format!("not a directory: {}", val));
                    }
                }
                InputTarget::SeeddbPath => {
                    if p.is_file() {
                        app.settings.seeddb = Some(p.clone());
                        app.seeddb_found = Some(tools::tilde(&p));
                        app.push_log(format!("seeddb.bin set → {}", tools::tilde(&p)));
                        app.input_editing = None;
                    } else {
                        app.push_log(format!("file not found: {}", val));
                    }
                }
                InputTarget::MakeromPath => {
                    if p.is_file() {
                        app.settings.makerom = Some(p.clone());
                        app.makerom_found = Some(tools::tilde(&p));
                        app.push_log(format!("makerom set → {}", tools::tilde(&p)));
                        app.input_editing = None;
                    } else {
                        app.push_log(format!("file not found: {}", val));
                    }
                }
            }
        }
        KeyCode::Backspace => {
            app.input.pop();
        }
        KeyCode::Char(c) => app.input.push(c),
        _ => {}
    }
    true
}

fn plain_key(app: &mut TuiApp, key: KeyEvent) -> anyhow::Result<bool> {
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => {
            if app.needs_confirm() {
                app.modal = Modal::ConfirmQuit;
                Ok(true)
            } else {
                Ok(false)
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if app.log_focus {
                app.log_follow = false;
                app.log_scroll = (app.log_scroll + 1).min(app.log_lines.len().saturating_sub(1));
            } else if app.cursor > 0 {
                app.cursor -= 1;
            }
            Ok(true)
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.log_focus {
                if app.log_scroll > 0 {
                    app.log_scroll -= 1;
                } else {
                    app.log_follow = true;
                }
            } else if app.cursor + 1 < app.files.len() {
                app.cursor += 1;
            }
            Ok(true)
        }
        KeyCode::Char(' ') => {
            if let Some(f) = app.files.get_mut(app.cursor) {
                f.selected = !f.selected;
            }
            Ok(true)
        }
        KeyCode::Enter => {
            if app.current_runnable() {
                app.dispatch(vec![app.cursor]);
            } else {
                app.push_log("nothing to do for that file (done, or already decrypted)");
            }
            Ok(true)
        }
        KeyCode::Char('a') => {
            let list = app.pending_indices();
            if list.is_empty() {
                app.push_log("nothing pending");
            } else {
                app.dispatch(list);
            }
            Ok(true)
        }
        KeyCode::Char('s') => {
            app.modal = Modal::Settings { field: 0 };
            Ok(true)
        }
        KeyCode::Char('d') => {
            app.modal = Modal::Downloads;
            Ok(true)
        }
        KeyCode::Char('h') | KeyCode::Char('?') => {
            app.modal = Modal::Help;
            Ok(true)
        }
        KeyCode::Char('m') => {
            app.modal = Modal::MainMenu;
            Ok(true)
        }
        KeyCode::Char('r') => {
            let d = app.dir.clone();
            app.scan(d);
            Ok(true)
        }
        KeyCode::Tab => {
            app.log_focus = !app.log_focus;
            app.log_follow = true;
            Ok(true)
        }
        KeyCode::PageUp => {
            app.log_follow = false;
            app.log_scroll = (app.log_scroll + 10).min(app.log_lines.len().saturating_sub(1));
            Ok(true)
        }
        KeyCode::PageDown => {
            if app.log_scroll >= 10 {
                app.log_scroll -= 10;
            } else {
                app.log_scroll = 0;
                app.log_follow = true;
            }
            Ok(true)
        }
        KeyCode::Char('L') => {
            app.log_huge = !app.log_huge;
            Ok(true)
        }
        _ => Ok(true),
    }
}

fn menu_key(app: &mut TuiApp, key: KeyEvent) -> anyhow::Result<bool> {
    match key.code {
        KeyCode::Esc | KeyCode::Char('m') | KeyCode::Char('q') => app.modal = Modal::None,
        KeyCode::Up | KeyCode::Char('k') => {
            if app.menu_cursor > 0 {
                app.menu_cursor -= 1;
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.menu_cursor + 1 < MENU.len() {
                app.menu_cursor += 1;
            }
        }
        KeyCode::Enter | KeyCode::Right => return menu_exec(app, app.menu_cursor),
        KeyCode::Char(c) if '1' <= c && c <= '7' => {
            return menu_exec(app, (c as u8 - b'1') as usize);
        }
        _ => {}
    }
    Ok(true)
}

fn menu_exec(app: &mut TuiApp, idx: usize) -> anyhow::Result<bool> {
    match idx {
        0 => {
            let list = app.pending_indices();
            if list.is_empty() {
                app.push_log("nothing pending");
            } else {
                app.dispatch(list);
            }
            app.modal = Modal::None;
        }
        1 => {
            let d = app.dir.clone();
            app.scan(d);
            app.modal = Modal::None;
        }
        2 => app.modal = Modal::Settings { field: 0 },
        3 => app.modal = Modal::Downloads,
        4 => app.modal = Modal::Help,
        5 => app.modal = Modal::About,
        _ => {
            app.modal = Modal::None;
            return Ok(!menu_quit(app));
        }
    }
    Ok(true)
}

/// true = quit now
fn menu_quit(app: &mut TuiApp) -> bool {
    if app.needs_confirm() {
        app.modal = Modal::ConfirmQuit;
        false
    } else {
        true
    }
}

fn settings_key(app: &mut TuiApp, key: KeyEvent, field: usize) -> bool {
    const LAST: usize = 5;
    match key.code {
        KeyCode::Esc | KeyCode::Char('s') | KeyCode::Char('m') | KeyCode::Char('q') => {
            app.modal = Modal::None;
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.modal = Modal::Settings { field: field.saturating_sub(1) };
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if field + 1 <= LAST {
                app.modal = Modal::Settings { field: field + 1 };
            }
        }
        KeyCode::Enter | KeyCode::Right | KeyCode::Left => match field {
            0 => open_input(app, InputTarget::Dir),
            1 => open_input(app, InputTarget::SeeddbPath),
            2 => open_input(app, InputTarget::MakeromPath),
            3 => app.settings.allow_online = !app.settings.allow_online,
            4 => app.settings.keep_intermediates = !app.settings.keep_intermediates,
            _ => app.modal = Modal::None,
        },
        _ => {}
    }
    true
}

fn downloads_key(app: &mut TuiApp, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Esc | KeyCode::Char('d') | KeyCode::Char('m') | KeyCode::Char('q') => {
            app.modal = Modal::None;
        }
        KeyCode::Enter => app.start_seed_download(),
        KeyCode::Char('s') => open_input(app, InputTarget::SeeddbPath),
        _ => {}
    }
    true
}

// --------------------------------------------------------------------------- //
// drawing
// --------------------------------------------------------------------------- //

pub fn draw(app: &mut TuiApp, f: &mut Frame) {
    let outer = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(4),
        Constraint::Length(3),
    ])
    .split(f.area());
    let mid = Layout::vertical([
        Constraint::Percentage(if app.log_huge { 32 } else { 62 }),
        Constraint::Percentage(if app.log_huge { 68 } else { 38 }),
    ])
    .split(outer[1]);

    draw_header(app, f, outer[0]);
    draw_files(app, f, mid[0]);
    draw_log(app, f, mid[1]);
    draw_footer(app, f, outer[2]);
    draw_modal(app, f);
}

fn busy_color(tick: u32) -> Color {
    if (tick / 8) % 2 == 0 {
        ACCENT
    } else {
        Color::Rgb(0x7a, 0xdc, 0xff)
    }
}

fn bar(frac: f64, width: usize) -> String {
    let fill = (frac.clamp(0.0, 1.0) * width as f64).round() as usize;
    let mut s = "▰".repeat(fill);
    s.push_str(&"▱".repeat(width - fill));
    s
}

fn chip(st: Status, tick: u32) -> (&'static str, Style) {
    match st {
        Status::Done => ("done ✓", Style::new().fg(GREEN)),
        Status::AlreadyDecrypted => ("no work", Style::new().fg(ACCENT)),
        Status::Failed => ("failed ✗", Style::new().fg(RED)),
        Status::NotSupported => ("n/a", Style::new().fg(MUTED)),
        Status::Pending => ("pending", Style::new().fg(MUTED)),
        Status::Queued => ("queued", Style::new().fg(YELLOW)),
        Status::Probing => ("probing", Style::new().fg(YELLOW)),
        Status::Decrypting => ("decrypting", Style::new().fg(busy_color(tick))),
        Status::Repacking => ("repacking", Style::new().fg(busy_color(tick + 4))),
    }
}

fn draw_header(app: &TuiApp, f: &mut Frame, area: Rect) {
    let spinner = if app.busy {
        Span::styled(format!("● working {} ", app.spinner()), Style::new().fg(busy_color(app.tick)).add_modifier(Modifier::BOLD))
    } else {
        Span::styled("● idle", Style::new().fg(MUTED))
    };
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(if app.busy { busy_color(app.tick) } else { ACCENT_DIM }))
        .title(Line::from(vec![
            Span::styled(" decryptor-tui ", Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
            spinner,
        ]));
    f.render_widget(block, area);

    let inner = area.inner(Margin { horizontal: 2, vertical: 1 });
    let pend = app.pending_indices().len();
    let line = Line::from(vec![
        Span::styled("batch cia/cci decryptor", Style::new().fg(MUTED)),
        Span::raw("   "),
        Span::styled(
            format!("{} file(s) · {} to do", app.files.len(), pend),
            Style::new().fg(LIGHT),
        ),
        Span::raw(if app.log_focus { "   (log focused)" } else { "" }),
    ]);
    f.render_widget(Paragraph::new(line), inner);
}

fn draw_files(app: &mut TuiApp, f: &mut Frame, area: Rect) {
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(if app.busy { busy_color(app.tick) } else { ACCENT_DIM }))
        .title(Span::styled(
            format!(" {} · {} files ", tools::tilde_str(app.dir.to_string_lossy().as_ref()), app.files.len()),
            Style::new().fg(ACCENT),
        ));
    f.render_widget(block, area);
    let inner = area.inner(Margin { horizontal: 1, vertical: 1 });

    if app.files.is_empty() {
        let hint = Paragraph::new(Line::from(Span::styled(
            "no .cia/.3ds/.cci/.3dz files here — press [m] → change directory",
            Style::new().fg(MUTED),
        )));
        f.render_widget(hint, inner);
        return;
    }

    let widths = [
        Constraint::Length(3),
        Constraint::Min(22),
        Constraint::Length(17),
        Constraint::Length(7),
        Constraint::Length(6),
        Constraint::Length(8),
        Constraint::Length(12),
        Constraint::Length(11),
    ];

    let rows_visible = inner.height.saturating_sub(1) as usize;
    let start = app.cursor.saturating_sub(rows_visible.saturating_sub(1));

    let mut rows: Vec<Row> = vec![Row::new(vec![
        Span::styled("   ", Style::new().fg(MUTED)),
        Span::styled("file", Style::new().fg(MUTED)),
        Span::styled("title id", Style::new().fg(MUTED)),
        Span::styled("kind", Style::new().fg(MUTED)),
        Span::styled("ver", Style::new().fg(MUTED)),
        Span::styled("size", Style::new().fg(MUTED)),
        Span::styled("", Style::new().fg(MUTED)),
        Span::styled("status", Style::new().fg(MUTED)),
    ])];

    for (i, fe) in app.files.iter().enumerate().skip(start).take(rows_visible) {
        let cur = i == app.cursor && !app.log_focus;
        let name_style = if cur {
            Style::new().fg(LIGHT).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(LIGHT)
        };
        let meta_style = Style::new().fg(MUTED);
        let (ch, ch_st) = chip(fe.status, app.tick);

        let (pd, pt) = fe.progress;
        let prog = if pt > 0 {
            let frac = pd as f64 / pt as f64;
            format!("{} {:>3}%", bar(frac, 6), (frac * 100.0) as u8)
        } else {
            String::new()
        };

        let p = fe.probe.as_ref();        let tid = p.and_then(|p| p.title_id.clone()).unwrap_or_default();
        let kind = p
            .and_then(|p| p.title_kind)
            .map(|k| k.label().to_string())
            .unwrap_or_default();
        let ver = p
            .and_then(|p| p.title_version)
            .map(|v| format!("{}.{}", v.0, v.1))
            .unwrap_or_default();

        rows.push(Row::new(vec![
            Span::styled(if cur { "▶" } else { " " }, Style::new().fg(ACCENT)),
            Span::styled(trunc(&fe.name, 30), name_style),
            Span::styled(trunc(&tid, 16), meta_style),
            Span::styled(trunc(&kind, 7), Style::new().fg(YELLOW)),
            Span::styled(ver, Style::new().fg(ACCENT)),
            Span::styled(tools::human(fe.size), meta_style),
            Span::styled(prog, Style::new().fg(ACCENT)),
            Span::styled(ch, ch_st),
        ]));
    }

    f.render_widget(Table::new(rows, widths).column_spacing(1), inner);
}

fn draw_log(app: &TuiApp, f: &mut Frame, area: Rect) {
    let title = if app.log_follow {
        " log ".to_string()
    } else {
        format!(" log · scrolled {} ", app.log_scroll)
    };
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(if app.log_focus { ACCENT } else { ACCENT_DIM }))
        .title(Span::styled(title, Style::new().fg(if app.log_focus { ACCENT } else { MUTED })));
    f.render_widget(block, area);
    let inner = area.inner(Margin { horizontal: 1, vertical: 1 });

    let h = inner.height as usize;
    let w = inner.width as usize;
    let total = app.log_lines.len();
    let end = total.saturating_sub(app.log_scroll);
    let start = end.saturating_sub(h);
    let lines: Vec<Line> = app.log_lines[start..end]
        .iter()
        .map(|l| Line::styled(trunc(l, w), Style::new().fg(LIGHT)))
        .collect();
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

fn draw_footer(app: &TuiApp, f: &mut Frame, area: Rect) {
    let (seed_txt, seed_st) = match &app.seeddb_found {
        Some(p) => (format!("seeddb ✓ {}", p), Style::new().fg(GREEN)),
        None => ("seeddb ✗ not found — [d] downloads".to_string(), Style::new().fg(RED)),
    };
    let (mk_txt, mk_st) = match &app.makerom_found {
        Some(p) => (format!("makerom ✓ {}", p), Style::new().fg(GREEN)),
        None => ("makerom ✗ missing — see install.sh".to_string(), Style::new().fg(RED)),
    };
    let online = Span::styled(
        format!("online {}", if app.settings.allow_online { "on" } else { "off" }),
        Style::new().fg(MUTED),
    );
    let l1 = Line::from(vec![
        Span::styled(seed_txt, seed_st),
        Span::raw("   "),
        Span::styled(mk_txt, mk_st),
        Span::raw("   "),
        online,
    ]);
    let l2 = Line::from(Span::styled(
        "[enter] run file · [a] run all · [space] select · [m] menu · [s] settings · [d] seeddb · [r] rescan · [tab] log · [q] quit",
        Style::new().fg(MUTED),
    ));
    f.render_widget(Paragraph::new(Text::from(vec![l1, l2])), area);
}

fn popup(f: &mut Frame, area: Rect, title: &str, accent: bool) -> Rect {
    f.render_widget(Clear, area);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(if accent { busy_color(0) } else { ACCENT_DIM }))
        .title(Span::styled(
            format!(" {} ", title),
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    f.render_widget(block, area);
    area.inner(Margin { horizontal: 1, vertical: 1 })
}

fn centered(total: Rect, w: u16, h: u16) -> Rect {
    let x = total.x + (total.width.saturating_sub(w)) / 2;
    let y = total.y + (total.height.saturating_sub(h)) / 2;
    Rect { x, y, width: w.min(total.width), height: h.min(total.height) }
}

fn draw_modal(app: &mut TuiApp, f: &mut Frame) {
    match app.modal {
        Modal::MainMenu => draw_menu(app, f),
        Modal::Settings { .. } => draw_settings(app, f),
        Modal::Downloads => draw_downloads(app, f),
        Modal::Help => draw_help(app, f),
        Modal::About => draw_about(app, f),
        Modal::ConfirmQuit => draw_confirm(app, f),
        Modal::None | Modal::DirInput => {}
    }
    if app.input_editing.is_some() {
        draw_input(app, f);
    }
}

fn draw_menu(app: &mut TuiApp, f: &mut Frame) {
    let area = centered(f.area(), 44, (MENU.len() as u16) + 5);
    let inner = popup(f, area, "menu", true);
    let mut lines: Vec<Line> = MENU
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let cur = if i == app.menu_cursor { "▶ " } else { "  " };
            let st = if i == app.menu_cursor {
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(LIGHT)
            };
            Line::from(Span::styled(format!("{} {}", cur, m), st))
        })
        .collect();
    lines.push(Line::from(Span::styled(
        "↑↓ move · enter select · number keys jump · esc closes",
        Style::new().fg(MUTED),
    )));
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

fn draw_settings(app: &mut TuiApp, f: &mut Frame) {
    let Modal::Settings { field } = app.modal else { return };
    let area = centered(f.area(), 62, 10);
    let inner = popup(f, area, "settings", true);

    let seed_val = app
        .seeddb_found
        .clone()
        .unwrap_or_else(|| "not found".to_string());
    let mk_val = app
        .makerom_found
        .clone()
        .unwrap_or_else(|| "not found".to_string());
    let rows: Vec<(String, String)> = vec![
        ("directory".into(), tools::tilde_str(app.dir.to_string_lossy().as_ref())),
        ("seeddb.bin".into(), seed_val),
        ("makerom".into(), mk_val),
        (
            "online seeds".into(),
            if app.settings.allow_online { "on".into() } else { "off".into() },
        ),
        (
            "keep intermediates".into(),
            if app.settings.keep_intermediates { "on".into() } else { "off".into() },
        ),
        ("close".into(), "".into()),
    ];

    let mut lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .map(|(i, (label, val))| {
            let cur = if i == field { "▶ " } else { "  " };
            let st = if i == field {
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(LIGHT)
            };
            Line::from(Span::styled(
                format!("{}{:<20} {}", cur, label, trunc(val, 36)),
                st,
            ))
        })
        .collect();
    lines.push(Line::from(Span::styled(
        "↑↓ · enter changes/opens · esc closes",
        Style::new().fg(MUTED),
    )));
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

fn draw_downloads(app: &mut TuiApp, f: &mut Frame) {
    let rows: Vec<Line> = vec![
        Line::from(Span::styled(
            "download seeddb.bin (seed-encrypted titles need it to decrypt)",
            Style::new().fg(LIGHT),
        )),
        Line::from(Span::styled(
            format!("install target: {}", tools::tilde_str(&tools::seeddb_install_dir().to_string_lossy())),
            Style::new().fg(MUTED),
        )),
        if app.seed_working() {
            let (d, t) = app.seed_progress();
            Line::from(Span::styled(
                format!("downloading… {} {}/{} B", app.spinner(), d, t.max(1)),
                Style::new().fg(ACCENT),
            ))
        } else {
            Line::from(Span::styled("[enter] download now   [s] use my own path", Style::new().fg(ACCENT)))
        },
        Line::from(Span::styled("esc back", Style::new().fg(MUTED))),
    ];
    let area = centered(f.area(), 56, 7);
    let inner = popup(f, area, "seeddb downloads", true);
    f.render_widget(Paragraph::new(Text::from(rows)), inner);
    if app.seed_working() {
        let (d, t) = app.seed_progress();
        if t > 0 {
            let w = (inner.width as f64 * 0.8) as usize;
            let pbar = Line::from(Span::styled(bar(d as f64 / t as f64, w), Style::new().fg(ACCENT)));
            let r = Rect { x: inner.x, y: inner.y + inner.height.saturating_sub(1), width: inner.width, height: 1 };
            f.render_widget(Paragraph::new(pbar), r);
        }
    }
}

fn draw_help(_app: &mut TuiApp, f: &mut Frame) {
    let pairs: Vec<(&str, &str)> = vec![
        ("enter", "decrypt/repack the file under the cursor"),
        ("a", "run every pending file (batch)"),
        ("space", "select / deselect the file under the cursor"),
        ("↑↓ / j k", "move cursor (or scroll the log with [tab])"),
        ("m", "main menu"),
        ("s", "settings (dir, seeddb, makerom, toggles)"),
        ("d", "seeddb downloads"),
        ("r", "rescan the directory"),
        ("tab", "focus the log pane"),
        ("pgup/pgdn", "scroll log · [L] big log view"),
        ("q", "quit"),
    ];
    let mut lines: Vec<Line> = Vec::new();
    for (k, v) in pairs {
        lines.push(Line::from(vec![
            Span::styled(format!("{:<12} ", k), Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(v.to_string(), Style::new().fg(LIGHT)),
        ]));
    }
    lines.push(Line::from(Span::styled("esc closes", Style::new().fg(MUTED))));
    let area = centered(f.area(), 62, 15);
    let inner = popup(f, area, "help · keys", true);
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

fn draw_about(_app: &mut TuiApp, f: &mut Frame) {
    let lines: Vec<Line> = vec![
        Line::from(Span::styled(
            format!("decryptor-tui {}", crate::VERSION),
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "batch CIA/CCI 3DS decryptor · linux native",
            Style::new().fg(LIGHT),
        )),
        Line::from(Span::styled(" ", Style::new())),
        Line::from(Span::styled(
            "fork of rcyggdra/decryptor — TUI & linux packaging by visadaking,",
            Style::new().fg(MUTED),
        )),
        Line::from(Span::styled(
            "in the spirit of ihaveacreditcard. repacks via Project_CTR makerom,",
            Style::new().fg(MUTED),
        )),
        Line::from(Span::styled(
            "seeddb from ihaveamac/3DS-rom-tools.",
            Style::new().fg(MUTED),
        )),
        Line::from(Span::styled("esc closes", Style::new().fg(MUTED))),
    ];
    let area = centered(f.area(), 62, 10);
    let inner = popup(f, area, "about", true);
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

fn draw_confirm(_app: &mut TuiApp, f: &mut Frame) {
    let lines = vec![
        Line::from(Span::styled(
            "jobs are still running.",
            Style::new().fg(YELLOW),
        )),
        Line::from(Span::styled(
            "quitting now loses progress; partial dumps may remain.",
            Style::new().fg(MUTED),
        )),
        Line::from(Span::styled(
            "[y] force quit     [esc] keep going",
            Style::new().fg(LIGHT).add_modifier(Modifier::BOLD),
        )),
    ];
    let area = centered(f.area(), 52, 6);
    let inner = popup(f, area, "quit?", false);
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

fn draw_input(app: &mut TuiApp, f: &mut Frame) {
    let (title, _) = match app.input_editing {
        Some(InputTarget::Dir) => ("directory path", 0),
        Some(InputTarget::SeeddbPath) => ("seeddb.bin path", 0),
        Some(InputTarget::MakeromPath) => ("makerom path", 0),
        None => return,
    };
    let area = centered(f.area(), 56, 3);
    let inner = popup(f, area, title, true);
    let caret = if (app.tick / 8) % 2 == 0 { "▌" } else { " " };
    let line = Line::from(vec![
        Span::styled(trunc(&app.input, 46), Style::new().fg(LIGHT)),
        Span::styled(caret, Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
    ]);
    f.render_widget(Paragraph::new(line), inner);
}

fn trunc(s: &str, w: usize) -> String {
    if s.chars().count() <= w {
        s.to_string()
    } else {
        let t: String = s.chars().take(w.saturating_sub(1)).collect();
        format!("{}…", t)
    }
}
