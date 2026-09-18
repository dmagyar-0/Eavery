//! The log file and its tail (M3-T08).

use std::io::Write;

use eavery_core::diagnostics::{
    self, Diagnostics, LOG_FILE_NAME, ROTATE_ABOVE_BYTES, ROTATED_FILE_NAME,
};

#[test]
fn a_first_run_has_no_log_and_that_is_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let read = Diagnostics::read(dir.path(), "0.1.0", 50);
    assert_eq!(read.log_tail, Vec::<String>::new());
    assert_eq!(read.data_dir, dir.path());
    assert!(read.log_path.ends_with(LOG_FILE_NAME));
}

#[test]
fn opening_the_log_makes_the_folder_and_appends() {
    let dir = tempfile::tempdir().unwrap();
    {
        let mut log = diagnostics::open_log(dir.path()).unwrap();
        writeln!(log, "first").unwrap();
    }
    {
        let mut log = diagnostics::open_log(dir.path()).unwrap();
        writeln!(log, "second").unwrap();
    }
    let read = Diagnostics::read(dir.path(), "0.1.0", 50);
    assert_eq!(read.log_tail, vec!["first", "second"]);
}

#[test]
fn the_tail_is_the_last_lines_oldest_first() {
    let dir = tempfile::tempdir().unwrap();
    let mut log = diagnostics::open_log(dir.path()).unwrap();
    for n in 1..=1000 {
        writeln!(log, "line {n}").unwrap();
    }
    drop(log);

    let tail = diagnostics::tail(&diagnostics::log_path(dir.path()), 3).unwrap();
    assert_eq!(tail, vec!["line 998", "line 999", "line 1000"]);

    // More lines than the file has: the whole file, no padding.
    let all = diagnostics::tail(&diagnostics::log_path(dir.path()), 5000).unwrap();
    assert_eq!(all.len(), 1000);
    assert_eq!(all[0], "line 1");

    assert!(
        diagnostics::tail(&diagnostics::log_path(dir.path()), 0)
            .unwrap()
            .is_empty()
    );
}

/// Lines longer than the block the tail reads in must still come back whole.
#[test]
fn a_long_line_crosses_the_read_block_intact() {
    let dir = tempfile::tempdir().unwrap();
    let mut log = diagnostics::open_log(dir.path()).unwrap();
    let long = "x".repeat(200 * 1024);
    writeln!(log, "before").unwrap();
    writeln!(log, "{long}").unwrap();
    writeln!(log, "after").unwrap();
    drop(log);

    let tail = diagnostics::tail(&diagnostics::log_path(dir.path()), 2).unwrap();
    assert_eq!(tail.len(), 2);
    assert_eq!(tail[0], long);
    assert_eq!(tail[1], "after");
}

#[test]
fn a_log_over_the_limit_is_rotated_on_the_next_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = diagnostics::log_path(dir.path());
    {
        let mut log = diagnostics::open_log(dir.path()).unwrap();
        log.write_all(&vec![b'a'; (ROTATE_ABOVE_BYTES + 1) as usize])
            .unwrap();
        writeln!(log).unwrap();
        writeln!(log, "old").unwrap();
    }

    let mut log = diagnostics::open_log(dir.path()).unwrap();
    writeln!(log, "new").unwrap();
    drop(log);

    assert_eq!(
        diagnostics::tail(&path, 10).unwrap(),
        vec!["new"],
        "the new log starts empty"
    );
    let rotated = path.with_file_name(ROTATED_FILE_NAME);
    assert_eq!(
        diagnostics::tail(&rotated, 1).unwrap(),
        vec!["old"],
        "and the old one is kept once"
    );
}

#[test]
fn the_copy_text_names_the_places_and_carries_the_tail() {
    let dir = tempfile::tempdir().unwrap();
    let mut log = diagnostics::open_log(dir.path()).unwrap();
    writeln!(log, "something happened").unwrap();
    drop(log);

    let text = Diagnostics::read(dir.path(), "0.1.0", 10).render();
    assert!(text.starts_with("Eavery 0.1.0\n"), "{text}");
    assert!(text.contains(&format!("data: {}", dir.path().display())));
    assert!(text.ends_with("something happened\n"), "{text}");
}
