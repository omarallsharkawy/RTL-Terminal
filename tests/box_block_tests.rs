use twitty::quad::try_render_box_or_block;

type Quad = (f32, f32, f32, f32, [f32; 4]);

// Cell used by the geometry tests: origin (10, 20), 9 x 22 px.
const X: f32 = 10.0;
const Y: f32 = 20.0;
const W: f32 = 9.0;
const H: f32 = 22.0;
const COLOR: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const EPS: f32 = 1e-3;

fn render(c: char) -> Option<Vec<Quad>> {
    let mut quads = Vec::new();
    if try_render_box_or_block(c, X, Y, W, H, COLOR, &mut quads) {
        Some(quads)
    } else {
        None
    }
}

fn approx(a: f32, b: f32) -> bool {
    (a - b).abs() < EPS
}

const NEW_CHARS: &[char] = &[
    // Double lines and mixed single/double joins
    '═', '║', '╔', '╗', '╚', '╝', '╠', '╣', '╦', '╩', '╬', '╒', '╓', '╕', '╖', '╘', '╙', '╛', '╜',
    '╞', '╟', '╡', '╢', '╤', '╥', '╧', '╨', '╪', '╫', // Heavy lines
    '━', '┃', '┏', '┓', '┗', '┛', '┣', '┫', '┳', '┻', '╋', // Dashed lines
    '┄', '┅', '┆', '┇', '┈', '┉', '┊', '┋', '╌', '╍', '╎', '╏', // Rounded corners
    '╭', '╮', '╯', '╰', // Block elements
    '▀', '▄', '█', '▌', '▐', '▁', '▂', '▃', '▅', '▆', '▇', '▉', '▊', '▋', '▍', '▎', '▏', '▔', '▕',
    '▖', '▗', '▘', '▙', '▚', '▛', '▜', '▝', '▞', '▟', // Shades
    '░', '▒', '▓',
];

#[test]
fn test_box_and_block_rendering() {
    let mut quads = Vec::new();
    let color = [1.0, 1.0, 1.0, 1.0];

    // 1. Full block
    let is_block = try_render_box_or_block('█', 10.0, 20.0, 9.0, 22.0, color, &mut quads);
    assert!(is_block);
    assert_eq!(quads.len(), 1);
    assert_eq!(quads[0], (10.0, 20.0, 9.0, 22.0, color));

    // 2. Vertical line (box drawing)
    quads.clear();
    let is_vline = try_render_box_or_block('│', 10.0, 20.0, 9.0, 22.0, color, &mut quads);
    assert!(is_vline);
    assert_eq!(quads.len(), 1);

    // 3. Normal letters should not be intercepted as box/block
    quads.clear();
    let is_letter = try_render_box_or_block('A', 10.0, 20.0, 9.0, 22.0, color, &mut quads);
    assert!(!is_letter);
    assert!(quads.is_empty());
}

#[test]
fn test_new_chars_are_handled_and_stay_in_cell() {
    for &c in NEW_CHARS {
        let quads = render(c).unwrap_or_else(|| panic!("{c} should be handled"));
        assert!(!quads.is_empty(), "{c} produced no quads");
        for &(qx, qy, qw, qh, _) in &quads {
            assert!(qw > 0.0 && qh > 0.0, "{c} produced an empty quad");
            assert!(
                qx >= X - EPS && qx + qw <= X + W + EPS,
                "{c} leaves the cell horizontally"
            );
            assert!(
                qy >= Y - EPS && qy + qh <= Y + H + EPS,
                "{c} leaves the cell vertically"
            );
        }
    }
}

#[test]
fn test_double_horizontal_line() {
    let quads = render('═').unwrap();
    assert_eq!(quads.len(), 2);
    for &(qx, _, qw, qh, _) in &quads {
        assert!(approx(qx, X));
        assert!(approx(qw, W), "double line should span the full width");
        assert!(qh < qw);
    }
    // Two light lines with one light-thickness gap between them.
    let t = quads[0].3;
    assert!(approx(quads[1].1 - (quads[0].1 + t), t));
}

#[test]
fn test_double_vertical_line() {
    let quads = render('║').unwrap();
    assert_eq!(quads.len(), 2);
    for &(_, qy, _, qh, _) in &quads {
        assert!(approx(qy, Y));
        assert!(approx(qh, H), "double line should span the full height");
    }
}

#[test]
fn test_lower_eighth_block_height() {
    let quads = render('▁').unwrap();
    assert_eq!(quads.len(), 1);
    let (_, qy, _, qh, _) = quads[0];
    assert!(approx(qh, H / 8.0));
    assert!(
        approx(qy + qh, Y + H),
        "lower eighth should sit on the bottom edge"
    );
}

#[test]
fn test_shade_alpha_order() {
    let alpha = |c| render(c).unwrap()[0].4[3];
    let light = alpha('░');
    let medium = alpha('▒');
    let dark = alpha('▓');
    assert!(light < medium, "░ should be lighter than ▒");
    assert!(medium < dark, "▒ should be lighter than ▓");
    assert!(dark < 1.0);
    // Each shade covers the whole cell.
    assert_eq!(render('▒').unwrap()[0].0, X);
}

#[test]
fn test_cross_connects_all_four_edges() {
    let quads = render('╬').unwrap();
    let touches = |f: &dyn Fn(&Quad) -> bool| quads.iter().any(f);
    assert!(
        touches(&|q| approx(q.0, X)),
        "no quad reaches the left edge"
    );
    assert!(
        touches(&|q| approx(q.0 + q.2, X + W)),
        "no quad reaches the right edge"
    );
    assert!(touches(&|q| approx(q.1, Y)), "no quad reaches the top edge");
    assert!(
        touches(&|q| approx(q.1 + q.3, Y + H)),
        "no quad reaches the bottom edge"
    );
}

#[test]
fn test_heavy_is_twice_light() {
    let light = render('─').unwrap()[0].3;
    let heavy = render('━').unwrap()[0].3;
    assert!(approx(heavy, 2.0 * light));
    assert!(
        light >= 1.0 && light.fract().abs() < EPS,
        "light stroke must be whole pixels"
    );

    let light_v = render('│').unwrap()[0].2;
    let heavy_v = render('┃').unwrap()[0].2;
    assert!(approx(heavy_v, 2.0 * light_v));
}

#[test]
fn test_dash_segment_counts() {
    assert_eq!(render('╌').unwrap().len(), 2);
    assert_eq!(render('┄').unwrap().len(), 3);
    assert_eq!(render('┈').unwrap().len(), 4);
    assert_eq!(render('╎').unwrap().len(), 2);
    assert_eq!(render('┆').unwrap().len(), 3);
    assert_eq!(render('┊').unwrap().len(), 4);
}

#[test]
fn test_unsupported_chars_fall_back_to_font() {
    for c in ['A', 'z', '╱', '╳'] {
        assert!(render(c).is_none(), "{c} should fall back to the font");
    }
}
