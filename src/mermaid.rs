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

    match chart.direction {
        Direction::TopDown => draw_top_down(chart, &by_rank, &rank, calc, box_w),
        Direction::LeftRight => draw_left_right(chart, &by_rank, &rank, calc, box_w),
    }
}

fn draw_top_down(
    chart: &Flowchart,
    by_rank: &[Vec<usize>],
    rank: &[usize],
    calc: &WidthCalc,
    box_w: impl Fn(usize) -> usize,
) -> Option<Vec<String>> {
    // Widest rank sets the canvas width; every other rank is centred in it.
    let rank_widths: Vec<usize> = by_rank
        .iter()
        .map(|nodes| {
            nodes.iter().map(|i| box_w(*i)).sum::<usize>() + GAP * nodes.len().saturating_sub(1)
        })
        .collect();
    let content_width = rank_widths.iter().copied().max().unwrap_or(0);

    // Edges that skip a rank get their own lane to the right of every box, so
    // a long connector never has to cross a node.
    let long_edges: Vec<usize> = (0..chart.edges.len())
        .filter(|i| {
            let e = &chart.edges[*i];
            rank[e.to] > rank[e.from] + 1
        })
        .collect();

    let mut boxes: Vec<Box> = Vec::with_capacity(chart.nodes.len());
    boxes.extend((0..chart.nodes.len()).map(|_| Box {
        x: 0,
        y: 0,
        w: 0,
        h: 0,
    }));

    // Columns first. Where a box sits across the canvas does not depend on how
    // tall the bands between the ranks turn out to be, and the bands cannot be
    // measured until the labels have columns to be placed against.
    for (r, nodes) in by_rank.iter().enumerate() {
        let mut x = (content_width - rank_widths[r]) / 2;
        for &i in nodes {
            let w = box_w(i);
            boxes[i] = Box { x, y: 0, w, h: 3 };
            x += w + GAP;
        }
    }

    // How many edges of this band each node is an end of. A label hangs off
    // whichever of its edge's two columns is that edge's alone, and these are
    // what say which one that is.
    let mut parents = vec![0usize; chart.nodes.len()];
    let mut children = vec![0usize; chart.nodes.len()];
    for edge in &chart.edges {
        if rank[edge.to] == rank[edge.from] + 1 {
            parents[edge.to] += 1;
            children[edge.from] += 1;
        }
    }

    // An edge label goes beside a column, on the far side from the other end of
    // the edge, and needs room there. Nothing accounted for it, so a label wide
    // enough ran off the right of the canvas and was cut off mid-word.
    let (mut margin, mut extent) = (0usize, 0usize);
    for edge in &chart.edges {
        let Some(label) = &edge.label else { continue };
        if rank[edge.to] != rank[edge.from] + 1 {
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
    let level = label_levels(chart, &boxes, rank, &parents, &children, calc);

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
                .filter(|(_, e)| rank[e.from] == r && rank[e.to] == r + 1)
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
            .filter(|i| {
                let e = &chart.edges[*i];
                e.from == parent && rank[e.to] == rank[parent] + 1
            })
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
        if rank[edge.to] > rank[edge.from] + 1 {
            let nth = long_edges.iter().position(|j| *j == i).unwrap_or(0);
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
    calc: &WidthCalc,
    box_w: impl Fn(usize) -> usize,
) -> Option<Vec<String>> {
    let column_width: Vec<usize> = by_rank
        .iter()
        .map(|nodes| nodes.iter().map(|i| box_w(*i)).max().unwrap_or(0))
        .collect();
    let rank_heights: Vec<usize> = by_rank
        .iter()
        .map(|nodes| nodes.len() * 3 + nodes.len().saturating_sub(1))
        .collect();
    let content_height = rank_heights.iter().copied().max().unwrap_or(3);

    // An edge that skips a rank gets a lane below every box, the way the
    // top-down layout gives one a lane to the right. Drawn straight it would
    // run at the boxes in between, and since a connector is only drawn where
    // the canvas is still blank, it disappeared behind them instead.
    let long_edges: Vec<usize> = (0..chart.edges.len())
        .filter(|i| rank[chart.edges[*i].to] > rank[chart.edges[*i].from] + 1)
        .collect();
    let lanes = if long_edges.is_empty() {
        0
    } else {
        long_edges.len() + 1
    };
    let height = content_height + lanes;

    let mut boxes: Vec<Box> = (0..chart.nodes.len())
        .map(|_| Box {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        })
        .collect();

    // How many edges of this band each node is an end of, which is what says
    // which end of its run a label can hang at without meeting another.
    let mut parents = vec![0usize; chart.nodes.len()];
    let mut children = vec![0usize; chart.nodes.len()];
    for edge in &chart.edges {
        if rank[edge.to] == rank[edge.from] + 1 {
            parents[edge.to] += 1;
            children[edge.from] += 1;
        }
    }
    let side = |edge: &Edge| label_at_left(chart.reversed, parents[edge.to], children[edge.from]);

    // Rows before columns. Which row a label lands on decides which other
    // labels it has to share space with, that decides how wide the run between
    // two columns has to be, and only then is there an x to put a box at.
    for (r, nodes) in by_rank.iter().enumerate() {
        let mut y = (content_height - rank_heights[r]) / 2;
        for &i in nodes {
            boxes[i] = Box {
                x: 0,
                y,
                w: box_w(i),
                h: 3,
            };
            y += 4;
        }
    }

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
        if rank[edge.to] != rank[edge.from] + 1 {
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
        if rank[edge.to] == rank[edge.from] + 1 {
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
    parents: &[usize],
    children: &[usize],
    calc: &WidthCalc,
) -> Vec<usize> {
    let mut level = vec![0usize; chart.edges.len()];
    let mut placed: Vec<(usize, usize, isize, isize)> = Vec::new();
    let mut spans: Vec<(usize, isize, isize)> = Vec::new();

    for (i, edge) in chart.edges.iter().enumerate() {
        let Some(label) = &edge.label else { continue };
        if rank[edge.to] != rank[edge.from] + 1 {
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
        grid.hline(left, right - 1, fy, calc);
    } else {
        grid.hline(left, mid, fy, calc);
        grid.put(mid, fy, if ty > fy { '┐' } else { '┘' }, calc);
        grid.vline(mid, fy.min(ty) + 1, fy.max(ty) - 1, calc);
        grid.put(mid, ty, if ty > fy { '└' } else { '┌' }, calc);
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
    // has to join what it finds: their corners are drawn hard, so the line used
    // to stop dead at the first one and start again below it.
    for (x, top) in [(fx, from.bottom() + 1), (tx, to.bottom() + 1)] {
        for y in top..lane.at {
            if y < lane.first {
                grid.put_soft(x, y, '│', calc);
            } else {
                grid.join(x, y, UP | DOWN, calc);
            }
        }
    }
    for x in fx + 1..tx {
        grid.join(x, lane.at, LEFT | RIGHT, calc);
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
    // rank, and there they have to join: a lane's corner is drawn hard, so the
    // run used to stop at the first one it met.
    for (y, x0) in [(start, from.right() + 1), (end, to.right() + 1)] {
        for x in x0..lane.at {
            if x < lane.first {
                grid.put_soft(x, y, '─', calc);
            } else {
                grid.join(x, y, LEFT | RIGHT, calc);
            }
        }
    }
    // The lane column itself is clear of every box all the way down, so it can
    // join the whole way and let another edge's run cross it.
    for y in start + 1..end {
        grid.join(lane.at, y, UP | DOWN, calc);
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
    fn one_lane_crossing_another_is_drawn_as_a_crossing() {
        // Here the lanes belong to different boxes, so one runs across the
        // other rather than out of the same corner.
        for dir in ["TD", "LR"] {
            let code = format!("flowchart {dir}\n A --> B --> C --> D --> E\n A --> C\n B --> D\n");
            let out = rows(&code);
            let joined = out.join("\n");
            assert!(joined.contains('┼'), "{dir}:\n{joined}");
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
