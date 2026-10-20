//! The hook's and the game's logs, for the server played on (approved D10
//! amendment): the hook's `hook.log` and the game's `stdout.txt`, tailed
//! from where they stood when the launcher's run began, and the game's
//! error reports (`.txt`, `.json`) in its `crash_dump` folder as they
//! appear. Their lines go into the launcher's [`Recorder`], each tagged
//! with its source, redacted there, and to the server with the launcher's
//! own (`Request::Telemetry`).
//!
//! - **Never** the game's `.dmp` minidumps, which are large and binary,
//!   and never a file whose name looks like a key, certificate or token
//!   ([`crate::logs::looks_secret`]).
//! - **Within budgets.** Each source reads so many bytes a minute
//!   ([`HOOK_PER_MINUTE`], [`GAME_PER_MINUTE`], [`CRASH_PER_MINUTE`]); when
//!   a log grows faster, its oldest unread part is skipped, and the
//!   launcher's log says how much once a minute. A run sends
//!   [`crate::diagnostics::RUN_QUOTA`] bytes at most. Files are read on a
//!   blocking thread of their own, never the game's or a connection's.
//! - **The switch.** "Send diagnostics" Off stops all of it: what was
//!   written meanwhile is passed over, never sent later.

use std::{
    collections::HashMap,
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

use tokio::{task::JoinHandle, time::MissedTickBehavior};
use tpf3mp_proto::{DiagnosticLevel, LogSource};
use tracing::warn;

use crate::diagnostics::{self, Recorder};

/// Bytes of the hook's log read a minute at most.
pub const HOOK_PER_MINUTE: u64 = 192 << 10;
/// Bytes of the game's log read a minute at most.
pub const GAME_PER_MINUTE: u64 = 96 << 10;
/// Bytes of the game's error reports read a minute at most: a crash
/// writes a few at once.
pub const CRASH_PER_MINUTE: u64 = 512 << 10;
/// Bytes of one error report read at most: its end.
pub const CRASH_FILE_MAX: u64 = 256 << 10;
/// How often the files are looked at.
const POLL: Duration = Duration::from_secs(1);
/// How often the error reports' folders are looked through.
const CRASH_POLL: Duration = Duration::from_secs(10);
/// How often what was skipped or dropped is logged.
const REPORT_EVERY: Duration = Duration::from_secs(60);
/// A line longer than this is cut, and goes as it is.
const LONGEST_LINE: usize = 64 << 10;

/// Bytes a source may read, refilled over a minute.
#[derive(Debug)]
struct Budget {
    per_minute: u64,
    tokens: f64,
    at: Instant,
}

impl Budget {
    fn new(per_minute: u64, now: Instant) -> Self {
        Self {
            per_minute,
            tokens: per_minute as f64,
            at: now,
        }
    }

    fn available(&mut self, now: Instant) -> u64 {
        let elapsed = now.saturating_duration_since(self.at).as_secs_f64();
        self.at = now;
        self.tokens =
            (self.tokens + elapsed * self.per_minute as f64 / 60.0).min(self.per_minute as f64);
        self.tokens as u64
    }

    fn spend(&mut self, bytes: u64) {
        self.tokens = (self.tokens - bytes as f64).max(0.0);
    }
}

/// What one look at a log came to: the whole lines added, and the bytes
/// skipped over the source's budget.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Polled {
    pub lines: Vec<String>,
    pub skipped: u64,
}

/// A log read as it grows, from where it stood when the run began, within
/// a budget of bytes a minute. When it grows faster, the oldest unread
/// part is skipped: the newest minute's worth is read. A file made anew
/// (shorter than what was read, or created again, as the game's
/// `stdout.txt` is at each start) is read from its start.
#[derive(Debug)]
pub struct Tail {
    path: PathBuf,
    pos: u64,
    created: Option<SystemTime>,
    partial: Vec<u8>,
    /// Skipped to the middle of a line: the rest of it goes too.
    mid_line: bool,
    budget: Budget,
}

