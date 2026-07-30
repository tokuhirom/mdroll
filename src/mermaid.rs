//! Mermaid diagrams, drawn with box-drawing characters.
//!
//! Supports `flowchart` / `graph` and `sequenceDiagram`, which is what appears
//! in practically every README. Anything else — and anything using a construct
//! this does not model, such as `subgraph` — is declined, and the caller falls
//! back to rendering the fenced block as highlighted code. Declining is always
//! better than drawing something subtly wrong.
//!
//! Everything is composed onto a grid of display *columns*, not characters, so
//! a CJK node label reserves the two columns it actually occupies.

use crate::width::WidthCalc;

/// Columns between adjacent boxes on the same rank.
const GAP: usize = 3;
/// Rows in the connector band between two ranks.
const BAND: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    TopDown,
    LeftRight,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shape {
    Rect,
    Round,
    Diamond,
    Stadium,
}

#[derive(Debug, Clone)]
pub struct Node {
    pub id: String,
    pub label: String,
    pub shape: Shape,
}

#[derive(Debug, Clone)]
pub struct Edge {
    pub from: usize,
    pub to: usize,
    pub label: Option<String>,
    pub dotted: bool,
}

#[derive(Debug, Clone)]
pub struct Flowchart {
    pub direction: Direction,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    /// `RL` and `BT`, whose edges were turned round to lay the chart out. The
    /// arrowheads have to be put back on the end the document asked for.
    pub reversed: bool,
}

/// Render a mermaid diagram, or return `None` if it uses anything unsupported.
pub fn render(code: &str, calc: &WidthCalc) -> Option<Vec<String>> {
    let kind = code
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("%%"))?;

    if kind.starts_with("sequenceDiagram") {
        return sequence::render(code, calc);
    }
    if kind.starts_with("flowchart") || kind.starts_with("graph") {
        let chart = parse_flowchart(code)?;
        return draw_flowchart(&chart, calc);
    }
    None
}

// ---------------------------------------------------------------------------
// A column grid
// ---------------------------------------------------------------------------

/// The directions a line leaves a cell in, as a set of bits.
const UP: u8 = 1;
const DOWN: u8 = 2;
const LEFT: u8 = 4;
const RIGHT: u8 = 8;

/// Every box-drawing character the diagrams use, and what it connects.
const JUNCTIONS: &[(char, u8)] = &[
    ('│', UP | DOWN),
    ('─', LEFT | RIGHT),
    ('┌', DOWN | RIGHT),
    ('┐', DOWN | LEFT),
    ('└', UP | RIGHT),
    ('┘', UP | LEFT),
    ('├', UP | DOWN | RIGHT),
    ('┤', UP | DOWN | LEFT),
    ('┬', DOWN | LEFT | RIGHT),
    ('┴', UP | LEFT | RIGHT),
    ('┼', UP | DOWN | LEFT | RIGHT),
];

/// The character for a set of directions. A line going only one way is drawn as
/// though it went both, since a stub end is not a character anyone has.
fn junction(dirs: u8) -> char {
    match JUNCTIONS.iter().find(|(_, d)| *d == dirs) {
        Some((c, _)) => *c,
        None if dirs & (UP | DOWN) != 0 => '│',
        None => '─',
    }
}

/// A canvas addressed in display columns.
///
/// A double-width character occupies its own cell plus a following
/// [`Cell::Skip`], so column arithmetic stays honest for CJK labels and for
/// box-drawing characters on terminals that treat East Asian Ambiguous as wide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cell {
    Blank,
    Skip,
    Char(char),
}

struct Grid {
    cells: Vec<Vec<Cell>>,
    width: usize,
}

impl Grid {
    fn new(width: usize, height: usize) -> Grid {
        Grid {
            cells: vec![vec![Cell::Blank; width]; height],
            width,
        }
    }

    fn put(&mut self, x: usize, y: usize, c: char, calc: &WidthCalc) {
        let w = calc.ch(c).max(1);
        if y >= self.cells.len() || x + w > self.width {
            return;
        }
        self.cells[y][x] = Cell::Char(c);
        for i in 1..w {
            self.cells[y][x + i] = Cell::Skip;
        }
    }

    fn text(&mut self, x: usize, y: usize, s: &str, calc: &WidthCalc) {
        let mut col = x;
        for c in s.chars() {
            self.put(col, y, c, calc);
            col += calc.ch(c).max(1);
        }
    }

    /// Draw a character only where the cell is still blank, so a crossing line
    /// never punches a hole through a box.
    fn put_soft(&mut self, x: usize, y: usize, c: char, calc: &WidthCalc) {
        if y < self.cells.len() && x < self.width && self.cells[y][x] == Cell::Blank {
            self.put(x, y, c, calc);
        }
    }

    fn hline(&mut self, x0: usize, x1: usize, y: usize, calc: &WidthCalc) {
        for x in x0.min(x1)..=x0.max(x1) {
            self.put_soft(x, y, '─', calc);
        }
    }

    fn vline(&mut self, x: usize, y0: usize, y1: usize, calc: &WidthCalc) {
        for y in y0.min(y1)..=y0.max(y1) {
            self.put_soft(x, y, '│', calc);
        }
    }

    /// Draw the box-drawing character that continues in `dirs`, keeping the
    /// directions whatever is already there continues in.
    ///
    /// A junction is drawn by each of the lines meeting at it, and each knows
    /// only its own half: two parents whose buses arrive over one child both
    /// see a corner, one turning down from the left and one from the right,
    /// where together they make a `┬`. Whichever drew second used to win.
    fn join(&mut self, x: usize, y: usize, dirs: u8, calc: &WidthCalc) {
        if y >= self.cells.len() || x >= self.width {
            return;
        }
        let here = match self.cells[y][x] {
            Cell::Blank => 0,
            // A label or an arrowhead is not a line and has nothing to say
            // about where one goes; leave it be, as a plain line does.
            Cell::Char(c) => match JUNCTIONS.iter().find(|(g, _)| *g == c) {
                Some((_, dirs)) => *dirs,
                None => return,
            },
            Cell::Skip => return,
        };
        self.put(x, y, junction(dirs | here), calc);
    }

    /// Draw a line through a cell another line already passes through, without
    /// saying the two meet.
    ///
    /// Two lanes are two different edges, and joining them into a `┼` says they
    /// touch. Box drawing has no character for one line passing over another, so
    /// the vertical is drawn and the horizontal is left with a one-cell gap where
    /// it runs behind: a gap in a long run is read straight across, and the one
    /// thing it cannot be read as is a junction. Anything already there that is
    /// not a straight line is a corner where two edges leaving one box really do
    /// share the cell, and joins as before.
    fn cross(&mut self, x: usize, y: usize, dirs: u8, calc: &WidthCalc) {
        let vertical = dirs & (UP | DOWN) != 0;
        match self.cells.get(y).and_then(|row| row.get(x)) {
            Some(Cell::Char('│')) if !vertical => {}
            Some(Cell::Char('─')) if vertical => self.put(x, y, '│', calc),
            _ => self.join(x, y, dirs, calc),
        }
    }

