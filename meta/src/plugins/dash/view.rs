//! What each view of the dashboard shows, as plain rows: no terminal here.
//! The interface in `tui.rs` draws these and acts on what a row carries.

use chrono::{Local, NaiveDate, TimeZone};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

use super::start::Wanted;
use super::workspaces::Workspace;
use super::{Ci, Issue, Item, Project, ProjectState, Section, Session, Snapshot, Tier};

/// The views, in tab order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum View {
    Now,
    Projects,
    Board,
    Issues,
    Sessions,
    Cost,
    Workspaces,
}

impl View {
    pub const ALL: [View; 7] = [
        View::Now,
        View::Projects,
        View::Board,
        View::Issues,
        View::Sessions,
        View::Cost,
        View::Workspaces,
    ];

    pub fn title(self) -> &'static str {
        match self {
            View::Now => "Now",
            View::Projects => "Projects",
            View::Board => "Board",
            View::Issues => "Issues",
            View::Sessions => "Sessions",
            View::Cost => "Cost",
            View::Workspaces => "Workspaces",
        }
    }
}

/// The period the Cost view adds up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Span {
    Today,
    Week,
    Month,
    All,
}

impl Span {
    pub const ALL: [Span; 4] = [Span::Today, Span::Week, Span::Month, Span::All];

    pub fn title(self) -> &'static str {
        match self {
            Span::Today => "Today",
            Span::Week => "7 days",
            Span::Month => "30 days",
            Span::All => "All",
        }
    }

    fn days(self) -> Option<i64> {
        match self {
            Span::Today => Some(1),
            Span::Week => Some(7),
            Span::Month => Some(30),
            Span::All => None,
        }
    }
}

/// How a piece of a row is coloured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Plain,
    Dim,
    Good,
    Warn,
    Bad,
    Accent,
    Title,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Opens and closes what is under it.
    Heading,
    Item,
    /// Not selectable: a label, a total, an empty state.
    Note,
}

/// A prompt ready to send to a session, and what to call it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub label: String,
    pub prompt: String,
}

/// What Enter does on a row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Act {
    /// Open or close the group with this key.
    Fold(String),
    /// Read the workspace with this config file instead.
    Switch(PathBuf),
    /// Bring a Claude session to the front: its short id, name and folder.
    Attach {
        id: String,
        name: String,
        cwd: String,
        /// Prompts worth sending it, from what its branch is doing.
        offers: Vec<Offer>,
    },
    /// Start work on an issue: a worktree, and a session on it.
    Start(Wanted),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub kind: Kind,
    pub indent: u16,
    pub cells: Vec<(String, Tone)>,
    /// Shown under the list while the row is selected.
    pub detail: Vec<String>,
    pub act: Option<Act>,
}

impl Row {
    /// The row as one line of text, without its indent.
    pub fn text(&self) -> String {
        self.cells.iter().map(|(text, _)| text.as_str()).collect()
    }

    pub fn is_selectable(&self) -> bool {
        self.kind != Kind::Note
    }
}

/// What a person has set on the views: the filter, what they opened or
/// closed, and the Cost period.
#[derive(Debug, Clone)]
pub struct Ui {
    pub filter: String,
    /// Groups switched from how they start.
    pub toggled: HashSet<String>,
    pub span: Span,
    /// Whether this machine is still being looked through for workspaces.
    pub looking: bool,
}

impl Default for Ui {
    fn default() -> Self {
        Ui {
            filter: String::new(),
            toggled: HashSet::new(),
            span: Span::Week,
            looking: false,
        }
    }
}

impl Ui {
    /// Groups that start closed: dormant projects, each project's issues,
    /// idle worktrees, and what a project's cost breaks down into.
    fn starts_folded(key: &str) -> bool {
        key == "projects:dormant"
            || key == "board:idle"
            || key.starts_with("cost:")
            || (key.starts_with("issues:") && key != "issues:@urgent")
    }

    /// Whether a group is closed. A filter opens every group, so what it
    /// matches is never hidden.
    pub fn is_folded(&self, key: &str) -> bool {
        self.filter.trim().is_empty() && (Ui::starts_folded(key) != self.toggled.contains(key))
    }
}

/// Whether a text has what was typed, loosely: every word typed must be in
/// it, either whole somewhere or as letters in order within one of its words,
/// so "mrdash" finds metarepo-dashboard and "claw 254" finds that branch.
pub fn fuzzy(typed: &str) -> impl Fn(&str) -> bool {
    let terms: Vec<String> = typed
        .to_lowercase()
        .split_whitespace()
        .map(String::from)
        .collect();
    move |text: &str| {
        let lower = text.to_lowercase();
        terms.iter().all(|term| {
            lower.contains(term.as_str())
                || lower
                    .split(|c: char| c.is_whitespace() || c == '/')
                    .any(|word| {
                        let mut wanted = term.chars().peekable();
                        for letter in word.chars() {
                            if wanted.peek() == Some(&letter) {
                                wanted.next();
                            }
                        }
                        wanted.peek().is_none()
                    })
        })
    }
}

/// $1,204 from a thousand up, $12.40 below.
pub fn money(usd: f64) -> String {
    if usd >= 1000.0 {
        let whole = format!("{}", usd.round() as u64);
        let mut out = String::new();
        for (n, digit) in whole.chars().enumerate() {
            if n > 0 && (whole.len() - n) % 3 == 0 {
                out.push(',');
            }
            out.push(digit);
        }
        format!("${out}")
    } else {
        format!("${usd:.2}")
    }
}

/// 7.4M, 200k, 512.
pub fn compact(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 1000 {
        format!("{}k", (n as f64 / 1e3).round() as u64)
    } else {
        n.to_string()
    }
}

/// claude-fable-5-2 as Fable 5.2; any other id as it is.
pub fn model_name(id: &str) -> String {
    let Some(rest) = id.strip_prefix("claude-") else {
        return id.to_string();
    };
    let mut parts = rest.split('-');
    let family = parts.next().unwrap_or_default();
    let version: Vec<&str> = parts.collect();
    if family.is_empty()
        || version.is_empty()
        || !version
            .iter()
            .all(|p| p.chars().all(|c| c.is_ascii_digit()))
    {
        return id.to_string();
    }
    format!(
        "{}{} {}",
        family[..1].to_uppercase(),
        &family[1..],
        version.join(".")
    )
}

fn today_of(snapshot: &Snapshot) -> NaiveDate {
    Local
        .timestamp_opt(snapshot.at as i64, 0)
        .single()
        .map(|at| at.date_naive())
        .unwrap_or_default()
}

/// How long ago a `YYYY-MM-DD` day was: now, 3d, 2w, 5mo.
pub fn age(day: &str, today: NaiveDate) -> String {
    let Ok(then) = NaiveDate::parse_from_str(day, "%Y-%m-%d") else {
        return String::new();
    };
    let days = (today - then).num_days().max(0);
    match days {
        0 => "now".to_string(),
        1..=13 => format!("{days}d"),
        14..=59 => format!("{}w", days / 7),
        _ => format!("{}mo", days / 30),
    }
}

fn tier_title(tier: Option<Tier>, none: &str) -> String {
    match tier {
        Some(tier) => {
            let name = tier.as_str();
            format!("{}{}", name[..1].to_uppercase(), &name[1..])
        }
        None => none.to_string(),
    }
}

/// A list a tier each, the most at work first, with the name each group goes
/// under: none where the workspace has no tiers, and none where every one
/// listed is active, the tier that needs no saying.
fn in_tiers<T>(
    list: Vec<T>,
    tier_of: impl Fn(&T) -> Option<Tier>,
    has_tiers: bool,
    none: &str,
) -> Vec<(String, Option<Tier>, Vec<T>)> {
    if !has_tiers {
        return vec![(String::new(), None, list)];
    }
    let mut groups: Vec<(Option<Tier>, Vec<T>)> = Tier::ALL
        .into_iter()
        .map(Some)
        .chain([None])
        .map(|tier| (tier, Vec::new()))
        .collect();
    for one in list {
        let tier = tier_of(&one);
        if let Some(group) = groups.iter_mut().find(|(of, _)| *of == tier) {
            group.1.push(one);
        }
    }
    groups.retain(|(_, own)| !own.is_empty());
    // One group of the tierless needs no saying either: there is nothing to tell it from.
    let is_plain = groups.iter().all(|(tier, _)| *tier == Some(Tier::Active))
        || (groups.len() == 1 && groups[0].0.is_none());
    groups
        .into_iter()
        .map(|(tier, own)| {
            let label = if is_plain {
                String::new()
            } else {
                tier_title(tier, none)
            };
            (label, tier, own)
        })
        .collect()
}

