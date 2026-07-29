//! Viewer state and key handling.
//!
//! [`App`] owns no terminal handle: every action is a pure state transition, so
//! the key bindings can be tested without a pty. Only [`App::draw`] and the
//! event loop in `main` touch the real terminal.

use crate::clipboard;
use crate::config::{MermaidMode, Settings};
use crate::graphics::{self, CellSize, ImageStore, Protocol};
use crate::ir::{Block, BlockKind, Document, HitTarget, Image, ImageId, Line, Span};
use crate::keys::{Action, Keymap};
use crate::layout::{self, Options, row_for_source_line};
use crate::render::{Decor, Frame, Highlight, Overlay, Placement, Renderer};
use crate::screen::Screen;
use crate::theme::{self, Theme};
use crate::width::WidthCalc;
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

pub const TOAST_DURATION: Duration = Duration::from_millis(1500);

/// Labels handed out by the link picker, in home-row-first order.
const PICK_KEYS: &[u8] = b"asdfghjklqwertyuiopzxcvbnm";

pub const HELP: &str = include_str!("../doc/help.md");

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Normal,
    /// Typing a search pattern. `forward` records which way `n` will go.
    Search {
        query: String,
        forward: bool,
    },
    /// Every visible link is labelled; the next keystroke picks one.
    LinkPick,
    /// Extending a line selection with `j`/`k`.
    Select,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Match {
    pub line: usize,
    pub start: usize,
    pub end: usize,
}

/// A link drawn on screen, and where it was drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisibleLink {
    /// Row in the layout, not on the screen.
    pub row: usize,
    /// Display column the link starts at.
    pub col: usize,
    /// Index into [`Document::links`].
    pub link: usize,
}

/// A labelled link, waiting for the keystroke that chooses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pick {
    pub label: char,
    pub row: usize,
    pub col: usize,
    /// Index into [`Document::links`].
    pub link: usize,
}

pub struct App {
    pub settings: Settings,
    pub theme: Theme,
    pub doc: Document,
    pub lines: Vec<Line>,
    pub screen: Screen,
    pub path: Option<PathBuf>,
    pub source_text: String,

    pub scroll: usize,
    pub hoffset: usize,
    pub wrap: bool,
    pub source_view: bool,
    /// Wrap setting to restore when leaving source view.
    wrap_before_source: bool,
    pub double_height: bool,

    pub cursor: Option<usize>,
    pub selection: Option<(usize, usize)>,
    pub mode: Mode,
    pub pending: Option<char>,
    pub help: bool,
    help_doc: Option<Document>,
    pub toc: bool,
    toc_doc: Option<Document>,

    pub matches: Vec<Match>,
    pub current_match: Option<usize>,
    pub last_query: String,
    pub last_forward: bool,

    pub picks: Vec<Pick>,
    pub toast: Option<(String, Instant)>,
    pub placement: Placement,
    pub quit: bool,
    pub keymap: Keymap,

    /// Graphics capability and cell geometry, detected once at startup.
    pub graphics: Protocol,
    pub cell: CellSize,
    pub raster_headings: bool,
    pub images: ImageStore,

    /// Set when `v` is pressed: the file and line to open an editor at. The
    /// event loop picks it up, because leaving and re-entering the alternate
    /// screen is the one thing [`App`] cannot do for itself.
    pub edit_request: Option<(PathBuf, usize)>,

    /// Diagrams being rendered by `mmdc` on a worker thread.
    diagrams: Option<Receiver<Diagram>>,
    /// How many diagrams this session would have drawn as pictures if `mmdc`
    /// were installed. Reported once on the way out; see [`App::prepare_diagrams`].
    pub mmdc_wanted: usize,
    /// Images being fetched over the network by worker threads.
    downloads: Option<Receiver<Fetched>>,
    /// Bumped on every reparse, so results for a document that has since been
    /// replaced are recognised as stale and dropped.
    generation: u64,
}

/// A finished `mmdc` render, on its way back to the UI thread.
struct Diagram {
    generation: u64,
    block: usize,
    path: PathBuf,
    size: (u32, u32),
}

/// A finished download, on its way back to the UI thread.
struct Fetched {
    generation: u64,
    /// Index into [`Document::images`].
    image: usize,
    path: PathBuf,
    size: (u32, u32),
}

/// How many downloads run at once.
///
/// A README's badge row is half a dozen requests to two or three hosts, so a
/// handful of threads turns a visible stagger into one blink. More than this
/// would just be rude to the hosts.
const FETCH_THREADS: usize = 4;

/// What the terminal can do with pictures. Passed in rather than detected
/// inside [`App`] so tests are not at the mercy of the environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphicsInfo {
    pub protocol: Protocol,
    pub cell: CellSize,
    /// Draw headings as bitmaps because the terminal has graphics but no
    /// DECDHL.
    pub raster_headings: bool,
}

impl GraphicsInfo {
    pub fn detect() -> GraphicsInfo {
        GraphicsInfo {
            protocol: graphics::detect(),
            cell: graphics::cell_size(),
            raster_headings: false,
        }
    }

    pub fn disabled() -> GraphicsInfo {
        GraphicsInfo {
            protocol: Protocol::None,
            cell: CellSize::default(),
            raster_headings: false,
        }
    }
}

impl App {
    pub fn new(
        source_text: String,
        path: Option<PathBuf>,
        settings: Settings,
        theme: Theme,
        screen: Screen,
        graphics: GraphicsInfo,
    ) -> App {
        let doc = crate::parse::parse(&source_text, &theme);
        let (keymap, problems) = Keymap::new(&settings.keys);
        let mut app = App {
            keymap,
            wrap: settings.wrap,
            source_view: settings.source,
            wrap_before_source: settings.wrap,
            double_height: settings.double_height,
            settings,
            theme,
            doc,
            lines: Vec::new(),
            screen,
            path,
            source_text,
            scroll: 0,
            hoffset: 0,
            cursor: None,
            selection: None,
            mode: Mode::Normal,
            pending: None,
            help: false,
            help_doc: None,
            toc: false,
            toc_doc: None,
            matches: Vec::new(),
            current_match: None,
            last_query: String::new(),
            last_forward: true,
            picks: Vec::new(),
            toast: None,
            placement: Placement::default(),
            quit: false,
            graphics: graphics.protocol,
            cell: graphics.cell,
            raster_headings: graphics.raster_headings,
            edit_request: None,
            diagrams: None,
            mmdc_wanted: 0,
            downloads: None,
            generation: 0,
            images: {
                let mut store = ImageStore::new(graphics.protocol, graphics.cell);
                if graphics.raster_headings {
                    store.big = crate::bigtext::Renderer::discover();
                }
                store
            },
        };
        // Source view means "show me exactly what is in the file", which only
        // works if one logical line stays on one row.
        if app.source_view {
            app.wrap = false;
        }
        // A key binding that could not be understood is worth saying out loud;
        // silently unbound keys look like the program is broken.
        if let Some(first) = problems.first() {
            app.toast(first);
        }
        app.measure_images();
        app.relayout();
        app.prepare_diagrams();
        app
    }

