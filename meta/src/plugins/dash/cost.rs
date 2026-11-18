//! What Claude Code has cost: token usage summed from its session transcripts
//! by where the session ran, day and model, priced from a table kept here.

use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{Timestamp, ToSpan, ZonedDateTime};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::scan::{git, github_slug};
use super::sessions::project_within;

/// Dollars per million tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rate {
    pub input: f64,
    pub output: f64,
    /// Five-minute cache writes.
    pub write_5m: f64,
    /// One-hour cache writes.
    pub write_1h: f64,
    pub read: f64,
}

/// A model's price.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Price {
    pub id: &'static str,
    pub rate: Rate,
    /// What fast mode multiplies the price by, where the model has one.
    pub fast: Option<f64>,
    /// The rate past a prompt size in tokens, where the model has one.
    pub over: Option<(u64, Rate)>,
}

const fn rate(input: f64, output: f64, write_5m: f64, write_1h: f64, read: f64) -> Rate {
    Rate {
        input,
        output,
        write_5m,
        write_1h,
        read,
    }
}

const fn price(id: &'static str, rate: Rate) -> Price {
    Price {
        id,
        rate,
        fast: None,
        over: None,
    }
}

/// From platform.claude.com/docs/en/about-claude/pricing as read on
/// 2026-11-12. A model not listed here is counted but not priced.
pub const PRICES: &[Price] = &[
    price("claude-fable-5-2", rate(8.0, 40.0, 10.0, 16.0, 0.2)),
    price("claude-fable-5-1", rate(10.0, 50.0, 12.5, 20.0, 0.25)),
    price("claude-fable-5", rate(10.0, 50.0, 12.5, 20.0, 1.0)),
    Price {
        fast: Some(2.0),
        ..price("claude-opus-5-5", rate(4.0, 20.0, 5.0, 8.0, 0.2))
    },
    Price {
        fast: Some(2.0),
        ..price("claude-opus-5", rate(5.0, 25.0, 6.25, 10.0, 0.5))
    },
    price("claude-opus-4-8", rate(5.0, 25.0, 6.25, 10.0, 0.5)),
    price("claude-sonnet-5-5", rate(2.0, 10.0, 2.5, 4.0, 0.1)),
    price("claude-sonnet-5", rate(2.0, 10.0, 2.5, 4.0, 0.2)),
    price("claude-sonnet-4-6", rate(3.0, 15.0, 3.75, 6.0, 0.3)),
    Price {
        over: Some((100_000, rate(0.5, 2.5, 0.625, 1.0, 0.05))),
        ..price("claude-haiku-5-5", rate(0.1, 0.5, 0.125, 0.2, 0.01))
    },
    price("claude-haiku-4-5", rate(1.0, 5.0, 1.25, 2.0, 0.1)),
];

/// One reply's usage, as a transcript line records it.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    /// The reply's message and request ids: the same reply is written once per
    /// content block, and again in any transcript that continues the session.
    pub key: String,
    pub cwd: String,
    pub session: String,
    /// Local day (`YYYY-MM-DD`), or empty where the line has no timestamp.
    pub day: String,
    pub model: String,
    pub input: u64,
    pub output: u64,
    pub read: u64,
    pub write_5m: u64,
    pub write_1h: u64,
    pub fast: bool,
}

impl Reply {
    /// Dollars for this reply, or `None` when its model has no listed price.
    pub fn usd(&self) -> Option<f64> {
        let price = PRICES.iter().find(|price| price.id == self.model)?;
        let sent = self.input + self.read + self.write_5m + self.write_1h;
        let rate = match price.over {
            Some((at, over)) if sent > at => over,
            _ => price.rate,
        };
        let usd = (self.input as f64 * rate.input
            + self.output as f64 * rate.output
            + self.write_5m as f64 * rate.write_5m
            + self.write_1h as f64 * rate.write_1h
            + self.read as f64 * rate.read)
            / 1e6;
        Some(match price.fast {
            Some(times) if self.fast => usd * times,
            _ => usd,
        })
    }
}

