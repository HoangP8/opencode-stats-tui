//! Application entry point

use std::io;

mod cost;
mod device;
mod live_watcher;
mod session;
mod stats;
mod stats_cache;
mod theme;
mod ui;
mod overview_stats;

/// Restore terminal to normal mode.
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

/// Force immediate terminal cleanup
fn force_cleanup_terminal() {
    use std::io::Write;
    let mut stdout = std::io::stdout();

    let _ = crossterm::execute!(
        stdout,
        crossterm::terminal::Clear(crossterm::terminal::ClearType::All),
        crossterm::cursor::MoveTo(0, 0)
    );

    let _ = crossterm::execute!(
        stdout,
        crossterm::terminal::LeaveAlternateScreen,
        crossterm::cursor::Show
    );

    let _ = crossterm::terminal::disable_raw_mode();
    let _ = stdout.flush();
}

/// Install panic hook to restore terminal before printing error.
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

/// Drain all pending input events until silence.
fn drain_input_events_until_silence(silence_duration: std::time::Duration) {
    use crossterm::event::{poll, read};

    for _ in 0..3 {
        let mut events_drained = 0;
        while poll(silence_duration).unwrap_or(false) {
            let _ = read();
            events_drained += 1;
        }
        if events_drained == 0 {
            break;
        }
    }
}

/// Disable all mouse tracking modes and bracketed paste.
fn disable_all_modes() {
    use std::io::Write;
    let mut stdout = std::io::stdout();

    let _ = stdout.flush();
    let _ = crossterm::execute!(
        stdout,
        crossterm::event::DisableMouseCapture,
        crossterm::event::DisableBracketedPaste
    );
    let _ = stdout.flush();

    // Kill all potential mouse tracking modes
    let combined = "\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1015l\x1b[?1005l\x1b[?2004l";
    let _ = stdout.write_all(combined.as_bytes());
    let _ = stdout.flush();

    std::thread::sleep(std::time::Duration::from_millis(30));
}

/// Flush OS-level terminal input buffer.
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

    // Background initialization
    std::thread::spawn(|| {
        device::get_device_info();
    });
    std::thread::spawn(|| {
        cost::init_pricing();
    });

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

    // Run application
    let result = ui::App::new().run(&mut terminal);

    // Terminal cleanup sequence
    disable_all_modes();
    drain_input_events_until_silence(std::time::Duration::from_millis(100));
    flush_stdin_buffer();
    force_cleanup_terminal();

    result
}