    /// Whether the theme's background is dark, which decides which mermaid
    /// palette `mmdc` should use. Unknown means the `terminal` theme, and
    /// terminals are dark far more often than not.
    fn theme_is_dark(&self) -> bool {
        match self.theme.background {
            Some(crate::ir::Color::Rgb { r, g, b }) => {
                // Rec. 601 luma, which is close enough for a binary choice.
                (r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000 < 128
            }
            _ => true,
        }
    }

    /// The mermaid blocks in this document that only `mmdc` can draw.
    ///
    /// In the default mode those are the ones the box renderer declines and the
    /// ones whose drawing is wider than the screen, both of which the reader is
    /// shown as source instead. `--mermaid image` asks for a picture of every
    /// diagram, box-drawable or not.
    fn diagrams_for_mmdc(&self) -> Vec<(usize, String)> {
        let width = self.screen.viewport().cols as usize;
        let calc = self.calc();
        self.doc
            .blocks
            .iter()
            .enumerate()
            .filter(|(_, b)| {
                matches!(&b.kind, BlockKind::Code { lang: Some(lang) }
                    if lang.eq_ignore_ascii_case("mermaid"))
            })
            .filter(|(_, b)| match self.settings.mermaid {
                MermaidMode::Image => true,
                // Box drawings win when they can render the diagram and it
                // fits; they are instant and they are selectable text.
                _ => match crate::mermaid::render(&b.text(), &calc) {
                    Some(rows) => rows.iter().any(|r| calc.str(r) > width),
                    None => true,
                },
            })
            .map(|(i, b)| (i, b.text()))
            .collect()
    }

    /// Hand any mermaid block that needs `mmdc` to a worker thread.
    ///
    /// Launching a browser takes long enough to be felt, so it never happens on
    /// the UI thread; the box-drawing render or the source shows immediately
    /// and the picture replaces it when it arrives.
    ///
    /// Where `mmdc` is missing there is nothing to hand over, and the count is
    /// kept instead so the reader can be told on the way out. Only here: this
    /// is the one place that knows a diagram was shown as source *and* that a
    /// picture was possible — with no graphics, or with images or `mmdc` turned
    /// off, installing anything would change nothing and saying so is noise.
    pub fn prepare_diagrams(&mut self) {
        self.diagrams = None;
        if self.settings.mermaid == MermaidMode::Text
            || !self.settings.images
            || !self.graphics.available()
        {
            return;
        }

        let jobs = self.diagrams_for_mmdc();
        if jobs.is_empty() {
            return;
        }
        if !crate::mmdc::available() {
            // Kept rather than replaced: a diagram shown as source is worth
            // mentioning even if the window was later resized until the next
            // one fitted.
            self.mmdc_wanted = self.mmdc_wanted.max(jobs.len());
            return;
        }

        let (tx, rx) = channel();
        let generation = self.generation;
        let dark = self.theme_is_dark();
        std::thread::spawn(move || {
            for (block, code) in jobs {
                let Ok(path) = crate::mmdc::render(&code, dark) else {
                    continue;
                };
                let Some(size) = graphics::dimensions(&path) else {
                    continue;
                };
                // A closed channel means the document moved on; stop working.
                if tx
                    .send(Diagram {
                        generation,
                        block,
                        path,
                        size,
                    })
                    .is_err()
                {
                    return;
                }
            }
        });
        self.diagrams = Some(rx);
    }

    /// Take delivery of any finished diagrams. Returns whether anything landed.
    pub fn poll_diagrams(&mut self) -> bool {
        let Some(rx) = &self.diagrams else {
            return false;
        };
        let arrived: Vec<Diagram> = rx.try_iter().collect();
        let mut changed = false;
        for diagram in arrived {
            if diagram.generation != self.generation || diagram.block >= self.doc.blocks.len() {
                continue;
            }
            let id = ImageId(self.doc.images.len());
            let mut image = Image::new(diagram.path.display().to_string(), "");
            image.measured(diagram.size);
            self.doc.images.push(image);
            self.images.register(id.0, diagram.path);
            self.doc.blocks[diagram.block].image = Some(id);
            changed = true;
        }
        if changed {
            self.relayout();
        }
        changed
    }

    /// Resolve every image reference against the document's directory and read
    /// its pixel size.
    ///
    /// Layout is pure and cannot touch the disk, so this has to happen first;
    /// an image whose size stays `None` renders as its alt text. A remote image
    /// already in the cache counts as measured; the rest are handed to worker
    /// threads and appear when they land.
    ///
    /// This is the first thing a new document does, so it is where the
    /// generation counter turns over.
    pub fn measure_images(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        crate::cache::prepare();
        let base = self
            .path
            .as_ref()
            .and_then(|p| p.parent())
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        self.images.forget_all();

        let mut wanted: Vec<(usize, String)> = Vec::new();
        for (i, image) in self.doc.images.iter_mut().enumerate() {
            image.size = None;
            let path = if crate::fetch::is_remote(&image.url) {
                match crate::fetch::cached(&image.url) {
                    Some(path) => path,
                    None => {
                        wanted.push((i, image.url.clone()));
                        continue;
                    }
                }
            } else if image.url.contains("://") {
                // Some other scheme — `data:`, `file:` — which nothing here
                // knows how to open. It stays as alt text.
                continue;
            } else {
                base.join(&image.url)
            };
            if let Some(size) = graphics::dimensions(&path) {
                image.measured(size);
                self.images.register(i, path);
            }
        }
        self.start_downloads(wanted);
    }

    /// Hand the images that are not on disk yet to worker threads.
    ///
    /// Nothing waits: the document is drawn with alt text where the pictures
    /// will go, and each one replaces its text as it arrives.
    fn start_downloads(&mut self, wanted: Vec<(usize, String)>) {
        self.downloads = None;
        if wanted.is_empty()
            || !self.settings.remote_images
            || !self.settings.images
            || !self.graphics.available()
        {
            return;
        }

        let (tx, rx) = channel();
        let generation = self.generation;
        for worker in 0..FETCH_THREADS.min(wanted.len()) {
            // Dealt round-robin rather than in chunks, so the images near the
            // top of the document — the ones on screen — are spread across the
            // threads instead of queued behind one of them.
            let jobs: Vec<(usize, String)> = wanted
                .iter()
                .skip(worker)
                .step_by(FETCH_THREADS)
                .cloned()
                .collect();
            let tx = tx.clone();
            std::thread::spawn(move || {
                for (image, url) in jobs {
                    // A failed fetch is not worth a toast: the alt text staying
                    // put says it well enough, and a document can point at a
                    // dozen dead badges.
                    let Ok(path) = crate::fetch::fetch(&url) else {
                        continue;
                    };
                    let Some(size) = graphics::dimensions(&path) else {
                        continue;
                    };
                    // A closed channel means the document moved on; stop.
                    if tx
                        .send(Fetched {
                            generation,
                            image,
                            path,
                            size,
                        })
                        .is_err()
                    {
                        return;
                    }
                }
            });
        }
        self.downloads = Some(rx);
    }

    /// Take delivery of any finished downloads. Returns whether anything landed.
    pub fn poll_downloads(&mut self) -> bool {
        let Some(rx) = &self.downloads else {
            return false;
        };
        let arrived: Vec<Fetched> = rx.try_iter().collect();
        let mut changed = false;
        for fetched in arrived {
            if fetched.generation != self.generation {
                continue;
            }
            let Some(image) = self.doc.images.get_mut(fetched.image) else {
                continue;
            };
            image.measured(fetched.size);
            self.images.register(fetched.image, fetched.path);
            changed = true;
        }
        if changed {
            self.relayout();
        }
        changed
    }

    pub fn calc(&self) -> WidthCalc {
        self.settings.calc()
    }

    fn options(&self) -> Options {
        Options {
            wrap: self.wrap,
            source: self.source_view && !self.overlaid(),
            max_width: self.settings.width,
            margin: self.settings.margin,
            calc: self.calc(),
            double_height: self.double_height,
            raster_headings: self.double_height && self.raster_headings,
            images: self.settings.images && self.graphics.available(),
            cell: self.cell,
        }
    }

    fn active_doc(&self) -> &Document {
        pick_doc(
            (self.help, &self.help_doc),
            (self.toc, &self.toc_doc),
            &self.doc,
        )
    }

    /// True when the main document is hidden behind help or the contents pane.
    fn overlaid(&self) -> bool {
        self.help || self.toc
    }

    pub fn relayout(&mut self) {
        let anchor = self.current_source_line();
        let opts = self.options();
        let view = self.screen.viewport();
        self.lines = layout::layout(self.active_doc(), view, &opts, &self.theme);
        // Mode switches and resizes preserve the reading position by mapping
        // the top row to a source line and back again.
        self.scroll = row_for_source_line(&self.lines, anchor).min(self.max_scroll());
        self.refresh_matches();
    }

    pub fn current_source_line(&self) -> usize {
        self.lines
            .get(self.scroll)
            .map(|l| l.source_line)
            .unwrap_or(1)
    }

    // ---- scrolling -------------------------------------------------------

    /// How many layout rows fit in `rows` screen rows starting at `from`,
    /// counting double-height lines twice.
    pub fn lines_fitting(&self, from: usize, rows: u16) -> usize {
        let mut used = 0u16;
        let mut count = 0usize;
        for line in &self.lines[from.min(self.lines.len())..] {
            let cost = line.scale.rows();
            if used + cost > rows {
                break;
            }
            used += cost;
            count += 1;
        }
        count.max(1)
    }

    /// Largest horizontal offset that still leaves content on screen.
    ///
    /// Without it, `l` in no-wrap mode walks off the right-hand end of the
    /// widest line and into empty screens, with nothing to say how far back the
    /// text is. The widest line is measured rather than remembered, because it
    /// changes with every relayout and this runs only on a keystroke.
    pub fn max_hoffset(&self) -> usize {
        let widest = self
            .lines
            .iter()
            .map(|line| self.calc().str(&line.text()))
            .max()
            .unwrap_or(0);
        // One screenful of the widest line stays visible.
        widest.saturating_sub(self.screen.viewport().cols as usize)
    }

    /// Largest scroll position that still fills the viewport from the bottom.
    pub fn max_scroll(&self) -> usize {
        let rows = self.screen.viewport().rows;
        let mut used = 0u16;
        let mut first = self.lines.len();
        for (i, line) in self.lines.iter().enumerate().rev() {
            let cost = line.scale.rows();
            if used + cost > rows {
                break;
            }
            used += cost;
            first = i;
        }
        first
    }

    pub fn scroll_by(&mut self, delta: isize) {
        let target = self.scroll as isize + delta;
        self.scroll = target.clamp(0, self.max_scroll() as isize) as usize;
    }

    pub fn page(&mut self, forward: bool) {
        let rows = self.screen.viewport().rows;
        let step = self.lines_fitting(self.scroll, rows) as isize;
        self.scroll_by(if forward { step } else { -step });
    }

    pub fn half_page(&mut self, forward: bool) {
        let rows = self.screen.viewport().rows / 2;
        let step = self.lines_fitting(self.scroll, rows.max(1)) as isize;
        self.scroll_by(if forward { step } else { -step });
    }

    pub fn ensure_visible(&mut self, line: usize) {
        if line < self.scroll {
            self.scroll = line;
            return;
        }
        let rows = self.screen.viewport().rows;
        let last = self.scroll + self.lines_fitting(self.scroll, rows);
        if line >= last {
            // Walk back from the target so it lands on the bottom row.
            let mut used = 0u16;
            let mut first = line;
            for (i, l) in self.lines[..=line.min(self.lines.len().saturating_sub(1))]
                .iter()
                .enumerate()
                .rev()
            {
                let cost = l.scale.rows();
                if used + cost > rows {
                    break;
                }
                used += cost;
                first = i;
            }
            self.scroll = first;
        }
    }

    pub fn resize(&mut self, screen: Screen) {
        self.screen = screen;
        self.relayout();
    }

    // ---- modes -----------------------------------------------------------

    pub fn toggle_wrap(&mut self) {
        self.wrap = !self.wrap;
        self.hoffset = 0;
        self.relayout();
        self.toast(if self.wrap { "wrap" } else { "no-wrap" });
    }

    /// Toggling into source view forces no-wrap and restores the previous wrap
    /// setting on the way back. Dragging a selection across reflowed text
    /// inserts hard line breaks into whatever you copy; one logical line per
    /// row is what makes the terminal's own selection give you the text as
    /// written.
    pub fn toggle_source(&mut self) {
        self.source_view = !self.source_view;
        if self.source_view {
            self.wrap_before_source = self.wrap;
            self.wrap = false;
        } else {
            self.wrap = self.wrap_before_source;
        }
        self.hoffset = 0;
        self.relayout();
        self.toast(if self.source_view {
            "source"
        } else {
            "rendered"
        });
    }

    pub fn cycle_theme(&mut self) {
        let names = theme::available_names();
        let idx = names
            .iter()
            .position(|n| *n == self.theme.name)
            .unwrap_or(0);
        let next = &names[(idx + 1) % names.len()];
        if let Ok(theme) = theme::load(next) {
            self.settings.theme = theme.name.clone();
            self.theme = theme;
            // Styles are baked into spans at parse time, so a theme change
            // means a reparse. Parsing is cheap next to a redraw.
            self.reparse();
            self.toast(&format!("theme: {}", self.settings.theme));
        }
    }

    fn reparse(&mut self) {
        self.doc = crate::parse::parse(&self.source_text, &self.theme);
        self.measure_images();
        self.help_doc = None;
        self.toc_doc = None;
        self.toc = false;
        self.prepare_diagrams();
        if self.help {
            self.help_doc = Some(crate::parse::parse(HELP, &self.theme));
        }
        self.relayout();
    }

    pub fn toggle_double_height(&mut self) {
        self.double_height = !self.double_height;
        self.relayout();
        self.toast(if self.double_height {
            "big headings"
        } else {
            "plain headings"
        });
    }

    pub fn toggle_images(&mut self) {
        self.settings.images = !self.settings.images;
        // Turning them on is the first chance the document has had to fetch
        // anything, if it started with images off.
        if self.settings.images {
            self.measure_images();
            self.prepare_diagrams();
        }
        self.relayout();
        self.toast(if self.settings.images {
            "images on"
        } else {
            "images off"
        });
    }

    pub fn toggle_help(&mut self) {
        self.help = !self.help;
        if self.help {
            self.toc = false;
            if self.help_doc.is_none() {
                self.help_doc = Some(crate::parse::parse(HELP, &self.theme));
            }
        }
        self.scroll = 0;
        self.cursor = None;
        self.relayout();
    }

    /// A table of contents, built as a Markdown document of links.
    ///
    /// Reusing the document machinery means the link picker, the block cursor,
    /// and `o` all work in the contents pane without any new modes.
    pub fn toggle_toc(&mut self) {
        self.toc = !self.toc;
        if self.toc {
            self.help = false;
            let markdown = self.toc_markdown();
            if markdown.is_none() {
                self.toc = false;
                self.toast("no headings");
                return;
            }
            self.toc_doc = Some(crate::parse::parse(&markdown.unwrap(), &self.theme));
        }
        self.scroll = 0;
        self.cursor = None;
        self.relayout();
    }

    fn toc_markdown(&self) -> Option<String> {
        let headings: Vec<(u8, String, usize)> = self
            .doc
            .blocks
            .iter()
            .filter_map(|b| {
                b.heading_level()
                    .map(|level| (level, b.text(), b.source_range.start))
            })
            .collect();
        if headings.is_empty() {
            return None;
        }
        let mut out = String::from(
            "# Contents

",
        );
        let top = headings.iter().map(|(l, _, _)| *l).min().unwrap_or(1);
        for (level, text, line) in headings {
            let indent = "  ".repeat((level.saturating_sub(top)) as usize);
            let label = text.replace(['[', ']'], "");
            out.push_str(&format!(
                "{indent}- [{label}](#line-{line})
"
            ));
        }
        Some(out)
    }

    /// The innermost heading at or above the top of the screen.
    pub fn breadcrumb(&self) -> String {
        if self.overlaid() {
            return String::new();
        }
        let line = self.current_source_line();
        let mut trail: Vec<(u8, String)> = Vec::new();
        for block in &self.doc.blocks {
            let Some(level) = block.heading_level() else {
                continue;
            };
            if block.source_range.start > line {
                break;
            }
            trail.retain(|(l, _)| *l < level);
            trail.push((level, block.text()));
        }
        trail
            .into_iter()
            .map(|(_, text)| text)
            .collect::<Vec<_>>()
            .join(" › ")
    }

    // ---- block cursor and yanking ---------------------------------------

    pub fn block_count(&self) -> usize {
        self.active_doc().blocks.len()
    }

    pub fn move_cursor(&mut self, forward: bool) {
        let count = self.block_count();
        if count == 0 {
            return;
        }
        self.cursor = Some(match self.cursor {
            None => {
                // Start from whatever is at the top of the screen, not from the
                // top of the document.
                self.lines.get(self.scroll).map(|l| l.block).unwrap_or(0)
            }
            Some(current) if forward => (current + 1).min(count - 1),
            Some(current) => current.saturating_sub(1),
        });
        if let Some(idx) = self.cursor
            && let Some(row) = self.lines.iter().position(|l| l.block == idx)
        {
            self.ensure_visible(row);
        }
    }

    pub fn cursor_block(&self) -> Option<&Block> {
        self.cursor.and_then(|i| self.active_doc().blocks.get(i))
    }

    /// The rendered plain text of the block under the cursor, taken from the
    /// laid-out rows so it matches what is on screen.
    pub fn cursor_rendered_text(&self) -> Option<String> {
        let idx = self.cursor?;
        // The margin is a reading aid, not part of the text, so it comes off
        // again on the way to the clipboard.
        let margin = " ".repeat(self.settings.margin);
        let text: Vec<String> = self
            .lines
            .iter()
            .filter(|l| l.block == idx)
            .map(|l| {
                let line = l.text();
                line.strip_prefix(&margin)
                    .unwrap_or(&line)
                    .trim_end()
                    .to_string()
            })
            .collect();
        (!text.is_empty()).then(|| text.join("\n"))
    }

    pub fn selected_source(&self) -> Option<String> {
        let (a, b) = self.selection?;
        let (lo, hi) = (a.min(b), a.max(b));
        let first = self.lines.get(lo)?.source_line;
        let last = self.lines.get(hi)?.source_line;
        let doc = self.active_doc();
        let start = first.saturating_sub(1);
        let end = last.min(doc.source_lines.len());
        (start < end).then(|| doc.source_lines[start..end].join("\n"))
    }

    pub fn yank<W: Write>(&mut self, out: &mut W, what: Yank) -> Result<()> {
        let text = match what {
            Yank::Source => self
                .cursor_block()
                .map(|b| self.active_doc().source_of(b))
                .or_else(|| self.selected_source()),
            Yank::Rendered => self.cursor_rendered_text(),
            Yank::CodeBody => self
                .cursor_block()
                .and_then(|b| matches!(b.kind, BlockKind::Code { .. }).then(|| b.text())),
            Yank::Path => self
                .path
                .as_ref()
                .map(|p| p.display().to_string())
                .or_else(|| Some("(stdin)".into())),
            Yank::Selection => self.selected_source(),
        };
        match text {
            Some(text) if !text.is_empty() => {
                let route = clipboard::copy(out, &text)?;
                let lines = text.lines().count();
                self.toast(&format!("yanked {lines} line(s) to {}", route.label()));
            }
            _ => self.toast(match what {
                Yank::CodeBody => "not a code block",
                _ => "nothing to yank",
            }),
        }
        Ok(())
    }

    // ---- links -----------------------------------------------------------

    /// Links currently on screen, each with the row and column it was drawn at.
    ///
    /// The column comes from the link's own hit rectangle. A row of badges is
    /// several links on one row, and labelling them all at the row's first
    /// column would stack them in one place.
    pub fn visible_links(&self) -> Vec<VisibleLink> {
        let rows = self.screen.viewport().rows;
        let end = self.scroll + self.lines_fitting(self.scroll, rows);
        let mut out = Vec::new();
        for (i, line) in self.lines[self.scroll.min(self.lines.len())..end.min(self.lines.len())]
            .iter()
            .enumerate()
        {
            for hit in &line.hits {
                if let HitTarget::Link(id) = hit.target {
                    out.push(VisibleLink {
                        row: self.scroll + i,
                        col: hit.rect.x as usize,
                        link: id.0,
                    });
                }
            }
        }
        out
    }

    pub fn start_link_pick(&mut self) {
        let links = self.visible_links();
        if links.is_empty() {
            self.toast("no links on screen");
            return;
        }
        // There are only so many label keys. Saying so is the difference
        // between a picker that ran out and a picker that missed something.
        if links.len() > PICK_KEYS.len() {
            let extra = links.len() - PICK_KEYS.len();
            self.toast(&format!(
                "{extra} more link(s) than labels; scroll for them"
            ));
        }
        self.picks = links
            .into_iter()
            .take(PICK_KEYS.len())
            .enumerate()
            .map(|(i, link)| Pick {
                label: PICK_KEYS[i] as char,
                row: link.row,
                col: link.col,
                link: link.link,
            })
            .collect();
        self.mode = Mode::LinkPick;
    }

    pub fn pick_overlays(&self) -> Vec<Overlay> {
        self.picks
            .iter()
            .map(|pick| Overlay {
                line: pick.row,
                col: pick.col,
                text: pick.label.to_string(),
                style: self.theme.hint,
            })
            .collect()
    }

    /// What `o` acts on: the first link in the block under the cursor, or the
    /// image if the block is one.
    pub fn target_under_cursor(&self) -> Option<String> {
        let idx = self.cursor?;
        let doc = self.active_doc();
        let block = doc.blocks.get(idx)?;
        // A badge row has several; `o` takes the first, the way it takes the
        // first link in a paragraph.
        if let BlockKind::Images(ids) = &block.kind
            && let Some(id) = ids.first()
        {
            return self.image_target(*id);
        }
        let id = block.spans.iter().find_map(|s| s.link)?;
        doc.links.get(id.0).map(|l| l.url.clone())
    }

    /// What opening an image should go to: where it links, if it links
    /// anywhere, and otherwise the picture itself.
    ///
    /// A badge is a picture of a build status wrapped in a link to the build.
    /// The link is the useful half.
    pub fn image_target(&self, id: ImageId) -> Option<String> {
        let doc = self.active_doc();
        let image = doc.images.get(id.0)?;
        image
            .link
            .and_then(|link| doc.links.get(link.0))
            .map(|link| link.url.clone())
            .or_else(|| Some(image.url.clone()))
    }

    pub fn open(&mut self, url: &str) {
        // Contents-pane entries link to a source line rather than a file.
        if let Some(line) = url.strip_prefix("#line-")
            && let Ok(line) = line.parse::<usize>()
        {
            self.jump_to_source_line(line);
            return;
        }
        // An anchor into this document — `[Terminal support](#terminal-support)`
        // — is a jump, not something to hand to a web browser.
        if let Some(anchor) = url.strip_prefix('#') {
            match self.source_line_of_anchor(anchor) {
                Some(line) => self.jump_to_source_line(line),
                None => self.toast(&format!("no heading matches #{anchor}")),
            }
            return;
        }
        // A relative Markdown path opens in the viewer; anything else is the
        // system's problem.
        let local = self.resolve_local(url);
        match local {
            Some(path) if is_markdown(&path) => {
                if let Err(err) = self.load(&path) {
                    self.toast(&format!("cannot open: {err}"));
                }
            }
            // A local file that is not Markdown — an image, a PDF — goes to the
            // system handler by its resolved path, not the relative link text.
            Some(path) => self.hand_off(&path.display().to_string()),
            None => self.hand_off(url),
        }
    }

    /// Leave whatever pane is open and put `line` of the file at the top.
    fn jump_to_source_line(&mut self, line: usize) {
        self.toc = false;
        self.help = false;
        self.cursor = None;
        self.relayout();
        self.scroll = row_for_source_line(&self.lines, line).min(self.max_scroll());
    }

    /// The line a GitHub-style `#anchor` points at.
    ///
    /// Headings are slugged the way GitHub slugs them and matched in document
    /// order, so a repeated heading resolves to the first one — GitHub's
    /// `-1`, `-2` suffixes for the later ones fall out of the same walk.
    fn source_line_of_anchor(&self, anchor: &str) -> Option<usize> {
        let wanted = anchor.to_lowercase();
        let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for block in &self.doc.blocks {
            if block.heading_level().is_none() {
                continue;
            }
            let base = slugify(&block.text());
            let count = seen.entry(base.clone()).or_insert(0);
            let slug = if *count == 0 {
                base
            } else {
                format!("{base}-{count}")
            };
            *count += 1;
            if slug == wanted {
                return Some(block.source_range.start);
            }
        }
        None
    }

    fn hand_off(&mut self, target: &str) {
        match open_external(target) {
            Ok(()) => self.toast(&format!("opened {target}")),
            Err(err) => self.toast(&format!("cannot open: {err}")),
        }
    }

    fn resolve_local(&self, url: &str) -> Option<PathBuf> {
        if url.contains("://") || url.starts_with("mailto:") {
            return None;
        }
        let base = self
            .path
            .as_ref()
            .and_then(|p| p.parent())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let path = base.join(url.split('#').next().unwrap_or(url));
        path.exists().then_some(path)
    }

    pub fn load(&mut self, path: &Path) -> Result<()> {
        let text = std::fs::read_to_string(path)?;
        self.source_text = text;
        self.path = Some(path.to_path_buf());
        self.scroll = 0;
        self.cursor = None;
        self.help = false;
        self.reparse();
        self.toast(&format!("opened {}", path.display()));
        Ok(())
    }

    /// Ask the event loop to open an editor at the line currently on top.
    pub fn edit(&mut self) {
        if self.overlaid() {
            self.toast("nothing to edit here");
            return;
        }
        match self.path.clone() {
            Some(path) => self.edit_request = Some((path, self.current_source_line())),
            None => self.toast("no file to edit"),
        }
    }

    /// Last modification time of the open file, for `--watch`.
    pub fn mtime(&self) -> Option<std::time::SystemTime> {
        std::fs::metadata(self.path.as_ref()?).ok()?.modified().ok()
    }

    pub fn reload(&mut self) {
        let Some(path) = self.path.clone() else {
            self.toast("nothing to reload");
            return;
        };
        let anchor = self.current_source_line();
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                self.source_text = text;
                self.reparse();
                self.scroll = row_for_source_line(&self.lines, anchor).min(self.max_scroll());
                self.toast("reloaded");
            }
            Err(err) => self.toast(&format!("reload failed: {err}")),
        }
    }

    // ---- search ----------------------------------------------------------

    pub fn refresh_matches(&mut self) {
        let query = match &self.mode {
            Mode::Search { query, .. } => query.clone(),
            _ => self.last_query.clone(),
        };
        self.matches = find_matches(&self.lines, &query, &self.calc());
        if self.matches.is_empty() {
            self.current_match = None;
        }
    }

    pub fn jump_match(&mut self, forward: bool) {
        if self.matches.is_empty() {
            self.toast("no matches");
            return;
        }
        let from = self.scroll;
        let next = if forward {
            self.matches.iter().position(|m| m.line > from).unwrap_or(0)
        } else {
            self.matches
                .iter()
                .rposition(|m| m.line < from)
                .unwrap_or(self.matches.len() - 1)
        };
        self.current_match = Some(next);
        let line = self.matches[next].line;
        self.ensure_visible(line);
        self.scroll = line.min(self.max_scroll());
    }

    pub fn highlights(&self) -> Vec<Highlight> {
        self.matches
            .iter()
            .enumerate()
            .map(|(i, m)| Highlight {
                line: m.line,
                start: m.start,
                end: m.end,
                style: if Some(i) == self.current_match {
                    self.theme.search_current
                } else {
                    self.theme.search_match
                },
            })
            .collect()
    }

    // ---- feedback --------------------------------------------------------

    pub fn toast(&mut self, text: &str) {
        self.toast = Some((text.to_string(), Instant::now()));
    }

    pub fn toast_expired(&self) -> bool {
        self.toast
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed() >= TOAST_DURATION)
    }

    /// The key that opens the help, as configured rather than as shipped —
    /// telling a reader who rebound it to press `H` would be a lie. `None`
    /// when the help has been left unbound.
    fn help_key(&self) -> Option<String> {
        self.keymap.keys_for(Action::Help).into_iter().next()
    }

    /// Contents of the bottom row: the prompt while searching, a toast if one
    /// is live, the status line if it is enabled, and otherwise nothing.
    pub fn bottom_row(&self) -> Vec<Span> {
        if let Mode::Search { query, forward } = &self.mode {
            let sigil = if *forward { '/' } else { '?' };
            let mut spans = vec![Span::new(format!("{sigil}{query}"), self.theme.body())];
            // `?` is backward search here, as in a pager, but it is the help
            // key nearly everywhere else. Someone who pressed it looking for
            // help is now staring at a prompt that says nothing; tell them
            // where the help is, until the first character says they meant
            // to search after all.
            if !*forward
                && query.is_empty()
                && let Some(key) = self.help_key()
            {
                spans.push(Span::new(format!("  help: {key}"), self.theme.dim));
            }
            return spans;
        }
        if let Mode::LinkPick = self.mode {
            return vec![Span::new(
                "pick a link, Esc to cancel".to_string(),
                self.theme.toast,
            )];
        }
        if let Some((text, at)) = &self.toast
            && at.elapsed() < TOAST_DURATION
        {
            return vec![Span::new(format!(" {text} "), self.theme.toast)];
        }
        if self.settings.status {
            return vec![Span::new(self.status_text(), self.theme.status)];
        }
        Vec::new()
    }

    pub fn status_text(&self) -> String {
        let name = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .unwrap_or("(stdin)");
        let total = self.lines.len().max(1);
        let percent = (self.scroll * 100) / total;
        let mode = if self.source_view { "source" } else { "render" };
        let wrap = if self.wrap { "wrap" } else { "no-wrap" };
        let where_ = self.breadcrumb();
        let suffix = if where_.is_empty() {
            String::new()
        } else {
            format!("  {where_}")
        };
        format!(" {name}  {mode}  {wrap}  {percent}%{suffix} ")
    }

    // ---- drawing ---------------------------------------------------------

    pub fn draw<W: Write>(&mut self, out: &mut W, renderer: &Renderer) -> Result<()> {
        let bottom = self.bottom_row();
        let highlights = self.highlights();
        let overlays = if self.mode == Mode::LinkPick {
            self.pick_overlays()
        } else {
            Vec::new()
        };
        // Field-by-field borrows: the image store is taken mutably while the
        // rest of the state is read, so no method call on `self` may be live.
        // Hence the free function rather than `active_doc`, which would borrow
        // all of `self` — but it has to be the *same* choice, or the links
        // drawn belong to a different document than the lines they sit on.
        let doc = pick_doc(
            (self.help, &self.help_doc),
            (self.toc, &self.toc_doc),
            &self.doc,
        );
        let frame = Frame {
            screen: self.screen,
            lines: &self.lines,
            scroll: self.scroll,
            hoffset: self.hoffset,
            links: &doc.links,
            bottom: &bottom,
            theme: &self.theme,
            // DECDHL and bitmaps are alternatives, never both at once.
            double_height: self.double_height && !self.raster_headings,
            raster_headings: self.double_height && self.raster_headings,
            decor: Decor {
                cursor_block: self.cursor,
                selection: self.selection,
                cursor_style: self.theme.cursor,
                highlights: &highlights,
                overlays: &overlays,
            },
        };
        let placement = renderer.draw(out, &frame, &mut self.images)?;
        self.placement = placement;
        Ok(())
    }

    // ---- key handling ----------------------------------------------------

    pub fn on_key<W: Write>(&mut self, out: &mut W, key: KeyEvent) -> Result<()> {
        match self.mode.clone() {
            Mode::Search { query, forward } => self.search_key(key, query, forward),
            Mode::LinkPick => self.pick_key(key),
            Mode::Select => self.select_key(out, key)?,
            Mode::Normal => self.normal_key(out, key)?,
        }
        Ok(())
    }

    fn search_key(&mut self, key: KeyEvent, mut query: String, forward: bool) {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.matches.clear();
            }
            KeyCode::Enter => {
                self.last_query = query.clone();
                self.last_forward = forward;
                self.mode = Mode::Normal;
                self.refresh_matches();
                self.jump_match(forward);
            }
            KeyCode::Backspace => {
                query.pop();
                self.mode = Mode::Search { query, forward };
                self.refresh_matches();
            }
            KeyCode::Char(c) => {
                query.push(c);
                self.mode = Mode::Search { query, forward };
                self.refresh_matches();
                // Incremental: jump as you type, without committing.
                if let Some(m) = self.matches.first() {
                    let line = m.line;
                    self.ensure_visible(line);
                }
            }
            _ => {}
        }
    }

    fn pick_key(&mut self, key: KeyEvent) {
        self.mode = Mode::Normal;
        if let KeyCode::Char(c) = key.code
            && let Some(pick) = self.picks.iter().find(|pick| pick.label == c)
        {
            let url = self
                .active_doc()
                .links
                .get(pick.link)
                .map(|l| l.url.clone());
            if let Some(url) = url {
                self.open(&url);
            }
        }
        self.picks.clear();
    }

    fn select_key<W: Write>(&mut self, out: &mut W, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.selection = None;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if let Some((anchor, current)) = self.selection {
                    let next = (current + 1).min(self.lines.len().saturating_sub(1));
                    self.selection = Some((anchor, next));
                    self.ensure_visible(next);
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if let Some((anchor, current)) = self.selection {
                    let next = current.saturating_sub(1);
                    self.selection = Some((anchor, next));
                    self.ensure_visible(next);
                }
            }
            KeyCode::Char('y') => {
                self.yank(out, Yank::Selection)?;
                self.mode = Mode::Normal;
                self.selection = None;
            }
            _ => {}
        }
        Ok(())
    }

    fn normal_key<W: Write>(&mut self, out: &mut W, key: KeyEvent) -> Result<()> {
        // Two-key sequences: `yy`, `yc`, `yp`. Anything else after `y` was not
        // meant as a yank, so the sequence is abandoned and the key is handled
        // as itself rather than swallowed — pressing `y` by accident should
        // cost nothing.
        if let Some('y') = self.pending {
            self.pending = None;
            match key.code {
                KeyCode::Char('c') => return self.yank(out, Yank::CodeBody),
                KeyCode::Char('p') => return self.yank(out, Yank::Path),
                KeyCode::Char('y') => return self.yank(out, Yank::Source),
                _ => {}
            }
        }

        let Some(action) = self.keymap.lookup(&key) else {
            return Ok(());
        };
        self.act(out, action)
    }

    pub fn act<W: Write>(&mut self, out: &mut W, action: Action) -> Result<()> {
        match action {
            // Escape backs out of whatever is on top before it quits.
            Action::Quit if self.help => self.toggle_help(),
            Action::Quit if self.toc => self.toggle_toc(),
            Action::Quit if !self.matches.is_empty() => {
                self.matches.clear();
                self.last_query.clear();
            }
            Action::Quit => self.quit = true,

            Action::ScrollDown => self.scroll_by(1),
            Action::ScrollUp => self.scroll_by(-1),
            Action::HalfPageDown => self.half_page(true),
            Action::HalfPageUp => self.half_page(false),
            Action::PageDown => self.page(true),
            Action::PageUp => self.page(false),
            Action::Top => self.scroll = 0,
            Action::Bottom => self.scroll = self.max_scroll(),
            Action::ScrollLeft => self.hoffset = self.hoffset.saturating_sub(4),
            Action::ScrollRight => {
                if !self.wrap {
                    self.hoffset = (self.hoffset + 4).min(self.max_hoffset());
                }
            }
            Action::ResetScroll => self.hoffset = 0,

            Action::ToggleWrap => self.toggle_wrap(),
            Action::ToggleSource => self.toggle_source(),
            Action::CycleTheme => self.cycle_theme(),
            Action::ToggleBigHeadings => self.toggle_double_height(),
            Action::ToggleImages => self.toggle_images(),

            Action::CursorNext => self.move_cursor(true),
            Action::CursorPrev => self.move_cursor(false),
            Action::YankPrefix => self.pending = Some('y'),
            Action::YankRendered => self.yank(out, Yank::Rendered)?,
            Action::SelectLines => {
                self.selection = Some((self.scroll, self.scroll));
                self.mode = Mode::Select;
            }

            Action::LinkPick => self.start_link_pick(),
            Action::OpenUnderCursor => match self.target_under_cursor() {
                Some(url) => self.open(&url),
                None => self.toast("no link under the cursor"),
            },

            Action::SearchForward => {
                self.mode = Mode::Search {
                    query: String::new(),
                    forward: true,
                }
            }
            Action::SearchBackward => {
                self.mode = Mode::Search {
                    query: String::new(),
                    forward: false,
                }
            }
            Action::NextMatch => self.jump_match(self.last_forward),
            Action::PrevMatch => self.jump_match(!self.last_forward),

            Action::Reload => self.reload(),
            Action::Edit => self.edit(),
            Action::Contents => self.toggle_toc(),
            Action::Help => self.toggle_help(),
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Yank {
    Source,
    Rendered,
    CodeBody,
    Path,
    Selection,
}

/// A heading's anchor, the way GitHub derives one.
///
/// GitHub does this in three steps, and the order is what makes the results
/// surprising: lower-case it, delete everything that is not a word character,
/// a hyphen, or a space, and only then turn each space into a hyphen.
///
/// Deleting punctuation *before* substituting means it leaves its neighbouring
/// spaces behind, so `v0.9 — Mermaid` becomes `v09--mermaid` with two hyphens
/// where the dash was. Runs are not collapsed and the ends are not trimmed.
/// Letters outside ASCII are word characters, so `## 見出し` really does anchor
/// at `#見出し`.
pub fn slugify(heading: &str) -> String {
    let mut out = String::with_capacity(heading.len());
    for c in heading.chars() {
        if c == ' ' {
            out.push('-');
        } else if c.is_alphanumeric() || c == '-' || c == '_' {
            out.extend(c.to_lowercase());
        }
        // Everything else — punctuation, emoji, symbols, tabs — is dropped.
    }
    out
}

/// Which document is on screen: the help pane, the contents pane, or the file.
///
/// A free function taking the fields it needs, because the draw path holds a
/// mutable borrow of the image store and so cannot call a method on `App`. It
/// is the single answer to the question, which is the point — laying out one
/// document and drawing another's links is exactly the bug this prevents.
fn pick_doc<'a>(
    help: (bool, &'a Option<Document>),
    toc: (bool, &'a Option<Document>),
    doc: &'a Document,
) -> &'a Document {
    if let (true, Some(pane)) = help {
        return pane;
    }
    if let (true, Some(pane)) = toc {
        return pane;
    }
    doc
}

fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "md" | "markdown" | "mdown"))
}

