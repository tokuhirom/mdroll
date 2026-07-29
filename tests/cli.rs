//! End-to-end checks on the binary itself.
//!
//! These exist because the things they cover — exit codes, what lands on
//! stderr, what a pipe gets — are invisible to the unit tests and are exactly
//! what a person notices first.

use std::process::{Command, Stdio};

fn mdroll() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mdroll"));
    // Piped stdout means the document is rendered once and the process exits,
    // so none of this needs a terminal.
    command.stdin(Stdio::null());
    command
}

fn run(args: &[&str]) -> (bool, String, String) {
    let output = mdroll().args(args).output().expect("mdroll runs");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn a_missing_file_is_one_clean_line_on_stderr() {
    let (ok, _, stderr) = run(&["definitely-not-here.md"]);
    assert!(!ok, "a missing file must not exit successfully");
    assert!(stderr.starts_with("mdroll: "), "{stderr:?}");
    assert!(stderr.contains("definitely-not-here.md"));
    // A `Result` returned from main is formatted with `Debug`, which drags a
    // stack backtrace along with it. Nobody needs one to be told a path does
    // not exist.
    assert!(!stderr.contains("backtrace"), "{stderr}");
    assert!(!stderr.contains("stack"), "{stderr}");
    assert_eq!(stderr.lines().count(), 1, "{stderr:?}");
}

#[test]
fn a_backtrace_is_not_dragged_in_even_when_the_environment_asks_for_one() {
    let output = mdroll()
        .arg("definitely-not-here.md")
        .env("RUST_BACKTRACE", "1")
        .output()
        .expect("mdroll runs");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
}

#[test]
fn an_unknown_theme_says_which_ones_exist() {
    let (ok, _, stderr) = run(&["--theme", "chartreuse", "README.md"]);
    assert!(!ok);
    assert!(stderr.contains("dracula"), "{stderr}");
}

#[test]
fn an_unknown_mermaid_mode_is_rejected() {
    let (ok, _, stderr) = run(&["--mermaid", "sideways", "README.md"]);
    assert!(!ok);
    assert!(stderr.contains("sideways"), "{stderr}");
}

#[test]
fn piped_output_renders_the_document_and_exits() {
    let (ok, stdout, stderr) = run(&["--no-color", "tests/fixtures/kitchen-sink.md"]);
    assert!(ok, "{stderr}");
    assert!(stdout.contains("Kitchen sink"));
    assert!(stdout.contains("Inline styling"));
    // Rendered, not dumped: the markup itself must not survive.
    assert!(!stdout.contains("## Inline styling"));
}

#[test]
fn listing_themes_prints_them_one_per_line() {
    let (ok, stdout, _) = run(&["--list-themes"]);
    assert!(ok);
    assert!(stdout.lines().any(|l| l == "terminal"));
    assert!(stdout.lines().any(|l| l == "dracula"));
}

#[test]
fn the_man_page_is_roff() {
    let (ok, stdout, _) = run(&["--man"]);
    assert!(ok);
    assert!(stdout.contains(".TH mdroll"));
    assert!(stdout.contains(".SH OPTIONS"));
    // roff escapes a leading hyphen, so the flag reads `\-\-theme`.
    assert!(stdout.contains("theme"));
}

#[test]
fn stdin_is_read_when_no_file_is_given() {
    use std::io::Write;
    let mut child = Command::new(env!("CARGO_BIN_EXE_mdroll"))
        .args(["--no-color"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("mdroll runs");
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(b"# From stdin\n")
        .expect("write");
    let output = child.wait_with_output().expect("mdroll finishes");
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("From stdin"));
}
