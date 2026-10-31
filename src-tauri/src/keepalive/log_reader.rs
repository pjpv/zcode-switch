//! Bounded, incremental observation of ZCode's structured daily CLI logs.
use chrono::{DateTime, NaiveDate};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

const MAX_FILES: usize = 3;
const MAX_POLL_BYTES: usize = 1024 * 1024;
const MAX_LINE_BYTES: usize = 64 * 1024;
const MAX_DEDUP: usize = 4096;
const ANCHOR_BYTES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventKind {
    CaptchaFailure,
    TurnStarted,
    Progress,
    TurnCompleted,
    OtherFailure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedEvent {
    pub session_id: String,
    pub turn_id: String,
    pub at_ms: i64,
    pub kind: EventKind,
}

#[derive(Default)]
struct Cursor {
    offset: u64,
    pending: Vec<u8>,
    discard_line: bool,
    anchor: Vec<u8>,
}

pub struct LogReader {
    log_dir: PathBuf,
    enabled_at_ms: i64,
    cursors: HashMap<PathBuf, Cursor>,
    seen: HashSet<(String, String, i64, EventKind)>,
    seen_order: VecDeque<(String, String, i64, EventKind)>,
    // When the cache evicts entries, reject older timestamps rather than replaying
    // rotated history. Same-millisecond overflow is also rejected, conservatively.
    retired_through_ms: i64,
}

impl LogReader {
    pub fn new(log_dir: PathBuf, enabled_at_ms: i64) -> Result<Self, String> {
        let mut reader = Self {
            log_dir,
            enabled_at_ms,
            cursors: HashMap::new(),
            seen: HashSet::new(),
            seen_order: VecDeque::new(),
            retired_through_ms: i64::MIN,
        };
        for path in reader.daily_files()? {
            let mut file = File::open(&path).map_err(io_error)?;
            let offset = file.metadata().map_err(io_error)?.len();
            let anchor = read_anchor(&mut file, offset)?;
            // If enable happens halfway through a record, its suffix is historical.
            let discard_line = anchor.last().is_some_and(|b| *b != b'\n');
            reader.cursors.insert(
                path,
                Cursor {
                    offset,
                    anchor,
                    discard_line,
                    pending: Vec::new(),
                },
            );
        }
        Ok(reader)
    }

    pub fn poll(&mut self) -> Result<Vec<ObservedEvent>, String> {
        let paths = self.daily_files()?;
        self.cursors.retain(|path, _| paths.contains(path));
        let mut remaining = MAX_POLL_BYTES;
        let mut candidates = Vec::new();
        // Oldest first prevents a busy new file starving the previous day's tail.
        for path in paths {
            if remaining == 0 {
                break;
            }
            let mut file = match File::open(&path) {
                Ok(file) => file,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(io_error(e)),
            };
            let cursor = self.cursors.entry(path).or_default();
            let len = file.metadata().map_err(io_error)?.len();
            if len < cursor.offset || read_anchor(&mut file, cursor.offset)? != cursor.anchor {
                *cursor = Cursor::default();
            }
            file.seek(SeekFrom::Start(cursor.offset))
                .map_err(io_error)?;
            let mut bytes = Vec::new();
            file.by_ref()
                .take(remaining as u64)
                .read_to_end(&mut bytes)
                .map_err(io_error)?;
            remaining -= bytes.len();
            cursor.offset += bytes.len() as u64;
            // Derive the anchor from bytes actually consumed, never from a later
            // reread which could silently accept a concurrent truncate/rewrite.
            cursor.anchor.extend_from_slice(&bytes);
            if cursor.anchor.len() > ANCHOR_BYTES {
                cursor.anchor.drain(..cursor.anchor.len() - ANCHOR_BYTES);
            }
            for byte in bytes {
                if byte == b'\n' {
                    if !cursor.discard_line {
                        if let Some(event) = parse_event(&cursor.pending, self.enabled_at_ms) {
                            candidates.push(event);
                        }
                    }
                    cursor.pending.clear();
                    cursor.discard_line = false;
                } else if !cursor.discard_line {
                    if cursor.pending.len() == MAX_LINE_BYTES {
                        cursor.pending.clear();
                        cursor.discard_line = true;
                    } else {
                        cursor.pending.push(byte);
                    }
                }
            }
        }
        candidates.sort_by_key(|event| event.at_ms);
        let mut events = Vec::new();
        for event in candidates {
            let key = (
                event.session_id.clone(),
                event.turn_id.clone(),
                event.at_ms,
                event.kind,
            );
            if event.at_ms <= self.retired_through_ms || !self.seen.insert(key.clone()) {
                continue;
            }
            self.seen_order.push_back(key);
            if self.seen_order.len() > MAX_DEDUP {
                if let Some(old) = self.seen_order.pop_front() {
                    self.retired_through_ms = self.retired_through_ms.max(old.2);
                    self.seen.remove(&old);
                }
            }
            events.push(event);
        }
        Ok(events)
    }

    fn daily_files(&self) -> Result<Vec<PathBuf>, String> {
        let entries = match fs::read_dir(&self.log_dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(io_error(e)),
        };
        let mut paths = Vec::new();
        for entry in entries {
            let entry = entry.map_err(io_error)?;
            if !entry.file_type().map_err(io_error)?.is_file() {
                continue;
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let Some(date) = name
                .strip_prefix("zcode-")
                .and_then(|s| s.strip_suffix(".jsonl"))
            else {
                continue;
            };
            if date.len() != 10
                || NaiveDate::parse_from_str(date, "%Y-%m-%d")
                    .map(|d| d.format("%Y-%m-%d").to_string() != date)
                    .unwrap_or(true)
            {
                continue;
            }
            paths.push(entry.path());
            paths.sort();
            if paths.len() > MAX_FILES {
                paths.remove(0);
            }
        }
        Ok(paths)
    }
}

fn io_error(error: std::io::Error) -> String {
    format!("Unable to read ZCode activity logs: {error}")
}

fn read_anchor(file: &mut File, offset: u64) -> Result<Vec<u8>, String> {
    let start = offset.saturating_sub(ANCHOR_BYTES as u64);
    file.seek(SeekFrom::Start(start)).map_err(io_error)?;
    let mut bytes = Vec::new();
    file.take(offset - start)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    Ok(bytes)
}

fn parse_event(line: &[u8], enabled_at_ms: i64) -> Option<ObservedEvent> {
    let value: Value = serde_json::from_slice(line).ok()?;
    let at_ms = DateTime::parse_from_rfc3339(value.get("timestamp")?.as_str()?)
        .ok()?
        .timestamp_millis();
    if at_ms < enabled_at_ms {
        return None;
    }
    let session_id = value.get("sessionId")?.as_str()?.trim();
    let turn_id = value.get("turnId")?.as_str()?.trim();
    if session_id.is_empty() || turn_id.is_empty() {
        return None;
    }
    let kind = match value.get("event")?.as_str()? {
        "turn.started" => EventKind::TurnStarted,
        "turn.completed" => EventKind::TurnCompleted,
        "model.request.completed" => EventKind::Progress,
        "turn.failed" if value.get("error").is_some_and(is_captcha_error) => {
            EventKind::CaptchaFailure
        }
        "turn.failed" => EventKind::OtherFailure,
        _ => return None,
    };
    Some(ObservedEvent {
        session_id: session_id.to_owned(),
        turn_id: turn_id.to_owned(),
        at_ms,
        kind,
    })
}

fn is_captcha_error(error: &Value) -> bool {
    let mut current = Some(error);
    let mut known_type = false;
    let mut known_message = false;
    for _ in 0..16 {
        let Some(node) = current else {
            break;
        };
        known_type |= matches!(
            node.get("name").and_then(Value::as_str),
            Some("RuntimeHeadersRefreshError" | "ProtocolRequestError")
        );
        known_message |= matches!(
            node.get("message").and_then(Value::as_str),
            Some(
                "Captcha verification failed. Please try again."
                    | "Captcha verification request timed out. Please send your message again."
                    | "Captcha instance timed out after 10000ms."
            )
        );
        current = node.get("cause");
    }
    known_type && known_message
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    const NOW: i64 = 1_790_726_400_000;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "zswitch-log-tests-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> PathBuf {
            self.0.join("zcode-2026-09-30.jsonl")
        }
        fn append(&self, bytes: &[u8]) {
            fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.path())
                .unwrap()
                .write_all(bytes)
                .unwrap();
        }
        fn reader(&self) -> LogReader {
            LogReader::new(self.0.clone(), NOW).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn event(id: &str, ms: i64, name: &str) -> String {
        format!(
            "{}\n",
            serde_json::json!({"timestamp": DateTime::from_timestamp_millis(ms).unwrap().to_rfc3339(),
            "event": name, "sessionId": "session", "turnId": id,
            "error": {"name":"Error","message":"Turn execution failed","cause":{
                "name":"AiSdkModelAdapterError","message":"Captcha verification failed. Please try again.",
                "cause":{"name":"RuntimeHeadersRefreshError","cause":{"name":"ProtocolRequestError"}}}}})
        )
    }
    #[test]
    fn ignores_history_and_requires_structured_captcha_chain() {
        let f = Fixture::new();
        f.append(event("old", NOW, "turn.failed").as_bytes());
        let mut reader = f.reader();
        assert!(reader.poll().unwrap().is_empty());
        f.append(event("too-old", NOW - 1, "turn.failed").as_bytes());
        f.append(event("good", NOW, "turn.failed").as_bytes());
        f.append(event("not-failure", NOW, "unrelated").as_bytes());
        f.append(
            event("ordinary", NOW, "turn.failed")
                .replace(
                    "Captcha verification failed. Please try again.",
                    "Network timeout",
                )
                .as_bytes(),
        );
        f.append(
            event("wrong-type", NOW, "turn.failed")
                .replace("RuntimeHeadersRefreshError", "OtherError")
                .replace("ProtocolRequestError", "OtherError")
                .as_bytes(),
        );
        let found = reader.poll().unwrap();
        assert_eq!(found.len(), 3);
        assert_eq!(found[0].kind, EventKind::CaptchaFailure);
        assert_eq!(found[1].kind, EventKind::OtherFailure);
        assert_eq!(found[2].kind, EventKind::OtherFailure);
    }
    #[test]
    fn partial_writes_and_partial_record_at_enable() {
        let f = Fixture::new();
        let old = event("historical", NOW, "turn.failed");
        f.append(&old.as_bytes()[..30]);
        let mut reader = f.reader();
        f.append(&old.as_bytes()[30..]);
        assert!(reader.poll().unwrap().is_empty());
        let next = event("new", NOW, "turn.failed");
        f.append(&next.as_bytes()[..40]);
        assert!(reader.poll().unwrap().is_empty());
        f.append(&next.as_bytes()[40..]);
        assert_eq!(reader.poll().unwrap()[0].turn_id, "new");
        assert!(reader.poll().unwrap().is_empty());
    }
    #[test]
    fn new_daily_file_and_truncation_do_not_replay() {
        let f = Fixture::new();
        let mut reader = f.reader();
        let first = event("first", NOW, "turn.failed");
        f.append(first.as_bytes());
        assert_eq!(reader.poll().unwrap().len(), 1);
        fs::write(f.path(), &first).unwrap();
        assert!(reader.poll().unwrap().is_empty());
        fs::write(
            f.path(),
            format!("{first}{}", event("second", NOW + 1, "turn.started")),
        )
        .unwrap();
        assert_eq!(reader.poll().unwrap()[0].kind, EventKind::TurnStarted);
        fs::write(f.path(), event("third", NOW + 2, "turn.completed")).unwrap();
        assert_eq!(reader.poll().unwrap()[0].kind, EventKind::TurnCompleted);
        fs::write(
            f.0.join("zcode-2026-10-01.jsonl"),
            format!(
                "{first}{}",
                event("fourth", NOW + 3, "model.request.completed")
            ),
        )
        .unwrap();
        let result = reader.poll().unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].kind, EventKind::Progress);
    }
    #[test]
    fn malformed_records_and_wrong_filenames_are_ignored() {
        let f = Fixture::new();
        let mut reader = f.reader();
        for line in [
            event("", NOW, "turn.failed"),
            event("x", NOW, "turn.failed").replace("sessionId", "missing"),
            event("x", NOW, "turn.failed").replace("timestamp", "missing"),
            "Captcha verification failed. Please try again.\n".into(),
        ] {
            f.append(line.as_bytes());
        }
        for name in [
            "zcode-2026-9-30.jsonl",
            "zcode-2026-02-30.jsonl",
            "other.jsonl",
        ] {
            fs::write(f.0.join(name), event("x", NOW, "turn.failed")).unwrap();
        }
        assert!(reader.poll().unwrap().is_empty());
    }
    #[test]
    fn known_timeout_variants_require_failure_event_and_error_chain() {
        for message in [
            "Captcha verification request timed out. Please send your message again.",
            "Captcha instance timed out after 10000ms.",
        ] {
            let line = event("x", NOW, "turn.failed")
                .replace("Captcha verification failed. Please try again.", message);
            assert_eq!(
                parse_event(line.as_bytes(), NOW).unwrap().kind,
                EventKind::CaptchaFailure
            );
        }
        let mut value: Value = serde_json::from_str(&event("x", NOW, "turn.failed")).unwrap();
        value["error"] =
            serde_json::json!({"name":"NetworkError","message":"Network request failed"});
        value["context"] = serde_json::json!({"name":"ProtocolRequestError","message":"Captcha verification failed. Please try again."});
        assert_eq!(
            parse_event(value.to_string().as_bytes(), NOW).unwrap().kind,
            EventKind::OtherFailure
        );
    }
    #[test]
    fn oversized_lines_and_poll_budget_are_bounded() {
        let f = Fixture::new();
        let mut reader = f.reader();
        f.append(&vec![b'x'; MAX_POLL_BYTES + MAX_LINE_BYTES]);
        f.append(b"\n");
        f.append(event("good", NOW, "turn.failed").as_bytes());
        assert!(reader.poll().unwrap().is_empty());
        let cursor = reader.cursors.get(&f.path()).unwrap();
        assert_eq!(cursor.offset, MAX_POLL_BYTES as u64);
        assert!(cursor.pending.len() <= MAX_LINE_BYTES);
        assert_eq!(reader.poll().unwrap()[0].turn_id, "good");
    }
    #[test]
    fn dedup_cache_is_bounded_and_evicted_history_is_rejected() {
        let f = Fixture::new();
        let mut reader = f.reader();
        for n in 0..MAX_DEDUP + 10 {
            f.append(event(&n.to_string(), NOW + n as i64, "turn.started").as_bytes());
        }
        let mut count = 0;
        for _ in 0..5 {
            count += reader.poll().unwrap().len();
        }
        assert_eq!(count, MAX_DEDUP + 10);
        assert_eq!(reader.seen.len(), MAX_DEDUP);
        fs::write(f.path(), event("0", NOW, "turn.started")).unwrap();
        assert!(reader.poll().unwrap().is_empty());
    }
}
