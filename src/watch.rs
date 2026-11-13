//! `run --watch`: after a run, the files that change are run again.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

/// How often the files are looked at, and how long a change must stay
/// quiet (an editor saving in two writes) before it is run.
const POLL: Duration = Duration::from_millis(300);

type Stamps = BTreeMap<PathBuf, (Option<SystemTime>, u64)>;

/// Each scenario file's modified time and size: polling them needs no
/// dependency, and a file that appears later is found on the next look.
pub fn stamps(paths: &[PathBuf]) -> Stamps {
    let mut out = Stamps::new();
    for f in crate::suite::discover(paths).unwrap_or_default() {
        if let Ok(m) = std::fs::metadata(&f) {
            out.insert(f, (m.modified().ok(), m.len()));
        }
    }
    out
}

/// The files in `now` that are new or differ from `before`.
pub fn changed(before: &Stamps, now: &Stamps) -> Vec<PathBuf> {
    now.iter().filter(|(f, s)| before.get(*f) != Some(*s)).map(|(f, _)| f.clone()).collect()
}

/// Waits for a change under `paths` and returns the files that changed, once
/// they have stayed quiet for one poll.
pub fn wait(paths: &[PathBuf], before: &mut Stamps) -> Vec<PathBuf> {
    loop {
        std::thread::sleep(POLL);
        let now = stamps(paths);
        let first = changed(before, &now);
        if first.is_empty() {
            // Only deletions: nothing to run, but they are seen.
            *before = now;
            continue;
        }
        // Still being written? Look again after another poll.
        std::thread::sleep(POLL);
        let settled = stamps(paths);
        let mut files = first;
        for f in changed(&now, &settled) {
            if !files.contains(&f) {
                files.push(f);
            }
        }
        files.retain(|f| settled.contains_key(f));
        *before = settled;
        if !files.is_empty() {
            return files;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_changed_or_new_file_is_found_and_an_untouched_one_is_not() {
        let dir = std::env::temp_dir().join(format!("bitti-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.bt"), "scenario \"a\"\n  goto /\n").unwrap();
        std::fs::write(dir.join("b.bt"), "scenario \"b\"\n  goto /\n").unwrap();
        let paths = vec![dir.clone()];
        let before = stamps(&paths);
        assert_eq!(before.len(), 2);
        assert!(changed(&before, &stamps(&paths)).is_empty());
        std::fs::write(dir.join("b.bt"), "scenario \"b changed\"\n  goto /\n").unwrap();
        std::fs::write(dir.join("c.bt"), "scenario \"c\"\n  goto /\n").unwrap();
        std::fs::write(dir.join("notes.txt"), "not a scenario").unwrap();
        let names: Vec<String> = changed(&before, &stamps(&paths))
            .iter()
            .map(|f| f.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, ["b.bt", "c.bt"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