fn plural(n: usize, one: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {one}s")
    }
}

/// Builds one view's rows.
struct Out<'a> {
    rows: Vec<Row>,
    ui: &'a Ui,
    snapshot: &'a Snapshot,
    tiers: HashMap<&'a str, Option<Tier>>,
    today: NaiveDate,
}

impl<'a> Out<'a> {
    fn new(snapshot: &'a Snapshot, ui: &'a Ui) -> Self {
        Out {
            rows: Vec::new(),
            ui,
            snapshot,
            tiers: snapshot
                .projects
                .iter()
                .map(|p| (p.name.as_str(), p.tier))
                .collect(),
            today: today_of(snapshot),
        }
    }

    fn tier_of(&self, project: &str) -> Option<Tier> {
        self.tiers.get(project).copied().flatten()
    }

    fn note(&mut self, indent: u16, cells: Vec<(String, Tone)>) {
        self.rows.push(Row {
            kind: Kind::Note,
            indent,
            cells,
            detail: Vec::new(),
            act: None,
        });
    }

    fn gap(&mut self) {
        if !self.rows.is_empty() {
            self.note(0, Vec::new());
        }
    }

    /// A heading that opens and closes; whether it is open.
    fn heading(&mut self, indent: u16, key: &str, title: &str, tone: Tone, count: &str) -> bool {
        let is_open = !self.ui.is_folded(key);
        let mut cells = vec![
            (if is_open { "▾ " } else { "▸ " }.to_string(), Tone::Dim),
            (title.to_string(), tone),
        ];
        if !count.is_empty() {
            cells.push((format!("  {count}"), Tone::Dim));
        }
        self.rows.push(Row {
            kind: Kind::Heading,
            indent,
            cells,
            detail: Vec::new(),
            act: Some(Act::Fold(key.to_string())),
        });
        is_open
    }

    fn item(
        &mut self,
        indent: u16,
        cells: Vec<(String, Tone)>,
        detail: Vec<String>,
        act: Option<Act>,
    ) {
        self.rows.push(Row {
            kind: Kind::Item,
            indent,
            cells,
            detail,
            act,
        });
    }

    /// A dim tag for a project not in the active tier, where tiers are in use.
    fn tier_tag(&self, project: &str) -> Option<(String, Tone)> {
        match self.tier_of(project) {
            Some(Tier::Active) | None => None,
            Some(tier) => Some((format!(" · {}", tier.as_str()), Tone::Dim)),
        }
    }

    fn board_item(&mut self, indent: u16, item: &Item, tagged: bool) {
        let mut cells = vec![
            (item.project.clone(), Tone::Accent),
            (format!(" {}", item.branch), Tone::Plain),
        ];
        let mut detail = Vec::new();
        if !item.path.is_empty() {
            detail.push(format!("worktree  {}", item.path));
        }
        if let Some(pr) = &item.pr {
            let (mark, tone, checks) = match pr.ci {
                Ci::Pass => (" ✓", Tone::Good, "passing"),
                Ci::Fail => (" ✗", Tone::Bad, "failing"),
                Ci::Pending => (" ●", Tone::Warn, "running"),
                Ci::None => ("", Tone::Dim, "none"),
            };
            cells.push((format!("  #{}", pr.number), Tone::Dim));
            cells.push((mark.to_string(), tone));
            if pr.is_draft {
                cells.push((" draft".to_string(), Tone::Dim));
            }
            match pr.review.as_str() {
                "CHANGES_REQUESTED" => cells.push((" changes requested".to_string(), Tone::Bad)),
                "APPROVED" => cells.push((" approved".to_string(), Tone::Good)),
                "REVIEW_REQUIRED" => cells.push((" review required".to_string(), Tone::Dim)),
                _ => {}
            }
            detail.push(format!("pull request  #{} {}", pr.number, pr.title));
            detail.push(format!(
                "checks  {checks}{}",
                if pr.review.is_empty() {
                    String::new()
                } else {
                    format!(" · review {}", pr.review.to_lowercase().replace('_', " "))
                }
            ));
        }
        if item.dirty > 0 {
            cells.push((format!(" *{}", item.dirty), Tone::Warn));
        }
        if item.unpushed > 0 {
            cells.push((format!(" ↑{}", item.unpushed), Tone::Warn));
        }
        if !item.date.is_empty() {
            cells.push((format!("  {}", age(&item.date, self.today)), Tone::Dim));
        }
        if let Some(pr) = &item.pr {
            cells.push((format!("  {}", pr.title), Tone::Dim));
        }
        if tagged {
            cells.extend(self.tier_tag(&item.project));
        }
        if item.dirty > 0 || item.unpushed > 0 {
            detail.push(format!(
                "local  {} uncommitted · {} not pushed{}",
                item.dirty,
                item.unpushed,
                if item.has_upstream {
                    ""
                } else {
                    " · no upstream"
                }
            ));
        }
        let snapshot = self.snapshot;
        let own: Vec<&Session> = snapshot
            .sessions
            .iter()
            .filter(|s| {
                super::sessions::item_of(s, &snapshot.board)
                    .is_some_and(|its| its.key() == item.key())
            })
            .collect();
        for session in &own {
            detail.push(format!(
                "session  {} · {} · enter attaches it",
                session.name, session.status
            ));
        }
        let act = own.first().map(|session| self.attach(session));
        self.item(indent, cells, detail, act);
    }

    fn issue(&mut self, indent: u16, issue: &Issue, with_project: bool, tagged: bool) {
        let mut cells = Vec::new();
        cells.push((
            if issue.in_flight { "● " } else { "· " }.to_string(),
            if issue.in_flight {
                Tone::Good
            } else {
                Tone::Dim
            },
        ));
        cells.push((format!("#{}", issue.number), Tone::Dim));
        if with_project {
            cells.push((format!(" {}", issue.project), Tone::Accent));
        }
        if let Some(rank) = issue.priority {
            cells.push((
                format!(" P{rank}"),
                if rank == 0 {
                    Tone::Bad
                } else if rank == 1 {
                    Tone::Warn
                } else {
                    Tone::Dim
                },
            ));
        }
        cells.push((format!(" {}", issue.title), Tone::Plain));
        if !issue.areas.is_empty() {
            cells.push((format!("  {}", issue.areas.join(" ")), Tone::Dim));
        }
        cells.push((format!("  {}", age(&issue.updated, self.today)), Tone::Dim));
        if tagged {
            cells.extend(self.tier_tag(&issue.project));
        }
        let mut detail = vec![format!(
            "{} #{} {}",
            issue.project, issue.number, issue.title
        )];
        if !issue.labels.is_empty() {
            detail.push(format!("labels  {}", issue.labels.join(", ")));
        }
        detail.push(format!(
            "assigned  {}",
            if issue.assignees.is_empty() {
                "no one".to_string()
            } else {
                issue.assignees.join(", ")
            }
        ));
        detail.push(format!(
            "updated  {}{}",
            issue.updated,
            if issue.in_flight {
                " · a branch on the board works on it"
            } else {
                ""
            }
        ));
        if let Some(slug) = self
            .snapshot
            .projects
            .iter()
            .find(|p| p.name == issue.project)
            .and_then(|p| p.slug.as_ref())
        {
            detail.push(format!("https://github.com/{slug}/issues/{}", issue.number));
        }
        // One branch an issue: what is already in flight is not started
        // again. Enter goes to the session at work on it, where there is one.
        let act = if issue.in_flight {
            self.session_on(issue).map(|session| {
                detail.push(format!(
                    "session  {} · {} · enter attaches it",
                    session.name, session.status
                ));
                self.attach(session)
            })
        } else {
            detail.push(
                "enter starts work on it: a worktree, and a Claude session there".to_string(),
            );
            Some(Act::Start(Wanted {
                project: issue.project.clone(),
                number: issue.number,
                title: issue.title.clone(),
                labels: issue.labels.clone(),
            }))
        };
        self.item(indent, cells, detail, act);
    }