impl Tail {
    /// Reads `path` from its end now, `per_minute` bytes a minute at most.
    pub fn from_end(path: PathBuf, per_minute: u64) -> Self {
        let meta = fs::metadata(&path).ok();
        Self {
            pos: meta.as_ref().map_or(0, fs::Metadata::len),
            created: meta.and_then(|meta| meta.created().ok()),
            path,
            partial: Vec::new(),
            mid_line: false,
            budget: Budget::new(per_minute, Instant::now()),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Passes over what was added since, unread: while diagnostics are
    /// off.
    pub fn skip_to_end(&mut self) {
        if let Ok(meta) = fs::metadata(&self.path) {
            self.pos = meta.len();
            self.created = meta.created().ok();
        }
        self.partial.clear();
        self.mid_line = false;
    }

    /// The whole lines added since the last look, as far as the budget
    /// lets.
    pub fn poll(&mut self, now: Instant) -> Polled {
        let mut polled = Polled::default();
        let Ok(meta) = fs::metadata(&self.path) else {
            return polled;
        };
        let len = meta.len();
        let created = meta.created().ok();
        if len < self.pos
            || (created.is_some() && self.created.is_some() && created != self.created)
        {
            // Made anew: from its start.
            self.pos = 0;
            self.partial.clear();
            self.mid_line = false;
        }
        self.created = created.or(self.created);
        let cap = self.budget.per_minute;
        if len - self.pos > cap {
            let skip = len - self.pos - cap;
            self.pos += skip;
            polled.skipped += skip + self.partial.len() as u64;
            self.partial.clear();
            self.mid_line = true;
        }
        let want = (len - self.pos).min(self.budget.available(now));
        if want == 0 {
            return polled;
        }
        let mut bytes = Vec::with_capacity(usize::try_from(want).unwrap_or(0));
        let read = File::open(&self.path).and_then(|mut file| {
            file.seek(SeekFrom::Start(self.pos))?;
            file.take(want).read_to_end(&mut bytes)
        });
        let Ok(read) = read else {
            return polled;
        };
        self.budget.spend(read as u64);
        self.pos += read as u64;
        let mut start = 0;
        if self.mid_line {
            match bytes.iter().position(|byte| *byte == b'\n') {
                Some(end) => {
                    polled.skipped += end as u64 + 1;
                    start = end + 1;
                    self.mid_line = false;
                }
                None => {
                    polled.skipped += bytes.len() as u64;
                    return polled;
                }
            }
        }
        self.partial.extend_from_slice(&bytes[start..]);
        let mut rest = self.partial.as_slice();
        while let Some(end) = rest.iter().position(|byte| *byte == b'\n') {
            push_line(&mut polled.lines, &rest[..end]);
            rest = &rest[end + 1..];
        }
        let rest = rest.to_vec();
        self.partial = if rest.len() > LONGEST_LINE {
            push_line(&mut polled.lines, &rest);
            Vec::new()
        } else {
            rest
        };
        polled
    }
}

fn push_line(lines: &mut Vec<String>, bytes: &[u8]) {
    let line = String::from_utf8_lossy(bytes);
    let line = line.trim_end_matches('\r');
    if !line.trim().is_empty() {
        lines.push(line.to_owned());
    }
}

/// The game's error reports in its `crash_dump` folders: the `.txt` and
/// `.json` files written or changed since the run began, each read once
/// for each change, its end alone when it is long. Never `stdout.txt`
/// (tailed as the game's log), never a `.dmp` minidump, and never a file
/// whose name looks like a key, certificate or token.
#[derive(Debug)]
pub struct CrashReports {
    dirs: Vec<PathBuf>,
    seen: HashMap<PathBuf, (u64, SystemTime)>,
    budget: Budget,
}

/// One error report read: its file's name and lines.
#[derive(Debug, PartialEq, Eq)]
pub struct Report {
    pub name: String,
    pub lines: Vec<String>,
    /// Bytes of its start left out, past [`CRASH_FILE_MAX`].
    pub cut: u64,
}

impl CrashReports {
    /// Watches reports added or changed after this metadata snapshot.
    /// Filesystem timestamps can lag the wall clock on Windows; comparing
    /// them with SystemTime::now() would silently miss a new report.
    pub fn new(dirs: Vec<PathBuf>) -> Self {
        let mut reports = Self {
            dirs,
            seen: HashMap::new(),
            budget: Budget::new(CRASH_PER_MINUTE, Instant::now()),
        };
        reports.skip_all();
        reports
    }

    /// Marks every report there now as read: while diagnostics are off.
    pub fn skip_all(&mut self) {
        self.seen.clear();
        for dir in &self.dirs {
            let Ok(entries) = fs::read_dir(dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if !path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(is_report)
                {
                    continue;
                }
                let Ok(meta) = entry.metadata() else {
                    continue;
                };
                if meta.is_file()
                    && let Ok(modified) = meta.modified()
                {
                    self.seen.insert(path, (meta.len(), modified));
                }
            }
        }
    }

    /// The reports written or changed since the last look, as far as the
    /// budget lets; the rest wait for the next look.
    pub fn poll(&mut self, now: Instant) -> Vec<Report> {
        let mut reports = Vec::new();
        for dir in &self.dirs {
            let Ok(entries) = fs::read_dir(dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                    continue;
                };
                if !is_report(name) {
                    continue;
                }
                let Ok(meta) = entry.metadata() else {
                    continue;
                };
                let Ok(modified) = meta.modified() else {
                    continue;
                };
                if !meta.is_file() {
                    continue;
                }
                let stamp = (meta.len(), modified);
                if self.seen.get(&path) == Some(&stamp) {
                    continue;
                }
                let take = meta.len().min(CRASH_FILE_MAX);
                if take > self.budget.available(now) {
                    continue;
                }
                let mut bytes = Vec::new();
                let read = File::open(&path).and_then(|mut file| {
                    file.seek(SeekFrom::Start(meta.len() - take))?;
                    file.take(take).read_to_end(&mut bytes)
                });
                if read.is_err() {
                    continue;
                }
                self.budget.spend(bytes.len() as u64);
                self.seen.insert(path.clone(), stamp);
                let mut lines = Vec::new();
                for line in bytes.split(|byte| *byte == b'\n') {
                    push_line(&mut lines, line);
                }
                reports.push(Report {
                    name: name.to_owned(),
                    lines,
                    cut: meta.len() - take,
                });
            }
        }
        reports
    }
}

/// Whether a file in `crash_dump` is an error report to send.
fn is_report(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let text = lower.ends_with(".txt") || lower.ends_with(".json");
    text && lower != "stdout.txt" && !crate::logs::looks_secret(name)
}

/// The level of one of the game's lines, by its own level field
/// (`[time - ERROR - thread - ...]`).
fn game_level(line: &str) -> DiagnosticLevel {
    let head = line.get(..48).unwrap_or(line);
    if head.contains("- ERROR") || head.contains("- FATAL") {
        DiagnosticLevel::Error
    } else if head.contains("- WARN") {
        DiagnosticLevel::Warn
    } else {
        DiagnosticLevel::Info
    }
}

/// The level of one of the hook's lines, by the words in it.
fn hook_level(line: &str) -> DiagnosticLevel {
    let lower = line.to_ascii_lowercase();
    if lower.contains("panic") || lower.contains("error") {
        DiagnosticLevel::Error
    } else if lower.contains("refus") || lower.contains("warn") || lower.contains("failed") {
        DiagnosticLevel::Warn
    } else {
        DiagnosticLevel::Info
    }
}

/// Where the hook's and the game's logs are on this computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Places {
    /// The hook's log.
    pub hook_log: PathBuf,
    /// The game's `crash_dump` folders, one for each Steam account.
    pub crash_dirs: Vec<PathBuf>,
}