    fn rows(self) -> Vec<String> {
        self.cells
            .into_iter()
            .map(|row| {
                let mut s = String::new();
                for cell in row {
                    match cell {
                        Cell::Char(c) => s.push(c),
                        Cell::Blank => s.push(' '),
                        Cell::Skip => {}
                    }
                }
                s.trim_end().to_string()
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Flowchart parsing
// ---------------------------------------------------------------------------

/// Edge operators, longest first so `-.->` is not read as `-.-`.
const OPERATORS: &[(&str, bool)] = &[
    ("-.->", true),
    ("-.-", true),
    ("==>", false),
    ("===", false),
    ("-->", false),
    ("---", false),
    ("--x", false),
    ("--o", false),
];

/// Split a line into statements.
///
/// A statement ends at a `;`, which mermaid takes as a separator and which its
/// own documentation writes at the end of every line, or at a `%%`, which
/// comments out the rest of the line wherever it appears.
///
/// Neither counts inside a label — `A[do this; then that]` — or inside quotes,
/// so the split only happens outside brackets and outside quotes.
fn split_statements(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut start, mut depth, mut quoted) = (0usize, 0i32, false);
    for (at, c) in line.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '[' | '(' | '{' if !quoted => depth += 1,
            ']' | ')' | '}' if !quoted => depth -= 1,
            ';' if !quoted && depth <= 0 => {
                out.push(line[start..at].trim());
                start = at + 1;
            }
            '%' if !quoted && depth <= 0 && line[at..].starts_with("%%") => {
                out.push(line[start..at].trim());
                start = line.len();
                break;
            }
            _ => {}
        }
    }
    out.push(line[start..].trim());
    out.retain(|s| !s.is_empty());
    out
}

/// Whether a statement opens with `word` as a word, rather than merely
/// beginning with those letters.
///
/// `endpoint[X] --> B` is a node called `endpoint`, and reading it as the `end`
/// of a subgraph declined a chart there was nothing wrong with.
fn keyword(line: &str, word: &str) -> bool {
    line.strip_prefix(word)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
}

pub fn parse_flowchart(code: &str) -> Option<Flowchart> {
    let mut lines = code.lines().map(str::trim).flat_map(split_statements);

    let header = lines.next()?;
    let direction = match header.split_whitespace().nth(1).unwrap_or("TD") {
        "LR" | "RL" => Direction::LeftRight,
        _ => Direction::TopDown,
    };
    let reversed = matches!(
        header.split_whitespace().nth(1).unwrap_or("TD"),
        "RL" | "BT"
    );

    let mut chart = Flowchart {
        direction,
        nodes: Vec::new(),
        edges: Vec::new(),
        reversed,
    };

    for line in lines {
        // Anything that changes the *shape* of the diagram, rather than its
        // styling, is a reason to decline the whole thing.
        if keyword(line, "subgraph") || keyword(line, "end") {
            return None;
        }
        if ["classDef", "class", "style", "click", "linkStyle"]
            .iter()
            .any(|word| keyword(line, word))
        {
            continue;
        }
        parse_statement(line, &mut chart)?;
    }

    if chart.nodes.is_empty() {
        return None;
    }
    // Turning the edges round is what puts the ranks where mermaid puts them:
    // a `BT` chart's source at the bottom, an `RL` chart's at the right. It
    // says nothing about which way the arrows point, which is why the drawing
    // is told about it rather than left to infer it from the edges.
    if reversed {
        for edge in &mut chart.edges {
            std::mem::swap(&mut edge.from, &mut edge.to);
        }
    }
    Some(chart)
}

/// One statement: a chain of node specs joined by edge operators.
fn parse_statement(line: &str, chart: &mut Flowchart) -> Option<()> {
    let mut parts: Vec<(String, Option<String>, bool)> = Vec::new();
    let mut rest = line;
    let mut pending_label: Option<String> = None;
    let mut pending_dotted = false;
    let mut first = true;

    loop {
        match find_operator(rest) {
            Some((start, len, dotted)) => {
                let (spec, after) = rest.split_at(start);
                // `A -- label --> B` writes the label before the arrow.
                let (spec, inline_label) = split_trailing_label(spec);
                parts.push((
                    spec.trim().to_string(),
                    pending_label.take(),
                    pending_dotted,
                ));
                let after = &after[len..];
                // `A -->|label| B` writes it after.
                let (label, after) = leading_pipe_label(after);
                pending_label = label.or(inline_label);
                pending_dotted = dotted;
                rest = after;
                first = false;
            }
            None => {
                parts.push((
                    rest.trim().to_string(),
                    pending_label.take(),
                    pending_dotted,
                ));
                break;
            }
        }
    }

    if first && parts.len() == 1 {
        // A bare node declaration such as `A[Label]`.
        let (spec, _) = &(parts[0].0.clone(), ());
        if spec.is_empty() {
            return Some(());
        }
        intern(chart, spec)?;
        return Some(());
    }

    let mut previous: Option<usize> = None;
    for (spec, label, dotted) in parts {
        if spec.is_empty() {
            return None;
        }
        let idx = intern(chart, &spec)?;
        if let Some(from) = previous {
            chart.edges.push(Edge {
                from,
                to: idx,
                label,
                dotted,
            });
        }
        previous = Some(idx);
    }
    Some(())
}

fn find_operator(s: &str) -> Option<(usize, usize, bool)> {
    // Walk char boundaries, not bytes: a node label may contain any script.
    for (i, _) in s.char_indices() {
        for (op, dotted) in OPERATORS {
            if s[i..].starts_with(op) {
                return Some((i, op.len(), *dotted));
            }
        }
    }
    None
}

/// Strip a `-- label` suffix from a node spec, as in `A -- yes --> B`.
fn split_trailing_label(spec: &str) -> (&str, Option<String>) {
    let trimmed = spec.trim_end();
    if let Some(at) = trimmed.rfind("--")
        && at > 0
    {
        let label = trimmed[at + 2..].trim();
        if !label.is_empty() && !label.contains(['[', ']', '(', ')', '{', '}']) {
            return (&trimmed[..at], Some(label.to_string()));
        }
    }
    (spec, None)
}

fn leading_pipe_label(s: &str) -> (Option<String>, &str) {
    let trimmed = s.trim_start();
    if let Some(body) = trimmed.strip_prefix('|')
        && let Some(end) = body.find('|')
    {
        return (Some(body[..end].trim().to_string()), &body[end + 1..]);
    }
    (None, s)
}

/// Register a node spec, returning its index. Repeating an id later without a
/// label reuses the earlier one, which is how mermaid behaves.
fn intern(chart: &mut Flowchart, spec: &str) -> Option<usize> {
    let (id, label, shape) = parse_node(spec)?;
    if let Some(existing) = chart.nodes.iter().position(|n| n.id == id) {
        if let Some(label) = label {
            chart.nodes[existing].label = label;
            chart.nodes[existing].shape = shape;
        }
        return Some(existing);
    }
    chart.nodes.push(Node {
        label: label.unwrap_or_else(|| id.clone()),
        id,
        shape,
    });
    Some(chart.nodes.len() - 1)
}

fn parse_node(spec: &str) -> Option<(String, Option<String>, Shape)> {
    let spec = spec.trim();
    if spec.is_empty() {
        return None;
    }
    let open = spec.find(['[', '(', '{']);
    let Some(open) = open else {
        return Some((spec.to_string(), None, Shape::Rect));
    };
    let id = spec[..open].trim().to_string();
    if id.is_empty() {
        return None;
    }
    // Longest delimiters first, so `([stadium])` is not read as a round node.
    const SHAPES: &[(&str, &str, Shape)] = &[
        ("([", "])", Shape::Stadium),
        ("[", "]", Shape::Rect),
        ("(", ")", Shape::Round),
        ("{", "}", Shape::Diamond),
    ];
    let body = &spec[open..];
    let (prefix, suffix, shape) = SHAPES.iter().find(|(p, _, _)| body.starts_with(p))?;
    let inner = body.strip_prefix(prefix)?.strip_suffix(suffix)?;
    Some((id, Some(unquote(inner)), shape.clone()))
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    s.strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(s)
        .trim()
        .to_string()
}

// ---------------------------------------------------------------------------
// Flowchart drawing
// ---------------------------------------------------------------------------

/// Assign each node to a rank: one past the deepest of its parents.
///
/// Returns `None` for a cyclic graph, which this renderer does not model —
/// including `A --> A`, the smallest cycle there is. A self-edge was excepted
/// from the count here and then had nowhere to be drawn, so it disappeared.
fn ranks(chart: &Flowchart) -> Option<Vec<usize>> {
    let n = chart.nodes.len();
    let mut indegree = vec![0usize; n];
    for edge in &chart.edges {
        indegree[edge.to] += 1;
    }
    let mut rank = vec![0usize; n];
    let mut queue: Vec<usize> = (0..n).filter(|i| indegree[*i] == 0).collect();
    let mut seen = 0usize;

    while let Some(node) = queue.pop() {
        seen += 1;
        for edge in chart.edges.iter().filter(|e| e.from == node) {
            rank[edge.to] = rank[edge.to].max(rank[node] + 1);
            indegree[edge.to] -= 1;
            if indegree[edge.to] == 0 {
                queue.push(edge.to);
            }
        }
    }
    (seen == n).then_some(rank)
}

struct Box {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
}

impl Box {
    fn center_x(&self) -> usize {
        self.x + self.w / 2
    }
    fn bottom(&self) -> usize {
        self.y + self.h - 1
    }
    fn right(&self) -> usize {
        self.x + self.w - 1
    }
    fn center_y(&self) -> usize {
        self.y + self.h / 2
    }
}

pub fn draw_flowchart(chart: &Flowchart, calc: &WidthCalc) -> Option<Vec<String>> {
    let rank = ranks(chart)?;
    let depth = rank.iter().copied().max().unwrap_or(0) + 1;
    let mut by_rank: Vec<Vec<usize>> = vec![Vec::new(); depth];
    for (i, r) in rank.iter().enumerate() {
        by_rank[*r].push(i);
    }

    let label_width = |i: usize| calc.str(&chart.nodes[i].label);
    let box_w = |i: usize| label_width(i) + 4;

    // Where each box sits along the axis a band's runs travel, which is what
    // says whether two of those runs overlap.
    let across = |by_rank: &[Vec<usize>]| -> Vec<usize> {
        match chart.direction {
            Direction::TopDown => column_starts(by_rank, chart.nodes.len(), &box_w)
                .0
                .iter()
                .enumerate()
                .map(|(i, x)| x + box_w(i) / 2)
                .collect(),
            Direction::LeftRight => row_starts(by_rank, chart.nodes.len())
                .0
                .iter()
                .map(|y| y + 1)
                .collect(),
        }
    };

    // The document's own order is kept wherever the chart can be drawn in it,
    // so a diagram that came out one way yesterday comes out the same way
    // today. Reordering a rank is a repair for a band whose runs would say
    // more than the document does, not a policy applied to every chart.
    let mut diverted: Vec<usize> = Vec::new();
    let drawable = |order: &[Vec<usize>], out: &[usize]| {
        let centres = across(order);
        bus_runs_are_honest(chart, &rank, out, |i| centres[i])
            && roads_out_are_clear(chart, &rank, order, out)
    };
    if !drawable(&by_rank, &diverted) {
        let orders = untangled(chart, &rank, &by_rank);
        // A plain drawing beats a routed one, so every order is tried on its
        // own before any of them is allowed to send an edge round the outside.
        let found = orders
            .iter()
            .find(|order| drawable(order, &[]))
            .map(|order| (order.clone(), Vec::new()))
            .or_else(|| {
                orders.iter().find_map(|order| {
                    let centres = across(order);
                    let out = divert(chart, &rank, order, &centres)?;
                    drawable(order, &out).then(|| (order.clone(), out))
                })
            })?;
        (by_rank, diverted) = found;
    }

    match chart.direction {
        Direction::TopDown => draw_top_down(chart, &by_rank, &rank, &diverted, calc, box_w),
        Direction::LeftRight => draw_left_right(chart, &by_rank, &rank, &diverted, calc, box_w),
    }
}

/// Which edges to take out of their band and route round the outside.
///
/// A band that no order untangles has two parents whose children overlap
/// without being the same set, and the run they share offers the pairs neither
/// of them wrote. Nothing about the *bus* can be fixed — but an edge does not
/// have to be on it. Taken out to a lane of its own, the way an edge that skips
/// a rank already is, it leaves a band the remaining edges can say honestly and
/// carries its own label where nothing else can reach it.
///
/// The fewest edges that do it, and among those the ones that break up the
/// fewest fans: a parent with one edge in the band loses nothing by having it
/// routed round, and a parent with four would lose the shape that makes it
/// readable.
fn divert(
    chart: &Flowchart,
    rank: &[usize],
    by_rank: &[Vec<usize>],
    centres: &[usize],
) -> Option<Vec<usize>> {
    let clear = |i: usize| road_is_clear(chart, rank, by_rank, i);
    let mut band: Vec<usize> = (0..chart.edges.len())
        .filter(|i| {
            let e = &chart.edges[*i];
            rank[e.to] == rank[e.from] + 1
        })
        .collect();
    // Searching pairs is quadratic in a band's edges and the check inside is
    // quadratic again; a chart big enough for that to matter is past what
    // anyone reads in a terminal.
    if band.len() > 24 {
        return None;
    }
    let mut fan = vec![0usize; chart.nodes.len()];
    for &i in &band {
        fan[chart.edges[i].from] += 1;
    }
    band.sort_by_key(|i| fan[chart.edges[*i].from]);

    band.retain(|i| clear(*i));

    let honest = |out: &[usize]| bus_runs_are_honest(chart, rank, out, |i| centres[i]);
    for &i in &band {
        if honest(&[i]) {
            return Some(vec![i]);
        }
    }
    for (n, &i) in band.iter().enumerate() {
        for &j in &band[n + 1..] {
            if honest(&[i, j]) {
                return Some(vec![i, j]);
            }
        }
    }
    None
}

/// Which edges are routed round the outside rather than drawn on their band.
fn routed(chart: &Flowchart, rank: &[usize], diverted: &[usize]) -> Vec<usize> {
    (0..chart.edges.len())
        .filter(|i| {
            let e = &chart.edges[*i];
            rank[e.to] > rank[e.from] + 1 || diverted.contains(i)
        })
        .collect()
}

/// Whether a routed edge's two ends both have a road to the lane with no box of
/// their own rank standing in it.
///
/// The lane is out past the far end of every rank, and a run reaching it passes
/// behind whatever boxes stand between. Only an edge whose two ends are the last
/// boxes of their ranks has a road that is clear the whole way.
fn road_is_clear(chart: &Flowchart, rank: &[usize], by_rank: &[Vec<usize>], i: usize) -> bool {
    let e = &chart.edges[i];
    [e.from, e.to]
        .iter()
        .all(|&n| by_rank[rank[n]].last() == Some(&n))
}

/// The same, asked of every edge the drawing would route.
///
/// A connector is only drawn where the canvas is still blank, so behind a box a
/// run vanishes and comes out the far side reading as that box's edge —
/// `│ C │◀──│ D │─┘` for something that starts at `B`. `divert` has always asked
/// this of an edge it takes off a band. An edge that *skips* a rank was let
/// through unasked, on the grounds that the boxes it hides behind belong to
/// ranks it has nothing to do with; the boxes in its way belong to its own rank,
/// and `A --> C` under `B --> Z` drew a line from `Z` to `C` that nobody wrote.
fn roads_out_are_clear(
    chart: &Flowchart,
    rank: &[usize],
    by_rank: &[Vec<usize>],
    diverted: &[usize],
) -> bool {
    routed(chart, rank, diverted)
        .into_iter()
        .all(|i| road_is_clear(chart, rank, by_rank, i))
}

/// Where each box starts across the canvas, and how wide the widest rank is.
///
/// The widest rank sets the canvas width and every other rank is centred in it,
/// so a box's column depends on the order of its own rank and on the widths of
/// every rank. Worked out here rather than in the drawing because whether an
/// order can be drawn at all is decided before there is anything to draw with.
fn column_starts(
    by_rank: &[Vec<usize>],
    nodes: usize,
    box_w: &impl Fn(usize) -> usize,
) -> (Vec<usize>, usize) {
    let rank_widths: Vec<usize> = by_rank
        .iter()
        .map(|nodes| {
            nodes.iter().map(|i| box_w(*i)).sum::<usize>() + GAP * nodes.len().saturating_sub(1)
        })
        .collect();
    let content_width = rank_widths.iter().copied().max().unwrap_or(0);
    let mut starts = vec![0usize; nodes];
    for (r, ns) in by_rank.iter().enumerate() {
        let mut x = (content_width - rank_widths[r]) / 2;
        for &i in ns {
            starts[i] = x;
            x += box_w(i) + GAP;
        }
    }
    (starts, content_width)
}

/// The same for a sideways chart, where a rank is a column of boxes and what
/// varies is which row each one is on.
fn row_starts(by_rank: &[Vec<usize>], nodes: usize) -> (Vec<usize>, usize) {
    let rank_heights: Vec<usize> = by_rank
        .iter()
        .map(|ns| ns.len() * 3 + ns.len().saturating_sub(1))
        .collect();
    let content_height = rank_heights.iter().copied().max().unwrap_or(3);
    let mut starts = vec![0usize; nodes];
    for (r, ns) in by_rank.iter().enumerate() {
        let mut y = (content_height - rank_heights[r]) / 2;
        for &i in ns {
            starts[i] = y;
            y += 4;
        }
    }
    (starts, content_height)
}

/// Reorder the ranks until no band's runs interleave, or give up.
///
/// Two runs overlap when the boxes they join are threaded through each other,
/// and that is often nothing the graph asked for: the order within a rank is
/// the order the nodes were written in, and `C --> E`, `D --> E`, `A --> D`,
/// `B --> C` introduces `C` before `D` and then hangs them off parents in the
/// other order. Swapping the pair is all it takes, and this is the sweep that
/// finds it — each rank ordered by where its neighbours in the rank before it
/// sit, which is the standard way of untangling a layered drawing.
///
/// Whether it worked is not assumed. Some bands are tangled by the graph and
/// not by the order — several parents reaching an overlapping set of children
/// is one no sweep can separate — and there the chart is declined as before.
///
/// The other reason an order will not do is that an edge routed round the
/// outside has no clear road to its lane, so each order is offered again with
/// the ends of the routed edges moved to the end of their ranks, which is where
/// one is. The sweeps come first: an order that draws every edge on its band
/// beats one that had to move a box to get a run out.
fn untangled(chart: &Flowchart, rank: &[usize], by_rank: &[Vec<usize>]) -> Vec<Vec<Vec<usize>>> {
    // The order in the document comes first, so it is what a chart is drawn in
    // whenever it can be. Then down, then back up, then down again: one pass
    // settles the common case, and the return passes catch a rank whose own
    // order was what put the one after it wrong.
    let mut orders = vec![by_rank.to_vec()];
    let mut order = by_rank.to_vec();
    for pass in 0..3 {
        if pass % 2 == 0 {
            for r in 1..order.len() {
                sort_by_neighbours(chart, rank, &mut order, r, true);
            }
        } else {
            for r in (0..order.len().saturating_sub(1)).rev() {
                sort_by_neighbours(chart, rank, &mut order, r, false);
            }
        }
        if !orders.contains(&order) {
            orders.push(order.clone());
        }
    }
    for candidate in orders.clone() {
        let cleared = roads_cleared(chart, rank, &candidate);
        if !orders.contains(&cleared) {
            orders.push(cleared);
        }
    }
    orders
}

/// One order with the ends of every edge that skips a rank moved to the end of
/// their ranks, which is the one place the road out to a lane is clear from.
///
/// Two such edges can want different boxes last in one rank, and then the second
/// gets it; whether the order that comes out draws anything is decided by the
/// same gate as every other candidate, so wanting the impossible costs nothing.
fn roads_cleared(chart: &Flowchart, rank: &[usize], by_rank: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let mut order = by_rank.to_vec();
    for i in routed(chart, rank, &[]) {
        let edge = &chart.edges[i];
        for node in [edge.from, edge.to] {
            let rank = &mut order[rank[node]];
            rank.retain(|n| *n != node);
            rank.push(node);
        }
    }
    order
}

/// Order one rank by where each of its nodes' neighbours in the next rank up
/// (or down) sit, leaving a node with no neighbours there where it is.
fn sort_by_neighbours(
    chart: &Flowchart,
    rank: &[usize],
    order: &mut [Vec<usize>],
    r: usize,
    upwards: bool,
) {
    let other = if upwards { r - 1 } else { r + 1 };
    let place = |n: usize| order[other].iter().position(|m| *m == n);
    // The mean of the neighbours' places, kept as a fraction so that ranks of
    // different sizes compare without rounding one of them into a tie.
    let key = |i: usize, at: usize| -> (usize, usize) {
        let places: Vec<usize> = chart
            .edges
            .iter()
            .filter_map(|e| {
                let (near, far) = if upwards {
                    (e.to, e.from)
                } else {
                    (e.from, e.to)
                };
                (near == i && rank[far] == other).then(|| place(far))?
            })
            .collect();
        match places.len() {
            0 => (at, 1),
            n => (places.iter().sum::<usize>(), n),
        }
    };
    let mut keyed: Vec<(usize, (usize, usize))> = order[r]
        .iter()
        .enumerate()
        .map(|(at, &i)| (i, key(i, at)))
        .collect();
    keyed.sort_by(|a, b| (a.1.0 * b.1.1).cmp(&(b.1.0 * a.1.1)));
    order[r] = keyed.into_iter().map(|(i, _)| i).collect();
}

fn draw_top_down(
    chart: &Flowchart,
    by_rank: &[Vec<usize>],
    rank: &[usize],
    // Edges taken off their band and routed round the outside instead.
    diverted: &[usize],
    calc: &WidthCalc,
    box_w: impl Fn(usize) -> usize,
) -> Option<Vec<String>> {
    // Widest rank sets the canvas width; every other rank is centred in it.
    let (starts, content_width) = column_starts(by_rank, chart.nodes.len(), &box_w);

    // Edges that skip a rank get their own lane to the right of every box, so
    // a long connector never has to cross a node. An edge taken off its band
    // takes the same road: it is the one place on the canvas that belongs to
    // one edge and to nothing else.
    let long_edges = routed(chart, rank, diverted);
    let on_band = |i: usize| {
        let e = &chart.edges[i];
        rank[e.to] == rank[e.from] + 1 && !diverted.contains(&i)
    };

    // Columns first. Where a box sits across the canvas does not depend on how
    // tall the bands between the ranks turn out to be, and the bands cannot be
    // measured until the labels have columns to be placed against.
    let mut boxes: Vec<Box> = (0..chart.nodes.len())
        .map(|i| Box {
            x: starts[i],
            y: 0,
            w: box_w(i),
            h: 3,
        })
        .collect();

    // How many edges of this band each node is an end of. A label hangs off
    // whichever of its edge's two columns is that edge's alone, and these are
    // what say which one that is.
    let mut parents = vec![0usize; chart.nodes.len()];
    let mut children = vec![0usize; chart.nodes.len()];
    for (i, edge) in chart.edges.iter().enumerate() {
        if on_band(i) {
            parents[edge.to] += 1;
            children[edge.from] += 1;
        }
    }

    // An edge label goes beside a column, on the far side from the other end of
    // the edge, and needs room there. Nothing accounted for it, so a label wide
    // enough ran off the right of the canvas and was cut off mid-word.
    let (mut margin, mut extent) = (0usize, 0usize);
    for (i, edge) in chart.edges.iter().enumerate() {
        let Some(label) = &edge.label else { continue };
        if !on_band(i) {
            continue;
        }
        let w = calc.str(label);
        let (fx, tx) = (boxes[edge.from].center_x(), boxes[edge.to].center_x());
        let shared = parents[edge.to] > 1 && children[edge.from] == 1;
        match label_start(fx, tx, w, shared) {
            start if start < 0 => margin = margin.max(start.unsigned_abs()),
            start => extent = extent.max(start as usize + w),
        }
    }
    for b in &mut boxes {
        b.x += margin;
    }

    // Every label of a band is written above the same bus, so two that overlap
    // land on each other. Where two parents each have two labelled children,
    // both ends of every edge are shared and there is no column left that would
    // tell the labels apart, so the row is what has to give: a label that will
    // not fit beside the ones already placed goes on the row above.
    let level = label_levels(chart, &boxes, rank, diverted, &parents, &children, calc);

    let mut y = 0usize;
    for (r, nodes) in by_rank.iter().enumerate() {
        for &i in nodes {
            boxes[i].y = y;
        }
        y += 3;
        if r + 1 < by_rank.len() {
            // The bus sits in the middle of the band and the labels stack up
            // from just above it, so a band holding n rows of labels has to be
            // deep enough that the topmost of them still clears the boxes.
            let stacked = chart
                .edges
                .iter()
                .enumerate()
                .filter(|(i, e)| rank[e.from] == r && rank[e.to] == r + 1 && on_band(*i))
                .map(|(i, _)| level[i])
                .max()
                .unwrap_or(0);
            y += BAND.max(2 * stacked + 2);
        }
    }
    let height = y;

    // A lane is a vertical run, which no label can be written along, so the
    // label hangs beside it and the next lane starts past the far end of it.
    let mut lane_off = Vec::with_capacity(long_edges.len());
    let mut lane_x = content_width + 1;
    for &i in &long_edges {
        lane_off.push(lane_x);
        lane_x += 2 + chart.edges[i]
            .label
            .as_deref()
            .map_or(0, |l| calc.str(l) + 1);
    }

    let width = margin + (lane_x + 1).max(extent + 1);
    let mut grid = Grid::new(width, height);

    for (i, node) in chart.nodes.iter().enumerate() {
        draw_box(&mut grid, &boxes[i], &node.label, &node.shape, calc);
    }

    // Children of the same parent share one horizontal bus, so the glyph where
    // the bus meets the parent has to know about all of them at once.
    for parent in 0..chart.nodes.len() {
        let fan: Vec<usize> = (0..chart.edges.len())
            .filter(|i| chart.edges[*i].from == parent && on_band(*i))
            .collect();
        if !fan.is_empty() {
            fan_out(
                &mut grid,
                &boxes[parent],
                &fan,
                &chart.edges,
                &boxes,
                &parents,
                &level,
                chart.reversed,
                calc,
            );
        }
    }
    for (i, edge) in chart.edges.iter().enumerate() {
        if let Some(nth) = long_edges.iter().position(|j| *j == i) {
            route_lane(
                &mut grid,
                &boxes[edge.from],
                &boxes[edge.to],
                Lane {
                    at: margin + lane_off[nth],
                    first: margin + content_width + 1,
                },
                edge.label.as_deref(),
                chart.reversed,
                calc,
            );
        }
    }
    Some(grid.rows())
}

fn draw_left_right(
    chart: &Flowchart,
    by_rank: &[Vec<usize>],
    rank: &[usize],
    // Edges taken off their band and routed round the outside instead.
    diverted: &[usize],
    calc: &WidthCalc,
    box_w: impl Fn(usize) -> usize,
) -> Option<Vec<String>> {
    let column_width: Vec<usize> = by_rank
        .iter()
        .map(|nodes| nodes.iter().map(|i| box_w(*i)).max().unwrap_or(0))
        .collect();
    let (starts, content_height) = row_starts(by_rank, chart.nodes.len());

    // An edge that skips a rank gets a lane below every box, the way the
    // top-down layout gives one a lane to the right. Drawn straight it would
    // run at the boxes in between, and since a connector is only drawn where
    // the canvas is still blank, it disappeared behind them instead.
    let long_edges = routed(chart, rank, diverted);
    let on_band = |i: usize| {
        let e = &chart.edges[i];
        rank[e.to] == rank[e.from] + 1 && !diverted.contains(&i)
    };
    let lanes = if long_edges.is_empty() {
        0
    } else {
        long_edges.len() + 1
    };
    let height = content_height + lanes;

    // Rows before columns. Which row a label lands on decides which other
    // labels it has to share space with, that decides how wide the run between
    // two columns has to be, and only then is there an x to put a box at.
    let mut boxes: Vec<Box> = (0..chart.nodes.len())
        .map(|i| Box {
            x: 0,
            y: starts[i],
            w: box_w(i),
            h: 3,
        })
        .collect();

    // How many edges of this band each node is an end of, which is what says
    // which end of its run a label can hang at without meeting another.
    let mut parents = vec![0usize; chart.nodes.len()];
    let mut children = vec![0usize; chart.nodes.len()];
    for (i, edge) in chart.edges.iter().enumerate() {
        if on_band(i) {
            parents[edge.to] += 1;
            children[edge.from] += 1;
        }
    }
    let side = |edge: &Edge| label_at_left(chart.reversed, parents[edge.to], children[edge.from]);

    // A label is written into the connector itself, so the space between two
    // columns has to hold it along with the arrowhead and a cell of line on
    // either side — and where labels hang at both ends of the runs, both of
    // them and the bend in between.
    //
    // Several edges can land on one row: two parents arriving at one child
    // share the child's row, and the rule that picks an end cannot separate
    // them when both parents also have several children. They are set down one
    // after another along the row instead, and what has to be reserved is the
    // whole row rather than its widest single label.
    let mut offset = vec![0usize; chart.edges.len()];
    let mut hangs_left = vec![false; chart.edges.len()];
    let mut widest = vec![(0usize, 0usize); by_rank.len()];
    let mut rows: Vec<(usize, usize, bool, usize)> = Vec::new();
    for (i, edge) in chart.edges.iter().enumerate() {
        let Some(label) = &edge.label else { continue };
        if !on_band(i) {
            continue;
        }
        let band = rank[edge.from];
        let width = calc.str(label);
        let row_at = |at_left: bool| {
            if at_left {
                boxes[edge.from].center_y()
            } else {
                boxes[edge.to].center_y()
            }
        };
        let occupied = |rows: &Vec<(usize, usize, bool, usize)>, at_left: bool| {
            rows.iter()
                .any(|(b, r, l, _)| *b == band && *r == row_at(at_left) && *l == at_left)
        };
        // The end the rule picks, and failing that the other one. Both ends can
        // be taken — every row of the boundary carries two edges when two
        // parents each have two children — and only then do labels share a row,
        // which is the case the offset is for.
        let preferred = side(edge);
        let at_left = if occupied(&rows, preferred) && !occupied(&rows, !preferred) {
            !preferred
        } else {
            preferred
        };
        hangs_left[i] = at_left;

        let row = row_at(at_left);
        let taken = match rows
            .iter_mut()
            .find(|(b, r, l, _)| *b == band && *r == row && *l == at_left)
        {
            Some((_, _, _, taken)) => {
                let at = *taken;
                *taken += width + 1;
                at
            }
            None => {
                rows.push((band, row, at_left, width + 1));
                0
            }
        };
        offset[i] = taken;
        let end = &mut widest[band];
        let w = if at_left { &mut end.0 } else { &mut end.1 };
        *w = (*w).max(taken + width);
    }
    let gap: Vec<usize> = widest
        .iter()
        .map(|end| {
            match *end {
                (0, 0) => 0,
                (l, 0) => l + 6,
                (0, r) => r + 5,
                (l, r) => l + r + 7,
            }
            .max(BAND * 2)
        })
        .collect();

    let mut x = 0usize;
    for (r, nodes) in by_rank.iter().enumerate() {
        for &i in nodes {
            boxes[i].x = x;
        }
        x += column_width[r] + gap[r];
    }
    // A lane label that outgrows the run it sits in is written past the far
    // corner instead, which can reach beyond the last column. Reserving for it
    // costs nothing: a row is trimmed of trailing blanks on its way out.
    let mut width = x + 2;
    for &i in &long_edges {
        let e = &chart.edges[i];
        let Some(label) = &e.label else { continue };
        let hi = boxes[e.from].center_x().max(boxes[e.to].center_x());
        width = width.max(hi + calc.str(label) + 3);
    }

    let mut grid = Grid::new(width, height);
    for (i, node) in chart.nodes.iter().enumerate() {
        draw_box(&mut grid, &boxes[i], &node.label, &node.shape, calc);
    }
    for (i, edge) in chart.edges.iter().enumerate() {
        if on_band(i) {
            connect_right(
                &mut grid,
                &boxes[edge.from],
                &boxes[edge.to],
                &EdgeLabel {
                    text: edge.label.as_deref(),
                    at_left: hangs_left[i],
                    offset: offset[i],
                    reserved: widest[rank[edge.from]],
                },
                chart.reversed,
                calc,
            );
        } else if let Some(nth) = long_edges.iter().position(|j| *j == i) {
            let lane = content_height + nth + 1;
            route_lane_below(
                &mut grid,
                &boxes[edge.from],
                &boxes[edge.to],
                Lane {
                    at: lane,
                    first: content_height,
                },
                edge.label.as_deref(),
                chart.reversed,
                calc,
            );
        }
    }
    Some(grid.rows())
}

fn draw_box(grid: &mut Grid, b: &Box, label: &str, shape: &Shape, calc: &WidthCalc) {
    let (tl, tr, bl, br) = match shape {
        Shape::Round | Shape::Stadium | Shape::Diamond => ('╭', '╮', '╰', '╯'),
        Shape::Rect => ('┌', '┐', '└', '┘'),
    };
    // A real lozenge cannot be drawn on a character grid without looking like
    // a mistake, so a decision node is marked by angle brackets instead.
    let (left, right) = match shape {
        Shape::Diamond => ('<', '>'),
        _ => ('│', '│'),
    };
    grid.put(b.x, b.y, tl, calc);
    grid.put(b.right(), b.y, tr, calc);
    grid.put(b.x, b.bottom(), bl, calc);
    grid.put(b.right(), b.bottom(), br, calc);
    for x in b.x + 1..b.right() {
        grid.put(x, b.y, '─', calc);
        grid.put(x, b.bottom(), '─', calc);
    }
    grid.put(b.x, b.y + 1, left, calc);
    grid.put(b.right(), b.y + 1, right, calc);
    // Clear the interior first: a sequence-diagram note is drawn over the
    // lifelines it spans, and they would otherwise show through the label.
    for x in b.x + 1..b.right() {
        grid.put(x, b.y + 1, ' ', calc);
    }
    grid.text(b.x + 2, b.y + 1, label, calc);
}

/// Whether every band can be drawn without offering a connection nobody wrote.
///
/// A band has one shared run between two ranks: top-down, the bus every
/// parent's edges hang off; left-to-right, the column every connector turns in.
/// Two of them that overlap are joined rather than drawn over each other, which
/// is deliberate — two parents arriving at one child do meet at that child's
/// column — but joining them also joins everything else the two runs touch. A
/// run with two boxes hanging off one side and two off the other offers all
/// four connections, so
///
/// ```text
/// A --> C      ┌───┐   ┌───┐
/// A --> D      │ A │   │ B │
/// B --> C      └───┘   └───┘
///                │       │
///                ├───────┤
///                ▼       ▼
///              ┌───┐   ┌───┐
///              │ C │   │ D │
///              └───┘   └───┘
/// ```
///
/// is the same picture as those three edges plus `B --> D`, with nothing in it
/// to say which of the two graphs it is. Three different charts drew it.
///
/// A run is honest exactly when the connections it offers are the edges that
/// were written — one parent fanning out to several children, or several
/// parents merging into one child, both of which are the whole of what a shared
/// run can say. Where it is not, there is nothing to redraw: a second run would
/// have to cross the first parent's connector, and box drawing has no glyph for
/// a crossing that does not join. The chart is declined instead, and the caller
/// falls back to `mmdc` or to showing the source, which is the same thing it
/// does for every other construct this renderer cannot model.
///
/// `across` is the axis the shared run travels along: the column of a box for a
/// top-down chart, its row for a left-to-right one. Sideways the bend column is
/// taken to be one per band, which is what the reserved widths are worked out
/// for; where two boxes of a rank differ in width the bends land a column or
/// two apart, and a chart declined for overlapping there was one whose
/// connectors ran through each other's corners anyway.
fn bus_runs_are_honest(
    chart: &Flowchart,
    rank: &[usize],
    diverted: &[usize],
    across: impl Fn(usize) -> usize,
) -> bool {
    let last = rank.iter().copied().max().unwrap_or(0);
    for band in 0..last {
        // One run per edge, spanning between its two ends. A parent's bus is
        // the hull of its own edges' runs, so merging runs edge by edge finds
        // the same components as merging bus by bus.
        let mut runs: Vec<(usize, usize, Vec<usize>, Vec<usize>)> = chart
            .edges
            .iter()
            .enumerate()
            .filter(|(i, e)| {
                rank[e.from] == band && rank[e.to] == band + 1 && !diverted.contains(i)
            })
            .map(|(_, e)| {
                let (f, t) = (across(e.from), across(e.to));
                (f.min(t), f.max(t), vec![e.from], vec![e.to])
            })
            .collect();
        runs.sort_by_key(|(lo, _, _, _)| *lo);

        // Sweep, absorbing every run that reaches the one before it. Merely
        // touching counts: two runs that share a single column share the glyph
        // in it, and a reader follows the line straight through.
        let mut merged: Vec<(usize, usize, Vec<usize>, Vec<usize>)> = Vec::new();
        for run in runs {
            match merged.last_mut() {
                Some(prev) if run.0 <= prev.1 => {
                    prev.1 = prev.1.max(run.1);
                    prev.2.extend(run.2);
                    prev.3.extend(run.3);
                }
                _ => merged.push(run),
            }
        }

        for (_, _, ups, downs) in merged {
            for from in &ups {
                for to in &downs {
                    if !chart.edges.iter().any(|e| e.from == *from && e.to == *to) {
                        return false;
                    }
                }
            }
        }
    }
    true
}

/// How many rows above the bus each edge's label sits.
///
/// Zero is the row immediately above it, which is where a label goes when
/// nothing is in the way — and where every label used to go unconditionally.
///
/// The rule that gives a label its column puts it beside whichever of the
/// edge's two ends is that edge's alone. Where two parents each have two
/// labelled children, neither end is: both parents' columns carry two edges and
/// so do both children's. The labels are then placed by the only thing left,
/// which is the order they are written in, so this is the standard sweep over
/// intervals sorted by where they start, taking the lowest row each will fit on.
///
/// One blank column is kept between neighbours. Two labels that merely touch
/// read as one word.
fn label_levels(
    chart: &Flowchart,
    boxes: &[Box],
    rank: &[usize],
    diverted: &[usize],
    parents: &[usize],
    children: &[usize],
    calc: &WidthCalc,
) -> Vec<usize> {
    let mut level = vec![0usize; chart.edges.len()];
    let mut placed: Vec<(usize, usize, isize, isize)> = Vec::new();
    let mut spans: Vec<(usize, isize, isize)> = Vec::new();

    for (i, edge) in chart.edges.iter().enumerate() {
        let Some(label) = &edge.label else { continue };
        if rank[edge.to] != rank[edge.from] + 1 || diverted.contains(&i) {
            continue;
        }
        let w = calc.str(label) as isize;
        let (fx, tx) = (boxes[edge.from].center_x(), boxes[edge.to].center_x());
        let shared = parents[edge.to] > 1 && children[edge.from] == 1;
        let start = label_start(fx, tx, calc.str(label), shared);
        spans.push((i, start, start + w + 1));
    }
    spans.sort_by_key(|(_, start, _)| *start);

    for (i, start, end) in spans {
        let band = rank[chart.edges[i].from];
        let lv = (0..)
            .find(|lv| {
                !placed
                    .iter()
                    .any(|(b, l, s, e)| *b == band && l == lv && start < *e && *s < end)
            })
            .unwrap_or(0);
        level[i] = lv;
        placed.push((band, lv, start, end));
    }
    level
}

/// Where an edge's label starts on the row above the bus.
///
/// It has to hang off a column that is this edge's alone, or two labels land on
/// each other. The children of one parent each have a column to themselves,
/// which is the usual case; but several parents arriving at one child share the
/// child's column, and then it is their own that tells them apart.
///
/// Signed, because a label to the left of the leftmost column falls off the
/// canvas, and how far it does is how much the canvas is widened to hold it.
fn label_start(fx: usize, tx: usize, w: usize, shared_child: bool) -> isize {
    let (fx, tx, w) = (fx as isize, tx as isize, w as isize);
    match (shared_child, tx > fx) {
        (false, true) => tx + 1,
        (true, false) => fx + 1,
        _ => fx - w - 1,
    }
}

/// Draw the band that carries one parent's edges down to the next rank.
///
/// All of a parent's children hang off a single horizontal bus. The glyph where
/// the bus meets a column depends on whether the line continues up (the parent),
/// down (a child), or both, which is why this cannot be done one edge at a time.
///
/// In a `BT` chart every one of these edges was written pointing the other way,
/// so the arrowheads belong at the top, on the parent, where all of them meet.
#[allow(clippy::too_many_arguments)]
fn fan_out(
    grid: &mut Grid,
    from: &Box,
    fan: &[usize],
    all_edges: &[Edge],
    boxes: &[Box],
    parents: &[usize],
    level: &[usize],
    reversed: bool,
    calc: &WidthCalc,
) {
    let edges: Vec<&Edge> = fan.iter().map(|i| &all_edges[*i]).collect();
    let fx = from.center_x();
    let top = from.bottom() + 1;
    let bottom = boxes[edges[0].to].y;
    let mid = top + (bottom - top) / 2;

    let mut targets: Vec<usize> = edges.iter().map(|e| boxes[e.to].center_x()).collect();
    targets.sort_unstable();
    targets.dedup();

    if targets == [fx] {
        grid.vline(fx, top, bottom - 1, calc);
        // Say so on the bus row even though the line through it needs no
        // saying: another parent's bus may reach this column, and it can only
        // draw the junction right if it can see that this line passes through.
        grid.join(fx, mid, UP | DOWN, calc);
    } else {
        let lo = targets.iter().copied().min().unwrap().min(fx);
        let hi = targets.iter().copied().max().unwrap().max(fx);
        grid.vline(fx, top, mid - 1, calc);

        // The whole bus is stated cell by cell as the directions the line
        // leaves in, the plain stretches along with the junctions. A column
        // another parent's bus also runs through then carries both, whichever
        // of them was drawn first.
        for x in lo..=hi {
            let dirs = (if x == fx { UP } else { 0 })
                | (if targets.contains(&x) { DOWN } else { 0 })
                | (if x > lo { LEFT } else { 0 })
                | (if x < hi { RIGHT } else { 0 });
            grid.join(x, mid, dirs, calc);
        }
        for &tx in &targets {
            if mid < bottom - 1 {
                grid.vline(tx, mid + 1, bottom - 1, calc);
            }
        }
    }

    if reversed {
        grid.put(fx, top, '▲', calc);
    }
    for (&i, edge) in fan.iter().zip(&edges) {
        let tx = boxes[edge.to].center_x();
        if !reversed {
            grid.put(tx, bottom.saturating_sub(1), '▼', calc);
        }
        if let Some(label) = &edge.label {
            // Above the bus, outside the column this edge has to itself, on the
            // far side from its other end. The parent's connector runs down
            // this same row, and a label written straight across it erased the
            // line it belongs to.
            //
            // The row comes from the placement pass, which is the only thing
            // that can see every label of the band at once; this fan knows
            // about its own parent's edges and not about the ones a second
            // parent hangs in the same space.
            let shared = parents[edge.to] > 1 && edges.len() == 1;
            let start = label_start(fx, tx, calc.str(label), shared);
            let row = mid.saturating_sub(1 + level[i]);
            grid.text(start.max(0) as usize, row, label, calc);
        }
    }
}

fn connect_right(
    grid: &mut Grid,
    from: &Box,
    to: &Box,
    label: &EdgeLabel,
    reversed: bool,
    calc: &WidthCalc,
) {
    let (fy, ty) = (from.center_y(), to.center_y());
    let left = from.right() + 1;
    let right = to.x;

    // The connector has to turn clear of the labels hanging at either end, and
    // of the *widest* of them rather than its own, so that everything leaving
    // one box still turns in one column. The gap was widened by that much.
    let (hold_left, hold_right) = label.reserved;
    let mut mid = left + (right - left) / 2;
    if fy != ty {
        if hold_left > 0 {
            mid = mid.max(left + hold_left + 3);
        }
        if hold_right > 0 {
            mid = mid.min(right.saturating_sub(hold_right + 4));
        }
    }
    // Written before the lines: a line is drawn only where the canvas is still
    // blank, so it parts around the label rather than erasing it.
    if let Some(text) = label.text {
        // Along the row from whichever end this label hangs at, past any that
        // are already there. Two edges arriving at one box share its row, and
        // written at the same place the second is drawn over the first: the
        // label that survived read as though it belonged to the other edge.
        let (x, y) = if label.at_left {
            (left + 2 + label.offset, fy)
        } else {
            (
                right
                    .saturating_sub(calc.str(text) + 2 + label.offset)
                    .max(left + 1),
                ty,
            )
        };
        grid.text(x, y, text, calc);
    }

    if fy == ty {
        // Stated as directions too, for the same reason: a child level with its
        // parent is reached by a straight run, and that run crosses the column
        // the parent's other connectors turn in. Drawn as plain line it was
        // refused there — the cell was not blank — so the run stopped dead at a
        // corner belonging to another edge of its own fan.
        for x in left..right {
            grid.join(x, fy, LEFT | RIGHT, calc);
        }
    } else {
        // Both corners are stated as the directions the line leaves in rather
        // than as the glyph one edge alone would need, because every connector
        // of a band turns in this same column and each knows only its own half
        // of what is there. A parent with a child above and one below turns
        // twice in the cell beside it — `┘` and `┐`, which together are `┤` —
        // and drawn as glyphs the second simply replaced the first, leaving the
        // run it carried stopping half a cell short of the corner.
        let (leaving, arriving) = if ty > fy { (DOWN, UP) } else { (UP, DOWN) };
        if mid > left {
            grid.hline(left, mid - 1, fy, calc);
        }
        grid.join(mid, fy, LEFT | leaving, calc);
        grid.vline(mid, fy.min(ty) + 1, fy.max(ty) - 1, calc);
        grid.join(mid, ty, RIGHT | arriving, calc);
        grid.hline(mid + 1, right - 1, ty, calc);
    }
    // An `RL` chart's edges were turned round to place the ranks, so the head
    // goes back on the left, against the box the document called the target.
    if reversed {
        grid.put(left, fy, '◀', calc);
    } else {
        grid.put(right.saturating_sub(1), ty, '▶', calc);
    }
}

/// An edge's label and which end of the run between two columns it hangs at.
struct EdgeLabel<'a> {
    text: Option<&'a str>,
    at_left: bool,
    /// Columns already spoken for by labels sharing this one's row, measured
    /// from the end it hangs at. Zero for a row with one label on it.
    offset: usize,
    /// The widest row of labels at each end of this rank boundary. The bend
    /// between them has to clear both.
    reserved: (usize, usize),
}

