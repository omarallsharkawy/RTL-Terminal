use twitty::layout::{
    dividers, layout, neighbor, ratio_at, set_ratio, tab_rects, truncate_title, FocusDir, Node,
    PaneId, Rect, SplitDir,
};

const GAP: f32 = 1.0;
const AREA: Rect = Rect {
    x: 0.0,
    y: 20.0,
    w: 801.0,
    h: 601.0,
};

fn leaf(n: u64) -> Node {
    Node::Leaf(PaneId(n))
}

fn split(dir: SplitDir, ratio: f32, a: Node, b: Node) -> Node {
    Node::Split {
        dir,
        ratio,
        a: Box::new(a),
        b: Box::new(b),
    }
}

fn rect_of(panes: &[(PaneId, Rect)], n: u64) -> Rect {
    panes.iter().find(|(id, _)| *id == PaneId(n)).unwrap().1
}

fn overlaps(a: Rect, b: Rect) -> bool {
    a.x < b.right() && b.x < a.right() && a.y < b.bottom() && b.y < a.bottom()
}

/// Area covered by the panes plus the dividers.
fn covered_area(node: &Node) -> f32 {
    let panes: f32 = layout(node, AREA, GAP).iter().map(|(_, r)| r.w * r.h).sum();
    let gaps: f32 = dividers(node, AREA, GAP)
        .iter()
        .map(|d| d.rect.w * d.rect.h)
        .sum();
    panes + gaps
}

#[test]
fn single_leaf_fills_the_area() {
    let panes = layout(&leaf(1), AREA, GAP);
    assert_eq!(panes, vec![(PaneId(1), AREA)]);
    assert!(dividers(&leaf(1), AREA, GAP).is_empty());
}

#[test]
fn horizontal_split_puts_a_left_and_b_right_with_a_gap() {
    let tree = split(SplitDir::Horizontal, 0.5, leaf(1), leaf(2));
    let panes = layout(&tree, AREA, GAP);
    let (a, b) = (rect_of(&panes, 1), rect_of(&panes, 2));
    assert_eq!(a.x, AREA.x);
    assert_eq!(b.right(), AREA.right());
    assert_eq!(b.x - a.right(), GAP);
    assert_eq!((a.y, a.h, b.y, b.h), (AREA.y, AREA.h, AREA.y, AREA.h));
    assert_eq!(a.w + GAP + b.w, AREA.w);
}

#[test]
fn vertical_split_stacks_a_over_b() {
    let tree = split(SplitDir::Vertical, 0.5, leaf(1), leaf(2));
    let panes = layout(&tree, AREA, GAP);
    let (a, b) = (rect_of(&panes, 1), rect_of(&panes, 2));
    assert_eq!(b.y - a.bottom(), GAP);
    assert_eq!(b.bottom(), AREA.bottom());
    assert_eq!(a.h + GAP + b.h, AREA.h);
}

#[test]
fn ratio_is_respected() {
    let tree = split(SplitDir::Horizontal, 0.25, leaf(1), leaf(2));
    let panes = layout(&tree, AREA, GAP);
    let a = rect_of(&panes, 1);
    let share = a.w / (AREA.w - GAP);
    assert!((share - 0.25).abs() < 0.01, "share {share}");
}

#[test]
fn nested_splits_do_not_overlap_and_cover_the_area() {
    let tree = split(
        SplitDir::Horizontal,
        0.4,
        leaf(1),
        split(
            SplitDir::Vertical,
            0.3,
            leaf(2),
            split(SplitDir::Horizontal, 0.6, leaf(3), leaf(4)),
        ),
    );
    let panes = layout(&tree, AREA, GAP);
    assert_eq!(panes.len(), 4);
    for (i, (_, a)) in panes.iter().enumerate() {
        assert!(a.x >= AREA.x && a.y >= AREA.y);
        assert!(a.right() <= AREA.right() && a.bottom() <= AREA.bottom());
        for (_, b) in &panes[i + 1..] {
            assert!(!overlaps(*a, *b), "{a:?} overlaps {b:?}");
        }
    }
    assert_eq!(covered_area(&tree), AREA.w * AREA.h);
    assert_eq!(dividers(&tree, AREA, GAP).len(), 3);
}

#[test]
fn dividers_sit_between_their_panes() {
    let tree = split(SplitDir::Horizontal, 0.5, leaf(1), leaf(2));
    let panes = layout(&tree, AREA, GAP);
    let d = &dividers(&tree, AREA, GAP)[0];
    assert_eq!(d.rect.x, rect_of(&panes, 1).right());
    assert_eq!(d.rect.right(), rect_of(&panes, 2).x);
    assert_eq!((d.rect.y, d.rect.h), (AREA.y, AREA.h));
    assert!(d.path.is_empty());
}

#[test]
fn dragging_a_divider_changes_only_its_own_split() {
    let mut tree = split(
        SplitDir::Horizontal,
        0.5,
        leaf(1),
        split(SplitDir::Vertical, 0.5, leaf(2), leaf(3)),
    );
    let before = rect_of(&layout(&tree, AREA, GAP), 1);
    let inner = dividers(&tree, AREA, GAP)
        .into_iter()
        .find(|d| d.dir == SplitDir::Vertical)
        .unwrap();
    assert_eq!(inner.path, vec![true]);
    let y = (inner.area.y + inner.area.h * 0.75) as f64;
    assert!(set_ratio(&mut tree, &inner.path, ratio_at(&inner, y, GAP)));
    let panes = layout(&tree, AREA, GAP);
    let (top, bottom) = (rect_of(&panes, 2), rect_of(&panes, 3));
    assert!(top.h > bottom.h * 2.5, "{top:?} {bottom:?}");
    assert_eq!(rect_of(&panes, 1), before);
}