impl Places {
    /// The hook's log in its data folder `hook_dir`, and the game's
    /// folders in each Steam account's.
    pub fn of_this_computer(hook_dir: &Path) -> Self {
        Self {
            hook_log: hook_dir.join("hook.log"),
            crash_dirs: crate::logs::game_candidates()
                .into_iter()
                .filter(|candidate| candidate.zip_dir.ends_with("crash_dump"))
                .flat_map(|candidate| candidate.paths)
                .collect(),
        }
    }
}

/// The files whose lines go with the launcher's log.
#[derive(Debug)]
pub struct Sources {
    hook: Tail,
    game: Vec<Tail>,
    crash: CrashReports,
    generation: u64,
}

impl Sources {
    /// The hook's log at `hook_log`, and the game's log and error reports
    /// in each of `crash_dirs` (`<Steam>/userdata/<account>/3493540/local/
    /// crash_dump`), from where they stand now.
    pub fn new(hook_log: PathBuf, crash_dirs: Vec<PathBuf>) -> Self {
        Self {
            hook: Tail::from_end(hook_log, HOOK_PER_MINUTE),
            game: crash_dirs
                .iter()
                .map(|dir| Tail::from_end(dir.join("stdout.txt"), GAME_PER_MINUTE))
                .collect(),
            crash: CrashReports::new(crash_dirs),
            generation: 0,
        }
    }