/// Which end of a run a label hangs at.
///
/// The arrowhead's end by default, since that is the end where each of the
/// edges a box fans out arrives on a row of its own. Where several edges arrive
/// at one box they share that row, and then it is the end they leave from that
/// tells them apart.
fn label_at_left(reversed: bool, parents: usize, children: usize) -> bool {
    if reversed {
        !(children > 1 && parents == 1)
    } else {
        parents > 1 && children == 1
    }
}

/// Where an edge that skips a rank is routed, and where the lanes start.
///
/// A route may only fill blank cells while it is still among the boxes, and has
/// to join what it finds once it is among the other lanes.
struct Lane {
    at: usize,
    first: usize,
}

/// Route a left-to-right edge that skips a rank under the boxes between it and
/// its target, and back up.
fn route_lane_below(
    grid: &mut Grid,
    from: &Box,
    to: &Box,
    lane: Lane,
    label: Option<&str>,
    reversed: bool,
    calc: &WidthCalc,
) {
    let (fx, tx) = (from.center_x(), to.center_x());
    // The lane is clear of every box, so the label can sit in the line itself,
    // centred on the run that spans the ranks the edge skips — but past the far
    // corner when the run is too short for it, since a corner is drawn hard and
    // would cut the label in half.
    if let Some(label) = label {
        let (lo, hi) = (fx.min(tx), fx.max(tx));
        let w = calc.str(label);
        let x = if w + 2 <= hi - lo {
            lo + (hi - lo - w) / 2
        } else {
            hi + 2
        };
        grid.text(x, lane.at, label, calc);
    }
    // Down to the lane and back up. Above the lanes the column runs behind the
    // boxes of its own rank and may only fill blank cells, but once among them
    // it crosses the lanes of the other edges that skip a rank, and there it
    // has to say so: their corners are drawn hard, so the line used to stop dead
    // at the first one and start again below it.
    for (x, top) in [(fx, from.bottom() + 1), (tx, to.bottom() + 1)] {
        for y in top..lane.at {
            if y < lane.first {
                grid.put_soft(x, y, '│', calc);
            } else {
                grid.cross(x, y, UP | DOWN, calc);
            }
        }
    }
    for x in fx + 1..tx {
        grid.cross(x, lane.at, LEFT | RIGHT, calc);
    }
    grid.join(fx, lane.at, UP | RIGHT, calc);
    grid.join(tx, lane.at, UP | LEFT, calc);
    let (head_x, head_y) = if reversed {
        (fx, from.bottom() + 1)
    } else {
        (tx, to.bottom() + 1)
    };
    grid.put(head_x, head_y, '▲', calc);
}