#[test]
fn ratio_is_clamped_so_a_pane_never_vanishes() {
    let mut tree = split(SplitDir::Horizontal, 0.5, leaf(1), leaf(2));
    let d = dividers(&tree, AREA, GAP).remove(0);
    set_ratio(&mut tree, &d.path, ratio_at(&d, -500.0, GAP));
    assert!(rect_of(&layout(&tree, AREA, GAP), 1).w > 0.0);
    set_ratio(&mut tree, &d.path, ratio_at(&d, 5000.0, GAP));
    assert!(rect_of(&layout(&tree, AREA, GAP), 2).w > 0.0);
}

#[test]
fn split_replaces_the_leaf() {
    let mut tree = leaf(1);
    assert!(tree.split(PaneId(1), PaneId(2), SplitDir::Horizontal));
    assert!(tree.split(PaneId(2), PaneId(3), SplitDir::Vertical));
    assert!(!tree.split(PaneId(9), PaneId(4), SplitDir::Vertical));
    assert_eq!(tree.leaves(), vec![PaneId(1), PaneId(2), PaneId(3)]);
}

#[test]
fn removing_a_leaf_collapses_its_parent() {
    let tree = split(
        SplitDir::Horizontal,
        0.5,
        leaf(1),
        split(SplitDir::Vertical, 0.5, leaf(2), leaf(3)),
    );
    let tree = tree.remove(PaneId(2)).unwrap();
    assert_eq!(tree, split(SplitDir::Horizontal, 0.5, leaf(1), leaf(3)));
    let tree = tree.remove(PaneId(1)).unwrap();
    assert_eq!(tree, leaf(3));
    assert_eq!(tree.remove(PaneId(3)), None);
}

#[test]
fn removing_an_unknown_pane_changes_nothing() {
    let tree = split(SplitDir::Horizontal, 0.5, leaf(1), leaf(2));
    assert_eq!(tree.clone().remove(PaneId(7)), Some(tree));
}

/// Two columns of two panes: 1 over 3 on the left, 2 over 4 on the right.
fn grid() -> Vec<(PaneId, Rect)> {
    let tree = split(
        SplitDir::Horizontal,
        0.5,
        split(SplitDir::Vertical, 0.5, leaf(1), leaf(3)),
        split(SplitDir::Vertical, 0.5, leaf(2), leaf(4)),
    );
    layout(&tree, AREA, GAP)
}

#[test]
fn neighbours_follow_geometry() {
    let panes = grid();
    assert_eq!(
        neighbor(&panes, PaneId(1), FocusDir::Right),
        Some(PaneId(2))
    );
    assert_eq!(neighbor(&panes, PaneId(1), FocusDir::Down), Some(PaneId(3)));
    assert_eq!(neighbor(&panes, PaneId(4), FocusDir::Left), Some(PaneId(3)));
    assert_eq!(neighbor(&panes, PaneId(4), FocusDir::Up), Some(PaneId(2)));
    assert_eq!(neighbor(&panes, PaneId(1), FocusDir::Left), None);
    assert_eq!(neighbor(&panes, PaneId(1), FocusDir::Up), None);
    assert_eq!(neighbor(&panes, PaneId(4), FocusDir::Right), None);
    assert_eq!(neighbor(&panes, PaneId(4), FocusDir::Down), None);
}

#[test]
fn neighbour_prefers_the_pane_with_the_larger_overlap() {
    // One tall pane on the left; a small pane over a big one on the right.
    let tree = split(
        SplitDir::Horizontal,
        0.5,
        leaf(1),
        split(SplitDir::Vertical, 0.2, leaf(2), leaf(3)),
    );
    let panes = layout(&tree, AREA, GAP);
    assert_eq!(
        neighbor(&panes, PaneId(1), FocusDir::Right),
        Some(PaneId(3))
    );
    assert_eq!(neighbor(&panes, PaneId(2), FocusDir::Left), Some(PaneId(1)));
    assert_eq!(neighbor(&panes, PaneId(3), FocusDir::Up), Some(PaneId(2)));
}

#[test]
fn neighbour_ignores_panes_that_only_touch_at_a_corner() {
    let panes = grid();
    assert_eq!(neighbor(&panes, PaneId(3), FocusDir::Up), Some(PaneId(1)));
    assert_eq!(
        neighbor(&panes, PaneId(3), FocusDir::Right),
        Some(PaneId(4))
    );
}

#[test]
fn tabs_share_the_bar_up_to_a_maximum_width() {
    let tabs = tab_rects(4, 800.0, 20.0, 300.0);
    assert_eq!(tabs.len(), 4);
    assert!(tabs.iter().all(|t| t.w == 200.0 && t.h == 20.0));
    assert_eq!(tabs[3].x, 600.0);

    let tabs = tab_rects(2, 800.0, 20.0, 300.0);
    assert!(tabs.iter().all(|t| t.w == 300.0));
    assert!(tab_rects(0, 800.0, 20.0, 300.0).is_empty());
}

#[test]
fn long_titles_end_with_an_ellipsis() {
    assert_eq!(truncate_title("short", 10), "short");
    assert_eq!(truncate_title("a long title here", 8), "a long …");
    assert_eq!(truncate_title("abc", 0), "");
    // Arabic letters are one column wide each.
    assert_eq!(truncate_title("مرحبا بالعالم", 6), "مرحبا…");
    // Wide characters count as two columns.
    assert_eq!(truncate_title("日本語のタイトル", 5), "日本…");
}
