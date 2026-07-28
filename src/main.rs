//! Entry point: resolve settings, set up the terminal, run the event loop.

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::event::{self, Event, KeyEventKind, MouseButton, MouseEventKind};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{cursor, execute};
use mdroll::app::{App, GraphicsInfo, browser_markdown};
use mdroll::cli::Cli;
use mdroll::config::{ConfigFile, Settings, default_config_path, env_var};
use mdroll::graphics::Protocol;
use mdroll::ir::HitTarget;
use mdroll::render::{Renderer, detect_double_height};
use mdroll::screen::Screen;
use mdroll::theme;
use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;
use std::time::Duration;

fn main() -> Result<()> {
    match real_main() {
        // A closed pipe is the reader saying it has had enough, not an error.
        Err(err) if is_broken_pipe(&err) => Ok(()),
        other => other,
    }
}

fn real_main() -> Result<()> {
    let cli = Cli::parse();

    if cli.man {
        // Generated from the same definition the parser uses, so it cannot
        // drift: `mdroll --man > mdroll.1`.
        let mut out = std::io::stdout();
        clap_mangen::Man::new(<Cli as clap::CommandFactory>::command()).render(&mut out)?;
        return Ok(());
    }

    if cli.list_themes {
        for name in theme::available_names() {
            println!("{name}");
        }
        return Ok(());
    }

    let settings = resolve(&cli)?;
    let theme = theme::load(&settings.theme)?;
    let (text, path) = read_input(&cli)?;

    // Images can be turned off without giving up big headings, so graphics are
    // detected either way and only the image path is gated on the setting.
    let mut graphics = GraphicsInfo::detect();
    if !settings.images {
        graphics.protocol = Protocol::None;
    }
    // A terminal with graphics but no DECDHL — kitty, ghostty — can still have
    // big headings, drawn as bitmaps.
    if settings.double_height && !detect_double_height() && graphics.protocol.available() {
        graphics.raster_headings = mdroll::bigtext::find_font().is_some();
    }
    let mut app = App::new(text, path, settings, theme, current_screen()?, graphics);

    // Piped output means nobody is there to press a key. Render the whole
    // document once and exit, the way a pager does when it is not on a
    // terminal, rather than failing to enter raw mode.
    if !std::io::stdout().is_terminal() {
        return dump(&mut app);
    }
    run(&mut app)
}

/// Write the entire rendered document to stdout and return.
fn dump(app: &mut App) -> Result<()> {
    let cols = app.screen.cols;
    // One row per line, so nothing is cut off, and no double-height headings,
    // which only make sense on a live terminal.
    app.double_height = false;
    app.screen = Screen::new(cols, u16::MAX);
    app.relayout();

    let mut renderer = Renderer::new(app.calc());
    renderer.no_color = app.settings.no_color;
    renderer.hyperlinks = false;

    let mut out = std::io::BufWriter::new(std::io::stdout());
    for line in &app.lines {
        // `mdroll README.md | head` closes the pipe early. That is the reader
        // saying it has had enough, not an error to report.
        if let Err(err) = renderer.write_line(&mut out, line) {
            return if is_broken_pipe(&err) {
                Ok(())
            } else {
                Err(err)
            };
        }
    }
    match out.flush() {
        Err(err) if err.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        other => Ok(other?),
    }
}

fn is_broken_pipe(err: &anyhow::Error) -> bool {
    err.downcast_ref::<std::io::Error>()
        .is_some_and(|e| e.kind() == std::io::ErrorKind::BrokenPipe)
}

