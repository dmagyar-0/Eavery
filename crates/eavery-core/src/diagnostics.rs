//! The log file, and what the Diagnostics panel reads from it
//! (`docs/plan/03-architecture.md` §8–9, M3-T08).
//!
//! `tracing` output goes to `<data_dir>/logs/eavery.log`. This module knows
//! where that is, opens it with a rotation that keeps the folder bounded, and
//! reads its tail back for the panel — so the desktop shell and the CLI agree
//! on the one file without either having to know the layout.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The current log, under `<data_dir>/logs/`.
pub const LOG_FILE_NAME: &str = "eavery.log";

/// The previous log, kept once so a crash on startup does not take the
/// evidence with it.
pub const ROTATED_FILE_NAME: &str = "eavery.log.1";

/// Above this, the log is rotated on the next start. Ten megabytes is a long
/// history of a chatty engine at `trace`, and the panel only ever reads the
/// end of it.
pub const ROTATE_ABOVE_BYTES: u64 = 10 * 1024 * 1024;

/// How many lines the panel is given when it does not say.
pub const DEFAULT_TAIL_LINES: usize = 200;

/// What the Diagnostics panel shows (`07-ui-vocabulary.md` §3): where the
/// data lives, where the log is, and the end of it.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct Diagnostics {
    /// Eavery's own version, from the crate that built the app.
    pub version: String,
    pub data_dir: PathBuf,
    pub log_path: PathBuf,
    /// The last lines of the log, oldest first. Empty when there is no log
    /// yet, which is not an error: a first run has nothing to say.
    pub log_tail: Vec<String>,
}

/// Where the log lives for a data directory.
pub fn log_path(data_dir: &Path) -> PathBuf {
    data_dir.join("logs").join(LOG_FILE_NAME)
}

/// Opens the log for appending, creating the folder on the way, and rotates
/// first if the previous run left it above [`ROTATE_ABOVE_BYTES`].
///
/// Rotation is a rename, so the old log survives exactly one more run. It
/// happens only at open — a log that grows past the limit during one run is
/// not cut mid-line — which keeps the writer a plain file with no timer
/// behind it.
pub fn open_log(data_dir: &Path) -> io::Result<File> {
    let path = log_path(data_dir);
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)?;
    }
    if std::fs::metadata(&path).is_ok_and(|meta| meta.len() > ROTATE_ABOVE_BYTES) {
        std::fs::rename(&path, path.with_file_name(ROTATED_FILE_NAME))?;
    }
    OpenOptions::new().create(true).append(true).open(path)
}

/// The last `lines` lines of a file, oldest first. A file that does not exist
/// yet reads as empty.
///
/// Reads from the end in blocks rather than the whole file: the log is the
/// one file here with no upper bound on its size within a run.
pub fn tail(path: &Path, lines: usize) -> io::Result<Vec<String>> {
    if lines == 0 {
        return Ok(Vec::new());
    }
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let len = file.metadata()?.len();

    const BLOCK: u64 = 64 * 1024;
    let mut collected: Vec<u8> = Vec::new();
    let mut end = len;
    // A trailing newline ends the last line rather than starting an empty one.
    let mut newlines_needed = lines + 1;
    while end > 0 && count_newlines(&collected) < newlines_needed {
        let start = end.saturating_sub(BLOCK);
        let mut block = vec![0; (end - start) as usize];
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(&mut block)?;
        block.extend_from_slice(&collected);
        collected = block;
        end = start;
        if end == 0 {
            // The whole file is in hand; there are no more newlines to find.
            newlines_needed = 0;
        }
    }

    let text = String::from_utf8_lossy(&collected);
    let mut found: Vec<String> = text.lines().map(str::to_owned).collect();
    if found.len() > lines {
        found.drain(..found.len() - lines);
    }
    Ok(found)
}

fn count_newlines(bytes: &[u8]) -> usize {
    bytes.iter().filter(|&&byte| byte == b'\n').count()
}

impl Diagnostics {
    /// Reads the panel's contents for a data directory. Missing pieces read as
    /// empty rather than failing: the panel exists for when something is
    /// wrong, so it must not be the thing that fails.
    pub fn read(data_dir: &Path, version: &str, lines: usize) -> Self {
        let log_path = log_path(data_dir);
        let log_tail = tail(&log_path, lines)
            .unwrap_or_else(|error| vec![format!("(the log could not be read: {error})")]);
        Self {
            version: version.to_owned(),
            data_dir: data_dir.to_path_buf(),
            log_path,
            log_tail,
        }
    }

    /// The panel's "Copy diagnostics" text: everything above, as one block.
    pub fn render(&self) -> String {
        let mut out = format!(
            "Eavery {}\ndata: {}\nlog: {}\n\n",
            self.version,
            self.data_dir.display(),
            self.log_path.display()
        );
        for line in &self.log_tail {
            out.push_str(line);
            out.push('\n');
        }
        out
    }
}
