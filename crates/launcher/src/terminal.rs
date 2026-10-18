// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Human-facing output. JSON and child log files remain separate and plain.
use anstyle::{AnsiColor, Style};
use std::fmt::Display;

const BLUE: Style = AnsiColor::Cyan.on_default().bold();
const GREEN: Style = AnsiColor::Green.on_default().bold();
const YELLOW: Style = AnsiColor::Yellow.on_default().bold();
const RED: Style = AnsiColor::Red.on_default().bold();

pub fn heading(title: &str) {
    anstream::eprintln!("\n{BLUE}{title}{BLUE:#}");
}

fn line(style: Style, label: &str, message: impl Display) {
    anstream::eprintln!("  {style}{label:>5}{style:#}  {message}");
}

pub fn step(message: impl Display) {
    line(BLUE, "...", message);
}
pub fn success(message: impl Display) {
    line(GREEN, "OK", message);
}
pub fn action(message: impl Display) {
    line(YELLOW, "WAIT", message);
}
pub fn error(message: impl Display) {
    line(RED, "ERROR", message);
}
pub fn detail(label: &str, value: impl Display) {
    anstream::eprintln!("         {label}: {value}");
}
pub fn note(message: impl Display) {
    anstream::eprintln!("         {message}");
}

fn diagnostic(line: &str) {
    let message = line
        .strip_prefix("[content] ")
        .or_else(|| line.strip_prefix("[setup] "));
    if let Some(message) = message {
        if message.starts_with("Done:")
            || message.contains("complete.")
            || message.ends_with("ready.")
            || message.starts_with("Verified content cache reused")
        {
            success(message.trim_start_matches("Done: "));
        } else {
            step(message);
        }
    } else {
        // Unknown diagnostics remain visible verbatim, including failures.
        anstream::eprintln!("{line}");
    }
}

/// Pipes can split diagnostic lines anywhere. Buffer incomplete lines from
/// the child reader so a label is styled only once. Bound retained text even
/// for a child with no newline.
#[derive(Default)]
pub struct Diagnostics {
    pending: String,
}
impl Diagnostics {
    const MAX_LINE: usize = 16 * 1024;

    fn collect(&mut self, text: &str, mut emit: impl FnMut(&str)) {
        for part in text.split_inclusive('\n') {
            // Overlong diagnostics are passed through in bounded fragments.
            if self.pending.len() + part.len() > Self::MAX_LINE {
                if !self.pending.is_empty() {
                    emit(&self.pending);
                    self.pending.clear();
                }
                emit(part.trim_end_matches(['\r', '\n']));
                continue;
            }
            self.pending.push_str(part);
            if self.pending.ends_with('\n') {
                emit(self.pending.trim_end_matches(['\r', '\n']));
                self.pending.clear();
            }
        }
    }

    pub fn push(&mut self, text: &str) {
        self.collect(text, diagnostic);
    }
    pub fn finish(&mut self) {
        if !self.pending.is_empty() {
            diagnostic(&self.pending);
            self.pending.clear();
        }
    }
}

impl Drop for Diagnostics {
    fn drop(&mut self) {
        self.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_diagnostics_keep_every_line_and_bound_unterminated_output() {
        let mut reader = Diagnostics::default();
        let mut lines = Vec::new();
        for text in [
            "[con",
            "tent] Extracting cars...\r",
            "\nFailure: details\nlast",
        ] {
            reader.collect(text, |line| lines.push(line.to_owned()));
        }
        assert_eq!(lines, ["[content] Extracting cars...", "Failure: details"]);
        assert_eq!(reader.pending, "last");
        let long = "x".repeat(Diagnostics::MAX_LINE + 1);
        reader.collect(&long, |line| lines.push(line.to_owned()));
        assert_eq!(lines[2], "last");
        assert_eq!(lines[3], long);
        assert!(reader.pending.is_empty());
    }
}
