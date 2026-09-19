//! The Documents pane's view of a Project folder
//! (`docs/plan/07-ui-vocabulary.md` §3, M5-T04).
//!
//! What the pane needs is a tree of names, not of contents: v1 never renders
//! a file in the window, it hands the path to the operating system and lets
//! whatever normally opens a `.xlsx` open it. So this returns names, relative
//! paths and sizes, and nothing that would require reading a byte of anyone's
//! documents.
//!
//! Two limits keep it honest on a real folder. A Project may hold up to
//! [`crate::journal::MAX_FILES`] files, and a pane that tried to list two
//! hundred thousand of them would hang the window, so the walk stops at
//! [`MAX_ENTRIES`] and says that it did. It also stops at [`MAX_DEPTH`],
//! because a tree deeper than that is not something anyone is going to
//! navigate by clicking, and one symlink loop would otherwise walk forever.
//!
//! The same directories the Journal refuses to walk are skipped here
//! (`05-git-journal.md` §3): a `node_modules` in the Documents pane would be
//! both useless and the only thing anyone could see.

use std::path::Path;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::journal::{JournalError, SKIPPED_DIRS};

/// How many entries one listing carries. Past this the tree is cut and
/// [`DocumentTree::truncated`] says so: the pane can tell the person their
/// folder is bigger than the list, which is better than a list that lies by
/// stopping quietly.
pub const MAX_ENTRIES: usize = 2_000;

/// How deep the walk goes. Also what stops a symlinked loop.
pub const MAX_DEPTH: usize = 8;

/// One file or folder in the Documents pane.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct DocumentNode {
    /// What it is called. The last component of `path`.
    pub name: String,
    /// Relative to the Project root, with `/` separators on every platform so
    /// that it compares equal to the paths in a [`crate::event::Digest`].
    pub path: String,
    pub directory: bool,
    /// Size in bytes. Zero for a folder, and zero for a file whose size could
    /// not be read — this is a label, not an accounting.
    pub bytes: u64,
    /// Empty for a file, and for a folder the walk stopped at.
    pub children: Vec<DocumentNode>,
}

/// A Project folder, as far as the pane is told about it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct DocumentTree {
    pub entries: Vec<DocumentNode>,
    /// How many files the listing carries — folders are not counted.
    pub files: usize,
    /// True when the folder holds more than the listing shows, because it hit
    /// [`MAX_ENTRIES`] or [`MAX_DEPTH`].
    pub truncated: bool,
}

/// Lists `root` for the Documents pane.
///
/// A folder that cannot be read is left out rather than failing the listing:
/// one unreadable directory in a Project is not a reason to show the person
/// nothing. Only a `root` that cannot be read at all is an error.
pub fn list(root: &Path) -> Result<DocumentTree, JournalError> {
    let mut budget = MAX_ENTRIES;
    let mut truncated = false;
    let mut files = 0;
    let entries = walk(root, root, 0, &mut budget, &mut truncated, &mut files)?;
    Ok(DocumentTree {
        entries,
        files,
        truncated,
    })
}

