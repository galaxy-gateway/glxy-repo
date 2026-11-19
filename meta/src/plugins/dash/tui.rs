//! The interactive `meta dash`: seven views over one snapshot, read again in
//! the background so the keys never wait on git or GitHub. It reads, with two
//! exceptions a person asks for: attaching a Claude session, and starting work
//! on an issue (a branch, a worktree and a session).

use anyhow::{anyhow, Result};
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
    MouseButton, MouseEvent, MouseEventKind,
};
use metarepo_core::tui::{init_terminal, poll_event, restore_terminal};
use metarepo_core::RuntimeConfig;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span as Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::branches::{self, Branch, Finish, Method};
use super::host::{self, Host};
use super::start::{self, Started, Wanted};
use super::view::{self, Act, Kind, Offer, Row, Span, Tone, Ui, View};
use super::workspaces::{self, Workspace};
use super::{actions, gather, retier, sessions, Move, Options, Session, Snapshot};

/// How long a snapshot is shown before it is read again unasked.
const STALE: Duration = Duration::from_secs(120);
const SPIN: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
const KEYS: &str =
    "1-7 view · j/k move · enter open, attach or start · p prompt · o issue · x finish · b branches · ? help · q quit";

/// The most rows the detail under the list takes.
const DETAIL_ROWS: usize = 10;

/// How often the sessions alone are asked after, between whole reads.
const SESSIONS_EVERY: Duration = Duration::from_secs(30);

/// The models a session can be started on; the first is whatever Claude
/// Code is set to use. A background session takes the full id.
const MODELS: [(&str, &str); 5] = [
    ("", "Claude's default model"),
    ("claude-fable-5-2", "Fable 5.2"),
    ("claude-opus-5-5", "Opus 5.5"),
    ("claude-sonnet-5-5", "Sonnet 5.5"),
    ("claude-haiku-5-5", "Haiku 5.5"),
];

/// What the interface asks of the reader thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// Read the workspace with this config file. A quick read is git and
    /// sessions alone, to have something to show while GitHub is asked.
    Read { config: PathBuf, quick: bool },
    /// Apply tier moves: the labels, the README's Tier column, one commit.
    Retier { config: PathBuf, moves: Vec<Move> },
    /// Create a worktree for an issue and start a session on it.
    Start {
        config: PathBuf,
        issue: Wanted,
        /// The model to start it on; `None` for Claude's default.
        model: Option<String>,
    },
    /// Fetch every project from its origin.
    Fetch { config: PathBuf, keys: Vec<String> },
    /// Clone what is missing and pull every project (`meta git sync`).
    Sync { config: PathBuf },
    /// Read an issue's text from GitHub.
    Issue {
        config: PathBuf,
        project: String,
        number: u64,
        title: String,
    },
    /// List a project's remote branches by what became of them.
    Branches { config: PathBuf, project: String },
    /// Delete remote branches, each named with the commit it was seen at.
    Prune {
        config: PathBuf,
        project: String,
        picks: Vec<(String, String)>,
    },
    /// Finish a branch: open its pull request, merge it, or clear up after.
    Finish {
        config: PathBuf,
        project: String,
        branch: String,
        how: Finish,
    },
    /// List the Claude sessions again, without reading anything else.
    Sessions {
        config: PathBuf,
        keys: Vec<String>,
        slugs: HashMap<String, String>,
    },
    /// Tell the person something happened: a toast, or the bell.
    Notify { text: String },
    /// Bring a session to the front: in the pane it already has (`pane`), a
    /// new pane, or this terminal where there is no host to ask.
    Attach {
        id: String,
        name: String,
        cwd: String,
        pane: Option<u64>,
    },
    /// Paste a prompt into the pane a session is in, and submit it.
    Send {
        name: String,
        pane: u64,
        text: String,
    },
}

/// What comes back.
#[derive(Debug)]
pub enum Tell {
    Read {
        config: PathBuf,
        quick: bool,
        result: Result<Box<Snapshot>, String>,
    },
    /// A workspace found on this machine.
    Found(Workspace),
    /// The whole home folder has been looked through.
    FoundAll,
    Started {
        config: PathBuf,
        result: Result<Started, String>,
    },
    /// A workspace action ended, with what it came to in one line.
    Did { config: PathBuf, said: String },
    /// An issue's text, or why it could not be read.
    Page {
        title: String,
        result: Result<Vec<String>, String>,
    },
    /// A project's remote branches, or why they could not be listed.
    Branches {
        project: String,
        result: Result<Vec<Branch>, String>,
    },
    /// The sessions, listed again; `None` when they could not be.
    Sessions {
        config: PathBuf,
        list: Option<Vec<Session>>,
    },
    /// Tier moves were applied, with what that came to in one line.
    Retiered {
        config: PathBuf,
        result: Result<String, String>,
    },
    /// A prompt was pasted into a session's pane, or why it was not.
    Sent {
        name: String,
        pane: u64,
        result: Result<(), String>,
    },
    /// A session was brought to the front, in `pane` where the host has panes.
    Attached {
        id: String,
        pane: Option<u64>,
        said: String,
    },
}

/// What can be sent to one session: a prompt typed now, or one on offer.
struct Menu {
    name: String,
    pane: u64,
    offers: Vec<Offer>,
    /// The choice under the cursor; 0 is typing one.
    at: usize,
}

/// A project's remote branches, with the ones picked to delete.
struct Picker {
    project: String,
    list: Vec<Branch>,
    picked: Vec<bool>,
    at: usize,
    /// A first Enter was pressed: the next deletes.
    armed: bool,
}

/// The ways to finish one branch.
struct Finishing {
    project: String,
    branch: String,
    ways: Vec<(String, Finish)>,
    at: usize,
    armed: bool,
}

/// Text to read, scrolled.
struct Page {
    title: String,
    lines: Vec<String>,
    at: usize,
}

/// What changed for a person between two lists of sessions: a session that
/// was at work and now waits on them, or has finished.
pub fn notices(before: &[Session], now: &[Session]) -> Vec<String> {
    now.iter()
        .filter(|session| session.status != "busy")
        .filter(|session| {
            before
                .iter()
                .any(|was| was.id == session.id && was.status == "busy")
        })
        .map(|session| {
            if session.needs_you {
                format!("{} is waiting on you", session.name)
            } else {
                format!("{} finished", session.name)
            }
        })
        .collect()
}

/// A prompt being typed for the session in `pane`.
struct Writing {
    name: String,
    pane: u64,
    text: String,
}

pub struct App {
    /// The config file of the workspace shown.
    config: PathBuf,
    snapshot: Option<Snapshot>,
    found: Option<Vec<Workspace>>,
    view: View,
    ui: Ui,
    /// Each view's selected row and first row drawn.
    places: HashMap<View, (usize, usize)>,
    rows: Vec<Row>,
    reading: bool,
    read_at: Option<Instant>,
    status: String,
    /// Where this runs: whether sessions can be opened in panes beside it.
    pub host: Host,
    /// The pane each session was last opened in, by session id and by name.
    panes: HashMap<String, u64>,
    /// The issue a first Enter armed: a second starts work on it.
    armed: Option<String>,
    /// What the last action came to, shown until the next one.
    said: String,
    /// A project's remote branches to pick from, while the list is open.
    picker: Option<Picker>,
    /// The ways to finish a branch, while the list is open.
    finishing: Option<Finishing>,
    /// An issue's text being read, while it is open.
    page: Option<Page>,
    /// Which of [`MODELS`] a started session runs on.
    model: usize,
    /// When the sessions were last asked after.
    sessions_at: Instant,
    /// The list of prompts to send a session, while it is open.
    menu: Option<Menu>,
    /// A prompt being typed for a session.
    writing: Option<Writing>,
    typing: bool,
    help: bool,
    pub quit: bool,
    frame: usize,
    /// Where the last draw put each tab and the list, for the mouse.
    tabs: Vec<(u16, u16, View)>,
    list: Rect,
}

impl App {
    /// The interface for a workspace, and the first read to ask for.
    pub fn new(config: PathBuf) -> (App, Vec<Ask>) {
        let ask = Ask::Read {
            config: config.clone(),
            quick: true,
        };
        // The workspace shown is listed at once, wherever it lives.
        let found = config
            .parent()
            .and_then(workspaces::describe)
            .map(|one| vec![one]);
        let app = App {
            config,
            snapshot: None,
            found,
            view: View::Now,
            ui: Ui {
                looking: true,
                ..Ui::default()
            },
            places: HashMap::new(),
            rows: Vec::new(),
            reading: true,
            read_at: None,
            status: String::new(),
            host: Host::None,
            panes: HashMap::new(),
            armed: None,
            said: String::new(),
            picker: None,
            finishing: None,
            page: None,
            model: 0,
            sessions_at: Instant::now(),
            menu: None,
            writing: None,
            typing: false,
            help: false,
            quit: false,
            frame: 0,
            tabs: Vec::new(),
            list: Rect::default(),
        };
        (app, vec![ask])
    }

