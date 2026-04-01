//! Terminal display helpers for Sena CLI.
#![allow(dead_code)]
//!
//! All ANSI formatting lives here. Every module in cli that produces
//! user-visible output should use these helpers for visual consistency.

use std::io::Write;

// ── ANSI escape constants ─────────────────────────────────────────────────────

pub(crate) const RESET: &str = "\x1b[0m";
pub(crate) const BOLD: &str = "\x1b[1m";
pub(crate) const DIM: &str = "\x1b[2m";
pub(crate) const RED: &str = "\x1b[31m";
pub(crate) const GREEN: &str = "\x1b[32m";
pub(crate) const YELLOW: &str = "\x1b[33m";
pub(crate) const CYAN: &str = "\x1b[36m";

// ── Structural print functions ────────────────────────────────────────────────

/// Print the Sena boot banner with version and branding.
pub fn banner() {
    let version = env!("CARGO_PKG_VERSION");

    println!();
    println!("{BOLD}{CYAN}  ╭────────────────────────────────────────╮{RESET}");
    println!("{BOLD}{CYAN}  │{RESET}                                        {BOLD}{CYAN}│{RESET}");
    println!("{BOLD}{CYAN}  │{RESET}     {BOLD}{CYAN}___  ___ _ __   __ _{RESET}            {BOLD}{CYAN}│{RESET}");
    println!("{BOLD}{CYAN}  │{RESET}    {BOLD}{CYAN}/ __|/ _ \\ '_ \\ / _` |{RESET}           {BOLD}{CYAN}│{RESET}");
    println!("{BOLD}{CYAN}  │{RESET}    {BOLD}{CYAN}\\__ \\  __/ | | | (_| |{RESET}           {BOLD}{CYAN}│{RESET}");
    println!("{BOLD}{CYAN}  │{RESET}    {BOLD}{CYAN}|___/\\___|_| |_|\\__,_|{RESET}           {BOLD}{CYAN}│{RESET}");
    println!("{BOLD}{CYAN}  │{RESET}                                        {BOLD}{CYAN}│{RESET}");
    println!("{BOLD}{CYAN}  │{RESET}   {DIM}local-first ambient intelligence{RESET}  {BOLD}{CYAN}│{RESET}");
    println!("{BOLD}{CYAN}  │{RESET}   {DIM}version {version}{RESET}                        {BOLD}{CYAN}│{RESET}");
    println!("{BOLD}{CYAN}  │{RESET}                                        {BOLD}{CYAN}│{RESET}");
    println!("{BOLD}{CYAN}  ╰────────────────────────────────────────╯{RESET}");
    println!();
}

/// Print the interactive prompt (`sena ›`). Does NOT add a newline.
pub fn prompt() {
    print!("{BOLD}{CYAN}sena{RESET} {DIM}›{RESET} ");
    let _ = std::io::stdout().flush();
}

/// Print a section header: `━━  Title`.
pub fn section(title: &str) {
    println!();
    println!("  {BOLD}{CYAN}━━  {title}{RESET}");
    println!();
}

/// Print a thin horizontal divider.
pub fn divider() {
    println!("  {DIM}────────────────────────────────────────{RESET}");
}

// ── Message-level print functions ─────────────────────────────────────────────

/// Print an error line to stderr: `  ✗  <msg>` in red.
pub fn error(msg: &str) {
    eprintln!("  {BOLD}{RED}✗{RESET}  {RED}{msg}{RESET}");
}

/// Print a success line: `  ✓  <msg>` in green.
pub fn success(msg: &str) {
    println!("  {GREEN}✓{RESET}  {msg}");
}

/// Print an info line: `  ·  <msg>` dimmed.
pub fn info(msg: &str) {
    println!("  {DIM}·  {msg}{RESET}");
}

/// Print a warning line: `  ⚠  <msg>` in yellow.
pub fn warn(msg: &str) {
    println!("  {YELLOW}⚠{RESET}  {YELLOW}{msg}{RESET}");
}

/// Print an inline prompt without a newline (caller flushes or uses prompt()).
pub fn prompt_inline(text: &str) {
    print!("  {CYAN}>{RESET} {text}");
    let _ = std::io::stdout().flush();
}

// ── Compound helpers ──────────────────────────────────────────────────────────

/// Print the /help command reference.
pub fn help() {
    section("Commands");
    println!("  {BOLD}{CYAN}/observation{RESET}  {DIM}or{RESET} /obs   What are you observing right now?");
    println!(
        "  {BOLD}{CYAN}/memory{RESET}       {DIM}or{RESET} /mem   What do you remember about me?"
    );
    println!("  {BOLD}{CYAN}/explanation{RESET}  {DIM}or{RESET} /why   Why did you say that?");
    println!("  {BOLD}{CYAN}/models{RESET}               Select which Ollama model to use");
    println!("  {BOLD}{CYAN}/copy{RESET}                 Copy last response to clipboard");
    println!("  {BOLD}{CYAN}/help{RESET}                 Show this message");
    println!("  {BOLD}{CYAN}/close{RESET} {DIM}or{RESET} /quit  Close CLI session");
    println!("  {BOLD}{CYAN}/shutdown{RESET}             Shut down Sena completely");
    println!("  {DIM}Shortcut:{RESET} Ctrl+Y {DIM}or{RESET} Ctrl+Shift+C to copy last response");
    println!();
}

// Print the session summary after TUI exit.
// Displays session metrics: duration, messages sent, tokens received.
// Uses ANSI colors for visual clarity. The "~" prefix on tokens indicates
// it's an estimate (some responses may not return exact token counts).
pub fn print_session_summary(stats: &crate::tui_state::SessionStats) {
    println!();
    println!("{DIM}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━{RESET}");
    println!("  {BOLD}{CYAN}Session Summary{RESET}");
    println!("  {DIM}──────────────────────────────────{RESET}");
    println!(
        "  Duration   {DIM}│{RESET}  {BOLD}{}{RESET}",
        stats.elapsed_formatted()
    );
    println!(
        "  Messages   {DIM}│{RESET}  {BOLD}{}{RESET}",
        stats.messages_sent
    );
    println!(
        "  Tokens     {DIM}│{RESET}  {BOLD}~{}{RESET}",
        stats.tokens_received
    );
    println!("{DIM}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━{RESET}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ansi_constants_are_nonempty() {
        assert!(!RESET.is_empty());
        assert!(!BOLD.is_empty());
        assert!(!CYAN.is_empty());
    }

    #[test]
    fn banner_does_not_panic() {
        // Should not panic when called
        banner();
    }

    #[test]
    fn print_session_summary_does_not_panic() {
        use crate::tui_state::SessionStats;

        // Create a mock SessionStats with some data
        let mut stats = SessionStats::new();
        stats.messages_sent = 5;
        stats.tokens_received = 1234;

        // Call should not panic
        print_session_summary(&stats);
    }
}