/// The usage a transcript line carries, with its day in `zone`. `None` for
/// any line that is not an assistant reply with usage and a real model.
pub fn reply_of(line: &str, zone: &TimeZone) -> Option<Reply> {
    // Most lines are prompts and tool results: skip them without parsing.
    if !line.contains("\"usage\"") {
        return None;
    }
    let entry: Value = serde_json::from_str(line).ok()?;
    if entry.get("type")?.as_str()? != "assistant" {
        return None;
    }
    let message = entry.get("message")?;
    let usage = message.get("usage").filter(|usage| usage.is_object())?;
    let model = message.get("model")?.as_str()?;
    if model.is_empty() || model.starts_with('<') {
        return None;
    }
    let count = |from: &Value, key: &str| from.get(key).and_then(Value::as_u64).unwrap_or(0);
    let text = |from: &Value, key: &str| -> String {
        from.get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let (write_5m, write_1h) = match usage.get("cache_creation").filter(|made| made.is_object()) {
        Some(made) => (
            count(made, "ephemeral_5m_input_tokens"),
            count(made, "ephemeral_1h_input_tokens"),
        ),
        None => (count(usage, "cache_creation_input_tokens"), 0),
    };
    let day = entry
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|stamp| stamp.parse::<Timestamp>().ok())
        .map(|stamp| stamp.to_zoned_datetime(zone.clone()).date().to_string())
        .unwrap_or_default();
    Some(Reply {
        key: format!(
            "{}|{}",
            message.get("id").and_then(Value::as_str).unwrap_or("None"),
            entry
                .get("requestId")
                .and_then(Value::as_str)
                .unwrap_or("None")
        ),
        cwd: text(&entry, "cwd"),
        session: text(&entry, "sessionId"),
        day,
        model: model.to_string(),
        input: count(usage, "input_tokens"),
        output: count(usage, "output_tokens"),
        read: count(usage, "cache_read_input_tokens"),
        write_5m,
        write_1h,
        fast: usage.get("speed").and_then(Value::as_str) == Some("fast"),
    })
}

/// Usage in one place on one day with one model.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CostRow {
    /// A project's name, `@root` for the workspace itself, or `@out:<folder>`
    /// for anywhere outside it.
    pub bucket: String,
    pub day: String,
    pub model: String,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub usd: f64,
    /// Whether the model has a listed price; without one `usd` is 0.
    pub priced: bool,
}

/// What one session has cost in all.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionCost {
    pub session_id: String,
    pub usd: f64,
    pub tokens: u64,
}

/// Every transcript on this machine, added up.
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct Cost {
    /// The local day the sums were made (`YYYY-MM-DD`).
    pub today: String,
    /// By bucket, day and model.
    pub rows: Vec<CostRow>,
    pub sessions: Vec<SessionCost>,
    /// Models seen that have no listed price.
    pub unpriced: Vec<String>,
}

impl Cost {
    /// Dollars over the `days` days ending today; every day when `None`.
    pub fn usd_over(&self, days: Option<i64>) -> f64 {
        let since = days.and_then(|days| {
            let today: Date = self.today.parse().ok()?;
            Some(today.checked_sub((days - 1).days()).ok()?.to_string())
        });
        self.rows
            .iter()
            .filter(|row| match &since {
                Some(since) => row.day.as_str() >= since.as_str() && row.day <= self.today,
                None => true,
            })
            .map(|row| row.usd)
            .sum()
    }
}

/// Sums replies into rows and session totals, each reply once whichever
/// transcripts hold it. `bucket_of` names where a session's folder is.
#[derive(Default)]
pub struct Sums {
    seen: HashSet<String>,
    rows: BTreeMap<(String, String, String), CostRow>,
    sessions: BTreeMap<String, (f64, u64)>,
}

impl Sums {
    pub fn add(&mut self, reply: Reply, bucket_of: &mut impl FnMut(&str) -> String) {
        if !self.seen.insert(reply.key.clone()) {
            return;
        }
        let usd = reply.usd();
        let bucket = bucket_of(&reply.cwd);
        let row = self
            .rows
            .entry((bucket.clone(), reply.day.clone(), reply.model.clone()))
            .or_insert_with(|| CostRow {
                bucket,
                day: reply.day.clone(),
                model: reply.model.clone(),
                input: 0,
                output: 0,
                cache_read: 0,
                cache_write: 0,
                usd: 0.0,
                priced: usd.is_some(),
            });
        let write = reply.write_5m + reply.write_1h;
        row.input += reply.input;
        row.output += reply.output;
        row.cache_read += reply.read;
        row.cache_write += write;
        row.usd += usd.unwrap_or(0.0);
        let session = self.sessions.entry(reply.session).or_default();
        session.0 += usd.unwrap_or(0.0);
        session.1 += reply.input + reply.output + reply.read + write;
    }

    pub fn finish(self, today: String) -> Cost {
        let rows: Vec<CostRow> = self.rows.into_values().collect();
        let mut unpriced: Vec<String> = rows
            .iter()
            .filter(|row| !row.priced)
            .map(|row| row.model.clone())
            .collect();
        unpriced.sort();
        unpriced.dedup();
        Cost {
            today,
            rows,
            sessions: self
                .sessions
                .into_iter()
                .map(|(session_id, (usd, tokens))| SessionCost {
                    session_id,
                    usd,
                    tokens,
                })
                .collect(),
            unpriced,
        }
    }
}