    /// Passes over everything written meanwhile, unread.
    fn skip(&mut self) {
        self.hook.skip_to_end();
        for tail in &mut self.game {
            tail.skip_to_end();
        }
        self.crash.skip_all();
    }

    /// Records what was added to the files since the last look; with
    /// `reports`, looks for new error reports too. Returns the bytes
    /// skipped over the budgets: the hook's, the game's.
    pub fn record(&mut self, recorder: &Recorder, now: Instant, reports: bool) -> [u64; 2] {
        let recording = recorder.recording();
        if !recording.on || recording.generation != self.generation {
            self.skip();
            self.generation = recording.generation;
            return [0, 0];
        }
        let at_ms = diagnostics::now_ms();
        let hook = self.hook.poll(now);
        for line in &hook.lines {
            recording.record_line(LogSource::Hook, hook_level(line), "hook.log", line, at_ms);
        }
        let mut game_skipped = 0;
        for tail in &mut self.game {
            let game = tail.poll(now);
            game_skipped += game.skipped;
            for line in &game.lines {
                recording.record_line(LogSource::Game, game_level(line), "stdout.txt", line, at_ms);
            }
        }
        if reports {
            for report in self.crash.poll(now) {
                if report.cut > 0 {
                    recording.record_line(
                        LogSource::Crash,
                        DiagnosticLevel::Warn,
                        &report.name,
                        &format!("({} bytes of its start left out)", report.cut),
                        at_ms,
                    );
                }
                for line in &report.lines {
                    recording.record_line(
                        LogSource::Crash,
                        DiagnosticLevel::Error,
                        &report.name,
                        line,
                        at_ms,
                    );
                }
            }
        }
        [hook.skipped, game_skipped]
    }
}

