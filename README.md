# mdroll

A terminal Markdown viewer with big headings and real GitHub Flavored Markdown
support.

`mdroll` renders Markdown in the terminal the way GitHub does — tables, task
lists, footnotes, alerts, and mermaid diagrams included — with a
horizontal-scroll mode for when you don't want reflow, and a source view you can
toggle into at any time. Headings are actually *big*, not just a different
color.

The name is `md` + *roll*, after 絵巻 (emaki), the horizontal picture scrolls you
read by unrolling sideways.

---

## Why

Existing terminal Markdown viewers are good, but three things kept coming up:

1. **Headings never look like headings.** Every viewer renders `# Title` in a
   color or a background bar. None of them make the text larger, so document
   structure is hard to scan.
2. **GitHub's Markdown is the Markdown people actually write.** Task lists,
   footnotes, `> [!NOTE]` alerts, and mermaid diagrams appear in every README,
   and most viewers render them as raw text or drop them entirely.
3. **You can't get the text back out.** Once content is rendered, copying the
   original Markdown means opening the file in an editor.

`mdroll` targets these directly. It is built for **WezTerm** first; other
terminals work, but features that depend on terminal capabilities degrade
gracefully rather than being the design center.

---

## Features

- **Full GitHub Flavored Markdown** — tables, task lists, strikethrough,
  autolinks, footnotes, and `> [!NOTE]` alerts.
- **Mermaid diagrams** — `flowchart` and `sequenceDiagram` drawn with box
  characters, laid out by rank. Anything unsupported falls back to source.
- **Two rendering modes** — rendered view, and raw source view showing `#`,
  `**`, and friends as written.
- **Two layout modes** — reflow to terminal width, or no-wrap with horizontal
  scrolling.
- **Double-height headings** using DECDHL, so `# Title` renders at twice the
  size on terminals that support it.
- **Correct CJK line breaking** via UAX #14, with kinsoku rules applied.
- **Block yank** — move a cursor over blocks and copy either the original
  Markdown source or the rendered plain text. Works over SSH via OSC 52.
- **Clickable links** via OSC 8 hyperlinks, with no mouse capture required, so
  your terminal's native text selection keeps working.
- **Bundled themes** — Dracula, Solarized Dark/Light, Nord, Gruvbox, selectable
  from the command line.
- **Single static binary**, installable with `mise`.

---

## Installation

### mise

```toml
# mise.toml
[tools]
"github:tokuhirom/mdroll" = "latest"
```

```console
$ mise install
```

### cargo

```console
$ cargo install --locked mdroll
```

### From source

```console
$ git clone https://github.com/tokuhirom/mdroll
$ cd mdroll
$ cargo build --release
```

