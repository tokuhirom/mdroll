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
use mdroll::config::{ConfigFile, MermaidMode, Settings, default_config_path, env_var};
use mdroll::graphics::Protocol;
use mdroll::ir::HitTarget;
use mdroll::render::{Renderer, detect_double_height};
use mdroll::screen::Screen;
use mdroll::theme;
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

fn main() {
    match real_main() {
        Ok(()) => {}
        // A closed pipe is the reader saying it has had enough, not an error.
        Err(err) if is_broken_pipe(&err) => {}
        Err(err) => {
            // Printed by hand rather than returned from main: a `Result` from
            // main is formatted with `Debug`, which means a missing file comes
            // with a stack backtrace attached. Nobody needs a backtrace to be
            // told a path does not exist.
            eprintln!("mdroll: {err:#}");
            std::process::exit(1);
        }
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

    // Piped output means nobody is there to press a key, and nothing to draw a
    // picture into: the document is printed once and the program exits.
    let piped = !std::io::stdout().is_terminal();

    // Images can be turned off without giving up big headings, so graphics are
    // detected either way and only the image path is gated on the setting.
    let mut graphics = GraphicsInfo::detect();
    // Down a pipe there is nothing to place an image on, so nothing is fetched
    // for one either — `mdroll README.md | head` must not talk to the network.
    if !settings.images || piped {
        graphics.protocol = Protocol::None;
    }
    // A terminal with graphics but no DECDHL — kitty, ghostty — can still have
    // big headings, drawn as bitmaps.
    if settings.double_height && !detect_double_height() && graphics.protocol.available() {
        graphics.raster_headings = mdroll::bigtext::find_font().is_some();
    }
    let mut app = App::new(text, path, settings, theme, current_screen()?, graphics);

    // The way a pager does when it is not on a terminal, rather than failing to
    // enter raw mode.
    if piped {
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
    if let Some(margin) = cli.margin {
        settings.margin = margin;
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
    if cli.no_remote_images {
        settings.remote_images = false;
    }
    if cli.no_color {
        settings.no_color = true;
    }
    if cli.watch {
        settings.watch = true;
    }
    if let Some(mode) = &cli.mermaid {
        settings.mermaid =
            MermaidMode::parse(mode).with_context(|| format!("unknown --mermaid mode {mode:?}"))?;
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

    /// Hand the terminal back, so another full-screen program can use it.
    fn suspend(&self) -> Result<()> {
        let mut out = std::io::stdout();
        if self.mouse {
            execute!(out, event::DisableMouseCapture)?;
        }
        execute!(out, cursor::Show, LeaveAlternateScreen)?;
        disable_raw_mode()?;
        Ok(())
    }

    fn resume(&self) -> Result<()> {
        enable_raw_mode()?;
        let mut out = std::io::stdout();
        execute!(out, EnterAlternateScreen, cursor::Hide)?;
        if self.mouse {
            execute!(out, event::EnableMouseCapture)?;
        }
        Ok(())
    }
}

/// Build the command that opens `path` at `line`.
///
/// `$VISUAL` wins over `$EDITOR`, both may carry arguments, and the way to ask
/// for a line number is different for almost every editor.
fn editor_command(spec: &str, path: &Path, line: usize) -> Option<Command> {
    let mut parts = spec.split_whitespace();
    let program = parts.next()?;
    let mut command = Command::new(program);
    command.args(parts);

    let name = Path::new(program)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(program)
        .to_ascii_lowercase();
    match name.as_str() {
        "vi" | "vim" | "nvim" | "view" | "nano" | "pico" | "micro" | "kak" | "joe" | "emacs"
        | "emacsclient" => {
            command.arg(format!("+{line}"));
            command.arg(path);
        }
        "hx" | "helix" | "subl" | "sublime_text" => {
            command.arg(format!("{}:{line}", path.display()));
        }
        "code" | "code-insiders" | "codium" | "cursor" => {
            command.arg("-g");
            command.arg(format!("{}:{line}", path.display()));
        }
        _ => {
            command.arg(path);
        }
    }
    Some(command)
}

fn editor_spec() -> String {
    for name in ["VISUAL", "EDITOR"] {
        if let Some(value) = env_var(name) {
            return value;
        }
    }
    if cfg!(windows) { "notepad" } else { "vi" }.to_string()
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

/// Run an editor on the open file, then come back and pick up the changes.
fn edit<W: Write>(
    app: &mut App,
    out: &mut W,
    terminal: &Terminal,
    path: &Path,
    line: usize,
) -> Result<()> {
    let spec = editor_spec();
    let Some(mut command) = editor_command(&spec, path, line) else {
        app.toast("no editor configured");
        return Ok(());
    };

    out.flush()?;
    terminal.suspend()?;
    let status = command.status();
    terminal.resume()?;
    // An editor has had the screen in the meantime, so nothing can be assumed
    // about what the terminal still holds.
    app.images.invalidate_uploads();

    match status {
        Ok(_) => app.reload(),
        Err(err) => app.toast(&format!("cannot run {spec:?}: {err}")),
    }
    Ok(())
}

fn run(app: &mut App) -> Result<()> {
    let terminal = Terminal::enter(app.settings.mouse)?;
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
            if app.poll_diagrams() {
                redraw = true;
            }
            if app.poll_downloads() {
                redraw = true;
            }
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
                if let Some((path, line)) = app.edit_request.take() {
                    edit(app, &mut out, &terminal, &path, line)?;
                    seen_mtime = app.mtime();
                }
            }
            Event::Resize(cols, rows) => app.resize(Screen::new(cols, rows)),
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    let url = match app.placement.target_at(mouse.column, mouse.row) {
                        Some(HitTarget::Link(id)) => app.doc.links.get(id.0).map(|l| l.url.clone()),
                        Some(HitTarget::Image(id)) => app.image_target(id),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn args(spec: &str) -> Vec<String> {
        let command = editor_command(spec, Path::new("/tmp/doc.md"), 42).unwrap();
        std::iter::once(command.get_program())
            .chain(command.get_args())
            .map(|s| s.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn vi_style_editors_take_a_plus_line_argument() {
        assert_eq!(args("vim"), ["vim", "+42", "/tmp/doc.md"]);
        assert_eq!(args("nano"), ["nano", "+42", "/tmp/doc.md"]);
    }

    #[test]
    fn helix_and_sublime_take_a_colon_suffix() {
        assert_eq!(args("hx"), ["hx", "/tmp/doc.md:42"]);
    }

    #[test]
    fn vscode_needs_its_goto_flag() {
        assert_eq!(args("code"), ["code", "-g", "/tmp/doc.md:42"]);
    }

    #[test]
    fn an_editor_spec_may_carry_arguments() {
        assert_eq!(args("code -w"), ["code", "-w", "-g", "/tmp/doc.md:42"]);
    }

    #[test]
    fn a_full_path_is_matched_on_its_basename() {
        assert_eq!(
            args("/usr/bin/nvim"),
            ["/usr/bin/nvim", "+42", "/tmp/doc.md"]
        );
    }

    #[test]
    fn an_unknown_editor_just_gets_the_file() {
        assert_eq!(args("ed"), ["ed", "/tmp/doc.md"]);
    }

    #[test]
    fn an_empty_spec_has_no_program_to_run() {
        assert!(editor_command("   ", Path::new("/tmp/doc.md"), 1).is_none());
    }
}