/// Where Claude Code keeps its files: `CLAUDE_CONFIG_DIR`, else `~/.claude`.
fn claude_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|dir| !dir.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join(".claude"))
}

/// Add up every transcript on this machine against the workspace at `root`.
/// `keys` are its project keys and `slugs` maps a lowercased GitHub
/// `owner/name` to its project, so a session run in another checkout of a
/// project still counts toward it. `None` when there is no transcript folder.
pub fn read(root: &Path, keys: &[String], slugs: &HashMap<String, String>) -> Option<Cost> {
    let transcripts = claude_dir()?.join("projects");
    if !transcripts.is_dir() {
        return None;
    }
    let mut paths: Vec<PathBuf> = walkdir::WalkDir::new(&transcripts)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.into_path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
        .collect();
    paths.sort();

    let mut placed: HashMap<String, String> = HashMap::new();
    let mut bucket_of = |cwd: &str| -> String {
        placed
            .entry(cwd.to_string())
            .or_insert_with(|| bucket(root, keys, slugs, cwd))
            .clone()
    };
    let zone = TimeZone::system();
    let mut sums = Sums::default();
    for path in paths {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        // A last line with no newline is still being written: left for next time.
        let whole = match bytes.iter().rposition(|byte| *byte == b'\n') {
            Some(end) => &bytes[..end],
            None => continue,
        };
        for line in whole.split(|byte| *byte == b'\n') {
            if let Some(reply) = reply_of(&String::from_utf8_lossy(line), &zone) {
                sums.add(reply, &mut bucket_of);
            }
        }
    }
    Some(sums.finish(ZonedDateTime::now().date().to_string()))
}

