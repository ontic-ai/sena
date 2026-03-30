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

/// Print the Sena boot banner.
pub fn banner() {
    println!("{BOLD}{CYAN}");
    println!("  ╔══════════════════════════════════╗");
    println!("  ║       · S E N A ·                ║");
    println!("  ║       local-first ambient AI     ║");
    println!("  ╚══════════════════════════════════╝");
    println!("{RESET}");
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
    println!("  {BOLD}{CYAN}/memory{RESET}       {DIM}or{RESET} /mem   What do you remember about me?");
    println!("  {BOLD}{CYAN}/explanation{RESET}  {DIM}or{RESET} /why   Why did you say that?");
    println!("  {BOLD}{CYAN}/models{RESET}               Select which Ollama model to use");
    println!("  {BOLD}{CYAN}/help{RESET}                 Show this message");
    println!("  {BOLD}{CYAN}/quit{RESET}                 Exit Sena");
    println!();
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
}