/// Command line → environment → config file → built-in default.
fn resolve(cli: &Cli) -> Result<Settings> {
    // Capability detection sets the default; the config file and the command
    // line can still override it in either direction.
    let mut settings = Settings {
        // Either DECDHL or a bitmap will do; whether either is available is
        // settled once the graphics protocol is known.
        double_height: detect_double_height() || mdroll::graphics::detect().available(),
        ..Settings::default()
    };

    let config_path = cli
        .config
        .clone()
        .or_else(|| env_var("MDROLL_CONFIG").map(PathBuf::from))
        .or_else(default_config_path);
    if let Some(path) = config_path
        && path.is_file()
    {
        settings.apply_file(&ConfigFile::load(&path)?);
    }

    settings.apply_env(&env_var);

    if let Some(theme) = &cli.theme {
        settings.theme = theme.clone();
    }
    if cli.wrap {
        settings.wrap = true;
    }
    if cli.no_wrap {
        settings.wrap = false;
    }
    if cli.source {
        settings.source = true;
    }
    if let Some(width) = cli.width {
        settings.width = width;
    }
    if cli.status {
        settings.status = true;
    }
    if cli.no_status {
        settings.status = false;
    }
    if cli.mouse {
        settings.mouse = true;
    }
    if cli.no_images {
        settings.images = false;
    }
    if cli.no_color {
        settings.no_color = true;
    }
    if cli.watch {
        settings.watch = true;
    }
    if cli.no_big_headings {
        settings.double_height = false;
    }
    if cli.ambiguous_wide {
        settings.ambiguous_wide = true;
    }
    Ok(settings)
}

/// A path argument wins; then piped stdin; otherwise browse the directory.
fn read_input(cli: &Cli) -> Result<(String, Option<PathBuf>)> {
    if let Some(path) = &cli.file {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        return Ok((text, Some(path.clone())));
    }
    if !std::io::stdin().is_terminal() {
        let mut text = String::new();
        std::io::stdin().read_to_string(&mut text)?;
        return Ok((text, None));
    }
    let dir = std::env::current_dir()?;
    Ok((browser_markdown(&dir), None))
}

fn current_screen() -> Result<Screen> {
    match crossterm::terminal::size() {
        Ok((cols, rows)) => Ok(Screen::new(cols, rows)),
        // No terminal at all, as when both ends are pipes.
        Err(_) => Ok(Screen::new(80, 24)),
    }
}

/// Owns the terminal mode changes so they are undone even on an error path.
struct Terminal {
    mouse: bool,
}

impl Terminal {
    fn enter(mouse: bool) -> Result<Terminal> {
        enable_raw_mode()?;
        let mut out = std::io::stdout();
        execute!(out, EnterAlternateScreen, cursor::Hide)?;
        if mouse {
            execute!(out, event::EnableMouseCapture)?;
        }
        Ok(Terminal { mouse })
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let mut out = std::io::stdout();
        if self.mouse {
            let _ = execute!(out, event::DisableMouseCapture);
        }
        let _ = execute!(out, cursor::Show, LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

fn run(app: &mut App) -> Result<()> {
    let _terminal = Terminal::enter(app.settings.mouse)?;
    let mut renderer = Renderer::new(app.calc());
    renderer.no_color = app.settings.no_color;

    let mut out = std::io::BufWriter::new(std::io::stdout());
    app.draw(&mut out, &renderer)?;
    let mut seen_mtime = app.mtime();

    while !app.quit {
        // Wake up often enough to retire a toast without a keystroke.
        if !event::poll(Duration::from_millis(200))? {
            let mut redraw = false;
            if app.toast_expired() {
                app.toast = None;
                redraw = true;
            }
            // Polling rather than an inotify dependency: a viewer that wakes up
            // five times a second costs nothing, and this works identically on
            // every platform and over network filesystems.
            if app.settings.watch {
                let now = app.mtime();
                if now != seen_mtime {
                    seen_mtime = now;
                    app.reload();
                    redraw = true;
                }
            }
            if redraw {
                app.draw(&mut out, &renderer)?;
            }
            continue;
        }
        match event::read()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                app.on_key(&mut out, key)?;
            }
            Event::Resize(cols, rows) => app.resize(Screen::new(cols, rows)),
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    let url = match app.placement.target_at(mouse.column, mouse.row) {
                        Some(HitTarget::Link(id)) => app.doc.links.get(id.0).map(|l| l.url.clone()),
                        Some(HitTarget::Image(id)) => {
                            app.doc.images.get(id.0).map(|i| i.url.clone())
                        }
                        None => None,
                    };
                    if let Some(url) = url {
                        app.open(&url);
                    }
                }
                MouseEventKind::ScrollDown => app.scroll_by(3),
                MouseEventKind::ScrollUp => app.scroll_by(-3),
                _ => continue,
            },
            _ => continue,
        }
        app.draw(&mut out, &renderer)?;
    }
    out.flush()?;
    Ok(())
}
