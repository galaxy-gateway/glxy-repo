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

/// The most text one [`send`] carries, in bytes: glxy-term's own limit.
pub const MAX_SEND: usize = 3072;

/// Paste `text` into a pane this program opened, leaving focus where it is,
/// and press Return after it. The host refuses any pane `open_pane` did not
/// open. Needs a Galaxy Terminal with the `send` verb (0.113.0).
pub fn send(run: Runner, pane: u64, text: &str) -> Result<(), String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("there is nothing to send".to_string());
    }
    if text.len() > MAX_SEND {
        return Err(format!(
            "that is {} bytes and a pane takes {MAX_SEND} at a time",
            text.len()
        ));
    }
    // Alone, a dash would mean "read the text from stdin" to glxy-term.
    if text == "-" {
        return Err("a lone dash cannot be sent".to_string());
    }
    let args = ["ctl", "send", &pane.to_string(), "--submit", text].map(String::from);
    let said =
        run(&args).ok_or_else(|| "glxy-term could not be run (is it on PATH?)".to_string())?;
    let reply = said.trim();
    if reply.is_empty() || reply == "ok" {
        return Ok(());
    }
    let why = reply
        .strip_prefix("error: ")
        .or_else(|| reply.strip_prefix("Error: "))
        .unwrap_or(reply);
    Err(
        if why.starts_with("unknown verb") || why.starts_with("usage:") {
            "this Galaxy Terminal cannot paste into a pane yet (ctl send needs 0.113.0)".to_string()
        } else {
            format!("glxy-term: {why}")
        },
    )
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

    #[test]
    fn text_is_sent_to_a_pane_with_return_after_it() {
        let asked = RefCell::new(Vec::new());
        let run = |args: &[String]| {
            asked.borrow_mut().push(args.to_vec());
            Some(String::new())
        };
        assert_eq!(send(&run, 9, "  Fix the checks.\nThen commit.\n"), Ok(()));
        // Line breaks go as they are: the client escapes them for the socket.
        assert_eq!(
            asked.borrow()[0],
            words(&[
                "ctl",
                "send",
                "9",
                "--submit",
                "Fix the checks.\nThen commit."
            ])
        );
    }

    #[test]
    fn what_cannot_be_sent_is_said_plainly() {
        let never = |_: &[String]| -> Option<String> { panic!("must not be run") };
        assert!(send(&never, 9, "  \n").is_err());
        assert!(send(&never, 9, "-").is_err());
        assert!(send(&never, 9, &"x".repeat(MAX_SEND + 1))
            .unwrap_err()
            .contains("3073 bytes"));
        let fits = |_: &[String]| Some(String::new());
        assert!(send(&fits, 9, &"x".repeat(MAX_SEND)).is_ok());

        let gone = |_: &[String]| Some("error: pane 9 was not opened by open-pane\n".to_string());
        assert_eq!(
            send(&gone, 9, "hi"),
            Err("glxy-term: pane 9 was not opened by open-pane".to_string())
        );
        // An older Galaxy Terminal does not know the verb, in the app or in its client.
        for old in [
            "error: unknown verb (try open-window, open-pane, ...)\n",
            "Error: usage: glxy-term ctl open-window [...]\n",
        ] {
            let old = move |_: &[String]| Some(old.to_string());
            assert!(send(&old, 9, "hi").unwrap_err().contains("needs 0.113.0"));
        }
        assert!(send(&|_: &[String]| None, 9, "hi")
            .unwrap_err()
            .contains("PATH"));
    }
}
