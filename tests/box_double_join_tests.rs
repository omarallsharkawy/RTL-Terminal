//! Double-line box drawing: crossings keep their channels open and single lines join both
//! double verticals.
use twitty::quad::try_render_box_or_block;

type Quad = (f32, f32, f32, f32, [f32; 4]);

// Cell origin (10, 20), 9 x 22 px: stroke 1px, double lines at x 13|15, y 30|32.
const X: f32 = 10.0;
const Y: f32 = 20.0;
const W: f32 = 9.0;
const H: f32 = 22.0;

fn geometry(c: char) -> Vec<Quad> {
    let mut quads = Vec::new();
    assert!(try_render_box_or_block(c, X, Y, W, H, [1.0; 4], &mut quads));
    quads
}

fn covered(quads: &[Quad], px: f32, py: f32) -> bool {
    quads
        .iter()
        .any(|&(x, y, w, h, _)| px >= x && px < x + w && py >= y && py < y + h)
}

#[test]
fn double_cross_leaves_both_channels_open() {
    let q = geometry('╬');
    // Inner channels between the double lines stay empty, like four ╝╚╗╔ corners.
    assert!(
        !covered(&q, 14.5, 30.5),
        "vertical channel crossed by a line"
    );
    assert!(
        !covered(&q, 14.5, 32.5),
        "vertical channel crossed by a line"
    );
    assert!(
        !covered(&q, 13.5, 31.5),
        "horizontal channel crossed by a line"
    );
    assert!(
        !covered(&q, 15.5, 31.5),
        "horizontal channel crossed by a line"
    );
    // Every arm still reaches its edge.
    for (px, py) in [(10.5, 30.5), (10.5, 32.5), (18.5, 30.5), (18.5, 32.5)] {
        assert!(covered(&q, px, py), "horizontal arm missing at {px},{py}");
    }
    for (px, py) in [(13.5, 20.5), (15.5, 20.5), (13.5, 41.5), (15.5, 41.5)] {
        assert!(covered(&q, px, py), "vertical arm missing at {px},{py}");
    }
}

#[test]
fn single_line_joins_both_double_verticals() {
    // ╓╖ start their double verticals at the single line, ╙╜ end them there: the single
    // line must reach both verticals or one of them has a loose end.
    for c in ['╓', '╖', '╙', '╜'] {
        let q = geometry(c);
        assert!(
            covered(&q, 14.5, 31.5),
            "{c}: single line stops short of a vertical"
        );
    }
}