    fn session(&mut self, indent: u16, session: &Session, tagged: bool) {
        let tone = if session.needs_you {
            Tone::Warn
        } else if session.status == "busy" {
            Tone::Accent
        } else {
            Tone::Dim
        };
        let mut cells = vec![
            (session.name.clone(), Tone::Plain),
            (format!("  {}", session.status), tone),
        ];
        if !session.project.is_empty() {
            let branch = if session.branch.is_empty() {
                String::new()
            } else {
                format!(" {}", session.branch)
            };
            cells.push((format!("  {}{branch}", session.project), Tone::Dim));
        } else if session.place == super::Place::Root {
            cells.push(("  workspace root".to_string(), Tone::Dim));
        }
        let cost = self.snapshot.cost.as_ref().and_then(|cost| {
            cost.sessions
                .iter()
                .find(|one| one.session_id == session.session_id)
        });
        if let Some(cost) = cost {
            cells.push((format!("  {}", money(cost.usd)), Tone::Dim));
        }
        if tagged {
            cells.extend(self.tier_tag(&session.project));
        }
        let mut detail = vec![
            format!(
                "{} session {} · {}",
                session.kind, session.id, session.status
            ),
            format!("folder  {}", session.cwd),
            format!("attach  claude attach {} · enter does it", session.id),
        ];
        if let Some(cost) = cost {
            detail.push(format!(
                "cost  {} · {} tokens",
                money(cost.usd),
                compact(cost.tokens)
            ));
        }
        let act = self.attach(session);
        self.item(indent, cells, detail, Some(act));
    }

    /// Prompts worth sending the session on a branch: about its failing
    /// checks, its review, and the issue its name leads with. Each is short
    /// and says where to look, so it fits what a host will paste.
    fn offers(&self, item: &Item) -> Vec<Offer> {
        let slug = self
            .snapshot
            .projects
            .iter()
            .find(|p| p.name == item.project)
            .and_then(|p| p.slug.clone())
            .unwrap_or_else(|| item.project.clone());
        let mut offers = Vec::new();
        if let Some(pr) = &item.pr {
            let n = pr.number;
            if pr.ci == Ci::Fail {
                offers.push(Offer {
                    label: format!("Fix the failing checks on #{n}"),
                    prompt: format!(
                        "The checks on pull request #{n} of {slug} (branch {}) are failing. Find out why (gh pr checks {n} -R {slug}, then gh run view --log-failed for the run that failed), fix it in this worktree and commit. Do not push.",
                        item.branch
                    ),
                });
            }
            offers.push(Offer {
                label: format!("Address the review on #{n}"),
                prompt: format!(
                    "Pull request #{n} of {slug} (branch {}) has review comments. Read them (gh pr view {n} -R {slug} --comments, and gh api repos/{slug}/pulls/{n}/comments for the ones on lines), address each in this worktree and commit. Do not push.",
                    item.branch
                ),
            });
        }
        if let Some(number) = super::github::issue_of_branch(&item.branch) {
            let title = self
                .snapshot
                .issues
                .iter()
                .find(|i| i.project == item.project && i.number == number)
                .map(|i| format!(": {}", i.title))
                .unwrap_or_default();
            offers.push(Offer {
                label: format!("Check the work against issue #{number}"),
                prompt: format!(
                    "Read GitHub issue #{number} of {slug} again (gh issue view {number} -R {slug} --comments){title}. Check that the work on this branch covers all of it, and say what is left."
                ),
            });
        }
        offers
    }

    /// Enter on a session's row attaches it; with it go the prompts worth
    /// sending it.
    fn attach(&self, session: &Session) -> Act {
        let offers = super::sessions::item_of(session, &self.snapshot.board)
            .map(|item| self.offers(item))
            .unwrap_or_default();
        Act::Attach {
            id: session.id.clone(),
            name: session.name.clone(),
            cwd: session.cwd.clone(),
            offers,
        }
    }

    /// The session at work on an issue, by the branch named for it.
    fn session_on(&self, issue: &Issue) -> Option<&'a Session> {
        let snapshot = self.snapshot;
        let item = snapshot.board.iter().find(|item| {
            item.project == issue.project
                && super::github::issue_of_branch(&item.branch) == Some(issue.number)
        })?;
        snapshot.sessions.iter().find(|session| {
            super::sessions::item_of(session, &snapshot.board)
                .is_some_and(|its| its.key() == item.key())
        })
    }

    /// Lists under a heading, a tier at a time where tiers need saying.
    fn tiered<T>(
        &mut self,
        list: Vec<T>,
        project_of: impl Fn(&T) -> &str,
        none: &str,
        mut each: impl FnMut(&mut Self, u16, &T),
    ) {
        let groups = in_tiers(
            list,
            |one| self.tier_of(project_of(one)),
            self.snapshot.has_tiers,
            none,
        );
        for (label, _, own) in groups {
            let indent = if label.is_empty() {
                1
            } else {
                self.note(1, vec![(label, Tone::Dim)]);
                2
            };
            for one in &own {
                each(self, indent, one);
            }
        }
    }
}

fn project_row(out: &mut Out, project: &Project) {
    let snapshot = out.snapshot;
    let mut cells = vec![(project.name.clone(), Tone::Plain)];
    match project.state {
        ProjectState::Ok => {
            let tone = if project.is_default_branch {
                Tone::Dim
            } else {
                Tone::Accent
            };
            cells.push((format!("  {}", project.branch), tone));
            if project.ahead > 0 {
                cells.push((format!(" ↑{}", project.ahead), Tone::Warn));
            }
            if project.behind > 0 {
                cells.push((format!(" ↓{}", project.behind), Tone::Warn));
            }
            if project.dirty > 0 {
                cells.push((format!(" *{}", project.dirty), Tone::Warn));
            }
        }
        ProjectState::Missing => cells.push(("  missing".to_string(), Tone::Bad)),
        ProjectState::NotGit => cells.push(("  not a git repo".to_string(), Tone::Bad)),
        ProjectState::NoWorktree => cells.push(("  no main worktree".to_string(), Tone::Bad)),
    }
    let flying = snapshot
        .board
        .iter()
        .filter(|i| i.project == project.name)
        .count();
    if flying > 0 {
        cells.push((format!("  {flying} in flight"), Tone::Accent));
    }
    let failing = snapshot
        .board
        .iter()
        .filter(|i| i.project == project.name && i.section == Section::Fix)
        .count();
    if failing > 0 {
        cells.push((format!(" · {failing} failing"), Tone::Bad));
    }
    let open = snapshot
        .issues
        .iter()
        .filter(|i| i.project == project.name)
        .count();
    if open > 0 {
        cells.push((format!("  {}", plural(open, "issue")), Tone::Dim));
    }
    // A month without a commit is worth a word; less is not.
    if project.quiet_days >= 30 {
        cells.push((format!("  quiet {}d", project.quiet_days), Tone::Dim));
    }
    if project.tier_due != project.tier {
        if let Some(due) = project.tier_due {
            cells.push((format!("  due {}", due.as_str()), Tone::Warn));
        }
    }
    let mut detail = vec![format!(
        "{}  {}",
        project.name,
        project
            .slug
            .as_ref()
            .map_or("not on GitHub".to_string(), |slug| format!(
                "github.com/{slug}"
            ))
    )];
    detail.push(format!(
        "tier  {}{}",
        project.tier.map_or("none", Tier::as_str),
        match (project.tier, project.tier_due) {
            (from, Some(due)) if from != Some(due) =>
                format!(" · due {} (meta dash --retier)", due.as_str()),
            _ => String::new(),
        }
    ));
    detail.push(format!(
        "last commit  {} days ago · work in flight {} · {} other remote branches",
        project.quiet_days, project.work, project.remote_branches
    ));
    detail.push(format!(
        "main worktree  {} · {} ahead · {} behind · {} uncommitted{}",
        project.branch,
        project.ahead,
        project.behind,
        project.dirty,
        if project.bare { " · bare clone" } else { "" }
    ));
    out.item(1, cells, detail, None);
}