    fn name(&self) -> String {
        self.config
            .parent()
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    fn cursor(&self) -> usize {
        self.places.get(&self.view).map_or(0, |place| place.0)
    }

    fn selected(&self) -> Option<&Row> {
        self.rows
            .get(self.cursor())
            .filter(|row| row.is_selectable())
    }

    /// Work the rows out again and keep the selection on something selectable.
    fn refresh(&mut self) {
        self.rows = match &self.snapshot {
            Some(snapshot) => view::rows(self.view, snapshot, &self.ui, self.found.as_deref()),
            None => Vec::new(),
        };
        let cursor = self.cursor().min(self.rows.len().saturating_sub(1));
        let nearest = (cursor..self.rows.len())
            .chain((0..cursor).rev())
            .find(|at| self.rows[*at].is_selectable())
            .unwrap_or(0);
        self.places.entry(self.view).or_default().0 = nearest;
    }

    fn step(&mut self, by: isize) {
        let mut at = self.cursor();
        let mut left = by.unsigned_abs();
        let mut next = at as isize;
        while left > 0 {
            next += by.signum();
            if next < 0 || next as usize >= self.rows.len() {
                break;
            }
            if self.rows[next as usize].is_selectable() {
                at = next as usize;
                left -= 1;
            }
        }
        let place = self.places.entry(self.view).or_default();
        place.0 = at;
        // At the first thing selectable, show the labels above it too.
        if !self.rows[..at.min(self.rows.len())]
            .iter()
            .any(Row::is_selectable)
        {
            place.1 = 0;
        }
    }

    fn show(&mut self, view: View) {
        self.view = view;
        self.refresh();
    }

    fn turn(&mut self, by: isize) {
        let at = View::ALL
            .iter()
            .position(|one| *one == self.view)
            .unwrap_or(0) as isize;
        let count = View::ALL.len() as isize;
        self.show(View::ALL[((at + by + count) % count) as usize]);
    }

    fn read_again(&mut self) -> Vec<Ask> {
        if self.reading {
            return Vec::new();
        }
        self.reading = true;
        vec![Ask::Read {
            config: self.config.clone(),
            quick: false,
        }]
    }

    fn activate(&mut self) -> Vec<Ask> {
        match self.selected().and_then(|row| row.act.clone()) {
            Some(Act::Fold(key)) => {
                if !self.ui.toggled.remove(&key) {
                    self.ui.toggled.insert(key);
                }
                self.refresh();
                Vec::new()
            }
            Some(Act::Switch(config)) => {
                self.config = config.clone();
                self.snapshot = None;
                self.places.clear();
                self.ui = Ui {
                    looking: self.ui.looking,
                    ..Ui::default()
                };
                self.reading = true;
                self.read_at = None;
                self.status = String::new();
                self.show(View::Now);
                vec![Ask::Read {
                    config,
                    quick: true,
                }]
            }
            Some(Act::Attach { id, name, cwd, .. }) => {
                let pane = self
                    .panes
                    .get(&id)
                    .or_else(|| self.panes.get(&name))
                    .copied();
                self.said = format!("attaching {name}…");
                vec![Ask::Attach {
                    id,
                    name,
                    cwd,
                    pane,
                }]
            }
            Some(Act::Retier(moves)) => {
                // It rewrites the config and commits: asked for twice.
                if self.armed.as_deref() != Some("@retier") {
                    self.said = format!(
                        "enter again moves {}: the labels, the README, one commit (not pushed)",
                        plural_projects(moves.len())
                    );
                    self.armed = Some("@retier".to_string());
                    return Vec::new();
                }
                self.armed = None;
                self.said = "retiering…".to_string();
                vec![Ask::Retier {
                    config: self.config.clone(),
                    moves,
                }]
            }
            Some(Act::Start(issue)) => {
                let name = issue.name();
                // Starting makes a branch and runs a session: asked for twice.
                if self.armed.as_deref() != Some(name.as_str()) {
                    let place = match self.host {
                        Host::GlxyTerm => "in a pane beside this one",
                        Host::None => "in the background",
                    };
                    self.said = format!(
                        "enter again starts {name}: a worktree on {}, and a Claude session {place}{}",
                        start::branch_for(&issue),
                        if self.model == 0 { String::new() } else { format!(" on {} (m changes it)", MODELS[self.model].1) }
                    );
                    self.armed = Some(name);
                    return Vec::new();
                }
                self.armed = None;
                self.said = format!("creating a worktree on {}…", start::branch_for(&issue));
                let model = Some(MODELS[self.model].0)
                    .filter(|id| !id.is_empty())
                    .map(String::from);
                vec![Ask::Start {
                    config: self.config.clone(),
                    issue,
                    model,
                }]
            }
            None => Vec::new(),
        }
    }

    /// Open the list of prompts for the selected row's session. A prompt can
    /// only be pasted into a pane this dashboard opened: the host refuses
    /// any other, so a session must be attached from here first.
    fn offer_to_send(&mut self) {
        let Some(Act::Attach {
            id, name, offers, ..
        }) = self.selected().and_then(|row| row.act.clone())
        else {
            self.said = "p sends a prompt to a session: select one, or a branch or issue with one"
                .to_string();
            return;
        };
        if self.host != Host::GlxyTerm {
            self.said = "sending a prompt needs Galaxy Terminal with its control socket on ([control] enable)".to_string();
            return;
        }
        let Some(pane) = self
            .panes
            .get(&id)
            .or_else(|| self.panes.get(&name))
            .copied()
        else {
            self.said = format!(
                "{name} is not in a pane this dashboard opened: enter attaches it, then p sends"
            );
            return;
        };
        self.said.clear();
        self.menu = Some(Menu {
            name,
            pane,
            offers,
            at: 0,
        });
    }

    /// Keys while the list of prompts is open.
    fn on_menu_key(&mut self, key: KeyEvent) -> Vec<Ask> {
        let Some(menu) = self.menu.as_mut() else {
            return Vec::new();
        };
        let last = menu.offers.len();
        let chosen = match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                menu.at = (menu.at + 1).min(last);
                return Vec::new();
            }
            KeyCode::Up | KeyCode::Char('k') => {
                menu.at = menu.at.saturating_sub(1);
                return Vec::new();
            }
            KeyCode::Enter | KeyCode::Char(' ') => menu.at,
            KeyCode::Char(digit @ '1'..='9') if (digit as usize - '1' as usize) <= last => {
                digit as usize - '1' as usize
            }
            _ => {
                self.menu = None;
                return Vec::new();
            }
        };
        let Some(menu) = self.menu.take() else {
            return Vec::new();
        };
        if chosen == 0 {
            self.writing = Some(Writing {
                name: menu.name,
                pane: menu.pane,
                text: String::new(),
            });
            return Vec::new();
        }
        self.said = format!("sending to {}…", menu.name);
        vec![Ask::Send {
            name: menu.name,
            pane: menu.pane,
            text: menu.offers[chosen - 1].prompt.clone(),
        }]
    }

    /// Keys while a prompt is being typed.
    fn on_writing_key(&mut self, key: KeyEvent) -> Vec<Ask> {
        let Some(writing) = self.writing.as_mut() else {
            return Vec::new();
        };
        match key.code {
            KeyCode::Esc => self.writing = None,
            KeyCode::Backspace => {
                writing.text.pop();
            }
            KeyCode::Char(letter) if writing.text.len() < host::MAX_SEND - 4 => {
                writing.text.push(letter)
            }
            KeyCode::Enter if !writing.text.trim().is_empty() => {
                let Some(done) = self.writing.take() else {
                    return Vec::new();
                };
                self.said = format!("sending to {}…", done.name);
                return vec![Ask::Send {
                    name: done.name,
                    pane: done.pane,
                    text: done.text,
                }];
            }
            _ => {}
        }
        Vec::new()
    }

    /// Keys while a project's branches are listed.
    fn on_picker_key(&mut self, key: KeyEvent) -> Vec<Ask> {
        let Some(picker) = self.picker.as_mut() else {
            return Vec::new();
        };
        let was_armed = std::mem::take(&mut picker.armed);
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                picker.at = (picker.at + 1).min(picker.list.len().saturating_sub(1))
            }
            KeyCode::Up | KeyCode::Char('k') => picker.at = picker.at.saturating_sub(1),
            KeyCode::Char(' ') => {
                // Only a branch whose work is merged or was turned down can be picked.
                if picker
                    .list
                    .get(picker.at)
                    .is_some_and(|b| b.kind.is_prunable())
                {
                    picker.picked[picker.at] = !picker.picked[picker.at];
                }
            }
            KeyCode::Char('a') => {
                let merged: Vec<bool> = picker
                    .list
                    .iter()
                    .map(|b| b.kind == branches::Kind::Merged)
                    .collect();
                picker.picked = if picker.picked == merged {
                    vec![false; merged.len()]
                } else {
                    merged
                };
            }
            KeyCode::Enter => {
                let picks: Vec<(String, String)> = picker
                    .list
                    .iter()
                    .zip(&picker.picked)
                    .filter(|(_, picked)| **picked)
                    .map(|(branch, _)| (branch.name.clone(), branch.sha.clone()))
                    .collect();
                if picks.is_empty() {
                    return Vec::new();
                }
                // Deleting is asked for twice.
                if !was_armed {
                    picker.armed = true;
                    return Vec::new();
                }
                let project = picker.project.clone();
                self.picker = None;
                self.said = format!("deleting {} remote branches of {project}…", picks.len());
                return vec![Ask::Prune {
                    config: self.config.clone(),
                    project,
                    picks,
                }];
            }
            _ => self.picker = None,
        }
        Vec::new()
    }

    /// The ways the selected branch can be finished, from what the board
    /// knows of it.
    fn offer_to_finish(&mut self) {
        let Some((project, branch)) = self.selected().and_then(|row| row.branch.clone()) else {
            self.said = "x finishes a branch: select one on the Board".to_string();
            return;
        };
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(item) = snapshot
            .board
            .iter()
            .find(|i| i.project == project && i.branch == branch)
        else {
            return;
        };
        let mut ways = Vec::new();
        match &item.pr {
            None if !item.path.is_empty() => {
                // Named for an issue that is open: the pull request closes it.
                let issue = super::github::issue_of_branch(&branch).filter(|n| {
                    snapshot
                        .issues
                        .iter()
                        .any(|i| i.project == project && i.number == *n)
                });
                let label = match issue {
                    Some(n) => format!("Push it and open a pull request (closes #{n})"),
                    None => "Push it and open a pull request".to_string(),
                };
                ways.push((label, Finish::OpenPr { issue }));
            }
            Some(pr) if !pr.is_draft => {
                for method in Method::ALL {
                    ways.push((
                        format!("Merge #{} ({}), then clear up", pr.number, method.word()),
                        Finish::Merge {
                            pr: pr.number,
                            method,
                        },
                    ));
                }
            }
            _ => {}
        }
        ways.push((
            "Clear up after a merge made elsewhere".to_string(),
            Finish::Cleanup,
        ));
        self.said.clear();
        self.finishing = Some(Finishing {
            project,
            branch,
            ways,
            at: 0,
            armed: false,
        });
    }

    /// Keys while the ways to finish a branch are listed.
    fn on_finishing_key(&mut self, key: KeyEvent) -> Vec<Ask> {
        let Some(menu) = self.finishing.as_mut() else {
            return Vec::new();
        };
        let was_armed = std::mem::take(&mut menu.armed);
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                menu.at = (menu.at + 1).min(menu.ways.len().saturating_sub(1))
            }
            KeyCode::Up | KeyCode::Char('k') => menu.at = menu.at.saturating_sub(1),
            KeyCode::Enter => {
                // It pushes, merges or deletes: asked for twice.
                if !was_armed {
                    menu.armed = true;
                    return Vec::new();
                }
                let Some(menu) = self.finishing.take() else {
                    return Vec::new();
                };
                let (label, how) = menu.ways[menu.at].clone();
                self.said = format!("{}: {}…", menu.branch, label.to_lowercase());
                return vec![Ask::Finish {
                    config: self.config.clone(),
                    project: menu.project,
                    branch: menu.branch,
                    how,
                }];
            }
            _ => self.finishing = None,
        }
        Vec::new()
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Vec<Ask> {
        if key.kind == KeyEventKind::Release {
            return Vec::new();
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.quit = true;
            return Vec::new();
        }
        if self.help {
            self.help = false;
            return Vec::new();
        }
        if let Some(page) = self.page.as_mut() {
            let last = page.lines.len().saturating_sub(1);
            match key.code {
                KeyCode::Down | KeyCode::Char('j') => page.at = (page.at + 1).min(last),
                KeyCode::Up | KeyCode::Char('k') => page.at = page.at.saturating_sub(1),
                KeyCode::PageDown | KeyCode::Char(' ') => page.at = (page.at + 10).min(last),
                KeyCode::PageUp => page.at = page.at.saturating_sub(10),
                KeyCode::Home | KeyCode::Char('g') => page.at = 0,
                _ => self.page = None,
            }
            return Vec::new();
        }
        if self.picker.is_some() {
            return self.on_picker_key(key);
        }
        if self.finishing.is_some() {
            return self.on_finishing_key(key);
        }
        if self.menu.is_some() {
            return self.on_menu_key(key);
        }
        if self.writing.is_some() {
            return self.on_writing_key(key);
        }
        if self.typing {
            match key.code {
                KeyCode::Enter => self.typing = false,
                KeyCode::Esc => {
                    self.typing = false;
                    self.ui.filter.clear();
                }
                KeyCode::Backspace => {
                    self.ui.filter.pop();
                }
                KeyCode::Char(letter) => self.ui.filter.push(letter),
                _ => {}
            }
            self.refresh();
            return Vec::new();
        }
        // Only a second Enter confirms: any other key takes the question back.
        if !matches!(key.code, KeyCode::Enter | KeyCode::Char(' ')) && self.armed.take().is_some() {
            self.said.clear();
        }
        if key.code == KeyCode::Enter && self.armed.as_deref() == Some("@sync") {
            self.armed = None;
            self.said = "syncing every project…".to_string();
            return vec![Ask::Sync {
                config: self.config.clone(),
            }];
        }
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char(digit @ '1'..='7') => self.show(View::ALL[digit as usize - '1' as usize]),
            KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => self.turn(1),
            KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h') => self.turn(-1),
            KeyCode::Down | KeyCode::Char('j') => self.step(1),
            KeyCode::Up | KeyCode::Char('k') => self.step(-1),
            KeyCode::PageDown => self.step(10),
            KeyCode::PageUp => self.step(-10),
            KeyCode::Home | KeyCode::Char('g') => self.step(-(self.rows.len() as isize)),
            KeyCode::End | KeyCode::Char('G') => self.step(self.rows.len() as isize),
            KeyCode::Enter | KeyCode::Char(' ') => return self.activate(),
            KeyCode::Char('/') => self.typing = true,
            KeyCode::Esc => {
                self.ui.filter.clear();
                self.said.clear();
                self.refresh();
            }
            KeyCode::Char('r') => return self.read_again(),
            KeyCode::Char('p') => self.offer_to_send(),
            KeyCode::Char('x') => self.offer_to_finish(),
            KeyCode::Char('b') => {
                let Some(project) = self.selected().and_then(|row| row.project.clone()) else {
                    self.said =
                        "b lists a project's remote branches: select one on Projects".to_string();
                    return Vec::new();
                };
                self.said = format!("reading the branches of {project}…");
                return vec![Ask::Branches {
                    config: self.config.clone(),
                    project,
                }];
            }
            KeyCode::Char('m') => {
                self.model = (self.model + 1) % MODELS.len();
                self.said = format!("sessions started here run on {}", MODELS[self.model].1);
            }
            KeyCode::Char('o') => {
                let Some((project, number)) = self.selected().and_then(|row| row.issue.clone())
                else {
                    self.said = "o reads an issue: select one".to_string();
                    return Vec::new();
                };
                let title = self
                    .snapshot
                    .iter()
                    .flat_map(|snapshot| &snapshot.issues)
                    .find(|issue| issue.project == project && issue.number == number)
                    .map(|issue| issue.title.clone())
                    .unwrap_or_default();
                self.said = format!("reading {project} #{number}…");
                return vec![Ask::Issue {
                    config: self.config.clone(),
                    project,
                    number,
                    title,
                }];
            }
            KeyCode::Char('f') => {
                let Some(snapshot) = &self.snapshot else {
                    return Vec::new();
                };
                self.said = "fetching every project…".to_string();
                return vec![Ask::Fetch {
                    config: self.config.clone(),
                    keys: snapshot.projects.iter().map(|p| p.name.clone()).collect(),
                }];
            }
            KeyCode::Char('S') => {
                // It pulls into working trees: asked for twice.
                self.armed = Some("@sync".to_string());
                self.said =
                    "enter syncs every project: clones what is missing, then pulls (meta git sync)"
                        .to_string();
            }
            KeyCode::Char(way @ ('[' | ']')) => {
                let at = Span::ALL
                    .iter()
                    .position(|one| *one == self.ui.span)
                    .unwrap_or(0);
                let next = if way == ']' {
                    (at + 1).min(Span::ALL.len() - 1)
                } else {
                    at.saturating_sub(1)
                };
                self.ui.span = Span::ALL[next];
                self.refresh();
            }
            _ => {}
        }
        Vec::new()
    }

    pub fn on_mouse(&mut self, mouse: MouseEvent) -> Vec<Ask> {
        match mouse.kind {
            MouseEventKind::ScrollDown => self.step(3),
            MouseEventKind::ScrollUp => self.step(-3),
            MouseEventKind::Down(MouseButton::Left) => {
                self.help = false;
                self.menu = None;
                if mouse.row == 0 {
                    let hit = self
                        .tabs
                        .iter()
                        .find(|(from, to, _)| (*from..*to).contains(&mouse.column));
                    if let Some((_, _, view)) = hit.copied() {
                        self.show(view);
                    }
                    return Vec::new();
                }
                let within = mouse.row >= self.list.y && mouse.row < self.list.y + self.list.height;
                let offset = self.places.get(&self.view).map_or(0, |place| place.1);
                let at = offset + (mouse.row.saturating_sub(self.list.y)) as usize;
                if !within || !self.rows.get(at).is_some_and(Row::is_selectable) {
                    return Vec::new();
                }
                // A click selects; a click on a heading, or on what is selected, opens.
                let again = at == self.cursor();
                self.places.entry(self.view).or_default().0 = at;
                if again || self.rows[at].kind == Kind::Heading {
                    return self.activate();
                }
            }
            _ => {}
        }
        Vec::new()
    }

    pub fn on_tell(&mut self, tell: Tell) -> Vec<Ask> {
        match tell {
            Tell::Found(one) => {
                let found = self.found.get_or_insert_with(Vec::new);
                if !found.iter().any(|known| known.config == one.config) {
                    found.push(one);
                }
                self.refresh();
                Vec::new()
            }
            Tell::FoundAll => {
                self.found.get_or_insert_with(Vec::new);
                self.ui.looking = false;
                self.refresh();
                Vec::new()
            }
            Tell::Sent { name, pane, result } => {
                self.said = match result {
                    Ok(()) => format!("sent to {name}"),
                    Err(why) => {
                        // A pane the host no longer has is forgotten: enter opens a new one.
                        if why.contains("was not opened") || why.contains("already closed") {
                            self.panes.retain(|_, known| *known != pane);
                        }
                        format!("could not send to {name}: {why}")
                    }
                };
                Vec::new()
            }
            Tell::Attached { id, pane, said } => {
                if let Some(pane) = pane {
                    self.panes.insert(id, pane);
                }
                self.said = said;
                Vec::new()
            }
            Tell::Did { config, said } => {
                self.said = said;
                if config == self.config {
                    return self.read_again();
                }
                Vec::new()
            }
            Tell::Branches { project, result } => {
                match result {
                    Ok(list) if list.is_empty() => {
                        self.said = format!("{project} has no remote branches besides its default")
                    }
                    Ok(list) => {
                        self.said.clear();
                        // What is merged starts picked; the rest is a person's to choose.
                        let picked = list
                            .iter()
                            .map(|b| b.kind == branches::Kind::Merged)
                            .collect();
                        self.picker = Some(Picker {
                            project,
                            list,
                            picked,
                            at: 0,
                            armed: false,
                        });
                    }
                    Err(why) => self.said = why,
                }
                Vec::new()
            }
            Tell::Page { title, result } => {
                match result {
                    Ok(lines) => {
                        self.said.clear();
                        self.page = Some(Page {
                            title,
                            lines,
                            at: 0,
                        });
                    }
                    Err(why) => self.said = why,
                }
                Vec::new()
            }
            Tell::Sessions { config, list } => {
                let (Some(list), Some(snapshot), true) =
                    (list, self.snapshot.as_mut(), config == self.config)
                else {
                    return Vec::new();
                };
                let told = notices(&snapshot.sessions, &list);
                super::place_sessions(&mut snapshot.board, &list);
                snapshot.sessions = list;
                self.refresh();
                self.say_notices(told)
            }
            Tell::Retiered { config, result } => {
                self.said = result.unwrap_or_else(|why| format!("could not retier: {why}"));
                if config == self.config {
                    return self.read_again();
                }
                Vec::new()
            }
            Tell::Started { config, result } => {
                match result {
                    Ok(started) => {
                        if let Some(pane) = started.pane {
                            self.panes.insert(started.name, pane);
                        }
                        self.said = started.said;
                    }
                    Err(why) => self.said = why,
                }
                // The board has a new branch, or half of one: read it.
                if config == self.config {
                    return self.read_again();
                }
                Vec::new()
            }
            // An answer for a workspace no longer shown.
            Tell::Read { config, .. } if config != self.config => Vec::new(),
            Tell::Read {
                quick,
                result: Ok(mut snapshot),
                ..
            } => {
                let mut said: Vec<String> = snapshot.notes.clone();
                if !snapshot.github.failed.is_empty() {
                    said.push(format!(
                        "no GitHub data for {}",
                        snapshot.github.failed.join(", ")
                    ));
                }
                self.status = said.join(" · ");
                if let Some(before) = &self.snapshot {
                    super::keep_last_seen(&mut snapshot, before);
                }
                let told = match &self.snapshot {
                    Some(before) => notices(&before.sessions, &snapshot.sessions),
                    None => Vec::new(),
                };
                self.snapshot = Some(*snapshot);
                self.sessions_at = Instant::now();
                self.refresh();
                let mut asks = self.say_notices(told);
                if quick {
                    // Git and sessions are in: now the slow half.
                    asks.push(Ask::Read {
                        config: self.config.clone(),
                        quick: false,
                    });
                    return asks;
                }
                self.reading = false;
                self.read_at = Some(Instant::now());
                asks
            }
            Tell::Read {
                result: Err(error), ..
            } => {
                self.status = format!("could not read the workspace: {error}");
                self.reading = false;
                self.read_at = Some(Instant::now());
                Vec::new()
            }
        }
    }

    /// Say what changed among the sessions, here and to the host.
    fn say_notices(&mut self, told: Vec<String>) -> Vec<Ask> {
        if told.is_empty() {
            return Vec::new();
        }
        self.said = told.join(" · ");
        told.into_iter().map(|text| Ask::Notify { text }).collect()
    }

    /// Called between events: reads again once what is shown has gone stale,
    /// and asks after the sessions alone more often than that.
    pub fn tick(&mut self) -> Vec<Ask> {
        self.frame = self.frame.wrapping_add(1);
        if self.read_at.is_some_and(|at| at.elapsed() >= STALE) {
            return self.read_again();
        }
        if let (Some(snapshot), false) = (&self.snapshot, self.reading) {
            if self.sessions_at.elapsed() >= SESSIONS_EVERY {
                self.sessions_at = Instant::now();
                return vec![Ask::Sessions {
                    config: self.config.clone(),
                    keys: snapshot.projects.iter().map(|p| p.name.clone()).collect(),
                    slugs: snapshot
                        .projects
                        .iter()
                        .filter_map(|p| Some((p.slug.as_ref()?.to_lowercase(), p.name.clone())))
                        .collect(),
                }];
            }
        }
        Vec::new()
    }

    pub fn draw(&mut self, frame: &mut Frame) {
        let detail: Vec<String> = self
            .selected()
            .map(|row| row.detail.clone())
            .unwrap_or_default();
        let detail_height = if detail.is_empty() {
            0
        } else {
            detail.len().min(DETAIL_ROWS) as u16 + 1
        };
        let [tabs, body, under, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(detail_height),
            Constraint::Length(1),
        ])
        .areas(frame.area());

        // Tabs, each with how much it holds, and where each one is for the mouse.
        self.tabs.clear();
        let mut spans = Vec::new();
        let mut at = tabs.x;
        for (n, one) in View::ALL.into_iter().enumerate() {
            let count = self
                .snapshot
                .as_ref()
                .and_then(|snapshot| view::count(one, snapshot));
            let label = match count {
                Some(count) if count > 0 => format!(" {} {} {count} ", n + 1, one.title()),
                _ => format!(" {} {} ", n + 1, one.title()),
            };
            let width = Line::from(label.as_str()).width() as u16;
            self.tabs.push((at, at + width, one));
            at += width;
            let style = if one == self.view {
                Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            spans.push(Text::styled(label, style));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), tabs);
        let state = if self.reading {
            format!("{} reading", SPIN[self.frame % SPIN.len()])
        } else {
            match self.read_at.map(|at| at.elapsed().as_secs()) {
                Some(secs) if secs >= 60 => format!("read {}m ago", secs / 60),
                Some(_) => "read just now".to_string(),
                None => String::new(),
            }
        };
        let right = format!("{} · {state} ", self.name());
        if (at as usize) + right.chars().count() < tabs.width as usize {
            frame.render_widget(
                Paragraph::new(right)
                    .alignment(Alignment::Right)
                    .style(Style::default().fg(Color::DarkGray)),
                tabs,
            );
        }

        // The list, scrolled to keep the selection in sight.
        self.list = body;
        let height = body.height as usize;
        let cursor = self.cursor();
        let place = self.places.entry(self.view).or_default();
        if self.rows.len() <= height {
            place.1 = 0;
        } else {
            place.1 = place.1.min(cursor).min(self.rows.len() - height);
            if cursor >= place.1 + height {
                place.1 = cursor + 1 - height;
            }
        }
        let offset = place.1;
        let lines: Vec<Line> = if self.snapshot.is_none() {
            let said = if self.status.is_empty() {
                "Reading the workspace…"
            } else {
                self.status.as_str()
            };
            vec![Line::styled(
                format!("  {said}"),
                Style::default().fg(Color::DarkGray),
            )]
        } else {
            self.rows
                .iter()
                .enumerate()
                .skip(offset)
                .take(height)
                .map(|(at, row)| line_of(row, at == cursor && row.is_selectable()))
                .collect()
        };
        frame.render_widget(Paragraph::new(lines), body);

        if detail_height > 0 {
            let lines: Vec<Line> = detail
                .iter()
                .take(DETAIL_ROWS)
                .map(|line| Line::from(format!("  {line}")))
                .collect();
            let block = Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(Color::DarkGray));
            frame.render_widget(Paragraph::new(lines).block(block), under);
        }

        let foot = if let Some(writing) = &self.writing {
            // The end of a long prompt is what is being typed: show that.
            let room = (footer.width as usize).saturating_sub(writing.name.chars().count() + 36);
            let typed: Vec<char> = writing.text.chars().collect();
            let shown: String = typed[typed.len().saturating_sub(room.max(8))..]
                .iter()
                .collect();
            Line::from(vec![
                Text::styled(
                    format!(" to {}: {shown}", writing.name),
                    Style::default().fg(Color::Cyan),
                ),
                Text::styled("█", Style::default().fg(Color::Cyan)),
                Text::styled(
                    "  enter sends it · esc cancels",
                    Style::default().fg(Color::DarkGray),
                ),
            ])
        } else if self.typing {
            Line::from(vec![
                Text::styled(
                    format!(" /{}", self.ui.filter),
                    Style::default().fg(Color::Cyan),
                ),
                Text::styled("█", Style::default().fg(Color::Cyan)),
                Text::styled(
                    "  enter keeps it · esc clears it",
                    Style::default().fg(Color::DarkGray),
                ),
            ])
        } else if !self.ui.filter.trim().is_empty() {
            Line::from(vec![
                Text::styled(
                    format!(" filter: {}", self.ui.filter),
                    Style::default().fg(Color::Cyan),
                ),
                Text::styled(
                    "  esc clears it · / changes it",
                    Style::default().fg(Color::DarkGray),
                ),
            ])
        } else if !self.said.is_empty() {
            Line::styled(format!(" {}", self.said), Style::default().fg(Color::Cyan))
        } else if !self.status.is_empty() && self.snapshot.is_some() {
            Line::styled(
                format!(" {}", self.status),
                Style::default().fg(Color::Yellow),
            )
        } else {
            Line::styled(format!(" {KEYS}"), Style::default().fg(Color::DarkGray))
        };
        frame.render_widget(Paragraph::new(foot), footer);

        if let Some(menu) = &self.menu {
            draw_menu(frame, menu);
        }
        if let Some(page) = &self.page {
            draw_page(frame, page);
        }
        if let Some(picker) = &self.picker {
            draw_picker(frame, picker);
        }
        if let Some(menu) = &self.finishing {
            draw_finishing(frame, menu);
        }
        if self.help {
            draw_help(frame);
        }
    }
}

