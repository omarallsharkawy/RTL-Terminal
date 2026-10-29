//! Pure geometry for tabs and split panes: no window, GPU or PTY involved, so it is unit-tested.
//!
//! Each tab owns a binary tree of splits. [`layout`] turns the tree plus the available rectangle
//! (physical pixels) into one rectangle per pane, leaving a `gap` between neighbours for the
//! divider line.

use unicode_width::UnicodeWidthStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PaneId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn contains(&self, px: f64, py: f64) -> bool {
        px >= self.x as f64
            && px < self.right() as f64
            && py >= self.y as f64
            && py < self.bottom() as f64
    }
}

/// How a split divides its rectangle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitDir {
    /// Side by side: `a` on the left, `b` on the right.
    Horizontal,
    /// Stacked: `a` on top, `b` below.
    Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusDir {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Leaf(PaneId),
    Split {
        dir: SplitDir,
        /// Share of the space (without the divider) that goes to `a`.
        ratio: f32,
        a: Box<Node>,
        b: Box<Node>,
    },
}

/// Smallest and largest share a dragged divider can give to `a`.
pub(crate) const MIN_RATIO: f32 = 0.1;

impl Node {
    /// Panes in reading order (left/top first).
    pub fn leaves(&self) -> Vec<PaneId> {
        match self {
            Node::Leaf(id) => vec![*id],
            Node::Split { a, b, .. } => {
                let mut v = a.leaves();
                v.extend(b.leaves());
                v
            }
        }
    }

    pub fn contains(&self, id: PaneId) -> bool {
        match self {
            Node::Leaf(l) => *l == id,
            Node::Split { a, b, .. } => a.contains(id) || b.contains(id),
        }
    }

    /// Replace the leaf `target` by a split holding it and `new` (which goes right/below).
    /// Returns false when `target` is not in the tree.
    pub fn split(&mut self, target: PaneId, new: PaneId, dir: SplitDir) -> bool {
        match self {
            Node::Leaf(id) if *id == target => {
                *self = Node::Split {
                    dir,
                    ratio: 0.5,
                    a: Box::new(Node::Leaf(target)),
                    b: Box::new(Node::Leaf(new)),
                };
                true
            }
            Node::Leaf(_) => false,
            Node::Split { a, b, .. } => a.split(target, new, dir) || b.split(target, new, dir),
        }
    }

    /// Remove a pane; its sibling takes over the parent split's space. `None` when the tree
    /// held nothing else.
    pub fn remove(self, id: PaneId) -> Option<Node> {
        match self {
            Node::Leaf(l) if l == id => None,
            leaf @ Node::Leaf(_) => Some(leaf),
            Node::Split { dir, ratio, a, b } => match (a.remove(id), b.remove(id)) {
                (Some(a), Some(b)) => Some(Node::Split {
                    dir,
                    ratio,
                    a: Box::new(a),
                    b: Box::new(b),
                }),
                (a, b) => a.or(b),
            },
        }
    }
}

/// Split `rect` in two with `gap` pixels between them. Returns (a, b, divider).
fn split_rect(rect: Rect, dir: SplitDir, ratio: f32, gap: f32) -> (Rect, Rect, Rect) {
    match dir {
        SplitDir::Horizontal => {
            let avail = (rect.w - gap).max(0.0);
            let aw = (avail * ratio).round().clamp(0.0, avail);
            (
                Rect::new(rect.x, rect.y, aw, rect.h),
                Rect::new(rect.x + aw + gap, rect.y, avail - aw, rect.h),
                Rect::new(rect.x + aw, rect.y, gap, rect.h),
            )
        }
        SplitDir::Vertical => {
            let avail = (rect.h - gap).max(0.0);
            let ah = (avail * ratio).round().clamp(0.0, avail);
            (
                Rect::new(rect.x, rect.y, rect.w, ah),
                Rect::new(rect.x, rect.y + ah + gap, rect.w, avail - ah),
                Rect::new(rect.x, rect.y + ah, rect.w, gap),
            )
        }
    }
}

/// One rectangle per pane, covering `rect` minus the `gap`-wide dividers.
pub fn layout(node: &Node, rect: Rect, gap: f32) -> Vec<(PaneId, Rect)> {
    let mut out = Vec::new();
    collect_layout(node, rect, gap, &mut out);
    out
}

fn collect_layout(node: &Node, rect: Rect, gap: f32, out: &mut Vec<(PaneId, Rect)>) {
    match node {
        Node::Leaf(id) => out.push((*id, rect)),
        Node::Split { dir, ratio, a, b } => {
            let (ra, rb, _) = split_rect(rect, *dir, *ratio, gap);
            collect_layout(a, ra, gap, out);
            collect_layout(b, rb, gap, out);
        }
    }
}

