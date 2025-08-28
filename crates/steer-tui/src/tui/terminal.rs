use ratatui::crossterm::{
    event::{
        DisableBracketedPaste, DisableMouseCapture, EnableMouseCapture, KeyboardEnhancementFlags,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};

/// Global terminal state to make cleanup idempotent across exit, panic and signal paths
pub struct TerminalState {
    pub(crate) raw: AtomicBool,
    pub(crate) alt_screen: AtomicBool,
    pub(crate) bracketed_paste: AtomicBool,
    pub(crate) keyboard_flags_pushed: AtomicBool,
    pub(crate) mouse_capture: AtomicBool,
}

impl Default for TerminalState {
    fn default() -> Self {
        Self::new()
    }
}

impl TerminalState {
    pub const fn new() -> Self {
        Self {
            raw: AtomicBool::new(false),
            alt_screen: AtomicBool::new(false),
            bracketed_paste: AtomicBool::new(false),
            keyboard_flags_pushed: AtomicBool::new(false),
            mouse_capture: AtomicBool::new(false),
        }
    }
}

pub static TERMINAL_STATE: TerminalState = TerminalState::new();

/// Set up terminal modes and features. Flags are updated only after each successful step.
pub fn setup<W: Write>(w: &mut W) -> io::Result<()> {
    // raw mode
    enable_raw_mode()?;
    TERMINAL_STATE.raw.store(true, Ordering::Relaxed);

    // alt screen
    execute!(w, EnterAlternateScreen)?;
    TERMINAL_STATE.alt_screen.store(true, Ordering::Relaxed);

    // bracketed paste
    execute!(w, ratatui::crossterm::event::EnableBracketedPaste)?;
    TERMINAL_STATE
        .bracketed_paste
        .store(true, Ordering::Relaxed);

    // keyboard enhancement flags
    execute!(
        w,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    )?;
    TERMINAL_STATE
        .keyboard_flags_pushed
        .store(true, Ordering::Relaxed);

    // mouse capture
    execute!(w, EnableMouseCapture)?;
    TERMINAL_STATE.mouse_capture.store(true, Ordering::Relaxed);

    Ok(())
}

/// Cleanup helper that writes escape sequences to the provided writer.
/// Uses global flags to avoid double-disabling.
pub fn cleanup_with_writer<W: Write>(writer: &mut W) {
    if TERMINAL_STATE
        .keyboard_flags_pushed
        .swap(false, Ordering::Relaxed)
    {
        let _ = execute!(writer, PopKeyboardEnhancementFlags);
    }
    if TERMINAL_STATE.mouse_capture.swap(false, Ordering::Relaxed) {
        let _ = execute!(writer, DisableMouseCapture);
    }
    if TERMINAL_STATE
        .bracketed_paste
        .swap(false, Ordering::Relaxed)
    {
        let _ = execute!(writer, DisableBracketedPaste);
    }
    if TERMINAL_STATE.alt_screen.swap(false, Ordering::Relaxed) {
        let _ = execute!(writer, LeaveAlternateScreen);
    }
    if TERMINAL_STATE.raw.swap(false, Ordering::Relaxed) {
        let _ = disable_raw_mode();
    }
    let _ = writer.flush();
}

/// Best-effort cleanup across common output streams.
pub fn cleanup() {
    {
        let mut out = io::stdout();
        cleanup_with_writer(&mut out);
        let _ = out.flush();
    }
    {
        let mut err = io::stderr();
        cleanup_with_writer(&mut err);
        let _ = err.flush();
    }
    #[cfg(not(windows))]
    if let Ok(mut tty) = std::fs::OpenOptions::new().write(true).open("/dev/tty") {
        cleanup_with_writer(&mut tty);
        let _ = tty.flush();
    }
}

/// RAII guard used during terminal setup to ensure cleanup on early-return paths.
/// It does not track per-step state; it relies on the global TERMINAL_STATE flags.
pub struct SetupGuard {
    armed: bool,
}

impl Default for SetupGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl SetupGuard {
    pub fn new() -> Self {
        Self { armed: true }
    }

    pub fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for SetupGuard {
    fn drop(&mut self) {
        if self.armed {
            cleanup();
        }
    }
}