fn projects(out: &mut Out) {
    let snapshot = out.snapshot;
    let mut list: Vec<&Project> = snapshot.projects.iter().collect();
    list.sort_by_key(|project| project.name.to_lowercase());
    if !snapshot.has_tiers {
        for project in list {
            project_row(out, project);
        }
        return;
    }
    for tier in Tier::ALL.into_iter().map(Some).chain([None]) {
        let own: Vec<&Project> = list.iter().copied().filter(|p| p.tier == tier).collect();
        if own.is_empty() {
            continue;
        }
        out.gap();
        let key = format!("projects:{}", tier.map_or("none", Tier::as_str));
        if out.heading(
            0,
            &key,
            &tier_title(tier, "No tier"),
            Tone::Title,
            &own.len().to_string(),
        ) {
            for project in own {
                project_row(out, project);
            }
        }
    }
}

const SECTIONS: [(Section, &str, Tone); 7] = [
    (Section::Attention, "Waiting on you", Tone::Warn),
    (Section::Fix, "Needs fixing", Tone::Bad),
    (Section::Working, "Agents working", Tone::Accent),
    (Section::Local, "Not pushed", Tone::Warn),
    (Section::Ready, "Ready", Tone::Good),
    (Section::Waiting, "Waiting", Tone::Title),
    (Section::Idle, "Idle worktrees", Tone::Title),
];

fn board(out: &mut Out) {
    let snapshot = out.snapshot;
    for (section, title, tone) in SECTIONS {
        let own: Vec<&Item> = snapshot
            .board
            .iter()
            .filter(|i| i.section == section)
            .collect();
        if own.is_empty() {
            continue;
        }
        out.gap();
        let key = format!("board:{}", format!("{section:?}").to_lowercase());
        if out.heading(0, &key, title, tone, &own.len().to_string()) {
            out.tiered(
                own,
                |item| item.project.as_str(),
                "No tier",
                |out, indent, item| out.board_item(indent, item, false),
            );
        }
    }
    if snapshot.board.is_empty() {
        out.note(0, vec![("Nothing in flight.".to_string(), Tone::Dim)]);
    }
    if snapshot.at_rest > 0 {
        out.gap();
        out.note(
            0,
            vec![(
                format!("{} at rest", plural(snapshot.at_rest as usize, "project")),
                Tone::Dim,
            )],
        );
    }
}

fn is_urgent(issue: &Issue) -> bool {
    issue.priority.is_some_and(|rank| rank <= 1)
}

fn by_priority(a: &&Issue, b: &&Issue) -> std::cmp::Ordering {
    a.priority
        .unwrap_or(9)
        .cmp(&b.priority.unwrap_or(9))
        .then(b.updated_at.cmp(&a.updated_at))
        .then(b.number.cmp(&a.number))
}

/// Urgent issues from every project: by priority, then tier, then newest.
fn urgent<'a>(out: &Out<'a>) -> Vec<&'a Issue> {
    let mut list: Vec<&Issue> = out
        .snapshot
        .issues
        .iter()
        .filter(|i| is_urgent(i))
        .collect();
    list.sort_by(|a, b| {
        let tier = |i: &Issue| out.tier_of(&i.project).map_or(3, |t| t as u8);
        a.priority
            .cmp(&b.priority)
            .then(tier(a).cmp(&tier(b)))
            .then(b.updated_at.cmp(&a.updated_at))
    });
    list
}

fn issues(out: &mut Out) {
    let snapshot = out.snapshot;
    if !snapshot.github.asked {
        out.note(
            0,
            vec![("GitHub was not asked (--no-github).".to_string(), Tone::Dim)],
        );
        return;
    }
    if snapshot.issues.is_empty() {
        out.note(0, vec![("No open issues.".to_string(), Tone::Dim)]);
        return;
    }
    let pressing = urgent(out);
    if !pressing.is_empty()
        && out.heading(
            0,
            "issues:@urgent",
            "Urgent",
            Tone::Bad,
            &pressing.len().to_string(),
        )
    {
        for issue in pressing {
            out.issue(1, issue, true, true);
        }
    }
    let mut by_project: BTreeMap<&str, Vec<&Issue>> = BTreeMap::new();
    for issue in &snapshot.issues {
        by_project
            .entry(issue.project.as_str())
            .or_default()
            .push(issue);
    }
    let names: Vec<&str> = by_project.keys().copied().collect();
    let groups = in_tiers(
        names,
        |name| out.tier_of(name),
        snapshot.has_tiers,
        "No tier",
    );
    for (label, _, names) in groups {
        out.gap();
        let total: usize = names.iter().map(|name| by_project[name].len()).sum();
        let title = if label.is_empty() {
            "Projects".to_string()
        } else {
            label
        };
        out.note(
            0,
            vec![
                (title, Tone::Title),
                (
                    format!(
                        "  {} · {}",
                        plural(names.len(), "project"),
                        plural(total, "issue")
                    ),
                    Tone::Dim,
                ),
            ],
        );
        for name in names {
            let mut own = by_project[name].clone();
            own.sort_by(by_priority);
            let pressing = own.iter().filter(|i| is_urgent(i)).count();
            let count = if pressing > 0 {
                format!("{} · {pressing} urgent", own.len())
            } else {
                own.len().to_string()
            };
            if out.heading(1, &format!("issues:{name}"), name, Tone::Accent, &count) {
                for issue in own {
                    out.issue(2, issue, false, false);
                }
            }
        }
    }
}

fn sessions(out: &mut Out) {
    let snapshot = out.snapshot;
    if snapshot.sessions.is_empty() {
        out.note(0, vec![("No Claude sessions.".to_string(), Tone::Dim)]);
        return;
    }
    type Group = (&'static str, &'static str, Tone, fn(&Session) -> bool);
    let groups: [Group; 4] = [
        ("asking", "Waiting on you", Tone::Warn, |s| s.needs_you),
        ("busy", "Working", Tone::Accent, |s| s.status == "busy"),
        ("idle", "Idle", Tone::Title, |s| s.status == "idle"),
        ("done", "Done", Tone::Title, |s| s.status == "done"),
    ];
    for (id, title, tone, is_own) in groups {
        let own: Vec<&Session> = snapshot.sessions.iter().filter(|s| is_own(s)).collect();
        if own.is_empty() {
            continue;
        }
        out.gap();
        if out.heading(
            0,
            &format!("sessions:{id}"),
            title,
            tone,
            &own.len().to_string(),
        ) {
            out.tiered(
                own,
                |s| s.project.as_str(),
                "Other",
                |out, indent, s| out.session(indent, s, false),
            );
        }
    }
}

/// Now is a glance: this many urgent issues, and a pointer to the rest.
const URGENT_ON_NOW: usize = 8;