pub fn open_external(url: &str) -> std::io::Result<()> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
}

/// Search every laid-out row, reporting display-column ranges.
///
/// Smartcase: an all-lowercase pattern matches case-insensitively.
pub fn find_matches(lines: &[Line], query: &str, calc: &WidthCalc) -> Vec<Match> {
    if query.is_empty() {
        return Vec::new();
    }
    let fold = query.chars().all(|c| !c.is_uppercase());
    let needle = if fold {
        query.to_lowercase()
    } else {
        query.to_string()
    };
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let text = line.text();
        let hay = if fold {
            text.to_lowercase()
        } else {
            text.clone()
        };
        let mut from = 0usize;
        while let Some(found) = hay[from..].find(&needle) {
            let start = from + found;
            let end = start + needle.len();
            // Lowercasing can change byte lengths, so map back through the
            // original text by counting characters rather than bytes.
            let start_col = calc.str(&prefix_of(&text, &hay, start));
            let end_col = calc.str(&prefix_of(&text, &hay, end));
            out.push(Match {
                line: i,
                start: start_col,
                end: end_col,
            });
            from = end.max(start + 1);
            if from >= hay.len() {
                break;
            }
        }
    }
    out
}

/// The prefix of `original` corresponding to `bytes` bytes of `folded`.
fn prefix_of(original: &str, folded: &str, bytes: usize) -> String {
    let chars = folded[..bytes.min(folded.len())].chars().count();
    original.chars().take(chars).collect()
}