/// A box in the middle of the screen, sized to its lines.
fn draw_box(frame: &mut Frame, title: String, lines: Vec<Line>) {
    let area = frame.area();
    let widest = lines
        .iter()
        .map(Line::width)
        .max()
        .unwrap_or(0)
        .max(title.chars().count());
    let width = (widest as u16 + 4).min(area.width);
    let height = (lines.len() as u16 + 2).min(area.height);
    let spot = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, spot);
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(title)),
        spot,
    );
}

fn draw_picker(frame: &mut Frame, picker: &Picker) {
    let room = (frame.area().height as usize).saturating_sub(6).max(3);
    let from = picker.at.saturating_sub(room - 1);
    let width = picker
        .list
        .iter()
        .map(|b| b.name.chars().count())
        .max()
        .unwrap_or(0);
    let mut lines: Vec<Line> = picker
        .list
        .iter()
        .enumerate()
        .skip(from)
        .take(room)
        .map(|(n, branch)| {
            let mark = match (branch.kind.is_prunable(), picker.picked[n]) {
                (false, _) => " · ",
                (true, true) => "[x]",
                (true, false) => "[ ]",
            };
            let pr = branch.pr.map_or(String::new(), |pr| format!(" #{pr}"));
            let text = format!(
                "{} {mark} {:<width$}  {}{pr}  {}  +{}",
                if n == picker.at { "›" } else { " " },
                branch.name,
                branch.kind.word(),
                branch.date,
                branch.ahead
            );
            let style = if n == picker.at {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else if branch.kind.is_prunable() {
                Style::default()
            } else {
                Style::default().fg(Color::DarkGray)
            };
            Line::styled(text, style)
        })
        .collect();
    let picked = picker.picked.iter().filter(|p| **p).count();
    lines.push(Line::raw(""));
    lines.push(if picker.armed {
        Line::styled(
            format!("  enter again deletes {picked} on the remote (each is logged, so it can be put back)"),
            Style::default().fg(Color::Yellow),
        )
    } else {
        Line::styled(
            format!("  space picks · a all merged · enter deletes the {picked} picked · esc closes"),
            Style::default().fg(Color::DarkGray),
        )
    });
    draw_box(
        frame,
        format!(" remote branches of {} ", picker.project),
        lines,
    );
}

fn draw_finishing(frame: &mut Frame, menu: &Finishing) {
    let mut lines: Vec<Line> = menu
        .ways
        .iter()
        .enumerate()
        .map(|(n, (label, _))| {
            let style = if n == menu.at {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            Line::styled(
                format!("{} {label}", if n == menu.at { "›" } else { " " }),
                style,
            )
        })
        .collect();
    lines.push(Line::raw(""));
    lines.push(if menu.armed {
        Line::styled("  enter again does it", Style::default().fg(Color::Yellow))
    } else {
        Line::styled(
            "  enter chooses · esc closes",
            Style::default().fg(Color::DarkGray),
        )
    });
    draw_box(
        frame,
        format!(" finish {} {} ", menu.project, menu.branch),
        lines,
    );
}

fn draw_page(frame: &mut Frame, page: &Page) {
    let area = frame.area();
    let width = 100
        .min(area.width.saturating_sub(4))
        .max(20)
        .min(area.width);
    let height = area.height.saturating_sub(2).max(3).min(area.height);
    let spot = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, spot);
    let lines: Vec<Line> = page
        .lines
        .iter()
        .skip(page.at)
        .map(|line| Line::raw(line.clone()))
        .collect();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ", page.title))
        .title_bottom(" j/k scroll · any other key closes ");
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(ratatui::widgets::Wrap { trim: false })
            .block(block),
        spot,
    );
}

