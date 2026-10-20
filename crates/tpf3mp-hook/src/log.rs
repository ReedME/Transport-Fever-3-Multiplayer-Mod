//! One line to `hook.log` from anywhere in the hook, the game's threads
//! included (`crate::order`, `crate::seeds`): the step gate's log, which
//! `crate::install` opens before it installs anything. Before then, or
//! without a log, the line is dropped rather than failing the hook.

/// Appends one line to the hook's log.
pub fn line(message: &str) {
    crate::install::log_line(message);
}

/// Appends a bounded batch in one write.
pub fn lines(messages: &[String]) {
    crate::install::log_lines(messages);
}