/// Reads `sources` into `recorder` for as long as the launcher runs,
/// while diagnostics are on, and logs once a minute how much was skipped
/// over the budgets and how many lines were lost waiting.
pub fn start(recorder: Recorder, mut sources: Sources) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticks = tokio::time::interval(POLL);
        ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut reported = Instant::now();
        let mut looked = Instant::now() - CRASH_POLL;
        let mut skipped = [0u64; 2];
        let mut dropped = recorder.dropped();
        loop {
            ticks.tick().await;
            let now = Instant::now();
            let reports = now.duration_since(looked) >= CRASH_POLL;
            if reports {
                looked = now;
            }
            let reading = recorder.clone();
            let Ok((back, more)) = tokio::task::spawn_blocking(move || {
                let more = sources.record(&reading, now, reports);
                (sources, more)
            })
            .await
            else {
                return;
            };
            sources = back;
            for (total, more) in skipped.iter_mut().zip(more) {
                *total += more;
            }
            if now.duration_since(reported) < REPORT_EVERY {
                continue;
            }
            reported = now;
            for (source, total) in ["the hook's log", "the game's log"].iter().zip(skipped) {
                if total > 0 {
                    warn!(
                        bytes = total,
                        "diagnostics: skipped the oldest {total} bytes of {source}, over its budget"
                    );
                }
            }
            skipped = [0; 2];
            let now_dropped = recorder.dropped();
            if now_dropped > dropped {
                warn!(
                    lines = now_dropped - dropped,
                    "diagnostics: {} lines were lost, waiting too long or over this run's quota",
                    now_dropped - dropped
                );
            }
            dropped = now_dropped;
        }
    })
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn append(path: &Path, text: &str) {
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
    }

    /// A log is read from where it stood when the run began, line by line,
    /// and read anew from its start once it is made anew.
    #[test]
    fn a_tail_starts_where_the_run_began() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hook.log");
        append(&path, "before the run\nhalf of a line");
        let mut tail = Tail::from_end(path.clone(), 1 << 20);
        let now = Instant::now();
        assert_eq!(tail.poll(now), Polled::default());
        append(&path, " ends\nsecond\r\nthird");
        let polled = tail.poll(now);
        assert_eq!(polled.lines, [" ends", "second"]);
        append(&path, "\n");
        assert_eq!(tail.poll(now).lines, ["third"]);
        // Made anew, shorter: from its start.
        fs::write(&path, "new game\n").unwrap();
        assert_eq!(tail.poll(now).lines, ["new game"]);
    }

    /// A log that grows faster than its budget loses its oldest unread
    /// part, and says how much.
    #[test]
    fn a_tail_over_its_budget_skips_the_oldest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hook.log");
        fs::write(&path, "").unwrap();
        let mut tail = Tail::from_end(path.clone(), 100);
        let mut text = String::new();
        for line in 0..100 {
            text.push_str(&format!("line {line:04}\n"));
        }
        append(&path, &text);
        let now = Instant::now();
        let polled = tail.poll(now);
        // 1000 bytes, 100 a minute: the newest ten lines, less the one cut.
        assert_eq!(polled.lines.last().map(String::as_str), Some("line 0099"));
        assert!(polled.lines.len() <= 10, "{:?}", polled.lines);
        assert!(polled.skipped >= 900, "{}", polled.skipped);
        assert!(!polled.lines.iter().any(|line| line.contains("0000")));
        // The budget is spent: nothing more this instant.
        append(&path, "more\n");
        assert_eq!(tail.poll(now).lines, Vec::<String>::new());
        // A minute later, it is read.
        assert_eq!(tail.poll(now + Duration::from_secs(60)).lines, ["more"]);
    }

    /// A legacy-sized route burst can exceed D10's hook budget. The hook
    /// puts optional path rows first, so the tail still uploads every base
    /// vehicle row and the summary from the newest 192 KiB.
    #[test]
    fn an_oversized_route_burst_keeps_the_vehicle_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hook.log");
        fs::write(&path, "").unwrap();
        let mut sources = Sources::new(path.clone(), Vec::new());
        let recorder = Recorder::new();
        let mut burst = String::new();
        for edge in 0..2048 {
            burst.push_str(&format!(
                "lane 3 step 13000 vehicle-0/path-{edge:04} route_index={edge} geometry={}\n",
                "x".repeat(120)
            ));
        }
        for vehicle in 0..13 {
            burst.push_str(&format!(
                "lane 3 step 13000 vehicle-{vehicle} state=2 pos={vehicle} speed=5 row=2:0:3~10.20\n"
            ));
        }
        burst.push_str("lane 3 step 13000 summary 13:unchanged\n");
        append(&path, &burst);
        let skipped = sources.record(&recorder, Instant::now(), false);
        assert!(skipped[0] > 0, "fixture must exceed the hook budget");
        let waiting = recorder.waiting();
        for vehicle in 0..13 {
            assert!(
                waiting.iter().any(|line| line
                    .text
                    .as_str()
                    .contains(&format!("lane 3 step 13000 vehicle-{vehicle} state="))),
                "vehicle {vehicle}'s base entry must reach diagnostics"
            );
        }
        assert!(
            waiting
                .iter()
                .any(|line| line.text.as_str().contains("summary 13:unchanged"))
        );
    }

    #[test]
    fn optional_routes_cannot_evict_another_lanes_base_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hook.log");
        fs::write(&path, "").unwrap();
        let mut sources = Sources::new(path.clone(), Vec::new());
        let recorder = Recorder::new();
        let mut burst = String::new();
        for edge in 0..600 {
            burst.push_str(&format!(
                "lane 3 step 13000 vehicle-0/path-{edge:04} geometry={}\n",
                "x".repeat(100)
            ));
        }
        for edge in 0..1300 {
            burst.push_str(&format!(
                "lane 0 step 13000 edge-{edge:04} geometry={}\n",
                "y".repeat(80)
            ));
        }
        burst.push_str("lane 0 step 13000 summary 1300:unchanged\n");
        for vehicle in 0..300 {
            burst.push_str(&format!(
                "lane 3 step 13000 vehicle-{vehicle} state=2 pos=5 speed=5 row={}\n",
                "z".repeat(50)
            ));
        }
        burst.push_str("lane 3 step 13000 summary 300:unchanged\n");
        assert!(burst.len() as u64 > HOOK_PER_MINUTE);
        append(&path, &burst);
        assert!(sources.record(&recorder, Instant::now(), false)[0] > 0);
        let waiting = recorder.waiting();
        assert!(
            waiting
                .iter()
                .any(|line| line.text.as_str().contains("edge-0000"))
        );
        assert!(
            waiting
                .iter()
                .any(|line| line.text.as_str().contains("vehicle-299 state="))
        );
    }

    /// Error reports written since the run began are read once for each
    /// change, but not the game's log, minidumps or anything that looks
    /// like a key.
    #[test]
    fn error_reports_are_read_once_and_minidumps_never() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old_1.txt");
        fs::write(&old, "from before").unwrap();
        fs::File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(3600))
            .unwrap();
        let mut reports = CrashReports::new(vec![dir.path().to_owned()]);
        fs::write(dir.path().join("a_1.txt"), "line one\nline two\n").unwrap();
        fs::write(
            dir.path().join("error.json"),
            "{\n  \"userId\": \"125253817\",\n}\n",
        )
        .unwrap();
        fs::write(dir.path().join("a_0.dmp"), [0u8, 1, 2]).unwrap();
        fs::write(dir.path().join("stdout.txt"), "the game's log\n").unwrap();
        fs::write(dir.path().join("private.key.txt"), "secret\n").unwrap();
        let now = Instant::now();
        let mut got = reports.poll(now);
        got.sort_by(|a, b| a.name.cmp(&b.name));
        let names: Vec<&str> = got.iter().map(|report| report.name.as_str()).collect();
        assert_eq!(names, ["a_1.txt", "error.json"]);
        assert_eq!(got[0].lines, ["line one", "line two"]);
        assert!(reports.poll(now).is_empty(), "read once");
        append(&dir.path().join("a_1.txt"), "line three\n");
        assert_eq!(reports.poll(now).len(), 1, "changed: read again");
    }

    /// What is read of the files carries its source and is redacted.
    #[test]
    fn the_files_lines_are_redacted_and_tagged() {
        let dir = tempfile::tempdir().unwrap();
        let hook = dir.path().join("hook.log");
        let crash = dir.path().join("crash_dump");
        fs::create_dir_all(&crash).unwrap();
        fs::write(&hook, "").unwrap();
        fs::write(crash.join("stdout.txt"), "").unwrap();
        let mut sources = Sources::new(hook.clone(), vec![crash.clone()]);
        let recorder = Recorder::new();
        append(
            &hook,
            "[1] cannot open C:\\Users\\Ann\\x.sav from 192.0.2.7\n",
        );
        append(
            &crash.join("stdout.txt"),
            "[2026-10-02 15:53:49Z - ERROR    - Main Thread - Main ]  bad\n",
        );
        fs::write(crash.join("e_1.json"), "\"userId\": \"125253817\",\n").unwrap();
        // Reproduce coarse/skewed filesystem time without a timing-dependent sleep.
        File::options()
            .write(true)
            .open(crash.join("e_1.json"))
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(2))
            .unwrap();
        sources.record(&recorder, Instant::now(), true);
        let lines = recorder.waiting();
        assert_eq!(lines.len(), 3, "{lines:?}");
        let by = |source| {
            lines
                .iter()
                .find(|line| line.source == source)
                .unwrap()
                .clone()
        };
        let hook_line = by(LogSource::Hook);
        assert_eq!(
            hook_line.text.as_str(),
            "[1] cannot open <path>/x.sav from <ip>"
        );
        assert_eq!(hook_line.target.as_str(), "hook.log");
        let game = by(LogSource::Game);
        assert_eq!(game.level, DiagnosticLevel::Error);
        let report = by(LogSource::Crash);
        assert_eq!(report.target.as_str(), "e_1.json");
        assert!(!report.text.as_str().contains("125253817"), "{report:?}");
    }
    #[test]
    fn turning_off_and_on_between_polls_never_replays_off_period_logs() {
        let dir = tempfile::tempdir().unwrap();
        let hook = dir.path().join("hook.log");
        let game = dir.path().join("stdout.txt");
        let crash = dir.path().join("error.json");
        let mut sources = Sources::new(hook.clone(), vec![dir.path().to_owned()]);
        let recorder = Recorder::new();
        let now = Instant::now();
        for sampled_while_off in [false, true] {
            recorder.set_on(false);
            fs::write(&hook, "private while off\n").unwrap();
            fs::write(&game, "private while off\n").unwrap();
            fs::write(&crash, "private while off\n").unwrap();
            if sampled_while_off {
                sources.record(&recorder, now, true);
            }
            recorder.set_on(true);
            sources.record(&recorder, now, true);
            assert!(recorder.is_empty(), "off-period files must be skipped");
            append(&hook, "new hook\n");
            append(&game, "new game\n");
            fs::write(&crash, "new crash\n").unwrap();
            sources.record(&recorder, now, true);
            let lines = recorder.waiting();
            assert_eq!(lines.len(), 3, "{lines:?}");
            assert!(lines.iter().all(|l| !l.text.as_str().contains("private")));
        }
    }
}