fn draw_menu(frame: &mut Frame, menu: &Menu) {
    let mut lines = Vec::new();
    for (n, label) in std::iter::once("Type a prompt…")
        .chain(menu.offers.iter().map(|offer| offer.label.as_str()))
        .enumerate()
    {
        let style = if n == menu.at {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        lines.push(Line::styled(
            format!(
                "{} {}. {label}",
                if n == menu.at { "›" } else { " " },
                n + 1
            ),
            style,
        ));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "  enter sends it · esc closes",
        Style::default().fg(Color::DarkGray),
    ));
    let area = frame.area();
    let title = format!(" send to {} ", menu.name);
    let width = (lines
        .iter()
        .map(Line::width)
        .max()
        .unwrap_or(0)
        .max(title.chars().count()) as u16
        + 4)
    .min(area.width);
    let height = (lines.len() as u16 + 2).min(area.height);
    let spot = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, spot);
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(title)),
        spot,
    );
}

fn plural_projects(n: usize) -> String {
    if n == 1 {
        "1 project".to_string()
    } else {
        format!("{n} projects")
    }
}

fn style_of(tone: Tone) -> Style {
    match tone {
        Tone::Plain => Style::default(),
        Tone::Dim => Style::default().fg(Color::DarkGray),
        Tone::Good => Style::default().fg(Color::Green),
        Tone::Warn => Style::default().fg(Color::Yellow),
        Tone::Bad => Style::default().fg(Color::Red),
        Tone::Accent => Style::default().fg(Color::Cyan),
        Tone::Title => Style::default().add_modifier(Modifier::BOLD),
    }
}