/// A synthetic document listing the Markdown files in a directory, used when
/// `mdroll` is started with no arguments.
pub fn browser_markdown(dir: &Path) -> String {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| is_markdown(p))
        .collect();
    files.sort();

    let mut out = format!("# {}\n\n", dir.display());
    if files.is_empty() {
        out.push_str("No Markdown files here.\n\nPress `q` to quit.\n");
        return out;
    }
    out.push_str("Move with `Tab`, open with `o` or `Enter`. `F` labels every link.\n\n");
    for file in files {
        let name = file
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        out.push_str(&format!("- [{name}]({name})\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::Screen;
    use crossterm::event::KeyModifiers;

    fn app(src: &str) -> App {
        App::new(
            src.to_string(),
            Some(PathBuf::from("test.md")),
            Settings::default(),
            Theme::default(),
            Screen::new(40, 10),
            GraphicsInfo::disabled(),
        )
    }

    #[test]
    fn only_the_diagrams_the_box_renderer_cannot_draw_are_asked_of_mmdc() {
        // Launching a browser costs a second or two, so a flowchart the box
        // renderer draws is never sent — and one it declines always is, because
        // the reader is looking at the source of it otherwise.
        let app = app("```mermaid\nflowchart TD\n A --> B\n```\n\n\
             ```mermaid\nflowchart TD\n Z --> C\n Z --> D\n A --> D\n B --> C\n```\n");
        let jobs = app.diagrams_for_mmdc();
        assert_eq!(jobs.len(), 1, "{jobs:?}");
        assert!(jobs[0].1.contains("Z --> C"), "the wrong one: {jobs:?}");
    }

    #[test]
    fn a_document_with_no_diagram_asks_nothing_of_mmdc() {
        // Nothing to say on the way out about a tool this document has no use
        // for, whether or not it is installed.
        let app = app("# Title\n\nSome prose.\n\n```rust\nfn main() {}\n```\n");
        assert!(app.diagrams_for_mmdc().is_empty());
        assert_eq!(app.mmdc_wanted, 0);
    }

    #[test]
    fn a_drawable_diagram_is_not_a_reason_to_mention_mmdc() {
        // The notice on the way out is for a reader who lost something. A
        // flowchart drawn as box characters is not lost — it is selectable
        // text into the bargain — so nothing is asked of `mmdc` for it and
        // there is nothing to say about `mmdc` afterwards.
        let app = app("```mermaid\nflowchart TD\n A --> B\n```\n");
        assert!(app.diagrams_for_mmdc().is_empty());
        assert_eq!(app.mmdc_wanted, 0);
    }

    #[test]
    fn a_diagram_too_wide_for_the_screen_is_asked_of_mmdc_after_all() {
        // Drawn, but not at a size that fits: there is nothing sensible to
        // reflow in a diagram, so the reader is shown the source of this one
        // too and a picture would have been the better answer.
        let wide = (0..12)
            .map(|i| format!(" A --> Node{i}[a rather long label]"))
            .collect::<Vec<_>>()
            .join("\n");
        let app = app(&format!("```mermaid\nflowchart TD\n{wide}\n```\n"));
        assert_eq!(app.diagrams_for_mmdc().len(), 1);
    }

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn press(app: &mut App, c: char) {
        let mut sink = Vec::new();
        app.on_key(&mut sink, key(c)).unwrap();
    }

    fn long_doc() -> App {
        let body: String = (1..=50).map(|i| format!("line {i}\n\n")).collect();
        app(&body)
    }

    #[test]
    fn opening_a_badge_goes_where_it_links_not_to_its_picture() {
        let app = app("[![Build](b.svg)](https://a.example)\n");
        assert_eq!(
            app.image_target(ImageId(0)).as_deref(),
            Some("https://a.example")
        );
    }

    #[test]
    fn opening_an_unlinked_picture_goes_to_the_picture() {
        let app = app("![a picture](pic.png)\n");
        assert_eq!(app.image_target(ImageId(0)).as_deref(), Some("pic.png"));
    }

    #[test]
    fn a_remote_image_is_never_looked_for_on_disk() {
        let app = app("![logo](https://example.invalid/never-fetched-3b91c.svg)\n");
        assert_eq!(app.doc.images[0].size, None, "nothing to measure yet");
    }

    #[test]
    fn scrolling_stops_at_the_top_and_bottom() {
        let mut a = long_doc();
        press(&mut a, 'k');
        assert_eq!(a.scroll, 0);
        press(&mut a, 'G');
        let bottom = a.scroll;
        press(&mut a, 'j');
        assert_eq!(a.scroll, bottom, "cannot scroll past the end");
        press(&mut a, 'g');
        assert_eq!(a.scroll, 0);
    }

    #[test]
    fn the_last_page_still_fills_the_viewport() {
        let a = long_doc();
        let rows = a.screen.viewport().rows as usize;
        assert_eq!(a.max_scroll(), a.lines.len() - rows);
    }

    #[test]
    fn source_view_forces_no_wrap_and_restores_it_afterwards() {
        let mut a = app("# Title\n\nbody\n");
        assert!(a.wrap);
        press(&mut a, 's');
        assert!(a.source_view && !a.wrap);
        press(&mut a, 's');
        assert!(!a.source_view && a.wrap, "wrap must come back");
    }

    #[test]
    fn source_view_entered_from_no_wrap_stays_no_wrap() {
        let mut a = app("body\n");
        press(&mut a, 'w');
        assert!(!a.wrap);
        press(&mut a, 's');
        press(&mut a, 's');
        assert!(!a.wrap);
    }

    #[test]
    fn a_mode_switch_keeps_the_reading_position() {
        let mut a = long_doc();
        press(&mut a, 'G');
        let before = a.current_source_line();
        press(&mut a, 's');
        let after = a.current_source_line();
        assert!(
            before.abs_diff(after) <= 2,
            "position drifted from {before} to {after}"
        );
    }

    #[test]
    fn horizontal_scrolling_only_applies_without_wrap() {
        // Wider than the 40-column test screen, or there is nowhere to scroll.
        let mut a = app(&format!("{}\n", "a long line of text ".repeat(6)));
        press(&mut a, 'l');
        assert_eq!(a.hoffset, 0, "wrap mode has nothing to scroll to");
        press(&mut a, 'w');
        press(&mut a, 'l');
        assert_eq!(a.hoffset, 4);
        press(&mut a, '0');
        assert_eq!(a.hoffset, 0);
    }

    #[test]
    fn tab_moves_the_block_cursor_and_shift_tab_moves_back() {
        let mut a = app("# One\n\nTwo\n\nThree\n");
        let mut sink = Vec::new();
        a.on_key(&mut sink, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(a.cursor, Some(0));
        a.on_key(&mut sink, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(a.cursor, Some(1));
        a.on_key(
            &mut sink,
            KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE),
        )
        .unwrap();
        assert_eq!(a.cursor, Some(0));
    }

    #[test]
    fn yanking_a_block_copies_the_original_markdown() {
        let mut a = app("# One\n\n**Two** words\n");
        a.cursor = Some(1);
        let mut sink = Vec::new();
        a.yank(&mut sink, Yank::Source).unwrap();
        let text = String::from_utf8_lossy(&sink);
        assert!(
            text.is_empty() || text.contains("\x1b]52"),
            "either the system clipboard or OSC 52 is used"
        );
        assert!(a.toast.as_ref().unwrap().0.contains("yanked"));
    }

    #[test]
    fn yanking_rendered_text_drops_the_markup() {
        let mut a = app("**Two** words\n");
        a.cursor = Some(0);
        assert_eq!(a.cursor_rendered_text().unwrap(), "Two words");
    }

    #[test]
    fn yc_only_works_on_a_code_block() {
        let mut a = app("Just a paragraph.\n");
        a.cursor = Some(0);
        let mut sink = Vec::new();
        a.yank(&mut sink, Yank::CodeBody).unwrap();
        assert_eq!(a.toast.as_ref().unwrap().0, "not a code block");
    }

    #[test]
    fn yc_yanks_the_code_without_its_fences() {
        let mut a = app("```rust\nlet x = 1;\n```\n");
        a.cursor = Some(0);
        assert_eq!(a.cursor_block().unwrap().text(), "let x = 1;");
    }

    #[test]
    fn yp_yanks_the_file_path() {
        let mut a = app("body\n");
        let mut sink = Vec::new();
        a.yank(&mut sink, Yank::Path).unwrap();
        assert!(a.toast.as_ref().unwrap().0.contains("yanked"));
    }

    #[test]
    fn the_y_prefix_waits_for_a_second_key() {
        let mut a = app("body\n");
        press(&mut a, 'y');
        assert_eq!(a.pending, Some('y'));
        press(&mut a, 'p');
        assert_eq!(a.pending, None);
    }

    #[test]
    fn line_selection_extends_and_yanks() {
        let mut a = app("one\n\ntwo\n\nthree\n");
        let mut sink = Vec::new();
        a.on_key(
            &mut sink,
            KeyEvent::new(KeyCode::Char('V'), KeyModifiers::SHIFT),
        )
        .unwrap();
        assert_eq!(a.mode, Mode::Select);
        press(&mut a, 'j');
        assert_eq!(a.selection, Some((0, 1)));
        press(&mut a, 'y');
        assert_eq!(a.mode, Mode::Normal);
        assert!(a.selection.is_none());
    }

    #[test]
    fn search_finds_matches_and_n_walks_them() {
        let mut a = long_doc();
        press(&mut a, '/');
        for c in "line 3".chars() {
            press(&mut a, c);
        }
        let mut sink = Vec::new();
        a.on_key(&mut sink, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert!(!a.matches.is_empty());
        assert_eq!(a.mode, Mode::Normal);
        let first = a.scroll;
        press(&mut a, 'n');
        assert_ne!(a.scroll, first);
    }

    #[test]
    fn search_is_case_insensitive_unless_the_pattern_has_capitals() {
        let calc = WidthCalc::default();
        let lines = vec![
            Line::new(1, 0, vec![Span::plain("Hello")]),
            Line::new(2, 0, vec![Span::plain("hello")]),
        ];
        assert_eq!(find_matches(&lines, "hello", &calc).len(), 2);
        assert_eq!(find_matches(&lines, "Hello", &calc).len(), 1);
    }

    #[test]
    fn search_columns_account_for_wide_characters() {
        let calc = WidthCalc::default();
        let lines = vec![Line::new(1, 0, vec![Span::plain("日本語text")])];
        let m = find_matches(&lines, "text", &calc)[0];
        assert_eq!(m.start, 6);
        assert_eq!(m.end, 10);
    }

    #[test]
    fn escape_from_search_leaves_the_document_alone() {
        let mut a = long_doc();
        let before = a.scroll;
        press(&mut a, '/');
        press(&mut a, 'x');
        let mut sink = Vec::new();
        a.on_key(&mut sink, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(a.mode, Mode::Normal);
        assert!(a.matches.is_empty());
        assert_eq!(a.scroll, before);
    }

    #[test]
    fn o_on_an_image_block_targets_the_image() {
        let mut a = app("![shot](shot.png)\n");
        a.cursor = Some(0);
        assert_eq!(a.target_under_cursor().as_deref(), Some("shot.png"));
    }

    #[test]
    fn o_on_a_paragraph_targets_its_first_link() {
        let mut a = app("see [docs](https://example.com) and [more](https://other.example)\n");
        a.cursor = Some(0);
        assert_eq!(
            a.target_under_cursor().as_deref(),
            Some("https://example.com")
        );
    }

    #[test]
    fn the_link_picker_labels_visible_links() {
        let mut a = app("[one](https://a.example) and [two](https://b.example)\n");
        press(&mut a, 'F');
        assert_eq!(a.mode, Mode::LinkPick);
        assert_eq!(a.picks.len(), 2);
        assert_eq!(a.picks[0].label, 'a');
    }

    #[test]
    fn two_links_on_one_row_are_labelled_at_their_own_columns() {
        // A badge row is this shape. Labelling both at the row's first column
        // stacks them, and only the one drawn last can be seen.
        let mut a = app("[one](https://a.example) and [two](https://b.example)\n");
        press(&mut a, 'F');
        let overlays = a.pick_overlays();
        assert_eq!(overlays.len(), 2);
        assert_eq!(overlays[0].line, overlays[1].line, "same row");
        assert_ne!(overlays[0].col, overlays[1].col, "different columns");
    }

    #[test]
    fn more_links_than_labels_is_said_out_loud() {
        let mut doc = String::new();
        for i in 0..PICK_KEYS.len() + 3 {
            doc.push_str(&format!("- [link {i}](https://example.com/{i})\n"));
        }
        let mut a = app(&doc);
        // Tall enough that every link is on screen; the picker only ever sees
        // what is drawn.
        a.resize(Screen::new(40, PICK_KEYS.len() as u16 + 8));
        press(&mut a, 'F');
        assert_eq!(a.picks.len(), PICK_KEYS.len());
        assert!(
            a.toast.as_ref().unwrap().0.contains("3 more link"),
            "the ones past the last label are accounted for"
        );
    }

    /// A document with `filler` body lines under each heading, long enough that
    /// there is somewhere to scroll to.
    fn sectioned(headings: &[&str], filler: usize) -> String {
        let mut out = String::new();
        for heading in headings {
            out.push_str(&format!("# {heading}\n\n"));
            for i in 0..filler {
                out.push_str(&format!("body {i}\n\n"));
            }
        }
        out
    }

    #[test]
    fn an_anchor_link_jumps_to_its_heading() {
        let src = sectioned(&["Top", "Terminal support"], 10);
        let wanted = src.lines().position(|l| l == "# Terminal support").unwrap() + 1;
        let mut a = app(&src);
        a.open("#terminal-support");
        assert_eq!(a.current_source_line(), wanted, "the heading's own line");
    }

    #[test]
    fn a_repeated_heading_resolves_the_way_github_numbers_it() {
        let src = sectioned(&["Notes", "Notes"], 10);
        let second = src
            .lines()
            .enumerate()
            .filter(|(_, l)| *l == "# Notes")
            .nth(1)
            .unwrap()
            .0
            + 1;
        let mut a = app(&src);
        a.open("#notes-1");
        assert_eq!(a.current_source_line(), second, "the second one");
        a.open("#notes");
        assert_eq!(a.current_source_line(), 1, "back to the first");
    }

    #[test]
    fn an_anchor_that_matches_no_heading_says_so() {
        let mut a = app("# Top\n\ntext\n");
        a.open("#nowhere");
        assert!(a.toast.as_ref().unwrap().0.contains("no heading"));
    }

    #[test]
    fn github_style_slugs() {
        assert_eq!(slugify("Terminal support"), "terminal-support");
        assert_eq!(
            slugify("What counts as Markdown here"),
            "what-counts-as-markdown-here"
        );
        // Punctuation goes, the words around it stay separated.
        assert_eq!(slugify("Over ssh, and tmux"), "over-ssh-and-tmux");
        assert_eq!(slugify("v0.9 — Mermaid"), "v09--mermaid");
        // Non-ASCII letters are kept, as GitHub keeps them.
        assert_eq!(slugify("見出し"), "見出し");
    }

    #[test]
    fn horizontal_scrolling_stops_at_the_widest_line() {
        let mut a = app("short\n");
        a.wrap = false;
        a.relayout();
        for _ in 0..50 {
            a.act(&mut Vec::new(), Action::ScrollRight).unwrap();
        }
        assert_eq!(
            a.hoffset,
            a.max_hoffset(),
            "no running off the end into empty screens"
        );
    }

    #[test]
    fn a_key_that_does_not_follow_y_is_still_itself() {
        // `y` alone is not a yank — `yy` is — so the key after it was meant as
        // itself and must not be swallowed.
        let mut a = app("# One\n\nline\n\n# Two\n\nline\n\n# Three\n\nline\n");
        let before = a.scroll;
        press(&mut a, 'y');
        press(&mut a, 'j');
        assert_eq!(a.scroll, before + 1, "j scrolled");
        assert!(a.pending.is_none());
    }

    #[test]
    fn the_link_picker_says_so_when_there_is_nothing_to_pick() {
        let mut a = app("no links here\n");
        press(&mut a, 'F');
        assert_eq!(a.mode, Mode::Normal);
        assert!(a.toast.as_ref().unwrap().0.contains("no links"));
    }

    #[test]
    fn toggling_help_swaps_the_document_and_comes_back() {
        let mut a = app("# Real\n");
        press(&mut a, 'H');
        assert!(a.help);
        assert!(a.lines.iter().any(|l| l.text().contains("Navigation")));
        press(&mut a, 'H');
        assert!(!a.help);
        assert!(a.lines.iter().any(|l| l.text().contains("Real")));
    }

    #[test]
    fn v_asks_for_an_editor_at_the_line_on_screen() {
        let body: String = (1..=40).map(|i| format!("line {i}\n\n")).collect();
        let mut a = app(&body);
        a.scroll = 10;
        press(&mut a, 'v');
        let (path, line) = a.edit_request.take().expect("an edit was requested");
        assert_eq!(path, PathBuf::from("test.md"));
        assert_eq!(line, a.current_source_line());
    }

    #[test]
    fn v_declines_when_there_is_no_file() {
        let mut a = App::new(
            "body\n".to_string(),
            None,
            Settings::default(),
            Theme::default(),
            Screen::new(40, 10),
            GraphicsInfo::disabled(),
        );
        press(&mut a, 'v');
        assert!(a.edit_request.is_none());
        assert_eq!(a.toast.as_ref().unwrap().0, "no file to edit");
    }

    #[test]
    fn v_declines_while_the_help_pane_is_up() {
        let mut a = app("# Real\n");
        press(&mut a, 'H');
        press(&mut a, 'v');
        assert!(a.edit_request.is_none());
        assert_eq!(a.toast.as_ref().unwrap().0, "nothing to edit here");
    }

    #[test]
    fn the_contents_pane_lists_every_heading_as_a_link() {
        let mut a = app("# One\n\ntext\n\n## Two\n\nmore\n\n# Three\n");
        press(&mut a, 'T');
        assert!(a.toc);
        let text: String = a.lines.iter().map(|l| l.text()).collect();
        assert!(text.contains("One") && text.contains("Two") && text.contains("Three"));
        assert_eq!(a.active_doc().links.len(), 3);
    }

    #[test]
    fn a_contents_entry_jumps_to_its_heading_and_closes_the_pane() {
        let body = format!("# Top\n\n{}\n## Bottom\n\ntail\n", "filler\n\n".repeat(40));
        let mut a = app(&body);
        press(&mut a, 'T');
        let url = a.active_doc().links[1].url.clone();
        assert!(url.starts_with("#line-"), "{url}");
        a.open(&url);
        assert!(!a.toc, "the pane closes once you pick something");
        assert!(a.current_source_line() > 40, "jumped to the heading");
    }

    #[test]
    fn a_document_with_no_headings_says_so() {
        let mut a = app("just a paragraph\n");
        press(&mut a, 'T');
        assert!(!a.toc);
        assert_eq!(a.toast.as_ref().unwrap().0, "no headings");
    }

    #[test]
    fn the_breadcrumb_names_the_innermost_enclosing_section() {
        let mut a = app("# Top\n\n## Middle\n\nbody\n\n### Leaf\n\nmore\n");
        a.scroll = a.lines.len() - 1;
        assert_eq!(a.breadcrumb(), "Top › Middle › Leaf");
    }

    #[test]
    fn the_breadcrumb_pops_back_out_of_a_finished_subsection() {
        let a = app("# One\n\n## Deep\n\nbody\n\n# Two\n\ntail\n");
        let mut a = a;
        a.scroll = a.lines.len() - 1;
        assert_eq!(a.breadcrumb(), "Two");
    }

    #[test]
    fn the_breadcrumb_is_empty_above_the_first_heading() {
        let a = app("intro text\n\n# Later\n");
        assert_eq!(a.breadcrumb(), "");
    }

    #[test]
    fn resizing_reflows_without_losing_the_position() {
        let mut a = long_doc();
        a.scroll = a.max_scroll() / 2;
        let before = a.current_source_line();
        a.resize(Screen::new(20, 20));
        assert!(
            a.current_source_line().abs_diff(before) <= 2,
            "drifted from {before} to {}",
            a.current_source_line()
        );
    }

    #[test]
    fn resizing_at_the_very_bottom_clamps_to_the_end() {
        let mut a = long_doc();
        press(&mut a, 'G');
        a.resize(Screen::new(20, 20));
        assert_eq!(a.scroll, a.max_scroll());
    }

    #[test]
    fn a_toast_expires() {
        let mut a = app("body\n");
        a.toast("hi");
        assert!(!a.toast_expired());
        assert_eq!(a.bottom_row().len(), 1);
    }

    #[test]
    fn an_empty_backward_search_prompt_says_where_the_help_is() {
        // `?` is the help key in most things that are not pagers, so the
        // prompt it opens here has to answer the question that was asked.
        let mut a = app("body\n");
        press(&mut a, '?');
        let text: String = a.bottom_row().iter().map(|s| s.text.as_str()).collect();
        assert_eq!(text, "?  help: H");
    }

    #[test]
    fn the_help_hint_goes_away_once_the_search_has_a_query() {
        let mut a = app("body\n");
        press(&mut a, '?');
        press(&mut a, 'b');
        let text: String = a.bottom_row().iter().map(|s| s.text.as_str()).collect();
        assert_eq!(text, "?b", "the hint is for an empty prompt only");
    }

    #[test]
    fn the_forward_search_prompt_carries_no_help_hint() {
        // Nobody presses `/` looking for help, and the prompt is not a place
        // to advertise from.
        let mut a = app("body\n");
        press(&mut a, '/');
        let text: String = a.bottom_row().iter().map(|s| s.text.as_str()).collect();
        assert_eq!(text, "/");
    }

    #[test]
    fn the_bottom_row_is_empty_when_nothing_needs_saying() {
        let a = app("body\n");
        assert!(a.bottom_row().is_empty(), "toasts cost no rows when idle");
    }

    #[test]
    fn the_status_line_reports_the_mode() {
        let mut a = app("body\n");
        a.settings.status = true;
        let text = a.status_text();
        assert!(text.contains("render") && text.contains("wrap"));
    }

    #[test]
    fn quitting_sets_the_flag() {
        let mut a = app("body\n");
        press(&mut a, 'q');
        assert!(a.quit);
    }

    #[test]
    fn the_browser_lists_markdown_files_as_links() {
        let dir = std::env::temp_dir().join("mdroll-browser-test");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.md"), "x").unwrap();
        std::fs::write(dir.join("b.txt"), "x").unwrap();
        let text = browser_markdown(&dir);
        assert!(text.contains("[a.md](a.md)"));
        assert!(!text.contains("b.txt"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
