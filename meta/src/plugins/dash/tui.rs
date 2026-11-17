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

use super::host::{self, Host};
use super::start::{self, Started, Wanted};
use super::view::{self, Act, Kind, Offer, Row, Span, Tone, Ui, View};
use super::workspaces::{self, Workspace};
use super::{gather, retier, Move, Options, Snapshot};

/// How long a snapshot is shown before it is read again unasked.
const STALE: Duration = Duration::from_secs(120);
const SPIN: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
const KEYS: &str =
    "1-7 view · j/k move · enter open, attach or start · p send a prompt · / filter · r read again · ? help · q quit";

/// What the interface asks of the reader thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// Read the workspace with this config file. A quick read is git and
    /// sessions alone, to have something to show while GitHub is asked.
    Read { config: PathBuf, quick: bool },
    /// Apply tier moves: the labels, the README's Tier column, one commit.
    Retier { config: PathBuf, moves: Vec<Move> },
    /// Create a worktree for an issue and start a session on it.
    Start { config: PathBuf, issue: Wanted },
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
                        "enter again starts {name}: a worktree on {}, and a Claude session {place}",
                        start::branch_for(&issue)
                    );
                    self.armed = Some(name);
                    return Vec::new();
                }
                self.armed = None;
                self.said = format!("creating a worktree on {}…", start::branch_for(&issue));
                vec![Ask::Start {
                    config: self.config.clone(),
                    issue,
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
                result: Ok(snapshot),
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
                self.snapshot = Some(*snapshot);
                self.refresh();
                if quick {
                    // Git and sessions are in: now the slow half.
                    return vec![Ask::Read {
                        config: self.config.clone(),
                        quick: false,
                    }];
                }
                self.reading = false;
                self.read_at = Some(Instant::now());
                Vec::new()
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

    /// Called between events: reads again once what is shown has gone stale.
    pub fn tick(&mut self) -> Vec<Ask> {
        self.frame = self.frame.wrapping_add(1);
        if self.read_at.is_some_and(|at| at.elapsed() >= STALE) {
            return self.read_again();
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
            detail.len().min(6) as u16 + 1
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
                .take(6)
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
        if self.help {
            draw_help(frame);
        }
    }
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
        "/                filter the view (esc clears)",
        "[ ]              the period Cost adds up",
        "r                read the workspace again",
        "q                quit",
        "",
        "Click a tab or a row; click a heading to open it; the wheel scrolls.",
        "Starting work on an issue makes a branch and a worktree, and runs Claude there.",
        "Nothing else here changes a repository.",
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
            Ask::Start { config, issue } => {
                let result = match config.parent() {
                    Some(root) => start::start(&meta, root, host, &issue),
                    None => Err("the workspace has no folder".to_string()),
                };
                Tell::Started { config, result }
            }
            // Attaching and sending are the interface's own to do: one may
            // need the terminal, the other is a moment's work.
            Ask::Attach { .. } | Ask::Send { .. } => continue,
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
            matches!(&asked[..], [Ask::Start { config, issue }] if config == Path::new("/ws/.metarepo") && issue.name() == "mind-7")
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
}