fn line_of(row: &Row, is_selected: bool) -> Line<'static> {
    let mut spans = vec![if is_selected {
        Text::styled(
            "› ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Text::raw("  ")
    }];
    spans.push(Text::raw("  ".repeat(row.indent as usize)));
    for (text, tone) in &row.cells {
        let style = style_of(*tone);
        spans.push(Text::styled(
            text.clone(),
            if is_selected {
                style.add_modifier(Modifier::BOLD)
            } else {
                style
            },
        ));
    }
    Line::from(spans)
}

fn draw_help(frame: &mut Frame) {
    let lines = [
        "1-7, tab, h/l    change view",
        "j/k, arrows      move",
        "g/G, page keys   jump",
        "enter, space     open or close a group; switch workspace;",
        "                 attach a session; start work on an issue (asks twice)",
        "p                send a prompt to the selected session (in a pane opened here)",
        "x                finish the selected branch: open its pull request, merge, clear up",
        "b                a project's remote branches; delete the merged ones",
        "o                read the selected issue",
        "m                the model a started session runs on",
        "f                fetch every project",
        "S                sync every project: clone what is missing, then pull (asks twice)",
        "/                filter the view (esc clears)",
        "[ ]              the period Cost adds up",
        "r                read the workspace again",
        "q                quit",
        "",
        "Click a tab or a row; click a heading to open it; the wheel scrolls.",
        "Starting work on an issue makes a branch and a worktree, and runs Claude there.",
        "Fetch, sync, finishing a branch and deleting branches change repositories too;",
        "each that pushes, merges or deletes asks twice.",
    ];
    let area = frame.area();
    let width = 72.min(area.width.saturating_sub(2));
    let height = (lines.len() as u16 + 2).min(area.height);
    let spot = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, spot);
    let block = Block::default().borders(Borders::ALL).title(" meta dash ");
    frame.render_widget(
        Paragraph::new(lines.map(Line::from).to_vec()).block(block),
        spot,
    );
}

/// Reads workspaces as asked, one at a time, for as long as someone listens.
fn reader(asks: mpsc::Receiver<Ask>, tells: mpsc::Sender<Tell>, host: Host, meta: PathBuf) {
    for ask in asks {
        let told = match ask {
            Ask::Read { config, quick } => {
                let options = Options {
                    github: !quick,
                    sessions: true,
                    retier: false,
                    cost: !quick,
                    commit: false,
                };
                let result = workspaces::config_at(&config)
                    .and_then(|runtime| gather(&runtime, options))
                    .map(Box::new)
                    .map_err(|error| error.to_string());
                Tell::Read {
                    config,
                    quick,
                    result,
                }
            }
            Ask::Retier { config, moves } => {
                let result = match config.parent() {
                    Some(root) => retier::retier(root, &config, &moves, true)
                        .map(|outcome| outcome.said(&moves)),
                    None => Err("the workspace has no folder".to_string()),
                };
                Tell::Retiered { config, result }
            }
            Ask::Fetch { config, keys } => {
                let said = match config.parent() {
                    Some(root) => actions::fetch_all(root, &keys),
                    None => "the workspace has no folder".to_string(),
                };
                Tell::Did { config, said }
            }
            Ask::Sync { config } => {
                let said = match config.parent() {
                    Some(root) => actions::sync_all(&meta, root),
                    None => "the workspace has no folder".to_string(),
                };
                Tell::Did { config, said }
            }
            Ask::Issue {
                config,
                project,
                number,
                title,
            } => {
                let result = match config.parent() {
                    Some(root) => actions::issue_text(root, &project, number),
                    None => Err("the workspace has no folder".to_string()),
                };
                Tell::Page {
                    title: format!("{project} #{number} {title}"),
                    result,
                }
            }
            Ask::Branches { config, project } => {
                let result = match config.parent() {
                    Some(root) => branches::list(root, &project),
                    None => Err("the workspace has no folder".to_string()),
                };
                Tell::Branches { project, result }
            }
            Ask::Prune {
                config,
                project,
                picks,
            } => {
                let said = match config.parent() {
                    Some(root) => {
                        branches::prune(root, &project, &picks, branches::log_path().as_deref())
                    }
                    None => "the workspace has no folder".to_string(),
                };
                Tell::Did { config, said }
            }
            Ask::Finish {
                config,
                project,
                branch,
                how,
            } => {
                let said = match config.parent() {
                    Some(root) => match branches::finish(
                        root,
                        &project,
                        &branch,
                        &how,
                        branches::log_path().as_deref(),
                    ) {
                        Ok(did) => format!("{branch}: {}", did.join(" · ")),
                        Err(why) => format!("{branch}: {why}"),
                    },
                    None => "the workspace has no folder".to_string(),
                };
                Tell::Did { config, said }
            }
            Ask::Sessions {
                config,
                keys,
                slugs,
            } => {
                let list = config
                    .parent()
                    .and_then(|root| sessions::read(root, &keys, &slugs));
                Tell::Sessions { config, list }
            }
            Ask::Start {
                config,
                issue,
                model,
            } => {
                let result = match config.parent() {
                    Some(root) => start::start(&meta, root, host, &issue, model.as_deref()),
                    None => Err("the workspace has no folder".to_string()),
                };
                Tell::Started { config, result }
            }
            // Attaching and sending are the interface's own to do: one may
            // need the terminal, the other is a moment's work.
            Ask::Attach { .. } | Ask::Send { .. } | Ask::Notify { .. } => continue,
        };
        if tells.send(told).is_err() {
            break;
        }
    }
}

/// Bring a session to the front where the host has panes: the pane it was
/// last opened in if that is still there, else a new one beside this.
fn attach_in_pane(root: &Path, id: String, name: &str, cwd: &str, pane: Option<u64>) -> Tell {
    if let Some(pane) = pane.filter(|pane| host::focus(&host::glxy_term, *pane)) {
        return Tell::Attached {
            id,
            pane: Some(pane),
            said: format!("{name} is in its pane"),
        };
    }
    let folder = Some(Path::new(cwd))
        .filter(|folder| folder.is_dir())
        .unwrap_or(root);
    let argv = ["claude", "attach", &id].map(String::from);
    match host::open_pane(&host::glxy_term, folder, name, &argv) {
        Ok(pane) => Tell::Attached {
            id,
            pane: Some(pane),
            said: format!("attached {name} beside this pane"),
        },
        Err(why) => Tell::Attached {
            id,
            pane: None,
            said: format!("could not attach {name}: {why}"),
        },
    }
}

/// With no host to open a pane: hand this terminal to `claude attach`, and
/// take it back when the person detaches.
fn attach_here(id: String, name: &str) -> Tell {
    let mut out = std::io::stdout();
    let _ = crossterm::terminal::disable_raw_mode();
    let _ = crossterm::execute!(
        out,
        DisableMouseCapture,
        crossterm::terminal::LeaveAlternateScreen,
        crossterm::cursor::Show
    );
    let ran = std::process::Command::new("claude")
        .args(["attach", &id])
        .status();
    let _ = crossterm::terminal::enable_raw_mode();
    let _ = crossterm::execute!(
        out,
        crossterm::terminal::EnterAlternateScreen,
        EnableMouseCapture
    );
    Tell::Attached {
        id,
        pane: None,
        said: match ran {
            Ok(_) => format!("back from {name}"),
            Err(why) => format!("could not run claude attach: {why}"),
        },
    }
}