fn now(out: &mut Out) {
    let snapshot = out.snapshot;
    let asking: Vec<&Session> = snapshot.sessions.iter().filter(|s| s.needs_you).collect();
    let busy: Vec<&Session> = snapshot
        .sessions
        .iter()
        .filter(|s| s.status == "busy")
        .collect();
    let of = |section: Section| -> Vec<&Item> {
        snapshot
            .board
            .iter()
            .filter(|i| i.section == section)
            .collect()
    };
    let pressing = urgent(out);

    if !asking.is_empty() {
        out.heading(
            0,
            "now:asking",
            "Waiting on you",
            Tone::Warn,
            &asking.len().to_string(),
        );
        out.tiered(
            asking,
            |s| s.project.as_str(),
            "Other",
            |out, indent, s| out.session(indent, s, false),
        );
    }
    let fixing = of(Section::Fix);
    if !fixing.is_empty() {
        out.gap();
        out.heading(
            0,
            "now:fix",
            "Needs fixing",
            Tone::Bad,
            &fixing.len().to_string(),
        );
        out.tiered(
            fixing,
            |i| i.project.as_str(),
            "No tier",
            |out, indent, i| out.board_item(indent, i, false),
        );
    }
    if !pressing.is_empty() {
        out.gap();
        out.heading(
            0,
            "now:urgent",
            "Urgent issues",
            Tone::Bad,
            &pressing.len().to_string(),
        );
        // Priority wins over tier here: one list, each row tagged with its tier.
        let more = pressing.len().saturating_sub(URGENT_ON_NOW);
        for issue in pressing.into_iter().take(URGENT_ON_NOW) {
            out.issue(1, issue, true, true);
        }
        if more > 0 {
            out.note(1, vec![(format!("… and {more} more on Issues"), Tone::Dim)]);
        }
    }
    if !busy.is_empty() {
        out.gap();
        out.heading(
            0,
            "now:busy",
            "Agents working",
            Tone::Accent,
            &busy.len().to_string(),
        );
        out.tiered(
            busy,
            |s| s.project.as_str(),
            "Other",
            |out, indent, s| out.session(indent, s, false),
        );
    }
    for (section, id, title, tone) in [
        (Section::Local, "local", "Not pushed", Tone::Warn),
        (Section::Ready, "ready", "Ready", Tone::Good),
    ] {
        let own = of(section);
        if !own.is_empty() {
            out.gap();
            out.heading(0, &format!("now:{id}"), title, tone, &own.len().to_string());
            out.tiered(
                own,
                |i| i.project.as_str(),
                "No tier",
                |out, indent, i| out.board_item(indent, i, false),
            );
        }
    }
    if out.rows.is_empty() {
        out.note(
            0,
            vec![("Nothing needs you right now.".to_string(), Tone::Good)],
        );
    }
    // The Now headings only label: they do not fold.
    for row in &mut out.rows {
        if row.kind == Kind::Heading {
            row.kind = Kind::Note;
            row.act = None;
            row.cells.remove(0);
        }
    }
}

/// Values as one row of block characters, the tallest filling the cell.
fn spark(values: &[f64]) -> String {
    let top = values.iter().copied().fold(0.0, f64::max);
    values
        .iter()
        .map(|value| {
            let at = if top <= 0.0 {
                0
            } else {
                ((value / top) * 7.0).round() as usize
            };
            ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'][at.min(7)]
        })
        .collect()
}

fn bar(share: f64) -> String {
    let eighths = (share.clamp(0.0, 1.0) * 64.0).round() as usize;
    if eighths == 0 {
        return "▏".to_string();
    }
    let mut out = "█".repeat(eighths / 8);
    if !eighths.is_multiple_of(8) {
        out.push(['▏', '▎', '▍', '▌', '▋', '▊', '▉'][eighths % 8 - 1]);
    }
    out
}

fn cost(out: &mut Out) {
    let snapshot = out.snapshot;
    let Some(cost) = &snapshot.cost else {
        out.note(0, vec![("Cost was not read.".to_string(), Tone::Dim)]);
        return;
    };
    let span = out.ui.span;
    let mut tiles = Vec::new();
    for one in Span::ALL {
        let tone = if one == span { Tone::Title } else { Tone::Dim };
        tiles.push((
            format!("{}{} ", if one == span { "● " } else { "  " }, one.title()),
            tone,
        ));
        tiles.push((
            format!("{}    ", money(cost.usd_over(one.days()))),
            if one == span { Tone::Accent } else { Tone::Dim },
        ));
    }
    out.note(0, tiles);

    let today = NaiveDate::parse_from_str(&cost.today, "%Y-%m-%d").unwrap_or_default();
    let since = span.days().map(|days| {
        (today - chrono::Duration::days(days - 1))
            .format("%Y-%m-%d")
            .to_string()
    });
    let in_span = |day: &str| {
        since
            .as_deref()
            .is_none_or(|since| day >= since && day <= cost.today.as_str())
    };

    let days: Vec<f64> = (0..14)
        .rev()
        .map(|back| {
            let day = (today - chrono::Duration::days(back))
                .format("%Y-%m-%d")
                .to_string();
            cost.rows
                .iter()
                .filter(|row| row.day == day)
                .map(|row| row.usd)
                .sum()
        })
        .collect();
    out.note(
        0,
        vec![
            (spark(&days), Tone::Accent),
            (
                format!(
                    "  a day, last 14 days · peak {} · average {}",
                    money(days.iter().copied().fold(0.0, f64::max)),
                    money(days.iter().sum::<f64>() / 14.0)
                ),
                Tone::Dim,
            ),
        ],
    );

    // By place: a project, the workspace root, or everything outside as one.
    #[derive(Default)]
    struct Sum {
        usd: f64,
        tokens: u64,
        parts: BTreeMap<String, (f64, u64)>,
    }
    let mut places: BTreeMap<String, Sum> = BTreeMap::new();
    let mut models: BTreeMap<&str, (f64, u64)> = BTreeMap::new();
    let mut total = 0.0;
    for row in cost.rows.iter().filter(|row| in_span(&row.day)) {
        let tokens = row.input + row.output + row.cache_read + row.cache_write;
        let (place, part) = match row.bucket.strip_prefix("@out:") {
            Some(folder) => ("@out".to_string(), folder.to_string()),
            None => (row.bucket.clone(), model_name(&row.model)),
        };
        let sum = places.entry(place).or_default();
        sum.usd += row.usd;
        sum.tokens += tokens;
        let part = sum.parts.entry(part).or_default();
        part.0 += row.usd;
        part.1 += tokens;
        let model = models.entry(row.model.as_str()).or_default();
        model.0 += row.usd;
        model.1 += tokens;
        total += row.usd;
    }
    out.gap();
    if places.is_empty() {
        out.note(
            0,
            vec![(
                format!("Nothing spent in {}.", span.title().to_lowercase()),
                Tone::Dim,
            )],
        );
        return;
    }
    out.note(0, vec![("By project".to_string(), Tone::Title)]);
    let top = places.values().map(|sum| sum.usd).fold(0.0, f64::max);
    let mut names: Vec<String> = places.keys().cloned().collect();
    names.sort_by(|a, b| places[b].usd.total_cmp(&places[a].usd));
    let tier_of = |name: &String| {
        if name.starts_with('@') {
            None
        } else {
            out.tier_of(name)
        }
    };
    let has_tiers = snapshot.has_tiers;
    for (label, _, names) in in_tiers(names, tier_of, has_tiers, "Other") {
        let (mut indent, usd) = (1, names.iter().map(|name| places[name].usd).sum::<f64>());
        if !label.is_empty() {
            let share = if total > 0.0 {
                (usd / total * 100.0).round()
            } else {
                0.0
            };
            out.note(
                1,
                vec![
                    (label, Tone::Title),
                    (format!("  {} · {share}%", money(usd)), Tone::Dim),
                ],
            );
            indent = 2;
        }
        for name in names {
            let sum = &places[&name];
            let shown = match name.as_str() {
                "@root" => "workspace root",
                "@out" => "outside this workspace",
                other => other,
            };
            let key = format!("cost:{name}");
            let is_open = !out.ui.is_folded(&key);
            out.rows.push(Row {
                kind: Kind::Heading,
                indent,
                cells: vec![
                    (if is_open { "▾ " } else { "▸ " }.to_string(), Tone::Dim),
                    (format!("{:>9}  ", money(sum.usd)), Tone::Plain),
                    (
                        format!("{:<8} ", bar(if top > 0.0 { sum.usd / top } else { 0.0 })),
                        Tone::Accent,
                    ),
                    (format!("{:>6}  ", compact(sum.tokens)), Tone::Dim),
                    (shown.to_string(), Tone::Plain),
                ],
                detail: Vec::new(),
                act: Some(Act::Fold(key)),
            });
            if is_open {
                let mut parts: Vec<(&String, &(f64, u64))> = sum.parts.iter().collect();
                parts.sort_by(|a, b| b.1 .0.total_cmp(&a.1 .0));
                for (part, (usd, tokens)) in parts {
                    out.note(
                        indent + 2,
                        vec![
                            (
                                format!("{:>9}  {:>6}  ", money(*usd), compact(*tokens)),
                                Tone::Dim,
                            ),
                            (part.clone(), Tone::Plain),
                        ],
                    );
                }
            }
        }
    }

    out.gap();
    out.note(0, vec![("By model".to_string(), Tone::Title)]);
    let mut by_model: Vec<(&str, (f64, u64))> = models.into_iter().collect();
    by_model.sort_by(|a, b| b.1 .0.total_cmp(&a.1 .0));
    for (model, (usd, tokens)) in by_model {
        let share = if total > 0.0 {
            (usd / total * 100.0).round()
        } else {
            0.0
        };
        out.note(
            1,
            vec![
                (
                    format!("{:>9}  {:>6}  ", money(usd), compact(tokens)),
                    Tone::Dim,
                ),
                (model_name(model), Tone::Plain),
                (format!("  {share}%"), Tone::Dim),
            ],
        );
    }

    let mut dearest: Vec<_> = cost.sessions.iter().collect();
    dearest.sort_by(|a, b| b.usd.total_cmp(&a.usd));
    out.gap();
    out.note(
        0,
        vec![
            ("Most expensive sessions".to_string(), Tone::Title),
            ("  all time".to_string(), Tone::Dim),
        ],
    );
    for one in dearest.into_iter().take(5) {
        let name = snapshot
            .sessions
            .iter()
            .find(|s| s.session_id == one.session_id)
            .map_or_else(
                || one.session_id.chars().take(8).collect(),
                |s| s.name.clone(),
            );
        out.note(
            1,
            vec![
                (
                    format!("{:>9}  {:>6}  ", money(one.usd), compact(one.tokens)),
                    Tone::Dim,
                ),
                (name, Tone::Plain),
            ],
        );
    }
    if !cost.unpriced.is_empty() {
        out.gap();
        out.note(
            0,
            vec![(
                format!("No price listed for {}", cost.unpriced.join(", ")),
                Tone::Warn,
            )],
        );
    }
}

