//! The OS-specific bits of the browser modes: which command opens a URL
//! (`cfg(target_os)` is allowed here, like a config path; W13 adds the
//! well-known browser install paths).

use std::process::{Command, Stdio};

use crate::error::{Error, Result};

/// The opener command and its arguments for `url`.
fn opener(url: &str) -> (&'static str, Vec<String>) {
    if cfg!(target_os = "macos") {
        ("open", vec![url.to_owned()])
    } else if cfg!(target_os = "windows") {
        // `start` is a cmd builtin; the empty string is the window title.
        (
            "cmd",
            vec!["/C".into(), "start".into(), String::new(), url.to_owned()],
        )
    } else {
        ("xdg-open", vec![url.to_owned()])
    }
}

/// Opens `url` in the default browser (does not wait for it).
pub fn open_url(url: &str) -> Result<()> {
    // Nothing a shell (cmd's `start`) would interpret.
    if !url.starts_with("http://") || url.bytes().any(|b| b <= b' ' || b"\"^&|<>%".contains(&b)) {
        return Err(Error::usage(format!("not opening {url:?}")));
    }
    let (cmd, args) = opener(url);
    let mut child = Command::new(cmd)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| Error::io(format!("run {cmd}"), e))?;
    // Reap it in the background; the opener exits quickly.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}