/// Route an edge that skips a rank out to its own lane and back.
fn route_lane(
    grid: &mut Grid,
    from: &Box,
    to: &Box,
    lane: Lane,
    label: Option<&str>,
    reversed: bool,
    calc: &WidthCalc,
) {
    let start = from.bottom();
    let end = to.center_y();
    // Beside the lane, not in it: the horizontal ends of this route pass behind
    // the boxes of their own rank, and a label written there would sit on top
    // of one. Only the lane column itself is clear all the way down.
    if let Some(label) = label {
        grid.text(lane.at + 1, start + (end - start) / 2, label, calc);
    }
    // Out to the lane and back. The two horizontal ends run behind the boxes of
    // their own rank and may only fill blank cells there, but where they reach
    // the lanes they cross the ones belonging to the other edges that skip a
    // rank, and there they have to say so: a lane's corner is drawn hard, so the
    // run used to stop at the first one it met.
    for (y, x0) in [(start, from.right() + 1), (end, to.right() + 1)] {
        for x in x0..lane.at {
            if x < lane.first {
                grid.put_soft(x, y, '─', calc);
            } else {
                grid.cross(x, y, LEFT | RIGHT, calc);
            }
        }
    }
    // The lane column itself is clear of every box all the way down, so it can
    // be drawn the whole way and let another edge's run cross it.
    for y in start + 1..end {
        grid.cross(lane.at, y, UP | DOWN, calc);
    }
    grid.join(lane.at, start, LEFT | DOWN, calc);
    grid.join(lane.at, end, UP | LEFT, calc);
    let (head_x, head_y) = if reversed {
        (from.right() + 1, start)
    } else {
        (to.right() + 1, end)
    };
    grid.put(head_x, head_y, '◀', calc);
}

