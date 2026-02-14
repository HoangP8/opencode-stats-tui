use std::io;
mod device;
mod live_watcher;
mod session;
mod stats;
mod stats_cache;
mod theme;
mod ui;

/// Cleanup terminal state - ensures terminal is restored even if ratatui fails
fn cleanup_terminal() {
    use std::io::Write;
    let mut stdout = std::io::stdout();
    let _ = crossterm::execute!(
        stdout,
        crossterm::event::DisableMouseCapture,
        crossterm::event::DisableBracketedPaste,
        crossterm::terminal::LeaveAlternateScreen,
        crossterm::cursor::Show
    );
    let _ = crossterm::terminal::disable_raw_mode();
    let _ = stdout.flush();
}

/// Custom panic hook to restore terminal before printing panic message
fn setup_panic_hook() {
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        cleanup_terminal();
        eprintln!("Application panicked!");
        if let Some(location) = panic_info.location() {
            eprintln!("Location: {}", location);
        }
        if let Some(payload) = panic_info.payload().downcast_ref::<&str>() {
            eprintln!("Message: {}", payload);
        } else if let Some(payload) = panic_info.payload().downcast_ref::<String>() {
            eprintln!("Message: {}", payload);
        } else {
            eprintln!("No panic message available");
        }
        original_hook(panic_info);
    }));
}

/// Force immediate terminal cleanup - blocking and synchronous
fn force_cleanup_terminal() {
    use std::io::Write;
    let mut stdout = std::io::stdout();

    // Clear screen and reset cursor
    let _ = crossterm::execute!(
        stdout,
        crossterm::terminal::Clear(crossterm::terminal::ClearType::All),
        crossterm::cursor::MoveTo(0, 0)
    );

    // Leave alternate screen and show cursor
    let _ = crossterm::execute!(
        stdout,
        crossterm::terminal::LeaveAlternateScreen,
        crossterm::cursor::Show
    );

    // Disable raw mode LAST
    let _ = crossterm::terminal::disable_raw_mode();
    let _ = stdout.flush();
}

/// Drain all pending input events until a period of silence is reached.
/// This ensures we catch every single byte of high-speed input streams.
fn drain_input_events_until_silence(silence_duration: std::time::Duration) {
    use crossterm::event::{poll, read};

    // Run multiple passes to catch buffered events
    for _ in 0..3 {
        let mut events_drained = 0;
        while poll(silence_duration).unwrap_or(false) {
            let _ = read();
            events_drained += 1;
        }
        // If we drained events, do another pass immediately
        if events_drained == 0 {
            break;
        }
    }
}

/// Nuclear cleanup: disable ALL mouse modes and bracketed paste
fn disable_all_modes() {
    use std::io::Write;
    let mut stdout = std::io::stdout();

    // IMPORTANT: Flush stdout first to ensure disable commands are sent immediately
    let _ = stdout.flush();

    // Disable standard modes using crossterm
    let _ = crossterm::execute!(
        stdout,
        crossterm::event::DisableMouseCapture,
        crossterm::event::DisableBracketedPaste
    );

    // Flush again to ensure crossterm commands are sent
    let _ = stdout.flush();

    // Explicitly kill ALL potential mouse tracking modes with direct escape sequences
    // This is redundant but necessary for some terminals
    let _disable_sequences = [
        "\x1b[?1000l", // X11 mouse reporting
        "\x1b[?1002l", // Button-event tracking
        "\x1b[?1003l", // Any-event tracking
        "\x1b[?1006l", // SGR extended mode
        "\x1b[?1015l", // URXVT extended mode
        "\x1b[?1005l", // UTF-8 extended mode
        "\x1b[?2004l", // Bracketed paste mode
    ];

    // Send all disable sequences in one write for efficiency
    // Optimized: use static string to avoid allocation
    let combined = "\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1015l\x1b[?1005l\x1b[?2004l";
    let _ = stdout.write_all(combined.as_bytes());
    let _ = stdout.flush();

    // Brief delay to ensure terminal processes all disable commands
    std::thread::sleep(std::time::Duration::from_millis(30));
}

/// Flush the OS-level terminal input buffer
#[cfg(unix)]
fn flush_stdin_buffer() {
    use std::os::unix::io::AsRawFd;
    unsafe {
        libc::tcflush(std::io::stdin().as_raw_fd(), libc::TCIFLUSH);
    }
}

#[cfg(not(unix))]
fn flush_stdin_buffer() {}

fn main() -> io::Result<()> {
    setup_panic_hook();

    // Kick off device detection in background thread immediately.
    // Uses OnceLock internally — resolves in parallel while TUI initializes.
    std::thread::spawn(|| { device::get_device_info(); });

    // OPTIMIZATION: Skip startup drain entirely for modern terminals
    // Modern terminals don't leave junk in stdin, and we have proper cleanup on exit
    // This saves 1-5ms on every startup

    // Enable terminal settings
    crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::EnterAlternateScreen,
        crossterm::event::EnableMouseCapture,
        crossterm::event::EnableBracketedPaste
    )?;
    crossterm::terminal::enable_raw_mode()?;

    let backend = ratatui::backend::CrosstermBackend::new(std::io::stdout());
    let mut terminal = ratatui::Terminal::new(backend)?;

    // Run the app
    let result = ui::App::new().run(&mut terminal);

    // THE FOREVER FIX:
    // 1. Send "STOP" commands to the terminal FIRST (stops new events)
    // 2. Wait for a SUSTAINED 150ms of silence (no bytes arriving)
    // 3. Flush the OS kernel buffer
    // 4. Finally leave raw mode

    disable_all_modes(); // Step 1: STOP generating new mouse events
    drain_input_events_until_silence(std::time::Duration::from_millis(100)); // Step 2: Clear the queue
    flush_stdin_buffer(); // Step 3: Flush OS buffer
    force_cleanup_terminal(); // Step 4: Leave raw mode

    result
}