fn walk(
    root: &Path,
    dir: &Path,
    depth: usize,
    budget: &mut usize,
    truncated: &mut bool,
    files: &mut usize,
) -> Result<Vec<DocumentNode>, JournalError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        // Only the root's own unreadability is worth an error; a folder
        // inside it is simply not listed.
        Err(error) if dir == root => return Err(JournalError::io(dir, error)),
        Err(_) => {
            *truncated = true;
            return Ok(Vec::new());
        }
    };

    let mut nodes = Vec::new();
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();

        if kind.is_dir() && SKIPPED_DIRS.contains(&name.as_str()) {
            continue;
        }
        if *budget == 0 {
            *truncated = true;
            break;
        }
        *budget -= 1;

        // A symlink is listed as what it is called and never followed: a link
        // out of the Project is not part of the Project, and a link back into
        // it is the same files twice.
        let directory = kind.is_dir();
        let children = if directory && !kind.is_symlink() {
            if depth + 1 < MAX_DEPTH {
                walk(root, &path, depth + 1, budget, truncated, files)?
            } else {
                *truncated = true;
                Vec::new()
            }
        } else {
            Vec::new()
        };
        if !directory {
            *files += 1;
        }

        nodes.push(DocumentNode {
            name,
            path: slashed(relative),
            directory,
            bytes: if directory {
                0
            } else {
                entry.metadata().map(|meta| meta.len()).unwrap_or(0)
            },
            children,
        });
    }

    // Folders first, then files, each in the order a person reads a list in.
    nodes.sort_by(|a, b| {
        b.directory
            .cmp(&a.directory)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(nodes)
}

/// A relative path with `/` separators, whatever the platform uses. The
/// digest's paths are written this way, and the pane marks a file as changed
/// by comparing the two.
fn slashed(relative: &Path) -> String {
    relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, relative: &str, contents: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn it_lists_folders_first_then_files_each_in_reading_order() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "zebra.txt", "z");
        write(root, "Apple.txt", "a");
        write(root, "reports/q1.xlsx", "q");
        write(root, "Admin/notes.docx", "n");

        let tree = list(root).unwrap();
        let names: Vec<&str> = tree.entries.iter().map(|node| node.name.as_str()).collect();
        assert_eq!(names, vec!["Admin", "reports", "Apple.txt", "zebra.txt"]);
        assert_eq!(
            tree.files, 4,
            "the files inside the folders are counted too"
        );
        assert!(!tree.truncated);
    }

    #[test]
    fn a_child_carries_the_path_the_digest_would_use() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "reports/2026/q1.xlsx", "q");

        let tree = list(root).unwrap();
        let reports = &tree.entries[0];
        assert_eq!(reports.path, "reports");
        assert!(reports.directory);
        let year = &reports.children[0];
        assert_eq!(year.path, "reports/2026");
        let file = &year.children[0];
        assert_eq!(file.path, "reports/2026/q1.xlsx");
        assert!(!file.directory);
        assert_eq!(file.bytes, 1);
        assert!(file.children.is_empty());
    }

    #[test]
    fn the_folders_the_journal_skips_are_not_documents() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "node_modules/left-pad/index.js", "x");
        write(root, ".git/config", "x");
        write(root, "budget.xlsx", "b");

        let tree = list(root).unwrap();
        let names: Vec<&str> = tree.entries.iter().map(|node| node.name.as_str()).collect();
        assert_eq!(names, vec!["budget.xlsx"]);
        assert_eq!(tree.files, 1);
    }

    #[test]
    fn a_folder_bigger_than_the_listing_says_so_instead_of_stopping_quietly() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for index in 0..(MAX_ENTRIES + 10) {
            write(root, &format!("file-{index:05}.txt"), "x");
        }

        let tree = list(root).unwrap();
        assert!(
            tree.truncated,
            "the person is told the list is not all of it"
        );
        assert_eq!(tree.entries.len(), MAX_ENTRIES);
    }

    #[test]
    fn it_stops_going_deeper_rather_than_walking_forever() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let deep = (0..MAX_DEPTH + 3)
            .map(|_| "down")
            .collect::<Vec<_>>()
            .join("/");
        write(root, &format!("{deep}/buried.txt"), "x");

        let tree = list(root).unwrap();
        assert!(tree.truncated);

        // Follow the one chain down and count how far the listing goes.
        let mut depth = 0;
        let mut node = &tree.entries[0];
        while let Some(child) = node.children.first() {
            depth += 1;
            node = child;
        }
        assert!(depth < MAX_DEPTH, "stopped at the limit, depth was {depth}");
    }

    #[test]
    fn a_root_that_is_not_there_is_an_error_a_folder_inside_it_is_not() {
        let dir = tempfile::tempdir().unwrap();
        assert!(list(&dir.path().join("nowhere")).is_err());
    }
}
