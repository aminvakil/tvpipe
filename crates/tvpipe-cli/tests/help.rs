//! The command lists (`tvpipe --help`, `tvpipe debug --help`, `tvpipe
//! game --help`) fit an 80-column terminal: no line wraps.

use std::process::Command;

fn help(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_tvpipe"))
        .args(args)
        .arg("--help")
        // Wider than the help's own limit (100): nothing wraps before
        // that, so a line too long for 80 columns shows as one.
        .env("COLUMNS", "200")
        .output()
        .unwrap();
    assert!(out.status.success(), "{args:?}");
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn command_lists_fit_80_columns() {
    for (args, commands) in [(&[][..], 9), (&["debug"][..], 4), (&["game"][..], 4)] {
        let text = help(args);
        for line in text.lines() {
            assert!(
                line.chars().count() <= 80,
                "tvpipe {} --help: {} columns: {line:?}",
                args.join(" "),
                line.chars().count()
            );
        }
        // One line per command: none wrapped onto a second.
        let list: Vec<&str> = text
            .split("Commands:\n")
            .nth(1)
            .unwrap()
            .lines()
            .take_while(|l| !l.is_empty())
            .collect();
        assert_eq!(list.len(), commands, "{args:?}: {list:?}");
        assert!(list
            .iter()
            .all(|l| l.starts_with("  ") && !l.starts_with("   ")));
    }
}