// ---------------------------------------------------------------------------
// Sequence diagrams
// ---------------------------------------------------------------------------

mod sequence {
    use super::*;

    #[derive(Debug)]
    enum Step {
        Message {
            from: usize,
            to: usize,
            text: String,
            dashed: bool,
        },
        Note {
            over: Vec<usize>,
            text: String,
        },
    }

    pub fn render(code: &str, calc: &WidthCalc) -> Option<Vec<String>> {
        let mut names: Vec<(String, String)> = Vec::new();
        let mut steps: Vec<Step> = Vec::new();

        let intern = |names: &mut Vec<(String, String)>, id: &str| -> usize {
            let id = id.trim().to_string();
            match names.iter().position(|(n, _)| *n == id) {
                Some(i) => i,
                None => {
                    names.push((id.clone(), id));
                    names.len() - 1
                }
            }
        };

        for line in code
            .lines()
            .map(str::trim)
            .skip(1)
            .filter(|l| !l.is_empty() && !l.starts_with("%%"))
        {
            if let Some(rest) = line
                .strip_prefix("participant ")
                .or_else(|| line.strip_prefix("actor "))
            {
                let (id, label) = match rest.split_once(" as ") {
                    Some((id, label)) => (id.trim(), label.trim()),
                    None => (rest.trim(), rest.trim()),
                };
                let idx = intern(&mut names, id);
                names[idx].1 = label.to_string();
                continue;
            }
            if let Some(rest) = line.strip_prefix("Note ") {
                let rest = rest
                    .strip_prefix("over ")
                    .or_else(|| rest.strip_prefix("right of "))
                    .or_else(|| rest.strip_prefix("left of "))?;
                let (targets, text) = rest.split_once(':')?;
                let over = targets
                    .split(',')
                    .map(|t| intern(&mut names, t))
                    .collect::<Vec<_>>();
                steps.push(Step::Note {
                    over,
                    text: text.trim().to_string(),
                });
                continue;
            }
            // Control-flow blocks change the shape of the diagram, so decline.
            // As words, though: `optional->>B: hi` names a participant.
            if [
                "loop",
                "alt",
                "opt",
                "par",
                "else",
                "end",
                "activate",
                "deactivate",
            ]
            .iter()
            .any(|word| keyword(line, word))
            {
                return None;
            }

            // The message text is separated off first. An arrow was looked for
            // anywhere in the line, so `A->B: use --> this` found the one in
            // the message, split there, and gave up on the whole diagram.
            let (participants, text) = line.split_once(':')?;
            let (arrow, dashed) = ["-->>", "->>", "-->", "->", "--x", "-x"]
                .iter()
                .find(|a| participants.contains(**a))
                .map(|a| (*a, a.starts_with("--")))?;
            let (left, right) = participants.split_once(arrow)?;
            let from = intern(&mut names, left);
            let to = intern(&mut names, right);
            steps.push(Step::Message {
                from,
                to,
                text: text.trim().to_string(),
                dashed,
            });
        }

        if names.is_empty() || steps.is_empty() {
            return None;
        }
        Some(draw(&names, &steps, calc))
    }

    /// Left and right columns a note stretches between.
    fn note_span(over: &[usize], centers: &[usize]) -> (usize, usize) {
        let lo = over.iter().copied().min().unwrap_or(0);
        let hi = over.iter().copied().max().unwrap_or(lo);
        (
            centers[lo].saturating_sub(1),
            centers[hi.min(centers.len() - 1)] + 2,
        )
    }

    fn note_width(text: &str, lo: usize, hi: usize, calc: &WidthCalc) -> usize {
        (hi + 1).saturating_sub(lo).max(calc.str(text) + 4)
    }