/// Run the dashboard for the workspace `config` belongs to, until quit.
pub fn run(config: &RuntimeConfig) -> Result<()> {
    let path = config
        .meta_file_path
        .as_ref()
        .ok_or_else(|| anyhow!("No workspace config found. Run 'meta init' first."))?;
    let path = path.canonicalize().unwrap_or_else(|_| path.clone());
    let (ask, asks) = mpsc::channel();
    let (tell, tells) = mpsc::channel();
    // Detached, not joined: quitting must not wait for a read under way.
    let found = tell.clone();
    let host = Host::detect();
    let meta = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("meta"));
    std::thread::spawn(move || reader(asks, tell, host, meta));
    std::thread::spawn(move || {
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
        if let Some(home) = home {
            workspaces::find(Path::new(&home), |one| {
                let _ = found.send(Tell::Found(one));
            });
        }
        let _ = found.send(Tell::FoundAll);
    });

    let root = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let (mut app, first) = App::new(path);
    app.host = host;
    let mut terminal = init_terminal()?;
    let _ = crossterm::execute!(std::io::stdout(), EnableMouseCapture);
    let result = (|| -> Result<()> {
        let mut asked = first;
        while !app.quit {
            // What the interface asked for: reads and starts go to the reader,
            // attaching is done here, and may ask for more.
            while let Some(one) = asked.pop() {
                match one {
                    Ask::Attach {
                        id,
                        name,
                        cwd,
                        pane,
                    } => {
                        let told = match host {
                            Host::GlxyTerm => attach_in_pane(&root, id, &name, &cwd, pane),
                            Host::None => {
                                let told = attach_here(id, &name);
                                terminal.clear()?;
                                told
                            }
                        };
                        asked.extend(app.on_tell(told));
                    }
                    Ask::Notify { text } => host::notify(&host::glxy_term, host, &text),
                    Ask::Send { name, pane, text } => {
                        let result = host::send(&host::glxy_term, pane, &text);
                        asked.extend(app.on_tell(Tell::Sent { name, pane, result }));
                    }
                    other => {
                        let _ = ask.send(other);
                    }
                }
            }
            while let Ok(told) = tells.try_recv() {
                asked.extend(app.on_tell(told));
            }
            asked.extend(app.tick());
            terminal.draw(|frame| app.draw(frame))?;
            match poll_event()? {
                Some(Event::Key(key)) => asked.extend(app.on_key(key)),
                Some(Event::Mouse(mouse)) => asked.extend(app.on_mouse(mouse)),
                _ => {}
            }
        }
        Ok(())
    })();
    let _ = crossterm::execute!(std::io::stdout(), DisableMouseCapture);
    restore_terminal(terminal)?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::dash::view::tests::snapshot;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// An app that has had its quick and full reads answered.
    fn app() -> App {
        let (mut app, first) = App::new("/ws/.metarepo".into());
        assert_eq!(
            first,
            vec![Ask::Read {
                config: "/ws/.metarepo".into(),
                quick: true
            }]
        );
        let more = app.on_tell(Tell::Read {
            config: "/ws/.metarepo".into(),
            quick: true,
            result: Ok(Box::new(snapshot())),
        });
        assert_eq!(
            more,
            vec![Ask::Read {
                config: "/ws/.metarepo".into(),
                quick: false
            }]
        );
        assert!(app.reading);
        assert!(app
            .on_tell(Tell::Read {
                config: "/ws/.metarepo".into(),
                quick: false,
                result: Ok(Box::new(snapshot()))
            })
            .is_empty());
        assert!(!app.reading);
        app
    }

    fn screen(app: &mut App, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn every_view_draws_and_the_tabs_count_what_they_hold() {
        let mut app = app();
        let lines = screen(&mut app, 120, 30);
        assert!(lines[0].starts_with(
            " 1 Now 4  2 Projects 4  3 Board 3  4 Issues 4  5 Sessions 3  6 Cost  7 Workspaces"
        ));
        assert!(lines[0].ends_with("ws · read just now"));
        assert!(lines.iter().any(|l| l.contains("Waiting on you")));
        assert!(lines[29].contains("? help"));
        for (digit, words) in [
            ('2', "▾ Active  1"),
            ('3', "▾ Needs fixing  1"),
            ('4', "▾ Urgent  2"),
            ('5', "▾ Working  1"),
            ('6', "By project"),
            ('7', "Looking for workspaces"),
        ] {
            app.on_key(key(KeyCode::Char(digit)));
            let lines = screen(&mut app, 120, 30);
            assert!(
                lines.iter().any(|l| l.contains(words)),
                "{digit}: {lines:#?}"
            );
        }
        // Narrow and short: still draws, and the selection stays in sight.
        app.on_key(key(KeyCode::Char('4')));
        app.on_key(key(KeyCode::Char('G')));
        let small = screen(&mut app, 40, 8);
        assert!(small.iter().any(|l| l.starts_with("› ")), "{small:#?}");
    }

    #[test]
    fn before_the_first_read_it_says_it_is_reading() {
        let (mut app, _) = App::new("/ws/.metarepo".into());
        let lines = screen(&mut app, 120, 10);
        assert!(lines[1].contains("Reading the workspace…"));
        assert!(lines[0].contains("reading"));
        app.on_tell(Tell::Read {
            config: "/ws/.metarepo".into(),
            quick: true,
            result: Err("no such file".into()),
        });
        assert!(screen(&mut app, 120, 10)[1].contains("could not read the workspace: no such file"));
    }

    #[test]
    fn keys_move_fold_and_show_detail() {
        let mut app = app();
        app.on_key(key(KeyCode::Char('2')));
        // The selection starts on the first heading and skips the gaps between groups.
        assert_eq!(app.selected().unwrap().text(), "▾ Active  1");
        app.on_key(key(KeyCode::Char('j')));
        assert!(app.selected().unwrap().text().starts_with("claw"));
        assert!(screen(&mut app, 100, 30)
            .iter()
            .any(|l| l.contains("github.com/org/claw")));
        app.on_key(key(KeyCode::Char('j')));
        assert_eq!(app.selected().unwrap().text(), "▾ Maintained  1");
        // Enter closes the group, and opens it again.
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.selected().unwrap().text(), "▸ Maintained  1");
        assert!(!app.rows.iter().any(|row| row.text().starts_with("earth")));
        app.on_key(key(KeyCode::Char(' ')));
        assert!(app.rows.iter().any(|row| row.text().starts_with("earth")));
        app.on_key(key(KeyCode::Char('G')));
        assert!(app.selected().unwrap().text().starts_with("loose"));
        app.on_key(key(KeyCode::Char('g')));
        assert_eq!(app.selected().unwrap().text(), "▾ Active  1");
        // Each view keeps its own place.
        app.on_key(key(KeyCode::Char('j')));
        app.on_key(key(KeyCode::Tab));
        assert_eq!(app.view, View::Board);
        app.on_key(key(KeyCode::BackTab));
        assert!(app.selected().unwrap().text().starts_with("claw"));
    }

    #[test]
    fn typing_filters_and_escape_clears() {
        let mut app = app();
        app.on_key(key(KeyCode::Char('4')));
        app.on_key(key(KeyCode::Char('/')));
        for letter in "globe".chars() {
            app.on_key(key(KeyCode::Char(letter)));
        }
        // While typing, q is a letter, not quit.
        app.on_key(key(KeyCode::Char('q')));
        assert!(!app.quit);
        app.on_key(key(KeyCode::Backspace));
        assert!(screen(&mut app, 80, 12)[11].starts_with(" /globe█"));
        app.on_key(key(KeyCode::Enter));
        assert_eq!(app.rows.len(), 2);
        assert!(screen(&mut app, 80, 12)[11].starts_with(" filter: globe"));
        app.on_key(key(KeyCode::Esc));
        assert!(app.rows.len() > 2);
        app.on_key(key(KeyCode::Char('q')));
        assert!(app.quit);
    }

    #[test]
    fn the_cost_period_moves_with_the_brackets() {
        let mut app = app();
        app.on_key(key(KeyCode::Char('6')));
        assert!(app.rows[0].text().contains("● 7 days"));
        app.on_key(key(KeyCode::Char(']')));
        app.on_key(key(KeyCode::Char(']')));
        app.on_key(key(KeyCode::Char(']')));
        assert!(app.rows[0].text().contains("● All"));
        app.on_key(key(KeyCode::Char('[')));
        assert!(app.rows[0].text().contains("● 30 days"));
    }

    #[test]
    fn the_mouse_picks_tabs_and_rows() {
        let mut app = app();
        screen(&mut app, 120, 30);
        let click = |column, row| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };
        // "2 Projects" starts after " 1 Now 4 ".
        app.on_mouse(click(12, 0));
        assert_eq!(app.view, View::Projects);
        screen(&mut app, 120, 30);
        // Row 2 of the screen is the first project; a click selects it.
        app.on_mouse(click(5, 2));
        assert!(app.selected().unwrap().text().starts_with("claw"));
        // A click on a heading closes it at once.
        app.on_mouse(click(5, 1));
        assert_eq!(app.selected().unwrap().text(), "▸ Active  1");
        app.on_mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 0,
            row: 5,
            modifiers: KeyModifiers::NONE,
        });
        assert_ne!(app.selected().unwrap().text(), "▸ Active  1");
    }

    #[test]
    fn switching_workspace_reads_the_other_and_drops_late_answers() {
        let mut app = app();
        for (root, config, name, projects, tiers) in [
            ("/other", "/other/.meta", "other", 2, [2, 0, 0]),
            ("/ws", "/ws/.metarepo", "ws", 4, [1, 1, 1]),
            // Found twice, listed once.
            ("/ws", "/ws/.metarepo", "ws", 4, [1, 1, 1]),
        ] {
            app.on_tell(Tell::Found(Workspace {
                root: root.into(),
                config: config.into(),
                name: name.into(),
                projects,
                tiers,
            }));
        }
        app.on_key(key(KeyCode::Char('7')));
        assert_eq!(
            app.rows.iter().filter(|row| row.kind == Kind::Item).count(),
            2
        );
        assert!(app
            .rows
            .iter()
            .any(|row| row.text() == "Still looking for more…"));
        app.on_tell(Tell::FoundAll);
        assert!(!app
            .rows
            .iter()
            .any(|row| row.text() == "Still looking for more…"));
        assert!(app.selected().unwrap().text().starts_with("● ws"));
        // Enter on the workspace shown does nothing.
        assert!(app.on_key(key(KeyCode::Enter)).is_empty());
        app.on_key(key(KeyCode::Char('j')));
        let asked = app.on_key(key(KeyCode::Enter));
        assert_eq!(
            asked,
            vec![Ask::Read {
                config: "/other/.meta".into(),
                quick: true
            }]
        );
        assert!(app.snapshot.is_none() && app.reading);
        assert_eq!(app.view, View::Now);
        // The old workspace's answer arrives late and is dropped.
        assert!(app
            .on_tell(Tell::Read {
                config: "/ws/.metarepo".into(),
                quick: false,
                result: Ok(Box::new(snapshot()))
            })
            .is_empty());
        assert!(app.snapshot.is_none());
    }

    #[test]
    fn a_stale_snapshot_is_read_again_once() {
        let mut app = app();
        assert!(app.tick().is_empty());
        app.read_at = Some(Instant::now() - STALE);
        assert_eq!(
            app.tick(),
            vec![Ask::Read {
                config: "/ws/.metarepo".into(),
                quick: false
            }]
        );
        // While that read is under way neither the clock nor r asks again.
        assert!(app.tick().is_empty());
        assert!(app.on_key(key(KeyCode::Char('r'))).is_empty());
    }

    #[test]
    fn enter_on_a_session_asks_to_attach_and_remembers_its_pane() {
        let mut app = app();
        app.host = Host::GlxyTerm;
        app.on_key(key(KeyCode::Char('5')));
        // Waiting on you comes first: its heading, then earth-3.
        app.on_key(key(KeyCode::Char('j')));
        assert!(app.selected().unwrap().text().starts_with("earth-3"));
        let asked = app.on_key(key(KeyCode::Enter));
        assert_eq!(
            asked,
            vec![Ask::Attach {
                id: "earth-3".into(),
                name: "earth-3".into(),
                cwd: "/ws/earth".into(),
                pane: None,
            }]
        );
        app.on_tell(Tell::Attached {
            id: "earth-3".into(),
            pane: Some(12),
            said: "attached earth-3 beside this pane".into(),
        });
        assert!(screen(&mut app, 120, 20)[19].contains("attached earth-3 beside this pane"));
        // The next Enter names the pane it has, to focus it and not open a second.
        let again = app.on_key(key(KeyCode::Enter));
        assert!(matches!(&again[0], Ask::Attach { pane: Some(12), .. }));
    }

    #[test]
    fn starting_on_an_issue_asks_twice_then_reads_the_board_again() {
        let mut app = app();
        app.host = Host::GlxyTerm;
        app.on_key(key(KeyCode::Char('4')));
        app.on_key(key(KeyCode::Char('j')));
        assert!(app
            .selected()
            .unwrap()
            .text()
            .contains("#7 mind P0 Data loss on save"));
        // The first Enter only says what a second would do.
        assert!(app.on_key(key(KeyCode::Enter)).is_empty());
        assert!(screen(&mut app, 140, 20)[19].contains(
            "enter again starts mind-7: a worktree on feat/7-data-loss-on-save, and a Claude session in a pane beside this one"
        ));
        // Moving away takes the question back.
        app.on_key(key(KeyCode::Char('j')));
        app.on_key(key(KeyCode::Char('k')));
        assert!(app.on_key(key(KeyCode::Enter)).is_empty());
        let asked = app.on_key(key(KeyCode::Enter));
        assert!(
            matches!(&asked[..], [Ask::Start { config, issue, model: None }] if config == Path::new("/ws/.metarepo") && issue.name() == "mind-7")
        );
        let more = app.on_tell(Tell::Started {
            config: "/ws/.metarepo".into(),
            result: Ok(Started {
                name: "mind-7".into(),
                pane: Some(31),
                said: "started mind-7 beside this pane on feat/7-data-loss-on-save".into(),
            }),
        });
        assert_eq!(
            more,
            vec![Ask::Read {
                config: "/ws/.metarepo".into(),
                quick: false
            }]
        );
        assert_eq!(app.panes.get("mind-7"), Some(&31));
        assert!(screen(&mut app, 120, 20)[19].contains("started mind-7 beside this pane"));
        // A failure is said, and the board is still read again.
        app.reading = false;
        let more = app.on_tell(Tell::Started {
            config: "/ws/.metarepo".into(),
            result: Err("branch feat/7-data-loss-on-save already exists in mind".into()),
        });
        assert_eq!(more.len(), 1);
        assert!(screen(&mut app, 120, 20)[19].contains("already exists in mind"));
    }

    #[test]
    fn an_issue_already_in_flight_is_not_started_again() {
        let mut snap = snapshot();
        for issue in &mut snap.issues {
            issue.in_flight = true;
        }
        let (mut app, _) = App::new("/ws/.metarepo".into());
        app.on_tell(Tell::Read {
            config: "/ws/.metarepo".into(),
            quick: false,
            result: Ok(Box::new(snap)),
        });
        app.on_key(key(KeyCode::Char('4')));
        app.on_key(key(KeyCode::Char('j')));
        assert!(app.on_key(key(KeyCode::Enter)).is_empty());
        assert!(app.on_key(key(KeyCode::Enter)).is_empty());
        assert!(app.armed.is_none());
    }

    /// An app in Galaxy Terminal with the Sessions view open on claw-254,
    /// the session whose branch has a failing pull request.
    fn on_claw_254() -> App {
        let mut app = app();
        app.host = Host::GlxyTerm;
        app.on_key(key(KeyCode::Char('5')));
        for _ in 0..3 {
            app.on_key(key(KeyCode::Char('j')));
        }
        assert!(
            app.selected().unwrap().text().starts_with("claw-254"),
            "{:?}",
            app.selected()
        );
        app
    }

    #[test]
    fn a_prompt_goes_only_to_a_pane_opened_here() {
        let mut app = on_claw_254();
        // Not attached from here yet: nowhere to paste.
        assert!(app.on_key(key(KeyCode::Char('p'))).is_empty());
        assert!(app.menu.is_none());
        assert!(screen(&mut app, 120, 20)[19]
            .contains("claw-254 is not in a pane this dashboard opened: enter attaches it"));
        // Outside Galaxy Terminal there is no pane to paste into at all.
        app.host = Host::None;
        app.on_key(key(KeyCode::Char('p')));
        assert!(screen(&mut app, 120, 20)[19].contains("needs Galaxy Terminal"));
        // And a row with no session says what p is for.
        app.host = Host::GlxyTerm;
        app.on_key(key(KeyCode::Char('2')));
        app.on_key(key(KeyCode::Char('p')));
        assert!(screen(&mut app, 120, 20)[19].contains("p sends a prompt to a session"));
    }

    #[test]
    fn an_offered_prompt_is_sent_to_the_sessions_pane() {
        let mut app = on_claw_254();
        app.on_tell(Tell::Attached {
            id: "claw-254".into(),
            pane: Some(12),
            said: String::new(),
        });
        assert!(app.on_key(key(KeyCode::Char('p'))).is_empty());
        let lines = screen(&mut app, 120, 24);
        assert!(lines.iter().any(|l| l.contains("send to claw-254")));
        assert!(lines.iter().any(|l| l.contains("› 1. Type a prompt…")));
        assert!(lines
            .iter()
            .any(|l| l.contains("2. Fix the failing checks on #7")));
        app.on_key(key(KeyCode::Char('j')));
        let asked = app.on_key(key(KeyCode::Enter));
        assert!(
            matches!(&asked[..], [Ask::Send { name, pane: 12, text }] if name == "claw-254" && text.contains("gh pr checks 7 -R org/claw")),
            "{asked:?}"
        );
        assert!(app.menu.is_none());
        app.on_tell(Tell::Sent {
            name: "claw-254".into(),
            pane: 12,
            result: Ok(()),
        });
        assert!(screen(&mut app, 120, 20)[19].contains("sent to claw-254"));
        // A digit picks at once; any other key closes the list and sends nothing.
        app.on_key(key(KeyCode::Char('p')));
        assert!(
            matches!(&app.on_key(key(KeyCode::Char('3')))[..], [Ask::Send { text, .. }] if text.contains("review comments"))
        );
        app.on_key(key(KeyCode::Char('p')));
        assert!(app.on_key(key(KeyCode::Esc)).is_empty());
        assert!(app.menu.is_none() && !app.quit);
    }

    #[test]
    fn a_typed_prompt_is_sent_on_enter_and_dropped_on_escape() {
        let mut app = on_claw_254();
        app.on_tell(Tell::Attached {
            id: "claw-254".into(),
            pane: Some(12),
            said: String::new(),
        });
        app.on_key(key(KeyCode::Char('p')));
        app.on_key(key(KeyCode::Enter));
        // Enter with nothing typed sends nothing; q is a letter here.
        assert!(app.on_key(key(KeyCode::Enter)).is_empty());
        for letter in "run the tests q".chars() {
            app.on_key(key(KeyCode::Char(letter)));
        }
        app.on_key(key(KeyCode::Backspace));
        assert!(!app.quit);
        assert!(screen(&mut app, 120, 20)[19].starts_with(" to claw-254: run the tests █"));
        let asked = app.on_key(key(KeyCode::Enter));
        assert_eq!(
            asked,
            vec![Ask::Send {
                name: "claw-254".into(),
                pane: 12,
                text: "run the tests ".into()
            }]
        );
        assert!(app.writing.is_none());
        app.on_key(key(KeyCode::Char('p')));
        app.on_key(key(KeyCode::Char('1')));
        app.on_key(key(KeyCode::Char('x')));
        assert!(app.on_key(key(KeyCode::Esc)).is_empty());
        assert!(app.writing.is_none());
    }

    #[test]
    fn a_pane_the_host_no_longer_has_is_forgotten() {
        let mut app = on_claw_254();
        app.on_tell(Tell::Attached {
            id: "claw-254".into(),
            pane: Some(12),
            said: String::new(),
        });
        app.on_tell(Tell::Sent {
            name: "claw-254".into(),
            pane: 12,
            result: Err("glxy-term: pane 12 was not opened by open-pane".into()),
        });
        assert!(screen(&mut app, 140, 20)[19]
            .contains("could not send to claw-254: glxy-term: pane 12 was not opened"));
        // The next p says to attach again, and the next enter opens a new pane.
        app.on_key(key(KeyCode::Char('p')));
        assert!(app.menu.is_none());
        assert!(matches!(
            &app.on_key(key(KeyCode::Enter))[..],
            [Ask::Attach { pane: None, .. }]
        ));
    }

    #[test]
    fn tier_moves_are_applied_on_a_second_enter() {
        let mut snap = snapshot();
        snap.projects[1].tier_due = Some(crate::plugins::dash::Tier::Dormant);
        snap.retier = vec![Move {
            project: "earth".into(),
            from: crate::plugins::dash::Tier::Maintained,
            to: crate::plugins::dash::Tier::Dormant,
            quiet_days: 200,
            applied: false,
        }];
        let (mut app, _) = App::new("/ws/.metarepo".into());
        app.on_tell(Tell::Read {
            config: "/ws/.metarepo".into(),
            quick: false,
            result: Ok(Box::new(snap)),
        });
        app.on_key(key(KeyCode::Char('2')));
        // The moves due are the first thing on Projects.
        assert!(app
            .selected()
            .unwrap()
            .text()
            .contains("1 project is due a tier move: earth maintained to dormant"));
        assert!(app.on_key(key(KeyCode::Enter)).is_empty());
        assert!(screen(&mut app, 120, 20)[19].contains(
            "enter again moves 1 project: the labels, the README, one commit (not pushed)"
        ));
        let asked = app.on_key(key(KeyCode::Enter));
        assert!(
            matches!(&asked[..], [Ask::Retier { moves, .. }] if moves.len() == 1 && moves[0].project == "earth")
        );
        let more = app.on_tell(Tell::Retiered {
            config: "/ws/.metarepo".into(),
            result: Ok("retiered earth to dormant · README updated · committed as abc1234".into()),
        });
        assert_eq!(more.len(), 1);
        assert!(screen(&mut app, 120, 20)[19].contains("committed as abc1234"));
    }

    #[test]
    fn a_session_that_stops_working_is_said_and_the_board_follows() {
        let mut app = app();
        let mut list = app.snapshot.as_ref().unwrap().sessions.clone();
        // claw-254 was busy on a branch with failing checks; now it asks for a person.
        let at = list.iter().position(|s| s.name == "claw-254").unwrap();
        list[at].status = "permission".into();
        list[at].needs_you = true;
        let asked = app.on_tell(Tell::Sessions {
            config: "/ws/.metarepo".into(),
            list: Some(list.clone()),
        });
        assert_eq!(
            asked,
            vec![Ask::Notify {
                text: "claw-254 is waiting on you".into()
            }]
        );
        assert!(screen(&mut app, 120, 20)[19].contains("claw-254 is waiting on you"));
        let snapshot = app.snapshot.as_ref().unwrap();
        let item = snapshot
            .board
            .iter()
            .find(|i| i.branch == "feat/254-pane")
            .unwrap();
        assert_eq!(item.section, crate::plugins::dash::Section::Attention);
        // Then it finishes. A session that was not busy says nothing by changing.
        list[at].status = "busy".into();
        list[at].needs_you = false;
        app.on_tell(Tell::Sessions {
            config: "/ws/.metarepo".into(),
            list: Some(list.clone()),
        });
        list[at].status = "done".into();
        let asked = app.on_tell(Tell::Sessions {
            config: "/ws/.metarepo".into(),
            list: Some(list.clone()),
        });
        assert_eq!(
            asked,
            vec![Ask::Notify {
                text: "claw-254 finished".into()
            }]
        );
        assert!(app
            .on_tell(Tell::Sessions {
                config: "/ws/.metarepo".into(),
                list: Some(list)
            })
            .is_empty());
        // No list, or another workspace's, changes nothing.
        assert!(app
            .on_tell(Tell::Sessions {
                config: "/ws/.metarepo".into(),
                list: None
            })
            .is_empty());
        assert_eq!(app.snapshot.as_ref().unwrap().sessions.len(), 3);
    }

    #[test]
    fn the_sessions_are_asked_after_between_whole_reads() {
        let mut app = app();
        assert!(app.tick().is_empty());
        app.sessions_at = Instant::now() - SESSIONS_EVERY;
        let asked = app.tick();
        assert!(
            matches!(&asked[..], [Ask::Sessions { keys, slugs, .. }] if keys.len() == 4 && slugs.get("org/claw").map(String::as_str) == Some("claw"))
        );
        assert!(app.tick().is_empty());
    }

    #[test]
    fn an_issue_is_read_in_a_page_that_scrolls_and_closes() {
        let mut app = app();
        // On a row that is no issue, o says what it is for.
        app.on_key(key(KeyCode::Char('2')));
        assert!(app.on_key(key(KeyCode::Char('o'))).is_empty());
        app.on_key(key(KeyCode::Char('4')));
        app.on_key(key(KeyCode::Char('j')));
        let asked = app.on_key(key(KeyCode::Char('o')));
        assert!(
            matches!(&asked[..], [Ask::Issue { project, number: 7, title, .. }] if project == "mind" && title == "Data loss on save")
        );
        app.on_tell(Tell::Page {
            title: "mind #7 Data loss on save".into(),
            result: Ok((0..40).map(|n| format!("line {n}")).collect()),
        });
        let lines = screen(&mut app, 100, 20);
        assert!(lines
            .iter()
            .any(|l| l.contains("mind #7 Data loss on save")));
        assert!(lines.iter().any(|l| l.contains("line 0")));
        app.on_key(key(KeyCode::Char(' ')));
        app.on_key(key(KeyCode::Char('j')));
        let lines = screen(&mut app, 100, 20);
        assert!(
            lines.iter().any(|l| l.contains("line 11"))
                && !lines.iter().any(|l| l.contains("line 10 "))
        );
        // Any other key closes it, and is not also acted on.
        app.on_key(key(KeyCode::Char('q')));
        assert!(app.page.is_none() && !app.quit);
        app.on_tell(Tell::Page {
            title: String::new(),
            result: Err("could not load #7 of org/mind".into()),
        });
        assert!(app.page.is_none());
        assert!(screen(&mut app, 100, 20)[19].contains("could not load #7"));
    }

    #[test]
    fn fetch_runs_at_once_and_sync_asks_first() {
        let mut app = app();
        let asked = app.on_key(key(KeyCode::Char('f')));
        assert!(matches!(&asked[..], [Ask::Fetch { keys, .. }] if keys.len() == 4));
        let more = app.on_tell(Tell::Did {
            config: "/ws/.metarepo".into(),
            said: "fetched 4 projects".into(),
        });
        assert_eq!(more.len(), 1);
        assert!(screen(&mut app, 120, 20)[19].contains("fetched 4 projects"));
        app.reading = false;
        assert!(app.on_key(key(KeyCode::Char('S'))).is_empty());
        assert!(screen(&mut app, 120, 20)[19].contains("enter syncs every project"));
        // Another key takes it back.
        app.on_key(key(KeyCode::Char('j')));
        assert!(!matches!(
            &app.on_key(key(KeyCode::Enter))[..],
            [Ask::Sync { .. }]
        ));
        app.on_key(key(KeyCode::Char('S')));
        assert!(matches!(
            &app.on_key(key(KeyCode::Enter))[..],
            [Ask::Sync { .. }]
        ));
    }

    #[test]
    fn the_model_a_session_starts_on_can_be_chosen() {
        let mut app = app();
        app.host = Host::GlxyTerm;
        app.on_key(key(KeyCode::Char('m')));
        app.on_key(key(KeyCode::Char('m')));
        assert!(screen(&mut app, 120, 20)[19].contains("sessions started here run on Opus 5.5"));
        app.on_key(key(KeyCode::Char('4')));
        app.on_key(key(KeyCode::Char('j')));
        app.on_key(key(KeyCode::Enter));
        assert!(screen(&mut app, 160, 20)[19].contains("on Opus 5.5 (m changes it)"));
        let asked = app.on_key(key(KeyCode::Enter));
        assert!(
            matches!(&asked[..], [Ask::Start { model: Some(model), .. }] if model == "claude-opus-5-5")
        );
        // Round to the default again: no model is named.
        for _ in 0..3 {
            app.on_key(key(KeyCode::Char('m')));
        }
        assert_eq!(app.model, 0);
    }

    fn branch(name: &str, kind: branches::Kind) -> Branch {
        Branch {
            name: name.into(),
            sha: format!("sha-{name}"),
            date: "2026-11-01".into(),
            ahead: 0,
            kind,
            pr: None,
        }
    }

    #[test]
    fn merged_remote_branches_are_picked_and_deleted_on_a_second_enter() {
        let mut app = app();
        // b is for a project row.
        assert!(app.on_key(key(KeyCode::Char('b'))).is_empty());
        app.on_key(key(KeyCode::Char('2')));
        app.on_key(key(KeyCode::Char('j')));
        let asked = app.on_key(key(KeyCode::Char('b')));
        assert!(matches!(&asked[..], [Ask::Branches { project, .. }] if project == "claw"));
        app.on_tell(Tell::Branches {
            project: "claw".into(),
            result: Ok(vec![
                branch("done", branches::Kind::Merged),
                branch("refused", branches::Kind::Closed),
                branch("wip", branches::Kind::None),
            ]),
        });
        let lines = screen(&mut app, 120, 20);
        assert!(lines.iter().any(|l| l.contains("remote branches of claw")));
        assert!(lines
            .iter()
            .any(|l| l.contains("› [x] done") && l.contains("merged")));
        assert!(lines.iter().any(|l| l.contains("[ ] refused")));
        assert!(lines.iter().any(|l| l.contains(" ·  wip")));
        // Work with no pull request cannot be picked; a turned-down branch can.
        app.on_key(key(KeyCode::Char('j')));
        app.on_key(key(KeyCode::Char(' ')));
        app.on_key(key(KeyCode::Char('j')));
        app.on_key(key(KeyCode::Char(' ')));
        assert_eq!(app.picker.as_ref().unwrap().picked, vec![true, true, false]);
        // The first Enter asks; moving takes it back; two in a row delete.
        assert!(app.on_key(key(KeyCode::Enter)).is_empty());
        assert!(screen(&mut app, 120, 20)
            .iter()
            .any(|l| l.contains("enter again deletes 2 on the remote")));
        app.on_key(key(KeyCode::Char('k')));
        assert!(app.on_key(key(KeyCode::Enter)).is_empty());
        let asked = app.on_key(key(KeyCode::Enter));
        assert_eq!(
            asked,
            vec![Ask::Prune {
                config: "/ws/.metarepo".into(),
                project: "claw".into(),
                picks: vec![
                    ("done".into(), "sha-done".into()),
                    ("refused".into(), "sha-refused".into())
                ],
            }]
        );
        assert!(app.picker.is_none());
        // Nothing picked: Enter does nothing. Esc closes without deleting.
        app.on_tell(Tell::Branches {
            project: "claw".into(),
            result: Ok(vec![branch("wip", branches::Kind::None)]),
        });
        assert!(app.on_key(key(KeyCode::Enter)).is_empty());
        assert!(app.on_key(key(KeyCode::Enter)).is_empty());
        app.on_key(key(KeyCode::Esc));
        assert!(app.picker.is_none());
        app.on_tell(Tell::Branches {
            project: "claw".into(),
            result: Ok(Vec::new()),
        });
        assert!(app.picker.is_none());
    }

    #[test]
    fn a_branch_is_finished_by_what_the_board_knows_of_it() {
        let mut app = app();
        // x is for a branch on the board.
        app.on_key(key(KeyCode::Char('2')));
        app.on_key(key(KeyCode::Char('x')));
        assert!(app.finishing.is_none());
        // Needs fixing leads the Board: claw feat/254-pane, with pull request #7.
        app.on_key(key(KeyCode::Char('3')));
        app.on_key(key(KeyCode::Char('j')));
        app.on_key(key(KeyCode::Char('x')));
        let lines = screen(&mut app, 120, 20);
        assert!(lines
            .iter()
            .any(|l| l.contains("finish claw feat/254-pane")));
        assert!(lines
            .iter()
            .any(|l| l.contains("› Merge #7 (squash), then clear up")));
        assert!(lines
            .iter()
            .any(|l| l.contains("Clear up after a merge made elsewhere")));
        app.on_key(key(KeyCode::Char('j')));
        assert!(app.on_key(key(KeyCode::Enter)).is_empty());
        assert!(screen(&mut app, 120, 20)
            .iter()
            .any(|l| l.contains("enter again does it")));
        let asked = app.on_key(key(KeyCode::Enter));
        assert_eq!(
            asked,
            vec![Ask::Finish {
                config: "/ws/.metarepo".into(),
                project: "claw".into(),
                branch: "feat/254-pane".into(),
                how: Finish::Merge {
                    pr: 7,
                    method: Method::Merge
                },
            }]
        );
        let more = app.on_tell(Tell::Did {
            config: "/ws/.metarepo".into(),
            said: "feat/254-pane: merged #7 · deleted the remote branch".into(),
        });
        assert_eq!(more.len(), 1);

        // A worktree with no pull request is offered a push and a pull request.
        app.reading = false;
        app.ui = Ui {
            toggled: ["board:idle".to_string()].into(),
            ..Ui::default()
        };
        app.refresh();
        let at = app
            .rows
            .iter()
            .position(|row| row.text().contains("feat/9-idle"))
            .unwrap();
        app.places.insert(View::Board, (at, 0));
        app.on_key(key(KeyCode::Char('x')));
        let ways: Vec<&str> = app
            .finishing
            .as_ref()
            .unwrap()
            .ways
            .iter()
            .map(|(label, _)| label.as_str())
            .collect();
        assert_eq!(
            ways,
            vec![
                "Push it and open a pull request",
                "Clear up after a merge made elsewhere"
            ]
        );
        app.on_key(key(KeyCode::Esc));
        assert!(app.finishing.is_none());
    }

    #[test]
    fn a_project_github_did_not_answer_for_keeps_what_was_last_seen() {
        let mut app = app();
        // The next read: GitHub answers for every project but claw.
        let mut next = snapshot();
        next.github.failed = vec!["claw".into()];
        next.issues.retain(|issue| issue.project != "claw");
        for item in next.board.iter_mut().filter(|item| item.project == "claw") {
            item.pr = None;
            item.section = crate::plugins::dash::Section::Idle;
        }
        app.on_tell(Tell::Read {
            config: "/ws/.metarepo".into(),
            quick: false,
            result: Ok(Box::new(next)),
        });
        let now = app.snapshot.as_ref().unwrap();
        // Its failing pull request is still on the branch, in the section it earns.
        let branch = now
            .board
            .iter()
            .find(|item| item.branch == "feat/254-pane")
            .unwrap();
        assert_eq!(branch.pr.as_ref().map(|pr| pr.number), Some(7));
        assert_eq!(branch.section, crate::plugins::dash::Section::Fix);
        // Its issues are still listed, once each, and the others are untouched.
        let claw: Vec<u64> = now
            .issues
            .iter()
            .filter(|issue| issue.project == "claw")
            .map(|issue| issue.number)
            .collect();
        assert_eq!(claw.len(), 2);
        assert_eq!(now.issues.len(), 4);
        // And the person is still told which project has no fresh data.
        assert!(screen(&mut app, 120, 20)[19].contains("no GitHub data for claw"));

        // A read where it answers again replaces what was kept.
        let mut fresh = snapshot();
        fresh
            .issues
            .retain(|issue| !(issue.project == "claw" && issue.number == 1));
        app.on_tell(Tell::Read {
            config: "/ws/.metarepo".into(),
            quick: false,
            result: Ok(Box::new(fresh)),
        });
        assert_eq!(app.snapshot.as_ref().unwrap().issues.len(), 3);
    }
}