Prebuilt binaries are attached to each
[release](https://github.com/tokuhirom/mdroll/releases): macOS on Intel and
Apple silicon, Linux on x86-64 and arm64 (statically linked against musl, so
there is no glibc version to match), and Windows on x86-64. `mise` picks the
right one for your machine automatically.

---

## Usage

```console
$ mdroll README.md
$ cat README.md | mdroll
$ mdroll                      # browse *.md from the current directory
$ mdroll README.md | less -R  # piped output renders once and exits
```

When stdout is not a terminal, `mdroll` renders the whole document to stdout and
exits instead of trying to page it — the same thing a pager does when it is not
on a terminal.

### Options

| Option | Description |
| --- | --- |
| `--theme <NAME>` | Color theme. Default: `terminal` (uses your terminal's own colors). |
| `--list-themes` | Print available theme names and exit. |
| `--wrap` / `--no-wrap` | Start in reflow or horizontal-scroll mode. |
| `--source` | Start in source view instead of rendered view. |
| `--width <N>` | Cap the reflow width. `0` means full terminal width. |
| `--status` / `--no-status` | Show a persistent status line instead of transient toasts. |
| `--mouse` | Enable mouse capture (needed for image click actions). Off by default. |
| `--no-images` | Disable inline image rendering. |
| `--no-color` | Plain output, no ANSI styling. |
| `-z`, `--no-big-headings` | Never use DECDHL double-height headings. |
| `--ambiguous-wide` | Treat East Asian Ambiguous characters as two columns. |
| `--config <PATH>` | Use an alternate config file. |

Environment variables: `MDROLL_THEME`, `MDROLL_CONFIG`, `NO_COLOR`.

Precedence is **command line → environment → config file → built-in default**.

---

## Modes

`mdroll` has two independent, orthogonal mode axes:

|  | **Wrap** | **No-wrap** |
| --- | --- | --- |
| **Render** | Reflowed, styled output. The default. | Styled output, horizontal scrolling. Good for wide tables. |
| **Source** | Raw Markdown, reflowed. | Raw Markdown, one logical line per row. Best for copying. |

Toggling into source view automatically switches to no-wrap, and restores your
previous wrap setting on the way back. This matters: dragging a selection across
reflowed text inserts hard line breaks into whatever you copy. In no-wrap source
view, one logical line stays on one row, so your terminal's native selection
gives you the text exactly as written.

---

## Key bindings

### Navigation

| Key | Action |
| --- | --- |
| `j` / `↓` | Scroll down one line |
| `k` / `↑` | Scroll up one line |
| `d` | Half page down |
| `u` | Half page up |
| `f` / `Space` / `PgDn` | Page down |
| `b` / `PgUp` | Page up |
| `g` | Jump to top |
| `G` | Jump to bottom |
| `h` / `←` | Scroll left (no-wrap mode) |
| `l` / `→` | Scroll right (no-wrap mode) |
| `0` | Reset horizontal scroll |

### Modes

| Key | Action |
| --- | --- |
| `w` | Toggle wrap / no-wrap |
| `s` | Toggle rendered / source view |
| `t` | Cycle theme |
| `z` | Toggle double-height headings |
| `i` | Toggle inline images |

### Copying

| Key | Action |
| --- | --- |
| `Tab` | Move block cursor forward |
| `Shift-Tab` | Move block cursor backward |
| `y` | Yank block as Markdown source |
| `Y` | Yank block as rendered plain text |
| `yc` | Yank code block contents only, without the fences |
| `V` | Line selection mode — extend with `j`/`k`, confirm with `y` |
| `yp` | Yank the file path |

### Links and search

| Key | Action |
| --- | --- |
| `F` | Link picker — label every link, jump by keystroke |
| `o` | Open the link under the block cursor |
| `/` | Search forward |
| `?` | Search backward |
| `n` / `N` | Next / previous match |

### Other

| Key | Action |
| --- | --- |
| `r` | Reload the file |
| `H` | Show help |
| `q` / `Esc` | Quit |

Links are also emitted as OSC 8 hyperlinks, so `Cmd`-click (macOS) or
`Ctrl`-click opens them directly through the terminal without `mdroll` capturing
the mouse.

---

## Terminal support

WezTerm is the reference. Everything works everywhere; the features below are
the ones that depend on what the terminal can actually do.

| Feature | Works on | Elsewhere |
| --- | --- | --- |
| Inline images | WezTerm, kitty, ghostty | Alt text, dimmed |
| Double-height headings | WezTerm, xterm, foot | Colour and weight only |
| Clickable links (OSC 8) | Most modern terminals | Use `F` or `o` instead |
| Clipboard over SSH (OSC 52) | Most modern terminals | Enable it in your terminal |
| Truecolor | `COLORTERM=truecolor` | Nearest 256-color match |

### Multiplexers

`tmux` rewrites both graphics escapes and line attributes, so `mdroll` turns
inline images and double-height headings off when `TMUX` is set rather than
emitting sequences that would arrive mangled.

[herdr](https://github.com/ogulcancelik/herdr) passes the Kitty graphics
protocol through, but only when you opt in. Add this to your herdr config, or
images will fall back to alt text:

```toml
[experimental]
kitty_graphics = true
```

### Cell geometry

Sizing an image in rows needs to know how many pixels a character cell is.
`mdroll` asks the terminal, and falls back to 8×16 if it will not say — which
distorts the aspect ratio slightly but never breaks the layout.

---

## Design

### Pipeline

```
source text
   │
   ▼  comrak (CommonMark + GFM)
  AST ── sourcepos ──┐
   │                 │
   ▼                 │
Vec<Block>  ◄────────┘   intermediate representation
   │
   ▼  layout(&[Block], Viewport, Mode) -> Vec<Line>     ← pure function
Vec<Line>
   │
   ▼  crossterm
 screen
```

### Intermediate representation

```rust
struct Span {
    text: String,
    style: Style,
    link: Option<LinkId>,
}

struct Block {
    source_range: Range<usize>,   // line range in the original file
    kind: BlockKind,              // Heading(u8) | Para | Code | Quote | List | Table | Image
    spans: Vec<Span>,
}

struct Line {
    source_line: usize,
    scale: Scale,                 // Normal | DoubleHeight
    spans: Vec<Span>,
    hits: Vec<Hit>,
}

struct Hit {
    rect: Rect,
    target: HitTarget,            // Link(LinkId) | Image(ImageId)
}
```

Two things carry the whole design:

**`source_range` on every block.** comrak attaches `sourcepos` to each AST node,
giving the start and end position in the original file. Propagating that range
means yanking a block is a slice of the original source, not a reconstruction
from the rendered form. It also lets mode switches preserve your reading
position — the current screen row maps to a source line, and the source line
maps back into whatever the new layout produced. Skipping this early is
expensive to retrofit.

**`layout()` is a pure function.** Wrap/no-wrap toggling, source/render
toggling, and terminal resize are all handled by discarding the layout and
recomputing it. No incremental state, no invalidation logic, no drift.

### Text measurement

Display width comes from `unicode-width`. The East Asian Ambiguous class is
configurable, because whether `─` or `→` occupies one column or two depends on
the terminal's own setting. For WezTerm, match it to your config:

```lua
-- ~/.wezterm.lua
treat_east_asian_ambiguous_width_as_wide = true
```

Line breaking uses `unicode-linebreak` (UAX #14) plus kinsoku adjustments: no
line may begin with `。、）」』ー` and none may end with `「（『`. Text without
spaces has to break somewhere, and these rules are what keep the result from
looking wrong.

### Horizontal scrolling

The horizontal offset is stored in **display columns**, never in bytes or
`char` counts. Slicing a line walks it accumulating width; when a full-width
character straddles the boundary, it is replaced with a single space. Getting
this rule wrong produces a one-column drift that compounds across the document.

### Screen layout and the status line

The terminal height is decremented in exactly one place, and the result has its
own type so it cannot be confused with the full screen:

```rust
struct Screen { rows: u16, cols: u16 }

impl Screen {
    fn viewport(&self) -> Viewport {
        Viewport { rows: self.rows.saturating_sub(1), cols: self.cols }
    }
    fn status_row(&self) -> u16 { self.rows.saturating_sub(1) }
}
```

`layout()` and all scroll arithmetic take a `Viewport`. Only the top-level draw
function sees a `Screen`.

The status line is drawn **last, at an absolute position**, after the content:

```
clear → draw content → MoveTo(0, status_row) → draw status
```

Because it overwrites whatever is there, a content region that miscounts by a
row cannot hide it. Autowrap (DECAWM) is disabled while the status line is
written, so writing into the bottom-right cell cannot trigger a scroll.

Rendering is a full redraw every frame. A viewer updates rarely, and differential
updates are the usual source of "the status line vanishes sometimes" bugs.

By default, mode changes surface as a **transient toast** on the bottom row for
about 1.5 seconds rather than a permanent status line — it costs no rows at all.
`--status` switches to a persistent line.

### Headings

On terminals supporting DECDHL, `# Heading` is emitted as double-height text:

```
\e#3Heading      ← top half
\e#4Heading      ← bottom half
```

This consumes two physical rows for one logical line, and halves the usable
column count for that row, so the layout pass must account for both.

Support is narrower than it looks. **kitty does not implement DECDHL** — it
reports `ESC # 3` as a parse error and then draws the line a second time, so a
heading appears twice. Because a terminal that ignores the sequence produces
visibly broken output rather than a graceful no-op, detection is an allowlist
rather than a denylist: WezTerm, xterm, and foot get double-height headings, and
everything else gets colour and weight. `-z` forces them off anywhere.

An alternative rasterized path (rendering heading text to a bitmap at an
arbitrary point size and placing it with the Kitty graphics protocol) is planned
for terminals where DECDHL is unavailable but graphics are not.

### Clipboard

Local sessions use `arboard`. When `SSH_CONNECTION` is set, or when `arboard`
fails, `mdroll` falls back to OSC 52, which carries text and therefore works for
every yank operation over SSH. Image data cannot be transported this way; on
remote sessions an image yank copies the path instead.

### Themes

Themes are TOML, embedded at build time with `include_str!`. Additional themes
are loaded from `~/.config/mdroll/themes/*.toml`.

```toml
name = "dracula"

[code]
syntect_theme = "Dracula"

[heading]
h1 = { fg = "#bd93f9", bold = true }
h2 = { fg = "#8be9fd", bold = true }
h3 = { fg = "#50fa7b" }

[inline]
link = { fg = "#8be9fd", underline = true }
code = { fg = "#ff79c6", bg = "#44475a" }
```

Two color systems are in play: the UI palette above, and syntect's `.tmTheme`
files for code block highlighting. syntect ships Solarized and the base16 family
but **not** Dracula, so `Dracula.tmTheme` is vendored and referenced by name.

The default theme is `terminal`, which sets no background color and inherits
your terminal's palette. This avoids fighting WezTerm's transparency and
background image settings. Named themes paint backgrounds only when explicitly
selected.

Truecolor is assumed; 256-color terminals get a nearest-color downgrade.

### Configuration

```toml
# ~/.config/mdroll/config.toml

theme = "dracula"
default_mode = "render"        # "render" | "source"
default_wrap = true
width = 100                    # 0 = full terminal width
status = false                 # false = toast, true = persistent line
double_height_headings = true
images = true
mouse = false
east_asian_ambiguous_wide = true

[keys]
# override any binding
quit = ["q", "Esc"]
toggle_wrap = ["w"]
```

---

## Roadmap

### v0.1 — Walking skeleton

- [x] comrak parsing into `Vec<Block>` with `source_range`
- [x] Pure `layout()` with reflow
- [x] Vertical scrolling, `Screen`/`Viewport` split
- [x] Absolute-position bottom row, DECAWM handling
- [x] Toast on mode change
- [x] Headings, paragraphs, lists, blockquotes, code blocks (no highlighting)

### v0.2 — Modes

- [x] Source view toggle
- [x] No-wrap mode with horizontal scrolling
- [x] Auto no-wrap when entering source view, restore on exit
- [x] Full-width character handling at slice boundaries

### v0.3 — GitHub Flavored Markdown

- [x] Tables, with column widths measured in display columns
- [x] Task lists and strikethrough
- [x] Autolinks
- [x] Footnotes, with a references section at the end
- [x] `> [!NOTE]` / `[!TIP]` / `[!IMPORTANT]` / `[!WARNING]` / `[!CAUTION]` alerts

### v0.4 — Text quality

- [x] UAX #14 line breaking
- [x] Kinsoku rules
- [x] Configurable East Asian Ambiguous width

### v0.5 — Links

- [x] OSC 8 hyperlink emission
- [x] Link picker (`F`)
- [x] Open under cursor (`o`)

### v0.6 — Copying

- [x] Block cursor (`Tab` / `Shift-Tab`)
- [x] `y` / `Y` / `yc` / `yp`
- [x] Line selection mode (`V`)
- [x] `arboard` with OSC 52 fallback

### v0.7 — Presentation

- [x] Theme loading, bundled themes, `--theme` and `--list-themes`
- [x] syntect code block highlighting
- [x] Config file
- [x] Persistent status line as an option

### v0.8 — Big headings

- [x] DECDHL double-height headings
- [x] Capability detection and graceful fallback
- [x] Layout accounting for halved column count and doubled row cost

### v0.9 — Mermaid

- [x] `flowchart` / `graph` rendered with box drawings, laid out by rank
- [x] `sequenceDiagram` rendered with lifelines and arrows
- [x] Fall back to a highlighted code block for unsupported diagram types
- [ ] Image rendering through `mmdc` where the terminal has graphics

### v0.10 — Finding things

- [x] Incremental search with `/`, `?`, `n`, `N`
- [ ] Section breadcrumb in the header region
- [ ] Table of contents pane

### v0.11 — Images and files

- [x] Inline images via the Kitty graphics protocol
- [x] Optional mouse capture (`--mouse`) with rectangle hit-testing
- [x] File browser when invoked with no arguments
- [x] `r` to reload
- [ ] `--watch` for live reload

### v1.0

- [ ] Key remapping through config
- [ ] Rasterized heading fallback for non-DECDHL terminals
- [ ] Release automation via `dist`, binaries for macOS/Linux/Windows
- [ ] Documentation and man page

### Beyond

- Math via `$...$` and `$$...$$`
- Definition lists
- Use as a library, for embedding in other TUIs

---

## Non-goals

- **Editing.** `mdroll` is a viewer. Use your editor.
- **HTML or PDF export.** Use pandoc.
- **A browser.** Link following opens your system handler; it does not render
  remote pages.
- **Universal terminal support.** WezTerm is the reference. Features degrade
  elsewhere rather than being designed down to the lowest common denominator.

---

## Prior art

`mdroll` owes ideas to [glow](https://github.com/charmbracelet/glow),
[mdcat](https://github.com/swsnr/mdcat), and
[md-tui](https://github.com/henriklovhaug/md-tui). Each solves a different part
of this problem well; none of them combines large headings, mermaid rendering,
and in-viewer copying, which is the gap this fills.

---

## Contributing

Issues and pull requests are welcome. For anything involving text measurement or
line breaking, please include a test case with the actual string — width bugs
are almost impossible to reason about from a description alone.

### Development

```console
$ cargo test
$ cargo clippy --all-targets -- -D warnings
$ cargo fmt --all
```

Git hooks are managed with [lefthook](https://lefthook.dev). `pre-commit` runs
the formatter check and clippy; `pre-push` runs the tests:

```console
$ mise use -g lefthook   # or: brew install lefthook
$ lefthook install
```

`tools/screenshot.sh` renders a document in a real terminal on a headless X
server and saves a PNG, which is how the screenshot above is produced:

```console
$ cargo build --release
$ tools/screenshot.sh tests/fixtures/kitchen-sink.md shot.png --theme dracula
```

Note that it drives kitty, which does not implement DECDHL, so double-height
headings never appear in screenshots taken this way.

---

## License

MIT License

Copyright (c) 2026 Tokuhiro Matsuno

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the "Software"), to deal in
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software is furnished to do so,
subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS
FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR
COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER
IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN
CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

Bundled theme definitions are derived from Dracula, Solarized, Nord, and
Gruvbox, each MIT-licensed. See `THIRDPARTY.md` for full attribution.