fn workspaces(out: &mut Out, found: Option<&[Workspace]>) {
    let snapshot = out.snapshot;
    let Some(found) = found else {
        out.note(0, vec![("Looking for workspaces…".to_string(), Tone::Dim)]);
        return;
    };
    let here = PathBuf::from(&snapshot.root);
    let mut list: Vec<&Workspace> = found.iter().collect();
    list.sort_by_key(|w| (w.root != here, w.root.clone()));
    for workspace in list {
        let is_here = workspace.root == here;
        out.gap();
        out.rows.push(Row {
            kind: Kind::Item,
            indent: 0,
            cells: vec![
                (
                    if is_here { "● " } else { "○ " }.to_string(),
                    if is_here { Tone::Good } else { Tone::Dim },
                ),
                (workspace.name.clone(), Tone::Title),
                (
                    format!("  {}", plural(workspace.projects, "project")),
                    Tone::Dim,
                ),
                (format!("  {}", workspace.root.display()), Tone::Dim),
            ],
            detail: vec![
                workspace.root.display().to_string(),
                if is_here {
                    "The workspace shown.".to_string()
                } else {
                    "Enter reads this workspace instead.".to_string()
                },
            ],
            act: (!is_here).then(|| Act::Switch(workspace.config.clone())),
        });
        for (n, tier) in Tier::ALL.into_iter().enumerate() {
            if workspace.tiers[n] == 0 {
                continue;
            }
            let mut cells = vec![
                (format!("{:<11}", tier_title(Some(tier), "")), Tone::Plain),
                (plural(workspace.tiers[n], "project"), Tone::Dim),
            ];
            if is_here {
                let own = |project: &str| out.tier_of(project) == Some(tier);
                let flying = snapshot.board.iter().filter(|i| own(&i.project)).count();
                let failing = snapshot
                    .board
                    .iter()
                    .filter(|i| own(&i.project) && i.section == Section::Fix)
                    .count();
                let open = snapshot.issues.iter().filter(|i| own(&i.project)).count();
                let pressing = snapshot
                    .issues
                    .iter()
                    .filter(|i| own(&i.project) && is_urgent(i))
                    .count();
                if flying > 0 {
                    cells.push((format!(" · {flying} in progress"), Tone::Dim));
                }
                if failing > 0 {
                    cells.push((format!(" · {failing} failing"), Tone::Bad));
                }
                if open > 0 {
                    cells.push((format!(" · {}", plural(open, "issue")), Tone::Dim));
                }
                if pressing > 0 {
                    cells.push((format!(" · {pressing} urgent"), Tone::Warn));
                }
            }
            out.note(2, cells);
        }
    }
    if out.ui.looking {
        out.gap();
        out.note(0, vec![("Still looking for more…".to_string(), Tone::Dim)]);
    } else if found.is_empty() {
        out.note(
            0,
            vec![(
                "No workspaces found near the home folder.".to_string(),
                Tone::Dim,
            )],
        );
    }
}

/// Keep what the filter matches: items by their own text, and a heading when
/// anything under it is kept. Labels and totals go while a filter is on.
fn filtered(rows: Vec<Row>, typed: &str) -> Vec<Row> {
    let hits = fuzzy(typed);
    let keep: Vec<bool> = rows
        .iter()
        .enumerate()
        .map(|(at, row)| match row.kind {
            Kind::Item => hits(&row.text()),
            Kind::Note => false,
            Kind::Heading => rows[at + 1..]
                .iter()
                // What is under a heading ends at the next heading or label no deeper than it.
                .take_while(|under| under.kind == Kind::Item || under.indent > row.indent)
                .any(|under| under.kind == Kind::Item && hits(&under.text())),
        })
        .collect();
    rows.into_iter()
        .zip(keep)
        .filter_map(|(row, keep)| keep.then_some(row))
        .collect()
}

/// One view's rows for a snapshot, as a person has the views set.
pub fn rows(view: View, snapshot: &Snapshot, ui: &Ui, found: Option<&[Workspace]>) -> Vec<Row> {
    let mut out = Out::new(snapshot, ui);
    match view {
        View::Now => now(&mut out),
        View::Projects => projects(&mut out),
        View::Board => board(&mut out),
        View::Issues => issues(&mut out),
        View::Sessions => sessions(&mut out),
        View::Cost => cost(&mut out),
        View::Workspaces => workspaces(&mut out, found),
    }
    let typed = ui.filter.trim();
    // Cost is sums, not a list to search.
    if typed.is_empty() || view == View::Cost {
        return out.rows;
    }
    let kept = filtered(out.rows, typed);
    if kept.is_empty() {
        return vec![Row {
            kind: Kind::Note,
            indent: 0,
            cells: vec![(format!("Nothing matches \"{typed}\"."), Tone::Dim)],
            detail: Vec::new(),
            act: None,
        }];
    }
    kept
}

