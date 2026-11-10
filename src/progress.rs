//! A one-line progress count on stderr, and messages that share stderr with it.
//!
//! A long run (`ath rank --in all`) redraws one line in place. A message
//! printed while that line is up would be glued onto it, so messages that can
//! come mid-run go through [`message`], which clears the line first; the next
//! count draws it again below the message.

use std::sync::atomic::{AtomicBool, Ordering};

/// Whether a progress line is drawn and not yet ended by a newline.
static SHOWN: AtomicBool = AtomicBool::new(false);

/// Back to the start of the line, and clear it.
const CLEAR_LINE: &str = "\r\x1b[2K";

// Every write holds the stderr lock across reading or setting SHOWN and the
// write itself, so a message from another thread (the relay server) can never
// land between a progress line being drawn and SHOWN saying so. The writes
// themselves go through `eprint!` (the lock is reentrant), which the test
// harness captures like any other output.

/// Draw `text` as the progress line, replacing the one drawn before.
pub fn line(text: &str) {
    let _held = std::io::stderr().lock();
    eprint!("{CLEAR_LINE}{text}");
    SHOWN.store(true, Ordering::Relaxed);
}

/// Take the progress line away.
pub fn clear() {
    let _held = std::io::stderr().lock();
    clear_held();
}

/// `eprintln!` for a message that can come while a progress line is up.
pub fn message(text: &str) {
    let _held = std::io::stderr().lock();
    clear_held();
    eprintln!("{text}");
}

/// [`clear`], with the stderr lock already held.
fn clear_held() {
    if SHOWN.swap(false, Ordering::Relaxed) {
        eprint!("{CLEAR_LINE}");
    }
}
