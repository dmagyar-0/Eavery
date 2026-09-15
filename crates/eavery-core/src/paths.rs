//! Path handling that behaves the same on all three platforms.
//!
//! `std::fs::canonicalize` returns a Windows *verbatim* path (`\\?\C:\...`).
//! Verbatim paths are handed to the filesystem literally: forward slashes stop
//! working as separators, and a prefix comparison against a path obtained any
//! other way fails. Engines are given the Project root over the wire and join
//! onto it with whichever separator they like, so a verbatim root breaks them.
//!
//! Everything here goes through `dunce`, which canonicalises to the verbatim
//! form only when there is no ordinary path that would do. This is the
//! normalisation `docs/plan/06-plan-gate-permissions.md` §3.1 requires, and
//! `policy::is_inside` (M4-T02) builds on it.

use std::path::{Path, PathBuf};

/// Resolves symlinks and `..`, without a Windows verbatim prefix where an
/// ordinary path exists.
pub fn canonicalize(path: impl AsRef<Path>) -> std::io::Result<PathBuf> {
    dunce::canonicalize(path)
}

/// The same, falling back to the path as given when it cannot be resolved —
/// because it does not exist, or because it cannot be read.
pub fn canonical_or_self(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Canonicalises as much of `path` as exists and keeps the rest, so a file
/// about to be created is judged by the directory it will land in.
pub fn nearest_existing(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    if path.exists() {
        return canonical_or_self(path);
    }
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => nearest_existing(parent).join(name),
        _ => path.to_path_buf(),
    }
}

/// Whether `path` sits inside `root`. A path that does not exist yet is judged
/// by its nearest existing ancestor.
///
/// Both sides are canonicalised the same way, which is the whole point: a
/// comparison between a verbatim path and an ordinary one is always false, and
/// on the `06` §3.1 decision table that turns every ordinary edit into a
/// `Destructive` one.
pub fn is_inside(path: impl AsRef<Path>, root: impl AsRef<Path>) -> bool {
    nearest_existing(path).starts_with(canonical_or_self(root))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The regression that failed CI on Windows and passed everywhere else: the
    /// canonical form of a real directory must be a path an engine can join
    /// onto, not a verbatim one.
    #[test]
    fn canonicalising_does_not_produce_a_verbatim_path() {
        let dir = tempfile::tempdir().unwrap();
        let canonical = canonicalize(dir.path()).unwrap();
        assert!(
            !canonical.to_string_lossy().starts_with(r"\\?\"),
            "canonicalize produced a verbatim path: {}",
            canonical.display()
        );
    }

    #[test]
    fn a_path_that_cannot_be_resolved_comes_back_unchanged() {
        let missing = Path::new("eavery-no-such-path-anywhere");
        assert_eq!(canonical_or_self(missing), missing);
    }

    #[test]
    fn a_file_that_does_not_exist_yet_is_judged_by_its_parent() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/here.txt"), "x").unwrap();

        assert!(is_inside(root.join("sub/here.txt"), root));
        assert!(is_inside(root.join("sub/new.txt"), root));
        assert!(is_inside(root.join("sub/deeper/new.txt"), root));
    }

    #[test]
    fn a_path_outside_the_root_is_outside() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        assert!(!is_inside(outside.path().join("other.txt"), dir.path()));
    }

    /// Engines join onto the root with whichever separator they like. On
    /// Windows a forward slash is fine in an ordinary path and meaningless in a
    /// verbatim one, which is the other half of the same bug.
    #[test]
    fn a_forward_slash_join_still_lands_inside() {
        let dir = tempfile::tempdir().unwrap();
        let root = canonicalize(dir.path()).unwrap();
        let joined = PathBuf::from(format!("{}/notes.txt", root.display()));
        assert!(
            is_inside(&joined, &root),
            "{} is not inside {}",
            joined.display(),
            root.display()
        );
    }

    /// Writing through a path built the way an engine builds it must reach the
    /// same file the caller sees.
    #[test]
    fn a_write_through_a_joined_path_reaches_the_same_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = canonicalize(dir.path()).unwrap();
        let joined = PathBuf::from(format!("{}/notes.txt", root.display()));

        std::fs::write(&joined, "FY26").unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("notes.txt")).unwrap(),
            "FY26"
        );
    }
}