/// A divider between two panes, with what is needed to draw and drag it.
#[derive(Clone, Debug, PartialEq)]
pub struct Divider {
    pub rect: Rect,
    pub dir: SplitDir,
    /// Route from the root to the split that owns it (`false` = `a`, `true` = `b`).
    pub path: Vec<bool>,
    /// Rectangle of that split, divider included.
    pub area: Rect,
}

pub fn dividers(node: &Node, rect: Rect, gap: f32) -> Vec<Divider> {
    let mut out = Vec::new();
    collect_dividers(node, rect, gap, &mut Vec::new(), &mut out);
    out
}

fn collect_dividers(
    node: &Node,
    rect: Rect,
    gap: f32,
    path: &mut Vec<bool>,
    out: &mut Vec<Divider>,
) {
    if let Node::Split { dir, ratio, a, b } = node {
        let (ra, rb, div) = split_rect(rect, *dir, *ratio, gap);
        out.push(Divider {
            rect: div,
            dir: *dir,
            path: path.clone(),
            area: rect,
        });
        path.push(false);
        collect_dividers(a, ra, gap, path, out);
        path.pop();
        path.push(true);
        collect_dividers(b, rb, gap, path, out);
        path.pop();
    }
}

/// Ratio that puts the divider of `d` under pointer coordinate `pos` (x for side-by-side
/// splits, y for stacked ones).
pub fn ratio_at(d: &Divider, pos: f64, gap: f32) -> f32 {
    let (start, len) = match d.dir {
        SplitDir::Horizontal => (d.area.x, d.area.w),
        SplitDir::Vertical => (d.area.y, d.area.h),
    };
    let avail = (len - gap).max(1.0);
    ((pos as f32 - gap / 2.0 - start) / avail).clamp(MIN_RATIO, 1.0 - MIN_RATIO)
}

/// Set the ratio of the split at `path`. Returns false when the path does not lead to a split.
pub fn set_ratio(node: &mut Node, path: &[bool], new_ratio: f32) -> bool {
    match (node, path.split_first()) {
        (Node::Split { ratio, .. }, None) => {
            *ratio = new_ratio.clamp(MIN_RATIO, 1.0 - MIN_RATIO);
            true
        }
        (Node::Split { a, b, .. }, Some((&to_b, rest))) => {
            set_ratio(if to_b { b } else { a }, rest, new_ratio)
        }
        _ => false,
    }
}

/// The pane next to `from` in direction `dir`: the nearest one that overlaps it on the other
/// axis, preferring the larger overlap on a tie.
pub fn neighbor(panes: &[(PaneId, Rect)], from: PaneId, dir: FocusDir) -> Option<PaneId> {
    let cur = panes.iter().find(|(id, _)| *id == from)?.1;
    let overlap = |a0: f32, a1: f32, b0: f32, b1: f32| a1.min(b1) - a0.max(b0);
    let mut best: Option<(PaneId, f32, f32)> = None;
    for &(id, r) in panes.iter().filter(|(id, _)| *id != from) {
        let (dist, over) = match dir {
            FocusDir::Left => (
                cur.x - r.right(),
                overlap(cur.y, cur.bottom(), r.y, r.bottom()),
            ),
            FocusDir::Right => (
                r.x - cur.right(),
                overlap(cur.y, cur.bottom(), r.y, r.bottom()),
            ),
            FocusDir::Up => (
                cur.y - r.bottom(),
                overlap(cur.x, cur.right(), r.x, r.right()),
            ),
            FocusDir::Down => (
                r.y - cur.bottom(),
                overlap(cur.x, cur.right(), r.x, r.right()),
            ),
        };
        if dist < -0.5 || over <= 0.0 {
            continue;
        }
        let better = match best {
            None => true,
            Some((_, d, o)) => dist < d - 0.5 || ((dist - d).abs() <= 0.5 && over > o),
        };
        if better {
            best = Some((id, dist, over));
        }
    }
    best.map(|(id, _, _)| id)
}

/// Rectangles of `count` tabs laid out from the left of a bar `width` pixels wide; each tab is at
/// most `max_tab` wide and they shrink evenly to fit.
pub fn tab_rects(count: usize, width: f32, height: f32, max_tab: f32) -> Vec<Rect> {
    if count == 0 {
        return Vec::new();
    }
    let tab_w = (width / count as f32).floor().min(max_tab).max(1.0);
    (0..count)
        .map(|i| Rect::new(i as f32 * tab_w, 0.0, tab_w, height))
        .collect()
}

/// Shorten `title` to at most `max_cols` terminal columns, ending with an ellipsis when cut.
pub fn truncate_title(title: &str, max_cols: usize) -> String {
    if title.width() <= max_cols {
        return title.to_owned();
    }
    if max_cols == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in title.chars() {
        let w = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if used + w > max_cols - 1 {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    out
}
