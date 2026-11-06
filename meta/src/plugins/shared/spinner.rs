//! Tiny terminal spinner for long single steps (clone, worktree creation).
//! Animates only on a TTY; otherwise prints plain start and done lines.

use colored::Colorize;
use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// "850ms", "12.3s", "2m 05s"
pub fn format_duration(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        format!("{}ms", ms)
    } else if ms < 60_000 {
        format!("{:.1}s", d.as_secs_f64())
    } else {
        format!("{}m {:02}s", d.as_secs() / 60, d.as_secs() % 60)
    }
}

pub fn format_bytes(b: usize) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = b as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{} B", b)
    } else {
        format!("{:.1} {}", v, UNITS[u])
    }
}

pub struct Spinner {
    label: String,
    started: Instant,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    animated: bool,
}

impl Spinner {
    /// `detail` is polled every frame for live status text (may be empty).
    pub fn start(label: &str, detail: impl Fn() -> String + Send + 'static) -> Self {
        let animated = std::io::stdout().is_terminal();
        let stop = Arc::new(AtomicBool::new(false));
        let started = Instant::now();
        let handle = if animated {
            let stop = Arc::clone(&stop);
            let label = label.to_string();
            Some(thread::spawn(move || {
                let mut i = 0;
                while !stop.load(Ordering::Relaxed) {
                    let d = detail();
                    let sep = if d.is_empty() { "" } else { " - " };
                    print!(
                        "\r\x1b[2K     {} {}{}{} {}",
                        FRAMES[i % FRAMES.len()].to_string().cyan(),
                        label,
                        sep,
                        d,
                        format_duration(started.elapsed()).dimmed()
                    );
                    let _ = std::io::stdout().flush();
                    i += 1;
                    thread::sleep(Duration::from_millis(100));
                }
            }))
        } else {
            println!("     {}...", label);
            None
        };
        Self {
            label: label.to_string(),
            started,
            stop,
            handle,
            animated,
        }
    }

    fn halt(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        if self.animated {
            print!("\r\x1b[2K");
            let _ = std::io::stdout().flush();
        }
    }

    /// Stop, print a done line with the elapsed time, return the elapsed time.
    pub fn finish(mut self, done: &str) -> Duration {
        self.halt();
        let el = self.started.elapsed();
        let text = if done.is_empty() { &self.label } else { done };
        println!(
            "     {} {} {}",
            "✓".green(),
            text,
            format!("({})", format_duration(el)).dimmed()
        );
        el
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        // Error / Ctrl+C path: just clear the animation line.
        self.halt();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(format_duration(Duration::from_millis(850)), "850ms");
        assert_eq!(format_duration(Duration::from_millis(12_340)), "12.3s");
        assert_eq!(format_duration(Duration::from_secs(125)), "2m 05s");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(3 * 1024 * 1024), "3.0 MB");
    }
}
