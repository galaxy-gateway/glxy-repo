//! The terminal the dashboard runs in, where it can open panes beside itself.
//!
//! Galaxy Terminal (glxy-term) gives its panes a control socket: a program in
//! one pane can open another pane running a command, and focus a pane it
//! opened. Anywhere else there is no host, and the dashboard falls back to
//! running a session in its own place or in the background.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(10);

/// Where the dashboard is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Host {
    /// A Galaxy Terminal pane with its control socket on.
    GlxyTerm,
    /// A terminal that cannot be asked for panes.
    None,
}

impl Host {
    /// The host this process runs in, by what the terminal put in the
    /// environment (`GLXY_CTL_SOCK` is set in panes when `[control] enable`
    /// is on).
    pub fn detect() -> Host {
        Host::from_env(std::env::var_os("GLXY_CTL_SOCK").is_some_and(|sock| !sock.is_empty()))
    }

    pub fn from_env(has_glxy_socket: bool) -> Host {
        if has_glxy_socket {
            Host::GlxyTerm
        } else {
            Host::None
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Host::GlxyTerm => "Galaxy Terminal",
            Host::None => "this terminal",
        }
    }
}

/// Runs a host's command line and gives what it printed, or `None` when it
/// could not run. A seam so the host can be stood in for.
pub type Runner<'a> = &'a dyn Fn(&[String]) -> Option<String>;

/// Runs the real `glxy-term`.
pub fn glxy_term(args: &[String]) -> Option<String> {
    let mut command = Command::new("glxy-term");
    command.args(args);
    super::run(command, TIMEOUT).map(|(_, stdout)| stdout)
}

/// The arguments of `glxy-term ctl open-pane` for a command, or why they
/// cannot be sent: the control socket carries fields a tab apart, so none may
/// hold a tab or a line break.
pub fn open_pane_args(cwd: &Path, title: &str, argv: &[String]) -> Result<Vec<String>, String> {
    let cwd = cwd.to_string_lossy().into_owned();
    if argv.is_empty() {
        return Err("no command to run in the pane".to_string());
    }
    if argv
        .iter()
        .map(String::as_str)
        .chain([cwd.as_str(), title])
        .any(|field| field.contains(['\t', '\n', '\r']))
    {
        return Err("a pane's command cannot hold tabs or line breaks".to_string());
    }
    let mut args: Vec<String> = [
        "ctl",
        "open-pane",
        "--cwd",
        &cwd,
        "--title",
        title,
        "--right",
        "--",
    ]
    .map(String::from)
    .to_vec();
    args.extend(argv.iter().cloned());
    Ok(args)
}

/// Open a pane to the right of the focused one, running `argv` in `cwd`.
/// Gives the new pane's id, the one [`focus`] takes.
pub fn open_pane(run: Runner, cwd: &Path, title: &str, argv: &[String]) -> Result<u64, String> {
    let said = run(&open_pane_args(cwd, title, argv)?)
        .ok_or_else(|| "glxy-term could not be run (is it on PATH?)".to_string())?;
    // The reply is `pane ID`; anything else is glxy-term saying why not.
    let reply = said.trim();
    reply
        .strip_prefix("pane ")
        .and_then(|id| id.trim().parse().ok())
        .ok_or_else(|| {
            let why = reply.strip_prefix("error: ").unwrap_or(reply);
            if why.is_empty() {
                "glxy-term gave no pane".to_string()
            } else {
                format!("glxy-term: {why}")
            }
        })
}

/// Select and focus a pane. False when it is gone (closed since) or the host
/// could not be asked: glxy-term prints nothing for a verb that worked.
pub fn focus(run: Runner, pane: u64) -> bool {
    run(&["ctl".to_string(), "focus".to_string(), pane.to_string()])
        .is_some_and(|said| !said.trim_start().to_lowercase().starts_with("error"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn words(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_host_is_told_by_the_environment() {
        assert_eq!(Host::from_env(true), Host::GlxyTerm);
        assert_eq!(Host::from_env(false), Host::None);
    }

    #[test]
    fn a_pane_is_opened_beside_with_its_folder_title_and_command() {
        let asked = RefCell::new(Vec::new());
        let run = |args: &[String]| {
            asked.borrow_mut().push(args.to_vec());
            Some("pane 42\n".to_string())
        };
        let pane = open_pane(
            &run,
            Path::new("/ws/app/feat/7-x"),
            "app-7",
            &words(&["claude", "attach", "abc"]),
        );
        assert_eq!(pane, Ok(42));
        assert_eq!(
            asked.borrow()[0],
            words(&[
                "ctl",
                "open-pane",
                "--cwd",
                "/ws/app/feat/7-x",
                "--title",
                "app-7",
                "--right",
                "--",
                "claude",
                "attach",
                "abc"
            ])
        );
    }

    #[test]
    fn what_the_socket_cannot_carry_is_refused_before_it_is_sent() {
        let run = |_: &[String]| -> Option<String> { panic!("must not be run") };
        let cwd = Path::new("/ws");
        assert!(open_pane(&run, cwd, "t", &words(&["claude", "two\nlines"])).is_err());
        assert!(open_pane(&run, cwd, "a\ttitle", &words(&["claude"])).is_err());
        assert!(open_pane(&run, cwd, "t", &[]).is_err());
    }

    #[test]
    fn a_refusal_is_passed_on_in_the_hosts_words() {
        let refused =
            |_: &[String]| Some("error: pane splits are unavailable in tmux tabs\n".to_string());
        assert_eq!(
            open_pane(&refused, Path::new("/ws"), "t", &words(&["claude"])),
            Err("glxy-term: pane splits are unavailable in tmux tabs".to_string())
        );
        let absent = |_: &[String]| None;
        assert!(
            open_pane(&absent, Path::new("/ws"), "t", &words(&["claude"]))
                .unwrap_err()
                .contains("PATH")
        );
        let silent = |_: &[String]| Some(String::new());
        assert_eq!(
            open_pane(&silent, Path::new("/ws"), "t", &words(&["claude"])),
            Err("glxy-term gave no pane".to_string())
        );
    }

    #[test]
    fn focus_succeeds_on_silence_and_fails_on_a_closed_pane() {
        // glxy-term prints nothing when a verb only acknowledges.
        assert!(focus(&|_: &[String]| Some(String::new()), 7));
        assert!(!focus(
            &|_: &[String]| Some("error: no such pane\n".to_string()),
            7
        ));
        assert!(!focus(
            &|_: &[String]| Some("Error: bad pane id\n".to_string()),
            7
        ));
        assert!(!focus(&|_: &[String]| None, 7));
        let asked = RefCell::new(Vec::new());
        focus(
            &|args: &[String]| {
                asked.borrow_mut().push(args.to_vec());
                Some(String::new())
            },
            7,
        );
        assert_eq!(asked.borrow()[0], words(&["ctl", "focus", "7"]));
    }
}