fn bucket(root: &Path, keys: &[String], slugs: &HashMap<String, String>, cwd: &str) -> String {
    let path = Path::new(cwd);
    let within = path
        .strip_prefix(root)
        .ok()
        .map(Path::to_path_buf)
        .or_else(|| {
            let real = path.canonicalize().ok()?;
            real.strip_prefix(root).ok().map(Path::to_path_buf)
        });
    if let Some(rel) = within {
        return project_within(&rel, keys).unwrap_or("@root").to_string();
    }
    let project = path
        .is_dir()
        .then(|| git(path, &["remote", "get-url", "origin"]))
        .flatten()
        .as_deref()
        .and_then(github_slug)
        .and_then(|slug| slugs.get(&slug.to_lowercase()).cloned());
    project.unwrap_or_else(|| {
        let folder = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| cwd.to_string());
        format!("@out:{folder}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(id: &str, model: &str, stamp: &str, usage: &str) -> String {
        format!(
            r#"{{"type":"assistant","requestId":"req-{id}","sessionId":"aaa11111-0000","cwd":"/ws/app/main","timestamp":"{stamp}","message":{{"id":"msg-{id}","model":"{model}","usage":{usage}}}}}"#
        )
    }

    fn utc() -> TimeZone {
        TimeZone::UTC
    }

    #[test]
    fn a_reply_is_read_with_its_cache_writes_and_local_day() {
        let usage = r#"{"input_tokens":1000,"output_tokens":200000,"cache_read_input_tokens":5000000,"cache_creation":{"ephemeral_5m_input_tokens":60000,"ephemeral_1h_input_tokens":40000},"speed":"standard"}"#;
        let text = line("1", "claude-fable-5-2", "2026-11-12T02:30:00.000Z", usage);
        let reply = reply_of(&text, &utc()).unwrap();
        assert_eq!(reply.key, "msg-1|req-1");
        assert_eq!(reply.day, "2026-11-12");
        assert_eq!(
            (reply.input, reply.output, reply.read),
            (1000, 200_000, 5_000_000)
        );
        assert_eq!((reply.write_5m, reply.write_1h), (60_000, 40_000));
        assert!(!reply.fast);
        // 1000*8 + 200000*40 + 60000*10 + 40000*16 + 5000000*0.2, per million.
        assert!((reply.usd().unwrap() - 10.248).abs() < 1e-9);
        // West of Greenwich that reply was made the evening before.
        let west = TimeZone::fixed(jiff::tz::offset(-6));
        assert_eq!(reply_of(&text, &west).unwrap().day, "2026-11-11");
    }

    #[test]
    fn older_transcripts_give_cache_writes_as_one_count() {
        let usage = r#"{"input_tokens":0,"output_tokens":10,"cache_creation_input_tokens":500}"#;
        let reply = reply_of(
            &line("2", "claude-sonnet-5-5", "2026-11-12T10:00:00Z", usage),
            &utc(),
        )
        .unwrap();
        assert_eq!((reply.write_5m, reply.write_1h), (500, 0));
    }

    #[test]
    fn lines_that_are_not_priced_replies_are_skipped() {
        let usage = r#"{"input_tokens":1,"output_tokens":1}"#;
        assert!(reply_of(
            r#"{"type":"user","message":{"content":"the word usage alone"}}"#,
            &utc()
        )
        .is_none());
        assert!(reply_of(
            &line("3", "<synthetic>", "2026-11-12T10:00:00Z", usage),
            &utc()
        )
        .is_none());
        assert!(reply_of(&line("3", "", "2026-11-12T10:00:00Z", usage), &utc()).is_none());
        assert!(reply_of("{\"usage\": not json", &utc()).is_none());
        let user = line("3", "claude-fable-5-2", "2026-11-12T10:00:00Z", usage)
            .replace("\"assistant\"", "\"user\"");
        assert!(reply_of(&user, &utc()).is_none());
    }

    #[test]
    fn fast_mode_and_large_prompts_change_the_rate() {
        let reply = |model: &str, input: u64, fast: bool| Reply {
            key: String::new(),
            cwd: String::new(),
            session: String::new(),
            day: String::new(),
            model: model.into(),
            input,
            output: 0,
            read: 0,
            write_5m: 0,
            write_1h: 0,
            fast,
        };
        assert!((reply("claude-opus-5-5", 1_000_000, false).usd().unwrap() - 4.0).abs() < 1e-9);
        assert!((reply("claude-opus-5-5", 1_000_000, true).usd().unwrap() - 8.0).abs() < 1e-9);
        // Fast mode costs no more on a model with no fast price.
        assert!((reply("claude-fable-5-2", 1_000_000, true).usd().unwrap() - 8.0).abs() < 1e-9);
        assert!((reply("claude-haiku-5-5", 100_000, false).usd().unwrap() - 0.01).abs() < 1e-9);
        assert!((reply("claude-haiku-5-5", 200_000, false).usd().unwrap() - 0.1).abs() < 1e-9);
        assert!(reply("claude-mystery-9", 1, false).usd().is_none());
    }

    #[test]
    fn each_reply_counts_once_and_sums_by_place_day_and_model() {
        let usage = r#"{"input_tokens":1000000,"output_tokens":0}"#;
        let mut sums = Sums::default();
        let mut bucket_of = |cwd: &str| {
            if cwd == "/ws/app/main" {
                "app".to_string()
            } else {
                "@root".to_string()
            }
        };
        for text in [
            line("1", "claude-sonnet-5-5", "2026-11-12T10:00:00Z", usage),
            // The same reply again, as a second content block or a resumed transcript.
            line("1", "claude-sonnet-5-5", "2026-11-12T10:00:01Z", usage),
            line("2", "claude-sonnet-5-5", "2026-11-12T11:00:00Z", usage),
            line("3", "claude-sonnet-5-5", "2026-11-06T11:00:00Z", usage),
            line("4", "claude-sonnet-5-5", "2026-10-01T11:00:00Z", usage),
            line("5", "claude-mystery-9", "2026-11-12T11:00:00Z", usage),
        ] {
            sums.add(reply_of(&text, &utc()).unwrap(), &mut bucket_of);
        }
        let cost = sums.finish("2026-11-12".to_string());
        assert_eq!(cost.rows.len(), 4);
        let today = cost
            .rows
            .iter()
            .find(|r| r.day == "2026-11-12" && r.priced)
            .unwrap();
        assert_eq!((today.bucket.as_str(), today.input), ("app", 2_000_000));
        assert!((today.usd - 4.0).abs() < 1e-9);
        assert_eq!(cost.unpriced, vec!["claude-mystery-9"]);
        assert_eq!(cost.sessions.len(), 1);
        assert_eq!(cost.sessions[0].tokens, 5_000_000);
        assert!((cost.sessions[0].usd - 8.0).abs() < 1e-9);
        // Today, the seven days ending today, thirty days, and all time.
        assert!((cost.usd_over(Some(1)) - 4.0).abs() < 1e-9);
        assert!((cost.usd_over(Some(7)) - 6.0).abs() < 1e-9);
        assert!((cost.usd_over(Some(30)) - 6.0).abs() < 1e-9);
        assert!((cost.usd_over(None) - 8.0).abs() < 1e-9);
    }

    #[test]
    fn a_folder_is_placed_against_the_workspace() {
        let keys = vec!["app".to_string()];
        let slugs = HashMap::new();
        let root = Path::new("/nowhere/ws");
        assert_eq!(bucket(root, &keys, &slugs, "/nowhere/ws"), "@root");
        assert_eq!(
            bucket(root, &keys, &slugs, "/nowhere/ws/app/feat/1-x"),
            "app"
        );
        assert_eq!(bucket(root, &keys, &slugs, "/nowhere/ws/docs"), "@root");
        assert_eq!(
            bucket(root, &keys, &slugs, "/nowhere/else/scratch"),
            "@out:scratch"
        );
    }
}