/// How many things a view holds, for its tab; `None` where a count says little.
pub fn count(view: View, snapshot: &Snapshot) -> Option<usize> {
    match view {
        View::Now => Some(
            snapshot.sessions.iter().filter(|s| s.needs_you).count()
                + snapshot
                    .board
                    .iter()
                    .filter(|i| i.section == Section::Fix)
                    .count()
                + snapshot.issues.iter().filter(|i| is_urgent(i)).count(),
        ),
        View::Projects => Some(snapshot.projects.len()),
        View::Board => Some(snapshot.board.len()),
        View::Issues => snapshot.github.asked.then_some(snapshot.issues.len()),
        View::Sessions => Some(snapshot.sessions.len()),
        View::Cost | View::Workspaces => None,
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::plugins::dash::{Cost, CostRow, GitHub, Place, PullRequest, SessionCost};

    fn project(name: &str, tier: Option<Tier>) -> Project {
        Project {
            name: name.into(),
            state: ProjectState::Ok,
            tier,
            tier_due: tier,
            branch: "main".into(),
            default_branch: "main".into(),
            is_default_branch: true,
            ahead: 0,
            behind: 0,
            dirty: 0,
            remote_branches: 0,
            bare: true,
            quiet_days: 3,
            work: 0,
            slug: Some(format!("org/{name}")),
        }
    }

    fn item(project: &str, branch: &str, section: Section, ci: Option<Ci>) -> Item {
        Item {
            project: project.into(),
            branch: branch.into(),
            path: format!("{project}/{branch}"),
            is_default: false,
            dirty: 0,
            unpushed: 0,
            has_upstream: true,
            date: "2026-11-10".into(),
            pr: ci.map(|ci| PullRequest {
                number: 7,
                title: "feat: a thing".into(),
                is_draft: false,
                review: String::new(),
                ci,
            }),
            section,
        }
    }

    fn issue(project: &str, number: u64, priority: Option<u8>, title: &str) -> Issue {
        Issue {
            project: project.into(),
            number,
            title: title.into(),
            labels: Vec::new(),
            assignees: Vec::new(),
            updated: "2026-11-10".into(),
            updated_at: format!("2026-11-10T10:00:{:02}Z", number % 60),
            priority,
            areas: Vec::new(),
            in_flight: false,
        }
    }

    fn session(name: &str, status: &str, project: &str) -> Session {
        Session {
            id: name.chars().take(8).collect(),
            session_id: format!("{name}-0000"),
            name: name.into(),
            kind: "background".into(),
            status: status.into(),
            needs_you: !matches!(status, "busy" | "idle" | "done"),
            started_at: 0,
            cwd: format!("/ws/{project}"),
            project: project.into(),
            branch: String::new(),
            place: Place::In,
        }
    }

    /// A workspace with a project in each tier and something in every view.
    pub fn snapshot() -> Snapshot {
        Snapshot {
            schema: 1,
            root: "/ws".into(),
            // 2026-11-12 12:00 UTC: the same day in every time zone tests run in.
            at: 1_794_484_800,
            has_tiers: true,
            projects: vec![
                project("claw", Some(Tier::Active)),
                project("earth", Some(Tier::Maintained)),
                project("mind", Some(Tier::Dormant)),
                project("loose", None),
            ],
            board: vec![
                item("claw", "feat/254-pane", Section::Fix, Some(Ci::Fail)),
                item("claw", "feat/9-idle", Section::Idle, None),
                item("earth", "fix/3-globe", Section::Ready, Some(Ci::Pass)),
            ],
            at_rest: 2,
            issues: vec![
                issue("claw", 1, None, "Tidy the logs"),
                issue("claw", 2, Some(1), "Crash on start"),
                issue("mind", 7, Some(0), "Data loss on save"),
                issue("earth", 4, Some(3), "Slow globe"),
            ],
            sessions: vec![
                session("claw-254", "busy", "claw"),
                session("earth-3", "permission", "earth"),
                session("old", "done", ""),
            ],
            cost: Some(Cost {
                today: "2026-11-12".into(),
                rows: vec![
                    CostRow {
                        bucket: "claw".into(),
                        day: "2026-11-12".into(),
                        model: "claude-fable-5-2".into(),
                        input: 1000,
                        output: 200_000,
                        cache_read: 5_000_000,
                        cache_write: 100_000,
                        usd: 10.0,
                        priced: true,
                    },
                    CostRow {
                        bucket: "claw".into(),
                        day: "2026-10-01".into(),
                        model: "claude-sonnet-5-5".into(),
                        input: 0,
                        output: 100_000,
                        cache_read: 0,
                        cache_write: 0,
                        usd: 1500.0,
                        priced: true,
                    },
                    CostRow {
                        bucket: "@root".into(),
                        day: "2026-11-11".into(),
                        model: "claude-fable-5-2".into(),
                        input: 0,
                        output: 1000,
                        cache_read: 0,
                        cache_write: 0,
                        usd: 3.0,
                        priced: true,
                    },
                    CostRow {
                        bucket: "@out:scratch".into(),
                        day: "2026-11-12".into(),
                        model: "claude-mystery-9".into(),
                        input: 0,
                        output: 500,
                        cache_read: 0,
                        cache_write: 0,
                        usd: 0.0,
                        priced: false,
                    },
                ],
                sessions: vec![SessionCost {
                    session_id: "claw-254-0000".into(),
                    usd: 2.5,
                    tokens: 9000,
                }],
                unpriced: vec!["claude-mystery-9".into()],
            }),
            retier: Vec::new(),
            github: GitHub {
                asked: true,
                failed: Vec::new(),
            },
            notes: Vec::new(),
        }
    }

    fn texts(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|row| format!("{}{}", "  ".repeat(row.indent as usize), row.text()))
            .collect()
    }

    fn at(lines: &[String], needle: &str) -> usize {
        lines
            .iter()
            .position(|line| line.contains(needle))
            .unwrap_or_else(|| panic!("no {needle:?} in {lines:#?}"))
    }

    #[test]
    fn projects_are_grouped_by_tier_with_dormant_closed() {
        let (snap, mut ui) = (snapshot(), Ui::default());
        let lines = texts(&rows(View::Projects, &snap, &ui, None));
        assert!(at(&lines, "▾ Active  1") < at(&lines, "▾ Maintained  1"));
        assert!(at(&lines, "▸ Dormant  1") < at(&lines, "▾ No tier  1"));
        assert!(lines.iter().any(|l| l.contains("claw  main")
            && l.contains("2 in flight · 1 failing")
            && l.contains("2 issues")));
        assert!(!lines.iter().any(|l| l.contains("mind")));
        ui.toggled.insert("projects:dormant".into());
        assert!(texts(&rows(View::Projects, &snap, &ui, None))
            .iter()
            .any(|l| l.contains("mind")));
    }

    #[test]
    fn a_workspace_without_tiers_is_listed_whole() {
        let mut snap = snapshot();
        snap.has_tiers = false;
        for project in &mut snap.projects {
            project.tier = None;
        }
        let found = rows(View::Projects, &snap, &Ui::default(), None);
        assert_eq!(found.len(), 4);
        assert!(found.iter().all(|row| row.kind == Kind::Item));
    }

    #[test]
    fn the_board_lists_sections_in_order_with_tier_labels() {
        let (snap, ui) = (snapshot(), Ui::default());
        let lines = texts(&rows(View::Board, &snap, &ui, None));
        assert!(at(&lines, "▾ Needs fixing  1") < at(&lines, "▾ Ready  1"));
        // Idle worktrees start closed.
        assert!(at(&lines, "▾ Ready  1") < at(&lines, "▸ Idle worktrees  1"));
        assert!(!lines.iter().any(|l| l.contains("feat/9-idle")));
        // Ready holds only a maintained project: its tier is said. Fixing is all active: no label.
        assert_eq!(lines[at(&lines, "▾ Ready  1") + 1], "  Maintained");
        assert!(lines[at(&lines, "▾ Needs fixing  1") + 1].contains("claw feat/254-pane  #7 ✗"));
        assert!(lines.iter().any(|l| l == "2 projects at rest"));
    }

    #[test]
    fn issues_show_urgent_first_by_priority_then_projects_closed() {
        let (snap, mut ui) = (snapshot(), Ui::default());
        let lines = texts(&rows(View::Issues, &snap, &ui, None));
        let urgent = at(&lines, "▾ Urgent  2");
        // Priority wins over tier: the dormant project's P0 comes before the active P1.
        assert!(
            lines[urgent + 1].contains("#7 mind P0 Data loss on save")
                && lines[urgent + 1].ends_with("· dormant")
        );
        assert!(lines[urgent + 2].contains("#2 claw P1 Crash on start"));
        assert!(lines.iter().any(|l| l == "Active  1 project · 2 issues"));
        assert!(lines.iter().any(|l| l == "  ▸ claw  2 · 1 urgent"));
        assert!(!lines.iter().any(|l| l.contains("Tidy the logs")));
        ui.toggled.insert("issues:claw".into());
        let open = texts(&rows(View::Issues, &snap, &ui, None));
        let claw = at(&open, "▾ claw");
        assert!(
            open[claw + 1].contains("#2 P1 Crash on start")
                && open[claw + 2].contains("#1 Tidy the logs")
        );
    }

    #[test]
    fn a_filter_opens_every_group_and_keeps_what_matches() {
        let snap = snapshot();
        let ui = Ui {
            filter: "globe".into(),
            ..Ui::default()
        };
        let lines = texts(&rows(View::Issues, &snap, &ui, None));
        assert_eq!(lines, vec!["  ▾ earth  1", "    · #4 P3 Slow globe  2d"]);
        let none = Ui {
            filter: "zzz".into(),
            ..Ui::default()
        };
        assert_eq!(
            texts(&rows(View::Board, &snap, &none, None)),
            vec!["Nothing matches \"zzz\"."]
        );
        let hits = fuzzy("mrdash 25");
        assert!(hits("metarepo-dashboard feat/254-x"));
        assert!(!hits("metarepo-dashboard main"));
    }

    #[test]
    fn now_gathers_what_needs_a_person() {
        let (snap, ui) = (snapshot(), Ui::default());
        let found = rows(View::Now, &snap, &ui, None);
        let lines = texts(&found);
        let order = [
            "Waiting on you  1",
            "Needs fixing  1",
            "Urgent issues  2",
            "Agents working  1",
            "Ready  1",
        ];
        let places: Vec<usize> = order.iter().map(|title| at(&lines, title)).collect();
        assert!(
            places.windows(2).all(|pair| pair[0] < pair[1]),
            "{lines:#?}"
        );
        assert!(lines.iter().any(|l| l.contains("earth-3  permission")));
        // Its headings label; they do not fold.
        assert!(found.iter().all(|row| row.kind != Kind::Heading));
        let mut calm = snapshot();
        calm.sessions.clear();
        calm.board.clear();
        calm.issues.clear();
        assert_eq!(
            texts(&rows(View::Now, &calm, &ui, None)),
            vec!["Nothing needs you right now."]
        );
    }

    #[test]
    fn sessions_are_grouped_by_what_they_are_doing() {
        let (snap, ui) = (snapshot(), Ui::default());
        let found = rows(View::Sessions, &snap, &ui, None);
        let lines = texts(&found);
        assert!(at(&lines, "▾ Waiting on you  1") < at(&lines, "▾ Working  1"));
        assert!(at(&lines, "▾ Working  1") < at(&lines, "▾ Done  1"));
        let busy = found
            .iter()
            .find(|row| row.text().starts_with("claw-254"))
            .unwrap();
        assert!(busy.text().contains("$2.50"));
        assert!(busy
            .detail
            .iter()
            .any(|line| line == "attach  claude attach claw-254 · enter does it"));
        assert!(matches!(&busy.act, Some(Act::Attach { id, .. }) if id == "claw-254"));
    }

    #[test]
    fn cost_adds_up_the_period_by_project_and_model() {
        let (snap, mut ui) = (snapshot(), Ui::default());
        let lines = texts(&rows(View::Cost, &snap, &ui, None));
        assert!(
            lines[0].contains("Today $10.00")
                && lines[0].contains("● 7 days $13.00")
                && lines[0].contains("All $1,513")
        );
        assert!(lines[1].contains("a day, last 14 days · peak $10.00"));
        assert!(lines.iter().any(|l| l.contains("Active  $10.00 · 77%")));
        assert!(lines
            .iter()
            .any(|l| l.contains("$10.00") && l.contains("5.3M") && l.ends_with("claw")));
        assert!(lines
            .iter()
            .any(|l| l.contains("$3.00") && l.ends_with("workspace root")));
        assert!(lines.iter().any(|l| l.ends_with("outside this workspace")));
        assert!(lines.iter().any(|l| l.contains("Fable 5.2  100%")));
        assert!(lines
            .iter()
            .any(|l| l.contains("$2.50") && l.ends_with("claw-254")));
        assert!(lines
            .iter()
            .any(|l| l == "No price listed for claude-mystery-9"));
        // A project opens to its models, the outside row to its folders.
        ui.span = Span::All;
        ui.toggled
            .extend(["cost:claw".to_string(), "cost:@out".to_string()]);
        let open = texts(&rows(View::Cost, &snap, &ui, None));
        assert!(open
            .iter()
            .any(|l| l.contains("$1,500") && l.ends_with("Sonnet 5.5")));
        assert!(open.iter().any(|l| l.ends_with("scratch")));
    }

    #[test]
    fn workspaces_show_the_open_one_first_with_its_tiers() {
        let (snap, ui) = (snapshot(), Ui::default());
        assert_eq!(
            texts(&rows(View::Workspaces, &snap, &ui, None)),
            vec!["Looking for workspaces…"]
        );
        let found = vec![
            Workspace {
                root: "/other".into(),
                config: "/other/.meta".into(),
                name: "other".into(),
                projects: 2,
                tiers: [2, 0, 0],
            },
            Workspace {
                root: "/ws".into(),
                config: "/ws/.metarepo".into(),
                name: "ws".into(),
                projects: 4,
                tiers: [1, 1, 1],
            },
        ];
        let listed = rows(View::Workspaces, &snap, &ui, Some(&found));
        let lines = texts(&listed);
        assert!(lines[0].starts_with("● ws  4 projects"));
        assert!(lines.iter().any(|l| l
            .contains("Active     1 project · 2 in progress · 1 failing · 2 issues · 1 urgent")));
        let other = listed
            .iter()
            .find(|row| row.text().starts_with("○ other"))
            .unwrap();
        assert_eq!(other.act, Some(Act::Switch("/other/.meta".into())));
        assert_eq!(listed[0].act, None);
    }

    #[test]
    fn words_for_money_tokens_models_and_ages() {
        assert_eq!(money(1204.4), "$1,204");
        assert_eq!(money(1_234_567.0), "$1,234,567");
        assert_eq!(money(12.4), "$12.40");
        assert_eq!(
            (compact(7_400_000), compact(200_000), compact(512)),
            ("7.4M".into(), "200k".into(), "512".into())
        );
        assert_eq!(model_name("claude-fable-5-2"), "Fable 5.2");
        assert_eq!(model_name("claude-mystery-x"), "claude-mystery-x");
        let today = NaiveDate::from_ymd_opt(2026, 11, 12).unwrap();
        assert_eq!(
            (
                age("2026-11-12", today),
                age("2026-11-09", today),
                age("2026-10-20", today),
                age("2026-06-01", today)
            ),
            ("now".into(), "3d".into(), "3w".into(), "5mo".into())
        );
    }

    #[test]
    fn a_session_is_offered_prompts_from_what_its_branch_is_doing() {
        let (mut snap, ui) = (snapshot(), Ui::default());
        // claw-254 is named for issue 254; its branch has a failing pull request.
        snap.issues
            .push(issue("claw", 254, None, "Provider test pane"));
        snap.issues.last_mut().unwrap().in_flight = true;
        let found = rows(View::Sessions, &snap, &ui, None);
        let busy = found
            .iter()
            .find(|row| row.text().starts_with("claw-254"))
            .unwrap();
        let Some(Act::Attach { offers, .. }) = &busy.act else {
            panic!("no attach on {busy:?}");
        };
        let labels: Vec<&str> = offers.iter().map(|o| o.label.as_str()).collect();
        assert_eq!(
            labels,
            vec![
                "Fix the failing checks on #7",
                "Address the review on #7",
                "Check the work against issue #254"
            ]
        );
        assert!(offers[0].prompt.contains("gh pr checks 7 -R org/claw"));
        assert!(offers[2]
            .prompt
            .contains("gh issue view 254 -R org/claw --comments): Provider test pane."));
        // Short enough for a host to paste, and one line each.
        assert!(offers
            .iter()
            .all(|o| o.prompt.len() < 600 && !o.prompt.contains('\n')));
        // A session on no branch is offered nothing but can still be attached.
        let old = found
            .iter()
            .find(|row| row.text().starts_with("old"))
            .unwrap();
        assert!(matches!(&old.act, Some(Act::Attach { offers, .. }) if offers.is_empty()));

        // An issue in flight goes to the session at work on it, not to a second start.
        let listed = rows(
            View::Issues,
            &snap,
            &Ui {
                filter: "provider".into(),
                ..Ui::default()
            },
            None,
        );
        let row = listed
            .iter()
            .find(|row| row.text().contains("#254"))
            .unwrap();
        assert!(matches!(&row.act, Some(Act::Attach { name, .. }) if name == "claw-254"));
    }
}