    fn draw(names: &[(String, String)], steps: &[Step], calc: &WidthCalc) -> Vec<String> {
        // Each lifeline gets a column wide enough for its own box, and wide
        // enough for the longest message that starts or ends on it.
        let mut column_width: Vec<usize> =
            names.iter().map(|(_, label)| calc.str(label) + 4).collect();
        for step in steps {
            if let Step::Message { from, to, text, .. } = step {
                let (lo, hi) = (*from.min(to), *from.max(to));
                let span = hi.abs_diff(lo).max(1);
                let need = (calc.str(text) + 4) / span;
                for width in &mut column_width[lo..=hi] {
                    *width = (*width).max(need);
                }
            }
        }

        let mut centers = Vec::with_capacity(names.len());
        let mut x = 0usize;
        for w in &column_width {
            centers.push(x + w / 2);
            x += w + GAP;
        }
        // A note is drawn as a box spanning its participants, and may be wider
        // than they are; the canvas has to leave room or its right border is
        // silently clipped off.
        let width = steps
            .iter()
            .filter_map(|step| match step {
                Step::Note { over, text } => {
                    let (lo, hi) = note_span(over, &centers);
                    Some(lo + note_width(text, lo, hi, calc))
                }
                _ => None,
            })
            .fold(x + 2, usize::max);
        let height = 3 + steps.len() * 3 + 1;
        let mut grid = Grid::new(width, height);

        for (i, (_, label)) in names.iter().enumerate() {
            let w = calc.str(label) + 4;
            let bx = centers[i] - w / 2;
            let b = Box {
                x: bx,
                y: 0,
                w,
                h: 3,
            };
            draw_box(&mut grid, &b, label, &Shape::Rect, calc);
        }

        // Lifelines run the whole height, under the boxes.
        for &cx in &centers {
            grid.vline(cx, 3, height - 1, calc);
        }

        for (i, step) in steps.iter().enumerate() {
            let y = 4 + i * 3;
            match step {
                Step::Message {
                    from,
                    to,
                    text,
                    dashed,
                } => {
                    let (fx, tx) = (centers[*from], centers[*to]);
                    let (lo, hi) = (fx.min(tx), fx.max(tx));
                    let glyph = if *dashed { '╌' } else { '─' };
                    for x in lo..=hi {
                        grid.put(x, y, glyph, calc);
                    }
                    grid.put(fx, y, if tx > fx { '├' } else { '┤' }, calc);
                    grid.put(tx, y, if tx > fx { '▶' } else { '◀' }, calc);
                    // The label sits on the row above its arrow, clear of both
                    // lifelines.
                    grid.text(lo + 2, y - 1, text, calc);
                }
                Step::Note { over, text } => {
                    let (lo, hi) = note_span(over, &centers);
                    let b = Box {
                        x: lo,
                        y: y - 1,
                        w: note_width(text, lo, hi, calc),
                        h: 3,
                    };
                    draw_box(&mut grid, &b, text, &Shape::Round, calc);
                }
            }
        }
        grid.rows()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CALC: WidthCalc = WidthCalc {
        ambiguous_wide: false,
    };

    fn rows(code: &str) -> Vec<String> {
        render(code, &CALC).unwrap_or_else(|| panic!("declined to render:\n{code}"))
    }

    #[test]
    fn a_linear_flowchart_draws_a_box_per_node() {
        let out = rows("flowchart TD\n    A[one] --> B[two]\n    B --> C[three]\n");
        let text = out.join("\n");
        assert!(text.contains("one") && text.contains("two") && text.contains("three"));
        assert_eq!(text.matches('┌').count(), 3, "{text}");
        assert!(text.contains('▼'), "arrows point at the target");
    }

    #[test]
    fn node_labels_default_to_the_node_id() {
        let out = rows("graph TD\n  A --> B\n");
        let text = out.join("\n");
        assert!(text.contains('A') && text.contains('B'));
    }

    #[test]
    fn a_branch_puts_siblings_on_the_same_rank() {
        let chart = parse_flowchart("flowchart TD\n A --> B\n A --> C\n").unwrap();
        let rank = ranks(&chart).unwrap();
        assert_eq!(rank[0], 0);
        assert_eq!(rank[1], 1);
        assert_eq!(rank[2], 1);
    }

    #[test]
    fn edge_labels_are_parsed_in_both_spellings() {
        let a = parse_flowchart("flowchart TD\n A -->|yes| B\n").unwrap();
        assert_eq!(a.edges[0].label.as_deref(), Some("yes"));
        let b = parse_flowchart("flowchart TD\n A -- no --> B\n").unwrap();
        assert_eq!(b.edges[0].label.as_deref(), Some("no"));
    }

    #[test]
    fn shapes_are_recognised() {
        let chart =
            parse_flowchart("flowchart TD\n A[rect] --> B(round)\n B --> C{choice}\n").unwrap();
        assert_eq!(chart.nodes[0].shape, Shape::Rect);
        assert_eq!(chart.nodes[1].shape, Shape::Round);
        assert_eq!(chart.nodes[2].shape, Shape::Diamond);
    }

    #[test]
    fn a_chain_on_one_line_becomes_two_edges() {
        let chart = parse_flowchart("flowchart LR\n A --> B --> C\n").unwrap();
        assert_eq!(chart.edges.len(), 2);
        assert_eq!(chart.nodes.len(), 3);
    }

    #[test]
    fn repeating_a_node_id_reuses_it() {
        let chart = parse_flowchart("flowchart TD\n A[first] --> B\n A --> C\n").unwrap();
        assert_eq!(chart.nodes.len(), 3);
        assert_eq!(chart.nodes[0].label, "first");
    }

    #[test]
    fn left_to_right_lays_ranks_out_horizontally() {
        let out = rows("flowchart LR\n A[one] --> B[two]\n");
        assert!(out.iter().any(|r| r.contains("one") && r.contains("two")));
        assert!(out.join("\n").contains('▶'));
    }

    #[test]
    fn a_left_to_right_edge_that_skips_a_rank_goes_under_the_boxes() {
        // Drawn straight, this one ran at B — and since a connector is only
        // drawn where the canvas is blank, it vanished behind it and the
        // diagram was simply missing an edge.
        let out = rows("flowchart LR\n A --> B\n B --> C\n A --> C\n");
        assert_eq!(
            out.len(),
            5,
            "two rows under the boxes:\n{}",
            out.join("\n")
        );
        assert!(out[4].contains('└') && out[4].contains('┘'), "{:?}", out[4]);
        assert_eq!(
            out.join("\n").matches('▲').count(),
            1,
            "one head, pointing up into C"
        );
    }

    #[test]
    fn a_left_to_right_chart_with_no_long_edge_gains_no_rows() {
        assert_eq!(rows("flowchart LR\n A --> B\n").len(), 3);
    }

    #[test]
    fn every_row_fits_the_widest_row() {
        let out = rows("flowchart TD\n A[a very long label indeed] --> B[b]\n A --> C[c]\n");
        let widths: Vec<usize> = out.iter().map(|r| CALC.str(r)).collect();
        let max = widths.iter().copied().max().unwrap();
        assert!(max > 0);
        assert!(out.iter().all(|r| CALC.str(r) <= max));
    }

    #[test]
    fn cjk_labels_are_measured_in_display_columns() {
        let out = rows("flowchart TD\n A[日本語] --> B[b]\n");
        let top = out.iter().find(|r| r.contains('┌')).unwrap();
        // Box is 日本語 (6 columns) plus two spaces and two borders.
        assert_eq!(CALC.str(top), 10);
    }

    #[test]
    fn siblings_hang_off_one_shared_bus() {
        let out = rows("flowchart TD\n A[p] --> B[l]\n A --> C[r]\n");
        let bus = out
            .iter()
            .find(|r| r.contains('┴'))
            .expect("the bus meets the parent at a T junction");
        assert!(bus.starts_with(' ') || bus.starts_with('┌'));
        assert_eq!(
            bus.matches('┴').count(),
            1,
            "one junction, not one per edge"
        );
        assert!(bus.contains('┌') && bus.contains('┐'), "{bus}");
    }

    #[test]
    fn an_edge_label_is_drawn_in_full_and_leaves_the_connector_alone() {
        let out = rows("flowchart TD\n A[p] -->|a fairly long label| B[left]\n A --> C[right]\n");
        let row = out
            .iter()
            .find(|r| r.contains("a fairly long label"))
            .expect("the label is drawn, and not cut off at the canvas edge");
        // The parent's connector runs down this same row; the label used to be
        // written straight over it.
        assert!(row.contains('│'), "{row:?}");
    }

    #[test]
    fn an_edge_label_survives_whatever_length_it_is() {
        // Every one of these was silently truncated at the right edge once the
        // label grew past the width of the boxes.
        for n in 1..40 {
            let label = "x".repeat(n);
            let code = format!("flowchart TD\n A[p] -->|{label}| B[l]\n A -->|{label}| C[r]\n");
            let out = rows(&code);
            assert_eq!(
                out.iter().filter(|r| r.contains(&label)).count(),
                1,
                "both labels share the row above the bus:\n{}",
                out.join("\n")
            );
            let row = out.iter().find(|r| r.contains(&label)).unwrap();
            assert_eq!(row.matches(&label).count(), 2, "{row:?}");
            assert!(row.contains('│'), "{row:?}");
        }
    }

    #[test]
    fn parents_that_meet_at_one_child_share_the_junction_under_it() {
        // Each parent drew the junction for itself and the second was drawn
        // over the first, so this came out `└────┌────┘` — a corner turning
        // away where the column carrying on downwards wants a `┬`.
        let out = rows("flowchart TD\n A[one] --> C[join]\n B[two] --> C\n");
        let bus = out
            .iter()
            .find(|r| r.contains('┬'))
            .unwrap_or_else(|| panic!("no junction:\n{}", out.join("\n")));
        assert_eq!(bus.trim(), "└────┬────┘", "\n{}", out.join("\n"));
    }

    #[test]
    fn a_parent_standing_over_the_junction_carries_its_line_through_it() {
        // B is directly above D, so its connector arrives from above and goes
        // on down while the buses from A and C come in from either side.
        let out = rows("flowchart TD\n A --> D\n B --> D\n C --> D\n");
        let bus = out
            .iter()
            .find(|r| r.contains('┼'))
            .unwrap_or_else(|| panic!("no junction:\n{}", out.join("\n")));
        assert_eq!(bus.trim(), "└───────┼───────┘", "\n{}", out.join("\n"));
    }

    #[test]
    fn a_junction_never_turns_away_from_the_child_below_it() {
        // Whichever parent draws last, only the two ends of the bus turn: a
        // `┌` or a `┐` anywhere in between is a line stopping where another
        // line was meant to carry on.
        for n in 2..7 {
            let mut code = String::from("flowchart TD\n");
            for i in 0..n {
                code.push_str(&format!(" {} --> Z\n", (b'A' + i) as char));
            }
            let out = rows(&code);
            let bus = out
                .iter()
                .find(|r| r.contains('┬') || r.contains('┼'))
                .unwrap_or_else(|| panic!("{n} parents, no junction:\n{}", out.join("\n")));
            assert!(
                !bus.contains('┌') && !bus.contains('┐'),
                "{n} parents: {bus:?}"
            );
            assert!(
                bus.trim().starts_with('└') && bus.trim().ends_with('┘'),
                "{n} parents: {bus:?}"
            );
        }
    }

    #[test]
    fn labels_meeting_at_one_node_hang_off_the_column_each_edge_has_to_itself() {
        // Both were written beside the child they share, so the second landed
        // on the first and `from a` beside `from b` read `from bm a`. Each is
        // now beside its own parent, which is what tells the two edges apart.
        for dir in ["TD", "LR", "RL"] {
            for n in 1..24 {
                let (q, z) = ("q".repeat(n), "z".repeat(n));
                let code =
                    format!("flowchart {dir}\n A[one] -->|{q}| C[join]\n B[two] -->|{z}| C\n");
                let joined = rows(&code).join("\n");
                // Either label surviving whole is the property: written over
                // each other, whichever went second leaves the first in
                // pieces — `from a` and `from b` came out as `from bm a`.
                for label in [&q, &z] {
                    assert!(joined.contains(label.as_str()), "{dir} lost it:\n{joined}");
                }
                for boxed in ["│ one │", "│ two │", "│ join │"] {
                    assert!(joined.contains(boxed), "{dir}:\n{joined}");
                }
            }
        }
    }

    #[test]
    fn a_fan_out_keeps_its_labels_beside_the_children() {
        // The switch is only for the edges that share a child. Where one parent
        // has them all, the children's columns are what tell the edges apart
        // and the labels stay where they were.
        let out = rows("flowchart TD\n A{ok?} -->|yes| B[go]\n A -->|no| C[stop]\n");
        let row = out
            .iter()
            .find(|r| r.contains("yes"))
            .unwrap_or_else(|| panic!("{}", out.join("\n")));
        assert!(
            row.contains("no"),
            "both beside the bus:\n{}",
            out.join("\n")
        );
    }

    /// The row a box sits on, out of a drawing.
    fn row_of<'a>(out: &'a [String], boxed: &str) -> &'a String {
        out.iter()
            .find(|r| r.contains(boxed))
            .unwrap_or_else(|| panic!("no {boxed}:\n{}", out.join("\n")))
    }

    #[test]
    fn a_sideways_fan_turns_in_one_glyph_rather_than_in_the_last_one_drawn() {
        // Every connector of a band turns in the same column and each knows
        // only its own half of what belongs there: `┘` for the child above and
        // `┐` for the one below are together a `┤`, and drawn as glyphs
        // whichever went second replaced the other. `│ A │───┐` with a `│`
        // standing on it is a run stopping half a cell short of its own corner.
        for n in 2..7 {
            let mut code = String::from("flowchart LR\n");
            for i in 0..n {
                code.push_str(&format!(" A --> {}\n", (b'B' + i) as char));
            }
            let out = rows(&code);
            // An odd fan has a child level with the parent, reached by a
            // straight run through the same cell, which makes the fourth arm.
            let want = if n % 2 == 1 { '┼' } else { '┤' };
            let row = row_of(&out, "│ A │");
            assert!(
                row.contains(want),
                "{n} children want {want}: {row:?}\n{}",
                out.join("\n")
            );
        }
    }

    #[test]
    fn a_sideways_merge_turns_in_one_glyph_as_well() {
        // The same cell on the other end of the run: every parent's connector
        // arrives at the child's row in the turning column, and the glyph there
        // has to carry all of them rather than the last one written.
        for n in 2..7 {
            let mut code = String::from("flowchart LR\n");
            for i in 0..n {
                code.push_str(&format!(" {} --> Z\n", (b'A' + i) as char));
            }
            let out = rows(&code);
            let want = if n % 2 == 1 { '┼' } else { '├' };
            let row = row_of(&out, "│ Z │");
            assert!(
                row.contains(want),
                "{n} parents want {want}: {row:?}\n{}",
                out.join("\n")
            );
        }
    }

    #[test]
    fn a_second_lane_leaving_one_box_carries_on_past_the_first() {
        // Two edges that skip a rank out of the same box each get a lane, and
        // the second has to cross the first's corner to reach its own. A corner
        // is drawn hard and a plain line only fills blank cells, so the second
        // lane used to stop dead there and start again on the far side.
        let td = rows("flowchart TD\n A --> B --> C --> D\n A --> C\n A --> D\n");
        let out = td
            .iter()
            .find(|r| r.contains('┬'))
            .unwrap_or_else(|| panic!("TD:\n{}", td.join("\n")));
        assert_eq!(out.trim(), "└───┘─┬─┐", "TD:\n{}", td.join("\n"));

        let lr = rows("flowchart LR\n A --> B --> C --> D\n A --> C\n A --> D\n");
        let out = lr
            .iter()
            .find(|r| r.contains('├'))
            .unwrap_or_else(|| panic!("LR:\n{}", lr.join("\n")));
        assert_eq!(
            out.trim(),
            "├─────────────────────┘          │",
            "LR:\n{}",
            lr.join("\n")
        );
    }

    #[test]
    fn one_lane_crossing_another_does_not_claim_to_meet_it() {
        // Here the lanes belong to different boxes, so one runs across the
        // other rather than out of the same corner. The two edges do not meet
        // there, and a `┼` said they did: the vertical is drawn and the
        // horizontal shows the one-cell gap where it runs behind.
        let td = rows("flowchart TD\n A --> B --> C --> D --> E\n A --> C\n B --> D\n");
        let joined = td.join("\n");
        assert!(!joined.contains('┼'), "TD:\n{joined}");
        // B's run out to its own lane, crossing the lane A --> C is in.
        assert!(td.iter().any(|r| r.trim() == "└───┘─│─┐"), "TD:\n{joined}");
        let lr = rows("flowchart LR\n A --> B --> C --> D --> E\n A --> C\n B --> D\n");
        let joined = lr.join("\n");
        assert!(!joined.contains('┼'), "LR:\n{joined}");
        // The lane A --> C is in, crossed by B's run down to its own, which
        // carries on past it.
        assert!(
            lr.iter()
                .any(|r| r.trim() == "└──────────│──────────┘          │"),
            "LR:\n{joined}"
        );
    }

    #[test]
    fn the_line_in_front_of_a_crossing_stays_unbroken() {
        // A crossing is drawn by leaving out one cell of the line behind it, so
        // the line in front has to be continuous: a gap in that one would read
        // as a line that stops rather than one that is crossed. Every chain
        // length is tried because the lanes cross at a different place in each.
        for n in 4..12 {
            let names: Vec<String> = (0..n).map(|i| format!("N{i}")).collect();
            let code = format!(
                "flowchart TD\n {}\n {} --> {}\n {} --> {}\n",
                names.join(" --> "),
                names[0],
                names[2],
                names[1],
                names[3],
            );
            let out = rows(&code);
            let joined = out.join("\n");
            assert!(!joined.contains('┼'), "{n} boxes:\n{joined}");
            // The lane of the first edge is the column its top corner is in,
            // and it has to be a line in every row down to the corner that
            // turns it back towards its target.
            let at = |r: &String, c: char| r.chars().position(|g| g == c);
            let (top, col) = out
                .iter()
                .enumerate()
                .find_map(|(y, r)| at(r, '┐').map(|x| (y, x)))
                .unwrap_or_else(|| panic!("{n} boxes, no lane:\n{joined}"));
            let end = out
                .iter()
                .enumerate()
                .skip(top + 1)
                .find_map(|(y, r)| (at(r, '┘') == Some(col)).then_some(y))
                .unwrap_or_else(|| panic!("{n} boxes, lane never turns back:\n{joined}"));
            for (y, row) in out.iter().enumerate().take(end).skip(top + 1) {
                let glyph = row.chars().nth(col);
                assert!(
                    matches!(glyph, Some('│' | '├' | '┤' | '┼')),
                    "{n} boxes, row {y} column {col} is {glyph:?}:\n{joined}"
                );
            }
        }
    }

    #[test]
    fn a_lane_never_writes_over_the_label_it_is_carrying() {
        // `join` reads the cell it is about to draw, and a label is not a line:
        // it has nothing to say about where one goes and must be left alone.
        let out = rows("flowchart LR\n A --> B --> C\n A -->|the skip| C\n");
        let joined = out.join("\n");
        assert!(joined.contains("the skip"), "{joined}");
    }

    #[test]
    fn a_sideways_edge_carries_its_label_to_the_box_it_points_at() {
        // Labels were drawn only by the fan that hangs a top-down parent's
        // children off one bus, so `flowchart LR` dropped every one it was
        // given: `A -->|yes| B` drew no `yes` anywhere.
        for dir in ["LR", "RL"] {
            for n in 1..24 {
                let (q, z) = ("q".repeat(n), "z".repeat(n));
                let code =
                    format!("flowchart {dir}\n A[start] -->|{q}| B[next]\n A -->|{z}| C[other]\n");
                let out = rows(&code);
                let joined = out.join("\n");
                let at = |label: &String| {
                    out.iter()
                        .position(|r| r.contains(label.as_str()))
                        .unwrap_or_else(|| panic!("{dir} lost {label}:\n{joined}"))
                };
                // Each label goes to the end its own arrowhead is on, which is
                // the one end where the edges of a fan are on rows of their
                // own. Written where they leave, they would share a row and
                // the second would be drawn over the first.
                assert!(out[at(&q)].contains("next"), "{dir}:\n{joined}");
                assert!(out[at(&z)].contains("other"), "{dir}:\n{joined}");
                assert_eq!(joined.matches('▶').count() + joined.matches('◀').count(), 2);
                assert!(joined.contains("│ start │"), "{dir}:\n{joined}");
            }
        }
    }

    #[test]
    fn four_labels_between_two_parents_and_two_children_all_survive() {
        // Both parents' columns carry two edges and so do both children's, so
        // no column tells the four labels apart and they were all written on
        // one row. Two of them were drawn over the other two: `from b` came
        // out as `fto d2`, which is not a word anybody wrote.
        for n in 1..14 {
            let labels: Vec<String> = ["fa", "td", "fb", "t2"]
                .iter()
                .map(|s| format!("{s}{}", "q".repeat(n)))
                .collect();
            let code = format!(
                "flowchart TD\n A -->|{}| C\n A -->|{}| D\n B -->|{}| C\n B -->|{}| D\n",
                labels[0], labels[1], labels[2], labels[3]
            );
            let out = rows(&code);
            let joined = out.join("\n");
            for label in &labels {
                assert!(
                    joined.contains(label.as_str()),
                    "lost {label} at width {n}:\n{joined}"
                );
            }
        }
    }

    #[test]
    fn four_sideways_labels_between_two_parents_and_two_children_all_survive() {
        // The `LR` version of the same collision, and it was the worse one: two
        // labels were lost outright and the two that survived sat on a row
        // belonging to an edge that was not theirs, so `A ──from b──▶ C` said
        // the label of `B --> C` about the edge from `A`.
        for dir in ["LR", "RL"] {
            for n in 1..12 {
                let labels: Vec<String> = ["fa", "td", "fb", "t2"]
                    .iter()
                    .map(|s| format!("{s}{}", "q".repeat(n)))
                    .collect();
                let code = format!(
                    "flowchart {dir}\n A -->|{}| C\n A -->|{}| D\n B -->|{}| C\n B -->|{}| D\n",
                    labels[0], labels[1], labels[2], labels[3]
                );
                let out = rows(&code);
                let joined = out.join("\n");
                for label in &labels {
                    assert!(
                        joined.contains(label.as_str()),
                        "{dir} lost {label} at width {n}:\n{joined}"
                    );
                }
                // And none of them was written over a box.
                for node in ["A", "B", "C", "D"] {
                    assert!(
                        joined.contains(&format!("│ {node} │")),
                        "{dir} lost box {node}:\n{joined}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_run_with_one_label_on_each_row_is_no_wider_than_it_was() {
        // Sharing a row is what costs width, and a diagram whose labels each
        // have a row to themselves must not pay for it.
        let out = rows("flowchart LR\n A[start] -->|yes| B[next]\n A -->|no| C[other]\n");
        let widest = out.iter().map(|r| r.chars().count()).max().unwrap();
        assert_eq!(widest, 26, "{}", out.join("\n"));
    }

    #[test]
    fn a_band_with_nothing_to_stack_is_no_taller_than_it_was() {
        // Stacking is what a collision costs, and a diagram without one must
        // not pay it: two children of one parent go left and right of it and
        // have a row to share.
        let plain = rows("flowchart TD\n A --> B\n A --> C\n").len();
        let labelled = rows("flowchart TD\n A -->|yes| B\n A -->|no| C\n").len();
        assert_eq!(plain, labelled, "a label made the diagram taller");
    }

    #[test]
    fn stacked_labels_stay_clear_of_the_boxes_above_them() {
        // The bus sits in the middle of the band and labels stack upwards from
        // it, so a band that does not grow with them writes the topmost one
        // over the row of boxes above.
        let long = "x".repeat(12);
        let code = format!(
            "flowchart TD\n A -->|{long}1| C\n A -->|{long}2| D\n B -->|{long}3| C\n B -->|{long}4| D\n"
        );
        let out = rows(&code);
        let joined = out.join("\n");
        for (i, row) in out.iter().enumerate() {
            if row.contains(&long) {
                assert!(
                    !row.contains('│') || !row.contains('┌'),
                    "row {i} has a label written into the boxes:\n{joined}"
                );
            }
        }
        // The two rows of boxes are still whole.
        assert_eq!(joined.matches('┌').count(), 4, "{joined}");
    }

    #[test]
    fn an_edge_that_skips_a_rank_carries_its_label_too() {
        // The other place with no label: an edge routed out to a lane of its
        // own, in either direction.
        for dir in ["TD", "LR"] {
            for n in 1..40 {
                let label = "q".repeat(n);
                let code = format!("flowchart {dir}\n A --> B\n B --> C\n A -->|{label}| C\n");
                let out = rows(&code);
                let joined = out.join("\n");
                assert!(
                    joined.contains(&label),
                    "{dir} truncated the label at the canvas edge:\n{joined}"
                );
                // Beside the lane, never over a box or over the corner that
                // turns the lane, both of which are drawn hard.
                for boxed in ["│ A │", "│ B │", "│ C │"] {
                    assert!(joined.contains(boxed), "{dir}:\n{joined}");
                }
                let heads = joined.matches(['▲', '▼', '◀', '▶']).count();
                assert_eq!(heads, 3, "one head per edge, {dir}:\n{joined}");
            }
        }
    }

    #[test]
    fn a_decision_node_is_marked_with_angle_brackets() {
        let out = rows("flowchart TD\n A{ok?} --> B[yes]\n");
        assert!(out.iter().any(|r| r.contains("< ok? >")), "{out:#?}");
    }

    #[test]
    fn a_note_is_drawn_over_the_lifelines_it_spans() {
        let out = rows(
            "sequenceDiagram\n participant A\n participant B\n A->>B: hi\n Note over A,B: careful\n",
        );
        let body = out
            .iter()
            .find(|r| r.contains("careful"))
            .expect("the note text is drawn");
        // The lifelines must not show through the note's interior.
        assert!(!body.contains("││"), "{body:?}");
    }

    #[test]
    fn a_reply_arrow_points_the_other_way() {
        let out = rows("sequenceDiagram\n A->>B: ask\n B-->>A: answer\n");
        let reply = out.iter().find(|r| r.contains('◀')).unwrap();
        assert!(reply.ends_with('┤'), "{reply:?}");
    }

    #[test]
    fn a_right_to_left_chart_points_its_arrow_at_the_target() {
        // Mermaid puts A on the right and B on the left, with the arrow into
        // B. Turning the edge round to place the ranks got the boxes right and
        // the one thing a diagram says — which way it goes — wrong.
        let out = rows("flowchart RL\n A --> B\n");
        let row = out
            .iter()
            .find(|r| r.contains('◀') || r.contains('▶'))
            .unwrap();
        assert!(!row.contains('▶'), "the arrow points at B, not A: {row:?}");
        assert!(row.find('B') < row.find('A'), "{row:?}");
    }

    #[test]
    fn a_bottom_to_top_chart_points_its_arrow_upwards() {
        let out = rows("flowchart BT\n A --> B\n");
        let text = out.join("\n");
        assert!(text.contains('▲'), "{text}");
        assert!(!text.contains('▼'), "{text}");
        assert!(text.find('B') < text.find('A'), "B is on top: {text}");
    }

    #[test]
    fn a_top_down_chart_still_points_downwards() {
        let out = rows("flowchart TD\n A --> B\n A --> C\n");
        let text = out.join("\n");
        assert_eq!(text.matches('▼').count(), 2, "one head per edge: {text}");
        assert!(!text.contains('▲'), "{text}");
    }

    /// One of the sixteen subsets of the four edges between `A`,`B` and `C`,`D`.
    fn k22_subset(dir: &str, mask: u8) -> String {
        let mut code = format!("flowchart {dir}\n");
        for (i, (from, to)) in [("A", "C"), ("A", "D"), ("B", "C"), ("B", "D")]
            .iter()
            .enumerate()
        {
            if mask & (1 << i) != 0 {
                code.push_str(&format!(" {from} --> {to}\n"));
            }
        }
        code
    }

    #[test]
    fn no_two_different_charts_come_out_as_the_same_drawing() {
        // A run with two boxes hanging off one side and two off the other
        // offers every connection between them, whichever of them were
        // written, so three of these subsets drew the picture of all four:
        //
        //     ┌───┐   ┌───┐
        //     │ A │   │ B │
        //     └───┘   └───┘
        //       │       │
        //       ├───────┤
        //       ▼       ▼
        //     ┌───┐   ┌───┐
        //     │ C │   │ D │
        //     └───┘   └───┘
        for dir in ["TD", "LR"] {
            let mut seen: Vec<(u8, String)> = Vec::new();
            for mask in 1u8..16 {
                let Some(out) = render(&k22_subset(dir, mask), &CALC) else {
                    continue;
                };
                let drawing = out.join("\n");
                if let Some((other, _)) = seen.iter().find(|(_, d)| *d == drawing) {
                    panic!("{dir}: {mask:04b} and {other:04b} draw the same picture:\n{drawing}");
                }
                seen.push((mask, drawing));
            }
            assert!(
                seen.iter().any(|(m, _)| *m == 0b1111),
                "{dir}: every connection was written, which is what a run says"
            );
        }
    }

    #[test]
    fn an_edge_a_band_cannot_carry_is_routed_round_it_rather_than_declined() {
        // The run cannot say these three edges without offering the fourth,
        // and no order changes that: the two parents meet at the child they
        // share. Taking one edge off the band and out to a lane of its own —
        // the road an edge that skips a rank already takes — leaves a band the
        // rest can say honestly, and gives that edge a run nothing else is on.
        for dir in ["TD", "LR", "BT", "RL"] {
            for code in [
                format!("flowchart {dir}\n A --> C\n A --> D\n B --> C\n"),
                format!("flowchart {dir}\n A --> C\n A --> D\n B --> D\n"),
                format!("flowchart {dir}\n A --> C\n B --> C\n B --> D\n"),
            ] {
                assert!(render(&code, &CALC).is_some(), "declined:\n{code}");
            }
        }
    }

    #[test]
    fn a_routed_edge_is_given_a_road_clear_of_every_box() {
        // A lane is out past the far end of every rank, and the run reaching
        // it passes behind whatever boxes stand between: it came out of hiding
        // beside the box next door and read as that box's edge —
        // `│ C │◀──│ D │─┘` for a run that starts at `B`. So the rank is
        // ordered to put both of its ends last, and where that cannot be had
        // the chart is declined.
        let out = rows("flowchart TD\n A --> C\n A --> D\n B --> C\n");
        let row = row_of(&out, "│ C │");
        assert!(
            row.find("│ D │") < row.find("│ C │"),
            "C is not the last box of its rank: {row:?}\n{}",
            out.join("\n")
        );

        // `Z`, `A` and `B` all reach the pair below them, so whichever edge
        // were taken out would have a box beside it whichever way round they
        // are put.
        for dir in ["TD", "LR", "BT", "RL"] {
            let code = format!("flowchart {dir}\n Z --> C\n Z --> D\n A --> D\n B --> C\n");
            assert!(render(&code, &CALC).is_none(), "drawn anyway:\n{code}");
        }
    }

    #[test]
    fn an_edge_that_skips_a_rank_is_given_a_clear_road_too() {
        // The same road, and a claim about it that was not true: an edge that
        // skips a rank was let out unasked, on the grounds that it hides behind
        // boxes belonging to ranks it has nothing to do with. The boxes in its
        // way belong to its own rank. `A --> C` beside `B --> Z` reached the
        // lane along C's row, passed behind `Z` and came out the far side as
        // `│ C │◀──│ Z │─┘`, joining two boxes nobody joined. Ordering `C` last
        // of its rank is all it takes, and the arrowhead then sits against the
        // box it points at however many boxes are beside it.
        for k in 1..5 {
            let siblings: String = (0..k).map(|i| format!(" B --> Z{i}\n")).collect();
            let code = format!("flowchart TD\n A --> B --> C\n A --> C\n{siblings}");
            let out = rows(&code);
            let joined = out.join("\n");
            assert!(
                out.iter().any(|r| r.ends_with("│ C │◀┘")),
                "{k} siblings, something stands beside C:\n{joined}"
            );
        }
    }

    #[test]
    fn a_chart_whose_routed_edges_cannot_all_have_a_clear_road_is_declined() {
        // Two edges skip a rank, and their four ends are the two boxes of one
        // rank and the two of another. Only one box of a rank can be its last,
        // so whichever way round the ranks are put, one of the two runs would
        // pass behind a box and come out as its edge.
        for dir in ["TD", "LR", "BT", "RL"] {
            let code =
                format!("flowchart {dir}\n A --> X --> C\n B --> Y --> D\n A --> C\n B --> D\n");
            assert!(render(&code, &CALC).is_none(), "drawn anyway:\n{code}");
        }
    }

    #[test]
    fn a_routed_edge_keeps_its_label_and_leaves_the_others_theirs() {
        // Written on the band all three shared one row and the second was set
        // down over the first. The one taken out carries its label beside its
        // own run, where nothing else on the canvas can reach it.
        let out = rows("flowchart TD\n A -->|ac| C\n A -->|ad| D\n B -->|bc| C\n");
        let joined = out.join("\n");
        for label in ["ac", "ad", "bc"] {
            assert!(joined.contains(label), "{label} lost:\n{joined}");
        }
        let lane = out
            .iter()
            .find(|r| r.contains("bc"))
            .unwrap_or_else(|| panic!("{joined}"));
        assert!(
            !lane.contains("│ A │") && !lane.contains("│ B │"),
            "the routed label is still on the band: {lane:?}\n{joined}"
        );
    }

    #[test]
    fn a_rank_written_in_the_wrong_order_is_swapped_rather_than_the_chart_declined() {
        // `C` is introduced before `D` and then hung off the later of the two
        // parents, so the two runs are threaded through each other for no
        // reason the graph gives. Swapping the pair is the whole of the fix,
        // and declining a chart over the order it happened to be written in
        // says nothing true about it.
        let code = "flowchart TD\n C --> E\n D --> E\n A --> D\n B --> C\n";
        let out = rows(code);
        let row = row_of(&out, "│ D │");
        assert!(
            row.find("│ D │") < row.find("│ C │"),
            "not swapped: {row:?}\n{}",
            out.join("\n")
        );
        // Each parent now drops straight onto its child, so only the band
        // below them — where `D` and `C` do meet at `E` — has a bus in it.
        let buses = out
            .iter()
            .filter(|r| r.contains('┬') || r.contains('┼') || r.contains('┴'))
            .count();
        assert_eq!(buses, 1, "still threaded:\n{}", out.join("\n"));

        // Sideways it is the rows that were the wrong way round.
        let out = rows("flowchart LR\n C --> E\n D --> E\n A --> D\n B --> C\n");
        assert!(
            row_of(&out, "│ A │").contains("│ D │"),
            "A and D are not level:\n{}",
            out.join("\n")
        );
    }

    #[test]
    fn untangling_one_band_does_not_tangle_the_one_after_it() {
        // The sweep runs over every rank, so a swap made to separate two runs
        // can put the rank below it wrong. Whether it worked is checked rather
        // than assumed, and a chart is only drawn on an order that holds for
        // every band of it at once.
        let out = rows("flowchart TD\n C --> E\n D --> E\n F --> G\n A --> D\n B --> C\n");
        let joined = out.join("\n");
        for boxed in [
            "│ A │",
            "│ B │",
            "│ C │",
            "│ D │",
            "│ E │",
            "│ F │",
            "│ G │",
        ] {
            assert!(joined.contains(boxed), "{boxed} missing:\n{joined}");
        }
        let row = row_of(&out, "│ D │");
        assert!(
            row.find("│ D │") < row.find("│ C │"),
            "first band still threaded: {row:?}\n{joined}"
        );
    }

    #[test]
    fn the_connections_a_shared_run_can_say_are_still_drawn() {
        // One parent reaching all of its children, all of a child's parents
        // reaching it, and the two of them stacked into a diamond, are the
        // whole of what a run says — and are most of what anyone draws.
        for dir in ["TD", "LR", "BT", "RL"] {
            for code in [
                format!("flowchart {dir}\n A --> C\n A --> D\n"),
                format!("flowchart {dir}\n A --> C\n B --> C\n"),
                format!("flowchart {dir}\n A --> B\n A --> C\n B --> D\n C --> D\n"),
                format!("flowchart {dir}\n A --> C\n A --> D\n B --> C\n B --> D\n"),
            ] {
                assert!(render(&code, &CALC).is_some(), "declined:\n{code}");
            }
        }
    }

    #[test]
    fn a_cycle_is_declined_rather_than_drawn_wrong() {
        assert!(render("flowchart TD\n A --> B\n B --> A\n", &CALC).is_none());
    }

    #[test]
    fn a_self_edge_is_declined_rather_than_dropped() {
        // The rank layout has nowhere to put one, so it was left out of the
        // drawing and the reader was shown a chart with an edge missing.
        assert!(render("flowchart TD\n A --> A\n", &CALC).is_none());
        assert!(render("flowchart TD\n A --> A\n A --> B\n", &CALC).is_none());
        assert!(render("flowchart LR\n A --> B\n B --> B\n", &CALC).is_none());
    }

    #[test]
    fn a_node_named_after_a_keyword_is_still_a_node() {
        // Every one of these was declined and shown as source, because the id
        // begins with the letters of a word that means something else.
        for code in [
            "flowchart TD\n endpoint[X] --> B\n",
            "flowchart TD\n subgraphs[X] --> B\n",
            "flowchart TD\n classification[X] --> B\n",
            "flowchart TD\n styles[X] --> B\n",
            "flowchart TD\n clicks[X] --> B\n",
        ] {
            let chart = parse_flowchart(code).unwrap_or_else(|| panic!("declined:\n{code}"));
            assert_eq!(chart.nodes.len(), 2, "{code}");
        }
    }

    #[test]
    fn a_participant_named_after_a_keyword_is_still_a_participant() {
        let out = rows("sequenceDiagram\n loopback->>optional: hi\n");
        let text = out.join("\n");
        assert!(
            text.contains("loopback") && text.contains("optional"),
            "{text}"
        );
    }

    #[test]
    fn subgraphs_are_declined() {
        assert!(render("flowchart TD\n subgraph one\n A --> B\n end\n", &CALC).is_none());
    }

    #[test]
    fn unknown_diagram_types_are_declined() {
        assert!(render("pie title Pets\n  \"Dogs\" : 42\n", &CALC).is_none());
        assert!(render("gantt\n title A\n", &CALC).is_none());
        assert!(render("", &CALC).is_none());
    }

    #[test]
    fn a_sequence_diagram_draws_lifelines_and_arrows() {
        let out = rows(
            "sequenceDiagram\n    participant A as Alice\n    participant B as Bob\n    A->>B: Hello\n    B-->>A: Hi there\n",
        );
        let text = out.join("\n");
        assert!(text.contains("Alice") && text.contains("Bob"));
        assert!(text.contains("Hello") && text.contains("Hi there"));
        assert!(text.contains('▶') && text.contains('◀'));
        assert!(text.contains('╌'), "dashed replies are drawn dashed");
    }

    #[test]
    fn sequence_participants_are_implied_by_messages() {
        let out = rows("sequenceDiagram\n    Alice->>Bob: Hi\n");
        let text = out.join("\n");
        assert!(text.contains("Alice") && text.contains("Bob"));
    }

    #[test]
    fn an_arrow_in_a_message_is_part_of_the_message() {
        let out = rows("sequenceDiagram\n A->B: use --> this\n B-->>A: and A -> B too\n");
        let text = out.join("\n");
        assert!(text.contains("use --> this"), "{text}");
        assert!(text.contains("and A -> B too"), "{text}");
        // Two participants, not four, and not one called `A->B: use `.
        assert_eq!(text.matches('┌').count(), 2, "{text}");
    }

    #[test]
    fn sequence_control_flow_is_declined() {
        assert!(
            render(
                "sequenceDiagram\n  loop every minute\n    A->>B: tick\n  end\n",
                &CALC
            )
            .is_none()
        );
    }

    #[test]
    fn a_statement_may_end_in_a_semicolon() {
        // Mermaid's own documentation writes them, and reading `B;` as a node
        // id drew a chart with two boxes called B.
        let chart = parse_flowchart("graph TD;\n A --> B;\n B --> C;\n").unwrap();
        let ids: Vec<&str> = chart.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, ["A", "B", "C"]);
        assert_eq!(chart.edges.len(), 2);
    }

    #[test]
    fn semicolons_separate_statements_on_one_line() {
        let chart = parse_flowchart("graph LR; A --> B; B --> C\n").unwrap();
        assert_eq!(chart.direction, Direction::LeftRight);
        assert_eq!(chart.nodes.len(), 3);
        assert_eq!(chart.edges.len(), 2);
    }

    #[test]
    fn a_semicolon_inside_a_label_stays_in_the_label() {
        let chart = parse_flowchart("flowchart TD\n A[stop; then go] --> B\n").unwrap();
        assert_eq!(chart.nodes[0].label, "stop; then go");
        assert_eq!(chart.nodes.len(), 2);
    }

    #[test]
    fn comments_are_ignored() {
        let chart = parse_flowchart("flowchart TD\n%% a comment\n A --> B\n").unwrap();
        assert_eq!(chart.edges.len(), 1);
    }

    #[test]
    fn a_comment_after_a_statement_is_not_a_node() {
        let chart = parse_flowchart("flowchart TD\n A --> B; %% a trailing note\n").unwrap();
        let ids: Vec<&str> = chart.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, ["A", "B"]);
    }

    #[test]
    fn styling_directives_are_ignored_rather_than_declined() {
        let chart = parse_flowchart(
            "flowchart TD\n A --> B\n classDef big fill:#f00\n class A big\n style B fill:#0f0\n",
        )
        .unwrap();
        assert_eq!(chart.nodes.len(), 2);
    }
}
