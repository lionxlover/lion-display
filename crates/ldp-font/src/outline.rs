//! The Lion Sans outline table (Phase 54): the compositor's own
//! typeface — 96 glyphs of a utilitarian geometric sans authored
//! directly in em units (1000/em), plus the notdef box every
//! uncovered codepoint draws honestly.
//!
//! The design grid: cap height 720, x-height 500, ascender 800,
//! descender −200, stems 84, horizontal strokes 72. The look is a
//! working title-bar face — round bowls as true ellipse arcs,
//! straight stems, single-story geometric forms — legible at 16 px
//! where a title bar wears it, unapologetic at display sizes.
//!
//! Authoring doctrine (enforced by the rasterizer's sign convention
//! and this module's tests):
//!
//! * Every outer contour is authored **counter-clockwise** in the
//!   y-up font space (the TrueType convention); the rasterizer's
//!   reflection reads them positive inside.
//! * Holes are authored counter-clockwise too, then wrapped in
//!   [`Contour::hole`] — which reverses them — so a hole can never
//!   be mis-directed by hand. The ring helpers pair an outer with
//!   its hole in one call.
//! * Overlapping same-winding contours merge under the nonzero rule
//!   (both +1 clamp to one ink) — the letter-construction trick the
//!   bowl letters lean on: a stem plus a ring whose *hole* clears
//!   the stem's territory is one connected glyph with two contours.
//! * Every point is an integer; every arc is a run of on-curve
//!   octant points with off-curve controls between (≤ 24° per
//!   segment, mid-angle controls at the tangent intersection).

/// The cap height (units above the baseline).
pub(crate) const CAP: i32 = 720;
/// The x-height (units above the baseline).
pub(crate) const X_HEIGHT: i32 = 500;
/// The ascender (the line box's top; units above the baseline).
pub(crate) const ASCENT: i32 = 800;
/// The descender (the line box's bottom; units below the baseline,
/// negative).
pub(crate) const DESCENT: i32 = -200;
/// The vertical stroke width.
pub(crate) const STEM: i32 = 84;

/// One contour point: integer em coordinates plus the on-curve flag
/// (off-curve points are quadratic Bézier controls).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Point {
    /// The x (em units).
    pub(crate) x: i32,
    /// The y (em units).
    pub(crate) y: i32,
    /// Whether the point sits on the curve.
    pub(crate) on: bool,
}

/// One contour: a closed point ring starting and ending logically at
/// the first point.
#[derive(Clone, Debug)]
pub(crate) struct Contour {
    /// The ring's points (the first is on-curve by table doctrine).
    pub(crate) points: Vec<Point>,
}

impl Contour {
    /// Build from `(x, y, on)` tuples.
    pub(crate) fn from_points(pts: Vec<(i32, i32, bool)>) -> Contour {
        Contour {
            points: pts
                .into_iter()
                .map(|(x, y, on)| Point { x, y, on })
                .collect(),
        }
    }

    /// Build a hole: the same ring reversed, so a counter-clockwise
    /// authored ring becomes the clockwise hole the nonzero rule
    /// subtracts.
    pub(crate) fn hole(pts: Vec<(i32, i32, bool)>) -> Contour {
        let mut pts = pts;
        pts.reverse();
        Contour::from_points(pts)
    }
}

/// One glyph: its advance width and its contours.
#[derive(Clone, Debug)]
pub(crate) struct Glyph {
    /// The advance (em units).
    pub(crate) advance: i32,
    /// The contours.
    pub(crate) contours: Vec<Contour>,
}

/// The notdef box: the honest answer to every codepoint the face
/// does not cover — a hollow rectangle, visible and truthful, never
/// a crash and never silent garbage.
pub(crate) fn notdef() -> Glyph {
    Glyph {
        advance: 600,
        contours: vec![
            Contour::from_points(vec![
                (60, -80, true),
                (540, -80, true),
                (540, 640, true),
                (60, 640, true),
            ]),
            Contour::hole(vec![
                (140, 0, true),
                (460, 0, true),
                (460, 560, true),
                (140, 560, true),
            ]),
        ],
    }
}

/// The glyph for one covered codepoint, `None` for the rest.
///
/// Coverage: ASCII 0x20–0x7E (the identity map — a glyph's id *is*
/// its codepoint), U+2026 (the ellipsis), and U+00A0 (mapped to the
/// space — it *is* one). Everything else draws the notdef box; the
/// caller decides the mapping's honesty (blank for control
/// characters, notdef for the uncovered).
/// The space glyph: an advance with no ink (the blank's own shape).
const SPACE: Glyph = Glyph {
    advance: 260,
    contours: Vec::new(),
};

/// The glyph table's walk: one match arm per covered codepoint (the
/// data IS the table — the lint's line budget steps aside for a
/// typeface's alphabet, same as the protocol tables' doc lines).
#[allow(clippy::too_many_lines)]
pub(crate) fn lookup(c: char) -> Option<Glyph> {
    let g = match c {
        ' ' => SPACE,
        '!' => Glyph {
            advance: 300,
            contours: vec![stem_pts(108, 0, 500), dot(150, 60)],
        },
        '"' => quotes(),
        '#' => hash(),
        '$' => dollar(),
        '%' => percent(),
        '&' => ampersand(),
        '\'' => Glyph {
            advance: 240,
            contours: vec![stem_pts(78, 400, 720)],
        },
        '(' => bracket(90, 300, true),
        ')' => bracket(90, 300, false),
        '*' => star(),
        '+' => plus(),
        ',' => comma(),
        '-' => Glyph {
            advance: 400,
            contours: vec![bar(40, 244, 360, 316)],
        },
        '.' => Glyph {
            advance: 300,
            contours: vec![dot(150, 60)],
        },
        '/' => slash(),
        '0' => digit_zero(),
        '1' => digit_one(),
        '2' => digit_two(),
        '3' => digit_three(),
        '4' => digit_four(),
        '5' => digit_five(),
        '6' => digit_six(),
        '7' => digit_seven(),
        '8' => digit_eight(),
        '9' => digit_nine(),
        ':' => Glyph {
            advance: 300,
            contours: vec![dot(150, 60), dot(150, 430)],
        },
        ';' => semicolon(),
        '<' => chevron(true),
        '=' => Glyph {
            advance: 620,
            contours: vec![bar(40, 200, 580, 272), bar(40, 380, 580, 452)],
        },
        '>' => chevron(false),
        '?' => question(),
        '@' => at(),
        'A' => letter_a(),
        'B' => letter_b(),
        'C' => letter_c(),
        'D' => letter_d(),
        'E' => letter_e(),
        'F' => letter_f(),
        'G' => letter_g(),
        'H' => letter_h(),
        'I' => Glyph {
            advance: 300,
            contours: vec![stem_pts(108, 0, 720)],
        },
        'J' => letter_j(),
        'K' => letter_k(),
        'L' => letter_l(),
        'M' => letter_m(),
        'N' => letter_n(),
        'O' => letter_o(),
        'P' => letter_p(),
        'Q' => letter_q(),
        'R' => letter_r(),
        'S' => letter_s(),
        'T' => letter_t(),
        'U' => letter_u(),
        'V' => letter_v(),
        'W' => letter_w(),
        'X' => letter_x(),
        'Y' => letter_y(),
        'Z' => letter_z(),
        '[' => square_bracket(true),
        '\\' => slash_back(),
        ']' => square_bracket(false),
        '^' => caret(),
        '_' => Glyph {
            advance: 600,
            contours: vec![bar(0, -160, 600, -88)],
        },
        '`' => Glyph {
            advance: 340,
            contours: vec![stem_pts(120, 560, 780)],
        },
        'a' => lower_a(),
        'b' => lower_b(),
        'c' => lower_c(),
        'd' => lower_d(),
        'e' => lower_e(),
        'f' => lower_f(),
        'g' => lower_g(),
        'h' => lower_h(),
        'i' => Glyph {
            advance: 280,
            contours: vec![stem_pts(98, 0, 500), dot(140, 720)],
        },
        'j' => lower_j(),
        'k' => lower_k(),
        'l' => Glyph {
            advance: 280,
            contours: vec![stem_pts(98, 0, 720)],
        },
        'm' => lower_m(),
        'n' => lower_n(),
        'o' => lower_o(),
        'p' => lower_p(),
        'q' => lower_q(),
        'r' => lower_r(),
        's' => lower_s(),
        't' => lower_t(),
        'u' => lower_u(),
        'v' => lower_v(),
        'w' => lower_w(),
        'x' => lower_x(),
        'y' => lower_y(),
        'z' => lower_z(),
        '{' => brace(true),
        '|' => Glyph {
            advance: 300,
            contours: vec![stem_pts(108, -200, 800)],
        },
        '}' => brace(false),
        '~' => tilde(),
        '\u{2026}' => ellipsis(),
        '\u{a0}' => Glyph {
            advance: 260,
            contours: vec![],
        },
        _ => return None,
    };
    Some(g)
}

// ————— construction helpers —————

/// A vertical stem: the rectangle `x .. x+STEM` from `y0` to `y1`
/// (authored CCW: right along the bottom, up the right, left along
/// the top, down the left).
fn stem_pts(x: i32, y0: i32, y1: i32) -> Contour {
    Contour::from_points(vec![
        (x, y0, true),
        (x + STEM, y0, true),
        (x + STEM, y1, true),
        (x, y1, true),
    ])
}

/// A horizontal bar: `x0 .. x1` between `y` and `y + THIN`.
fn bar(x0: i32, y: i32, x1: i32, y1: i32) -> Contour {
    Contour::from_points(vec![
        (x0, y, true),
        (x1, y, true),
        (x1, y1, true),
        (x0, y1, true),
    ])
}

/// A filled circle (the dots of period, colon, exclamation).
fn dot(cx: i32, cy: i32) -> Contour {
    Contour::from_points(arc_run(cx, cy, 60, 60, 0, 360))
}

/// One ellipse ring (the bowl letters): an outer counter-clockwise
/// contour plus, when the inner radii are positive, the reversed
/// hole. `hx`/`hy` inner radii of `0` mean solid.
fn ring(cx: i32, cy: i32, rx: i32, ry: i32, hx: i32, hy: i32) -> Vec<Contour> {
    let outer = Contour::from_points(arc_run(cx, cy, rx, ry, 0, 360));
    if hx > 0 && hy > 0 {
        let mut hole_pts = arc_run(cx, cy, hx, hy, 0, 360);
        hole_pts.reverse();
        vec![outer, Contour::from_points(hole_pts)]
    } else {
        vec![outer]
    }
}

/// One arch band (the M/N shoulders): the half-annulus from the
/// left springing over the top to the right — outer arc CCW, inner
/// arc back, the end cuts vertical. The ends sit *inside* the
/// flanking stems so the clamp doctrine merges them seamlessly.
fn arch_band(cx: i32, cy: i32, rx: i32, ry: i32, hx: i32, hy: i32) -> Contour {
    let mut pts = arc_run(cx, cy, rx, ry, 0, 180);
    pts.extend(arc_run(cx, cy, hx, hy, 180, 0));
    Contour::from_points(pts)
}

/// The point run along an ellipse arc from `a0` to `a1` (degrees,
/// y-up math convention: 0° = +x, 90° = +y). The run goes
/// counter-clockwise (increasing angle) when `a1 > a0`, clockwise
/// when `a1 < a0`; the sweep takes ≤ 24° segments with on-curve
/// points at the segment ends and off-curve controls at the
/// mid-angles (pushed out to the tangent intersection by
/// `1 / cos(Δ/2)`).
fn arc_run(cx: i32, cy: i32, rx: i32, ry: i32, a0: i32, a1: i32) -> Vec<(i32, i32, bool)> {
    let sweep = (a1 - a0).abs();
    let cw = a1 < a0;
    let steps = ((sweep + 23) / 24).max(1);
    let mut pts: Vec<(i32, i32, bool)> = Vec::new();
    for k in 0..=steps {
        let a = a0
            + if cw {
                -(k * sweep / steps)
            } else {
                k * sweep / steps
            };
        let (c, s) = (
            f64::from(a).to_radians().cos(),
            f64::from(a).to_radians().sin(),
        );
        pts.push((
            cx + (f64::from(rx) * c).round() as i32,
            cy + (f64::from(ry) * s).round() as i32,
            true,
        ));
        if k < steps {
            // The control at the mid-angle, out at the tangent
            // intersection.
            let am = a + if cw {
                -(sweep / (2 * steps))
            } else {
                sweep / (2 * steps)
            };
            let (cm, sm) = (
                f64::from(am).to_radians().cos(),
                f64::from(am).to_radians().sin(),
            );
            let grow = 1.0 / (f64::from(sweep / steps) / 2.0).to_radians().cos();
            pts.push((
                cx + (f64::from(rx) * grow * cm).round() as i32,
                cy + (f64::from(ry) * grow * sm).round() as i32,
                false,
            ));
        }
    }
    pts
}

/// A parallelogram stroke from the line `a → b` extended by the
/// vertical thickness `t` (diagonal strokes — the legs of A, K, R,
/// V, Y and the digits' spines), authored CCW.
fn stroke_v(a: (i32, i32), b: (i32, i32), t: i32) -> Contour {
    let (ax, ay) = a;
    let (bx, by) = b;
    // The upper edge a→b, the lower edge (a − t)→(b − t): walk
    // lower-left → upper-left → upper-right → lower-right.
    Contour::from_points(vec![
        (ax, ay - t, true),
        (ax, ay, true),
        (bx, by, true),
        (bx, by - t, true),
    ])
}

// ————— uppercase —————

fn letter_a() -> Glyph {
    // The flat-apex A: two 80-unit-wide legs, a crossbar at the
    // waist, the triangular counter above it. The outer walks
    // counter-clockwise (the orientation doctrine).
    Glyph {
        advance: 740,
        contours: vec![
            Contour::from_points(vec![
                (80, 0, true),
                (150, 148, true),
                (590, 148, true),
                (660, 0, true),
                (740, 0, true),
                (420, 720, true),
                (340, 720, true),
                (0, 0, true),
            ]),
            Contour::hole(vec![(185, 222, true), (555, 222, true), (370, 614, true)]),
        ],
    }
}

fn letter_b() -> Glyph {
    // The overlap-clamp doctrine's showcase: the stem, then two
    // rings whose holes clear the stem — one glyph, three contours.
    let mut contours = vec![stem_pts(0, 0, 720)];
    contours.extend(ring(260, 540, 230, 180, 146, 108));
    contours.extend(ring(260, 180, 250, 180, 160, 108));
    Glyph {
        advance: 560,
        contours,
    }
}

fn letter_c() -> Glyph {
    // The open ring: outer arc CCW 40°→320°, inner arc back, butt
    // terminals at the mouth's edge.
    let mut pts = arc_run(370, 360, 330, 360, 40, 320);
    pts.extend(arc_run(370, 360, 240, 270, 320, 40));
    Glyph {
        advance: 740,
        contours: vec![Contour::from_points(pts)],
    }
}

fn letter_d() -> Glyph {
    let mut contours = vec![stem_pts(0, 0, 720)];
    contours.extend(ring(354, 360, 340, 360, 250, 270));
    Glyph {
        advance: 740,
        contours,
    }
}

fn letter_e() -> Glyph {
    Glyph {
        advance: 640,
        contours: vec![Contour::from_points(vec![
            (0, 0, true),
            (560, 0, true),
            (560, 72, true),
            (84, 72, true),
            (84, 284, true),
            (480, 284, true),
            (480, 356, true),
            (84, 356, true),
            (84, 648, true),
            (560, 648, true),
            (560, 720, true),
            (0, 720, true),
        ])],
    }
}

fn letter_f() -> Glyph {
    Glyph {
        advance: 600,
        contours: vec![Contour::from_points(vec![
            (0, 0, true),
            (84, 0, true),
            (84, 284, true),
            (440, 284, true),
            (440, 356, true),
            (84, 356, true),
            (84, 648, true),
            (520, 648, true),
            (520, 720, true),
            (0, 720, true),
        ])],
    }
}

fn letter_g() -> Glyph {
    // The C's open ring plus the spur bar riding into the mouth.
    let mut pts = arc_run(370, 360, 330, 360, 40, 320);
    pts.extend(arc_run(370, 360, 240, 270, 320, 40));
    Glyph {
        advance: 760,
        contours: vec![Contour::from_points(pts), bar(560, 322, 740, 394)],
    }
}

fn letter_h() -> Glyph {
    Glyph {
        advance: 720,
        contours: vec![Contour::from_points(vec![
            (0, 0, true),
            (84, 0, true),
            (84, 324, true),
            (636, 324, true),
            (636, 0, true),
            (720, 0, true),
            (720, 720, true),
            (636, 720, true),
            (636, 396, true),
            (84, 396, true),
            (84, 720, true),
            (0, 720, true),
        ])],
    }
}

fn letter_j() -> Glyph {
    // The stem and its hook: one contour — down the stem's inner,
    // under the hook's inner arc, across the tip, back under the
    // outer arc, up the stem's outer, across the top.
    let mut pts = vec![(336, 720, true), (336, 180, true)];
    pts.extend(arc_run(210, 180, 126, 96, 0, -180));
    pts.push((0, 180, true));
    pts.extend(arc_run(210, 180, 210, 180, 180, 360));
    pts.push((420, 180, true));
    pts.push((420, 720, true));
    Glyph {
        advance: 460,
        contours: vec![Contour::from_points(pts)],
    }
}

fn letter_k() -> Glyph {
    // The stem, the arm, and the leg — three strokes meeting at the
    // stem's waist (the overlap-clamp doctrine again).
    Glyph {
        advance: 700,
        contours: vec![
            stem_pts(0, 0, 720),
            stroke_v((84, 198), (700, 720), 102),
            Contour::from_points(vec![
                (110, 450, true),
                (110, 350, true),
                (700, 0, true),
                (700, 100, true),
            ]),
        ],
    }
}

fn letter_l() -> Glyph {
    Glyph {
        advance: 560,
        contours: vec![Contour::from_points(vec![
            (0, 0, true),
            (560, 0, true),
            (560, 72, true),
            (84, 72, true),
            (84, 720, true),
            (0, 720, true),
        ])],
    }
}

fn letter_m() -> Glyph {
    // Three stems and two arch bands — the clamp doctrine's M: each
    // contour individually honest, the overlaps merging under the
    // nonzero rule. The middle stem rises from the baseline into
    // both bands; the shoulders square above the springing.
    Glyph {
        advance: 900,
        contours: vec![
            stem_pts(0, 0, 720),
            stem_pts(424, 0, 640),
            stem_pts(764, 0, 720),
            arch_band(254, 430, 194, 290, 110, 206),
            arch_band(594, 430, 194, 290, 110, 206),
        ],
    }
}

fn letter_n() -> Glyph {
    // Two stems and the diagonal — the diagonal's ends live inside
    // both stems' territory (the clamp doctrine merges them).
    Glyph {
        advance: 740,
        contours: vec![
            stem_pts(0, 0, 720),
            stem_pts(616, 0, 720),
            Contour::from_points(vec![
                (0, 720, true),
                (0, 611, true),
                (700, 0, true),
                (700, 70, true),
            ]),
        ],
    }
}

fn letter_o() -> Glyph {
    Glyph {
        advance: 760,
        contours: ring(380, 360, 340, 360, 252, 272),
    }
}

fn letter_p() -> Glyph {
    let mut contours = vec![stem_pts(0, 0, 720)];
    contours.extend(ring(270, 530, 230, 190, 146, 118));
    Glyph {
        advance: 540,
        contours,
    }
}

fn letter_q() -> Glyph {
    // The O's ring plus the tail crossing into the descender.
    let mut contours = ring(380, 360, 340, 360, 252, 272);
    contours.push(Contour::from_points(vec![
        (450, 330, true),
        (450, 220, true),
        (700, -150, true),
        (700, -40, true),
    ]));
    Glyph {
        advance: 760,
        contours,
    }
}

fn letter_r() -> Glyph {
    // The P's stem and bowl plus the leg springing from the waist.
    let mut contours = vec![stem_pts(0, 0, 720)];
    contours.extend(ring(270, 530, 230, 190, 146, 118));
    contours.push(Contour::from_points(vec![
        (150, 430, true),
        (150, 330, true),
        (660, 0, true),
        (660, 100, true),
    ]));
    Glyph {
        advance: 700,
        contours,
    }
}

fn letter_s() -> Glyph {
    // The arch, the spine, and the cup — three contours whose
    // overlaps merge (the clamp doctrine). An honest mechanical S.
    let mut arch = arc_run(380, 540, 220, 180, 30, 250);
    arch.extend(arc_run(380, 540, 136, 96, 250, 30));
    let mut cup = arc_run(280, 180, 220, 180, 200, 390);
    cup.extend(arc_run(280, 180, 136, 96, 30, -160));
    Glyph {
        advance: 640,
        contours: vec![
            Contour::from_points(arch),
            Contour::from_points(cup),
            Contour::from_points(vec![
                (290, 470, true),
                (290, 371, true),
                (500, 100, true),
                (500, 199, true),
            ]),
        ],
    }
}

fn letter_t() -> Glyph {
    // The bar across the top, the stem under its center.
    Glyph {
        advance: 620,
        contours: vec![bar(0, 648, 620, 720), stem_pts(268, 72, 656)],
    }
}

fn letter_u() -> Glyph {
    // One contour: the stems and the bottom arc joined, the mouth
    // open at the top. Both arcs sweep *under* (the ink above).
    let mut pts = vec![(0, 720, true), (0, 180, true)];
    pts.extend(arc_run(360, 180, 360, 180, 180, 360));
    pts.push((720, 180, true));
    pts.push((720, 720, true));
    pts.push((636, 720, true));
    pts.push((636, 180, true));
    pts.extend(arc_run(360, 180, 276, 84, 0, -180));
    pts.push((84, 180, true));
    pts.push((84, 720, true));
    Glyph {
        advance: 744,
        contours: vec![Contour::from_points(pts)],
    }
}

fn letter_v() -> Glyph {
    // One contour: the flat apex, the notch open at the top.
    Glyph {
        advance: 700,
        contours: vec![Contour::from_points(vec![
            (0, 720, true),
            (270, 0, true),
            (350, 0, true),
            (700, 720, true),
            (620, 720, true),
            (310, 148, true),
            (80, 720, true),
        ])],
    }
}

fn letter_w() -> Glyph {
    // Two overlapping V's — the middle vertex where they cross is
    // the W's peak (the clamp doctrine merges the crossing).
    let v = |dx: i32| {
        Contour::from_points(vec![
            (dx, 720, true),
            (dx + 270, 0, true),
            (dx + 350, 0, true),
            (dx + 620, 720, true),
            (dx + 540, 720, true),
            (dx + 310, 148, true),
            (dx + 80, 720, true),
        ])
    };
    Glyph {
        advance: 1000,
        contours: vec![v(0), v(380)],
    }
}

fn letter_x() -> Glyph {
    Glyph {
        advance: 640,
        contours: vec![
            Contour::from_points(vec![
                (0, 602, true),
                (600, 0, true),
                (600, 118, true),
                (0, 720, true),
            ]),
            Contour::from_points(vec![
                (0, 0, true),
                (600, 602, true),
                (600, 720, true),
                (0, 118, true),
            ]),
        ],
    }
}

fn letter_y() -> Glyph {
    // The two arms and the descender stem joining them below the
    // baseline (the clamp doctrine at the junction).
    Glyph {
        advance: 700,
        contours: vec![
            Contour::from_points(vec![
                (0, 720, true),
                (0, 620, true),
                (270, 80, true),
                (350, 160, true),
            ]),
            Contour::from_points(vec![
                (290, 80, true),
                (370, 160, true),
                (640, 620, true),
                (640, 720, true),
            ]),
            stem_pts(270, -200, 160),
        ],
    }
}

fn letter_z() -> Glyph {
    // The top bar, the diagonal, the bottom bar — one polygon whose
    // junctions ride the diagonal's own crossings of the bars.
    Glyph {
        advance: 620,
        contours: vec![Contour::from_points(vec![
            (0, 0, true),
            (620, 0, true),
            (620, 72, true),
            (68, 72, true),
            (620, 648, true),
            (620, 720, true),
            (0, 720, true),
            (0, 648, true),
            (550, 648, true),
            (0, 72, true),
        ])],
    }
}

// ————— lowercase —————

fn lower_a() -> Glyph {
    // The single-story a: the bowl plus the right stem.
    let mut contours = ring(280, 255, 210, 245, 126, 161);
    contours.push(stem_pts(448, 0, 520));
    Glyph {
        advance: 560,
        contours,
    }
}

fn lower_b() -> Glyph {
    let mut contours = vec![stem_pts(84, 0, 720)];
    contours.extend(ring(300, 250, 226, 250, 142, 166));
    Glyph {
        advance: 560,
        contours,
    }
}

fn lower_c() -> Glyph {
    let mut pts = arc_run(260, 255, 220, 245, 55, 305);
    pts.extend(arc_run(260, 255, 136, 161, 305, 55));
    Glyph {
        advance: 520,
        contours: vec![Contour::from_points(pts)],
    }
}

fn lower_d() -> Glyph {
    let mut contours = vec![stem_pts(416, 0, 720)];
    contours.extend(ring(230, 250, 226, 250, 142, 166));
    Glyph {
        advance: 560,
        contours,
    }
}

fn lower_e() -> Glyph {
    // The open ring (the mouth below the bar) plus the crossbar
    // riding into both bands (the clamp doctrine).
    let mut pts = arc_run(280, 255, 220, 245, 15, 325);
    pts.extend(arc_run(280, 255, 136, 161, 325, 15));
    Glyph {
        advance: 560,
        contours: vec![Contour::from_points(pts), bar(70, 250, 460, 322)],
    }
}

fn lower_f() -> Glyph {
    // The stem, the crossbar, the top hook.
    let mut pts = arc_run(230, 500, 130, 200, 0, 180);
    pts.extend(arc_run(230, 500, 46, 116, 180, 0));
    Glyph {
        advance: 380,
        contours: vec![
            stem_pts(98, 0, 700),
            bar(0, 350, 340, 422),
            Contour::from_points(pts),
        ],
    }
}

fn lower_g() -> Glyph {
    // The single-story a's bowl plus the descender tail hooking left.
    let mut contours = ring(280, 255, 210, 245, 126, 161);
    let mut pts = vec![(448, 210, true), (448, -20, true)];
    pts.extend(arc_run(330, -20, 118, 96, 0, -180));
    pts.push((128, -20, true));
    pts.extend(arc_run(330, -20, 202, 180, 180, 360));
    pts.push((532, -20, true));
    pts.push((532, 210, true));
    contours.push(Contour::from_points(pts));
    Glyph {
        advance: 560,
        contours,
    }
}

fn lower_h() -> Glyph {
    Glyph {
        advance: 580,
        contours: vec![
            stem_pts(84, 0, 720),
            stem_pts(416, 0, 440),
            arch_band(250, 400, 166, 100, 82, 16),
        ],
    }
}

fn lower_j() -> Glyph {
    // The dot, the stem, the hook.
    let mut pts = vec![(108, 480, true), (108, -20, true)];
    pts.extend(arc_run(96, -20, 96, 180, 0, -180));
    pts.push((0, -20, true));
    pts.extend(arc_run(96, -20, 192, 180, 180, 360));
    pts.push((192, -20, true));
    pts.push((192, 480, true));
    Glyph {
        advance: 280,
        contours: vec![Contour::from_points(pts), dot(150, 700)],
    }
}

fn lower_k() -> Glyph {
    Glyph {
        advance: 540,
        contours: vec![
            stem_pts(84, 0, 720),
            Contour::from_points(vec![
                (84, 250, true),
                (84, 140, true),
                (460, 390, true),
                (460, 500, true),
            ]),
            Contour::from_points(vec![
                (84, 430, true),
                (84, 320, true),
                (500, 0, true),
                (500, 138, true),
            ]),
        ],
    }
}

fn lower_m() -> Glyph {
    // Three stems, two arch bands (the uppercase M's doctrine at
    // x-height).
    Glyph {
        advance: 824,
        contours: vec![
            stem_pts(0, 0, 436),
            stem_pts(340, 0, 436),
            stem_pts(680, 0, 436),
            arch_band(212, 340, 148, 160, 64, 76),
            arch_band(534, 340, 148, 160, 64, 76),
        ],
    }
}

fn lower_n() -> Glyph {
    Glyph {
        advance: 500,
        contours: vec![
            stem_pts(0, 0, 444),
            stem_pts(376, 0, 444),
            arch_band(230, 340, 192, 160, 108, 76),
        ],
    }
}

fn lower_o() -> Glyph {
    Glyph {
        advance: 520,
        contours: ring(240, 250, 220, 250, 136, 166),
    }
}

fn lower_p() -> Glyph {
    let mut contours = vec![stem_pts(84, -200, 500)];
    contours.extend(ring(270, 250, 226, 250, 142, 166));
    Glyph {
        advance: 560,
        contours,
    }
}

fn lower_q() -> Glyph {
    let mut contours = vec![stem_pts(416, -200, 500)];
    contours.extend(ring(230, 250, 226, 250, 142, 166));
    Glyph {
        advance: 560,
        contours,
    }
}

fn lower_r() -> Glyph {
    // The stem plus the shoulder arc.
    let mut pts = arc_run(200, 360, 130, 140, 30, 180);
    pts.extend(arc_run(200, 360, 46, 56, 180, 30));
    Glyph {
        advance: 400,
        contours: vec![stem_pts(84, 0, 460), Contour::from_points(pts)],
    }
}

fn lower_s() -> Glyph {
    // The uppercase S's doctrine scaled to x-height.
    let mut arch = arc_run(300, 380, 160, 130, 30, 250);
    arch.extend(arc_run(300, 380, 76, 46, 250, 30));
    let mut cup = arc_run(220, 130, 160, 130, 200, 390);
    cup.extend(arc_run(220, 130, 76, 46, 30, -160));
    Glyph {
        advance: 500,
        contours: vec![
            Contour::from_points(arch),
            Contour::from_points(cup),
            Contour::from_points(vec![
                (250, 340, true),
                (250, 260, true),
                (370, 80, true),
                (370, 160, true),
            ]),
        ],
    }
}

fn lower_t() -> Glyph {
    Glyph {
        advance: 400,
        contours: vec![bar(0, 310, 380, 382), stem_pts(148, 0, 500)],
    }
}

fn lower_u() -> Glyph {
    let mut pts = vec![(0, 500, true), (0, 130, true)];
    pts.extend(arc_run(230, 130, 192, 130, 180, 360));
    pts.push((460, 130, true));
    pts.push((460, 500, true));
    pts.push((376, 500, true));
    pts.push((376, 130, true));
    pts.extend(arc_run(230, 130, 108, 46, 0, -180));
    pts.push((84, 130, true));
    pts.push((84, 500, true));
    Glyph {
        advance: 540,
        contours: vec![Contour::from_points(pts)],
    }
}

fn lower_v() -> Glyph {
    Glyph {
        advance: 520,
        contours: vec![Contour::from_points(vec![
            (0, 500, true),
            (180, 0, true),
            (240, 0, true),
            (480, 500, true),
            (425, 500, true),
            (210, 100, true),
            (55, 500, true),
        ])],
    }
}

fn lower_w() -> Glyph {
    // Two overlapping v's (the uppercase W's doctrine).
    let v = |dx: i32| {
        Contour::from_points(vec![
            (dx, 500, true),
            (dx + 180, 0, true),
            (dx + 240, 0, true),
            (dx + 480, 500, true),
            (dx + 425, 500, true),
            (dx + 210, 100, true),
            (dx + 55, 500, true),
        ])
    };
    Glyph {
        advance: 780,
        contours: vec![v(0), v(260)],
    }
}

fn lower_x() -> Glyph {
    Glyph {
        advance: 540,
        contours: vec![
            Contour::from_points(vec![
                (0, 402, true),
                (500, 0, true),
                (500, 80, true),
                (0, 500, true),
            ]),
            Contour::from_points(vec![
                (0, 0, true),
                (500, 402, true),
                (500, 500, true),
                (0, 80, true),
            ]),
        ],
    }
}

fn lower_y() -> Glyph {
    Glyph {
        advance: 460,
        contours: vec![
            Contour::from_points(vec![
                (0, 500, true),
                (0, 424, true),
                (180, 56, true),
                (234, 110, true),
            ]),
            Contour::from_points(vec![
                (196, 56, true),
                (252, 110, true),
                (424, 424, true),
                (424, 500, true),
            ]),
            stem_pts(180, -200, 110),
        ],
    }
}

fn lower_z() -> Glyph {
    Glyph {
        advance: 480,
        contours: vec![Contour::from_points(vec![
            (0, 0, true),
            (480, 0, true),
            (480, 50, true),
            (52, 50, true),
            (480, 430, true),
            (480, 500, true),
            (0, 500, true),
            (0, 430, true),
            (424, 430, true),
            (0, 50, true),
        ])],
    }
}

// ————— digits —————

fn digit_zero() -> Glyph {
    Glyph {
        advance: 640,
        contours: ring(320, 360, 300, 360, 212, 272),
    }
}

fn digit_one() -> Glyph {
    Glyph {
        advance: 400,
        contours: vec![
            stem_pts(158, 0, 720),
            Contour::from_points(vec![
                (40, 650, true),
                (40, 560, true),
                (158, 630, true),
                (158, 720, true),
            ]),
            bar(40, 0, 360, 72),
        ],
    }
}

fn digit_two() -> Glyph {
    // The bowl, the diagonal, the base bar.
    let mut bowl = arc_run(310, 480, 250, 240, -30, 170);
    bowl.extend(arc_run(310, 480, 166, 156, 170, -30));
    Glyph {
        advance: 620,
        contours: vec![
            Contour::from_points(bowl),
            Contour::from_points(vec![
                (0, 20, true),
                (500, 350, true),
                (500, 446, true),
                (0, 116, true),
            ]),
            bar(0, 0, 620, 72),
        ],
    }
}

fn digit_three() -> Glyph {
    // Two right-opening bowls meeting at the waist.
    let mut upper = arc_run(290, 530, 200, 190, -60, 160);
    upper.extend(arc_run(290, 530, 116, 106, 160, -60));
    let mut lower = arc_run(290, 190, 200, 190, -160, 70);
    lower.extend(arc_run(290, 190, 116, 106, 70, -160));
    Glyph {
        advance: 560,
        contours: vec![Contour::from_points(upper), Contour::from_points(lower)],
    }
}

fn digit_four() -> Glyph {
    Glyph {
        advance: 660,
        contours: vec![
            Contour::from_points(vec![
                (125, 47, true),
                (60, 100, true),
                (560, 720, true),
                (625, 667, true),
            ]),
            bar(0, 100, 620, 172),
            stem_pts(380, 0, 500),
        ],
    }
}

fn digit_five() -> Glyph {
    // The top bar, the stem, the bowl.
    let mut bowl = arc_run(249, 200, 190, 200, -140, 160);
    bowl.extend(arc_run(249, 200, 106, 116, 160, -140));
    Glyph {
        advance: 600,
        contours: vec![
            bar(0, 648, 560, 720),
            stem_pts(0, 260, 660),
            Contour::from_points(bowl),
        ],
    }
}

fn digit_six() -> Glyph {
    // The ring at the bottom, the tail sweeping from the top right.
    let mut tail = arc_run(310, 380, 190, 240, 45, 210);
    tail.extend(arc_run(310, 380, 106, 156, 210, 45));
    let mut contours = ring(300, 190, 200, 190, 116, 106);
    contours.push(Contour::from_points(tail));
    Glyph {
        advance: 560,
        contours,
    }
}

fn digit_seven() -> Glyph {
    Glyph {
        advance: 600,
        contours: vec![
            bar(0, 648, 570, 720),
            Contour::from_points(vec![
                (0, 0, true),
                (565, 614, true),
                (565, 720, true),
                (0, 106, true),
            ]),
        ],
    }
}

fn digit_eight() -> Glyph {
    // Two rings overlapping at the waist.
    let mut contours = ring(320, 545, 190, 180, 106, 96);
    contours.extend(ring(320, 175, 210, 180, 126, 96));
    Glyph {
        advance: 640,
        contours,
    }
}

fn digit_nine() -> Glyph {
    // The six flipped: the ring at the top, the tail down the right.
    let mut tail = arc_run(310, 340, 190, 240, 150, 315);
    tail.extend(arc_run(310, 340, 106, 156, 315, 150));
    let mut contours = ring(300, 530, 200, 190, 116, 106);
    contours.push(Contour::from_points(tail));
    Glyph {
        advance: 560,
        contours,
    }
}

// ————— punctuation —————

fn quotes() -> Glyph {
    Glyph {
        advance: 420,
        contours: vec![stem_pts(60, 440, 720), stem_pts(240, 440, 720)],
    }
}

fn hash() -> Glyph {
    Glyph {
        advance: 620,
        contours: vec![
            stem_pts(140, 0, 720),
            stem_pts(400, 0, 720),
            bar(0, 240, 620, 312),
            bar(0, 480, 620, 552),
        ],
    }
}

fn dollar() -> Glyph {
    // The S plus the vertical stroke crossing through.
    let mut contours = letter_s().contours;
    contours.push(stem_pts(270, -100, 800));
    Glyph {
        advance: 640,
        contours,
    }
}

fn percent() -> Glyph {
    // Two rings and the slash.
    Glyph {
        advance: 800,
        contours: vec![
            Contour::from_points(vec![
                (0, 0, true),
                (760, 598, true),
                (760, 720, true),
                (0, 122, true),
            ]),
            Contour::from_points(arc_run(160, 560, 120, 120, 0, 360)),
            Contour::from_points({
                let mut h = arc_run(160, 560, 44, 44, 0, 360);
                h.reverse();
                h
            }),
            Contour::from_points(arc_run(640, 160, 120, 120, 0, 360)),
            Contour::from_points({
                let mut h = arc_run(640, 160, 44, 44, 0, 360);
                h.reverse();
                h
            }),
        ],
    }
}

fn ampersand() -> Glyph {
    // The loop, the spine, the leg, the waist — four contours whose
    // overlaps merge.
    let mut spine = arc_run(280, 340, 210, 280, 60, 227);
    spine.extend(arc_run(280, 340, 126, 196, 227, 60));
    let mut contours = ring(350, 560, 160, 150, 76, 66);
    contours.push(Contour::from_points(spine));
    contours.push(Contour::from_points(vec![
        (524, 480, true),
        (440, 480, true),
        (500, 60, true),
        (584, 60, true),
    ]));
    contours.push(bar(140, 290, 460, 362));
    Glyph {
        advance: 620,
        contours,
    }
}

/// One parenthesis: a crescent from the top to the bottom bulging
/// `left` (the "(" face) or right (the ")").
fn bracket(_unused: i32, adv: i32, left: bool) -> Glyph {
    // The arc kit in the ( orientation; the ) mirrors x.
    let mut pts = arc_run(170, 360, 130, 360, 80, 280);
    pts.extend(arc_run(170, 360, 46, 276, 280, 80));
    let pts = if left {
        pts
    } else {
        // The x-mirror flips orientation; the reversed ring restores
        // the counter-clockwise walk (the rasterizer's positive
        // wind), rotated so the ring still starts on-curve.
        let mut mirrored: Vec<(i32, i32, bool)> =
            pts.into_iter().map(|(x, y, on)| (adv - x, y, on)).collect();
        mirrored.reverse();
        if !mirrored[0].2 {
            mirrored.rotate_right(1);
        }
        mirrored
    };
    Glyph {
        advance: adv,
        contours: vec![Contour::from_points(pts)],
    }
}

/// One square bracket: the C-rotated contour (the stem plus the two
/// stubs), mirrored for the "]".
fn square_bracket(left: bool) -> Glyph {
    let mk = |pts: Vec<(i32, i32, bool)>| {
        if left {
            pts
        } else {
            // Mirror + reverse (the orientation-preserving mirror).
            let mut mirrored: Vec<(i32, i32, bool)> =
                pts.into_iter().map(|(x, y, on)| (400 - x, y, on)).collect();
            mirrored.reverse();
            mirrored
        }
    };
    Glyph {
        advance: 400,
        contours: vec![Contour::from_points(mk(vec![
            (160, 0, true),
            (340, 0, true),
            (340, 72, true),
            (240, 72, true),
            (240, 648, true),
            (340, 648, true),
            (340, 720, true),
            (160, 720, true),
        ]))],
    }
}

fn star() -> Glyph {
    Glyph {
        advance: 460,
        contours: vec![
            stem_pts(188, 380, 660),
            Contour::from_points(vec![
                (120, 380, true),
                (196, 380, true),
                (340, 660, true),
                (264, 660, true),
            ]),
            Contour::from_points(vec![
                (264, 380, true),
                (340, 380, true),
                (196, 660, true),
                (120, 660, true),
            ]),
        ],
    }
}

fn plus() -> Glyph {
    Glyph {
        advance: 620,
        contours: vec![bar(40, 324, 580, 396), stem_pts(268, 0, 720)],
    }
}

fn comma() -> Glyph {
    Glyph {
        advance: 300,
        contours: vec![
            dot(150, 60),
            Contour::from_points(vec![
                (210, 80, true),
                (90, 80, true),
                (20, -160, true),
                (100, -160, true),
            ]),
        ],
    }
}

fn slash() -> Glyph {
    Glyph {
        advance: 480,
        contours: vec![Contour::from_points(vec![
            (0, 0, true),
            (460, 620, true),
            (460, 720, true),
            (0, 100, true),
        ])],
    }
}

fn slash_back() -> Glyph {
    Glyph {
        advance: 480,
        contours: vec![Contour::from_points(vec![
            (0, 720, true),
            (0, 620, true),
            (460, 0, true),
            (460, 100, true),
        ])],
    }
}

fn semicolon() -> Glyph {
    Glyph {
        advance: 300,
        contours: vec![
            dot(150, 430),
            Contour::from_points(vec![
                (210, 330, true),
                (90, 330, true),
                (20, 90, true),
                (100, 90, true),
            ]),
        ],
    }
}

fn chevron(left: bool) -> Glyph {
    // The "<" as two strokes meeting at the left vertex; ">" mirrors.
    let mk = |flip: bool| {
        let pts = if flip {
            vec![
                (60, 440, true),
                (60, 280, true),
                (560, 530, true),
                (560, 690, true),
            ]
        } else {
            vec![
                (60, 440, true),
                (60, 280, true),
                (560, 30, true),
                (560, 190, true),
            ]
        };
        if left {
            pts
        } else {
            let mut mirrored: Vec<(i32, i32, bool)> =
                pts.into_iter().map(|(x, y, on)| (620 - x, y, on)).collect();
            mirrored.reverse();
            mirrored
        }
    };
    Glyph {
        advance: 620,
        contours: vec![
            Contour::from_points(mk(true)),
            Contour::from_points(mk(false)),
        ],
    }
}

fn question() -> Glyph {
    let mut hook = arc_run(280, 460, 190, 260, -90, 170);
    hook.extend(arc_run(280, 460, 106, 176, 170, -90));
    Glyph {
        advance: 560,
        contours: vec![Contour::from_points(hook), dot(280, 60)],
    }
}

fn at() -> Glyph {
    // The shell, the inner ring, the tail exiting at the right.
    let mut shell = arc_run(360, 320, 300, 340, 30, 330);
    shell.extend(arc_run(360, 320, 216, 256, 330, 30));
    let mut contours = vec![Contour::from_points(shell)];
    contours.extend(ring(360, 320, 150, 170, 66, 86));
    contours.push(bar(400, 240, 700, 312));
    Glyph {
        advance: 740,
        contours,
    }
}

fn caret() -> Glyph {
    Glyph {
        advance: 600,
        contours: vec![
            Contour::from_points(vec![
                (60, 279, true),
                (60, 180, true),
                (300, 560, true),
                (300, 659, true),
            ]),
            Contour::from_points(vec![
                (540, 279, true),
                (540, 180, true),
                (300, 560, true),
                (300, 659, true),
            ]),
        ],
    }
}

/// One curly brace: two L-halves and the middle wedge (the clamp
/// doctrine), mirrored for the "}".
fn brace(left: bool) -> Glyph {
    let mk = |pts: Vec<(i32, i32, bool)>| {
        if left {
            pts
        } else {
            // Mirror + reverse (the orientation-preserving mirror).
            let mut mirrored: Vec<(i32, i32, bool)> =
                pts.into_iter().map(|(x, y, on)| (400 - x, y, on)).collect();
            mirrored.reverse();
            mirrored
        }
    };
    Glyph {
        advance: 400,
        contours: vec![
            Contour::from_points(mk(vec![
                (240, 360, true),
                (240, 648, true),
                (340, 648, true),
                (340, 720, true),
                (160, 720, true),
                (160, 360, true),
            ])),
            Contour::from_points(mk(vec![
                (160, 360, true),
                (160, 0, true),
                (340, 0, true),
                (340, 72, true),
                (240, 72, true),
                (240, 360, true),
            ])),
            Contour::from_points(mk(vec![
                (240, 300, true),
                (240, 420, true),
                (140, 360, true),
            ])),
        ],
    }
}

fn tilde() -> Glyph {
    // Two arcs: the hump and the valley, their terminals meeting.
    let mut hump = arc_run(220, 280, 180, 110, 20, 200);
    hump.extend(arc_run(220, 280, 120, 50, 200, 20));
    let mut valley = arc_run(558, 280, 180, 110, 160, 380);
    valley.extend(arc_run(558, 280, 120, 50, 380, 160));
    Glyph {
        advance: 760,
        contours: vec![Contour::from_points(hump), Contour::from_points(valley)],
    }
}

fn ellipsis() -> Glyph {
    Glyph {
        advance: 800,
        contours: vec![dot(150, 60), dot(400, 60), dot(650, 60)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The signed area (shoelace) of one contour's on-curve polygon —
    /// positive = counter-clockwise in the y-up font space.
    fn shoelace(c: &Contour) -> i64 {
        let pts: Vec<(i32, i32)> = c
            .points
            .iter()
            .filter(|p| p.on)
            .map(|p| (p.x, p.y))
            .collect();
        let n = pts.len();
        if n < 3 {
            return 0;
        }
        let mut sum: i64 = 0;
        for i in 0..n {
            let (x0, y0) = pts[i];
            let (x1, y1) = pts[(i + 1) % n];
            sum += i64::from(x0) * i64::from(y1) - i64::from(x1) * i64::from(y0);
        }
        sum
    }

    #[test]
    fn every_covered_codepoint_yields_a_glyph() {
        for c in 0x20u8..=0x7e {
            let ch = char::from(c);
            let g = lookup(ch).unwrap_or_else(|| panic!("no glyph for {ch:?}"));
            assert!(g.advance > 0, "{ch:?} advance");
        }
        assert!(lookup('\u{2026}').is_some());
        assert!(lookup('\u{a0}').is_some());
        assert!(lookup('é').is_none());
        assert!(lookup('中').is_none());
    }

    #[test]
    fn the_ink_bounds_live_in_the_line_box() {
        // Every glyph's ink stays within the em box's honest bounds
        // (the descender to −200, the cap to 720, the advance's side
        // bearing clamped at −20 — the tail glyphs' deliberate
        // overhangs excepted by name).
        for c in 0x21u8..=0x7e {
            let ch = char::from(c);
            let Some(g) = lookup(ch) else { continue };
            for c2 in &g.contours {
                for p in &c2.points {
                    assert!(
                        p.y >= -220 && p.y <= 800,
                        "{ch:?} ink y {} outside the line box",
                        p.y
                    );
                }
            }
        }
    }

    #[test]
    fn rasterizing_every_glyph_produces_ink() {
        // The full-coverage smoke: every printable glyph inks, the
        // space and nbsp do not. Direction mistakes read as missing
        // ink here first.
        for c in 0x21u8..=0x7e {
            let ch = char::from(c);
            let g = lookup(ch).expect("glyph");
            let total: u32 = g
                .contours
                .iter()
                .map(|ct| {
                    let b = crate::raster::rasterize(std::slice::from_ref(ct), 16.0 / 1000.0);
                    b.w * b.h
                })
                .sum();
            assert!(total > 0, "{ch:?} produced no bitmap at all");
        }
        for blank in [' ', '\u{a0}'] {
            let g = lookup(blank).expect("blank");
            assert!(g.contours.is_empty());
        }
    }

    #[test]
    fn the_orientation_doctrine_holds() {
        // Every glyph's outer contour reads counter-clockwise (the
        // rasterizer's positive-wind convention) and every hole
        // clockwise — the authored ring pairs carry their own truth.
        for c in 0x21u8..=0x7e {
            let ch = char::from(c);
            let Some(g) = lookup(ch) else { continue };
            let mut saw_outer = false;
            for contour in &g.contours {
                let area = shoelace(contour);
                match area.signum() {
                    1 => saw_outer = true,
                    -1 => {
                        // A hole: legal only when an outer exists in
                        // the same glyph (the pairs the rings build).
                        assert!(g.contours.len() > 1, "{ch:?} hole without an outer");
                    }
                    _ => panic!("{ch:?} has a degenerate contour (zero area)"),
                }
            }
            assert!(saw_outer, "{ch:?} has no counter-clockwise outer");
        }
    }

    #[test]
    fn the_notdef_box_is_hollow() {
        let g = notdef();
        let b = crate::raster::rasterize(&g.contours, 16.0 / 1000.0);
        // The center of the box is empty, the frame is inked.
        let (w, h) = (b.w as usize, b.h as usize);
        assert!(w >= 6 && h >= 10);
        let center = b.alpha[h / 2 * w + w / 2];
        assert_eq!(center, 0, "the notdef center is hollow");
        // Row 0 rides the frame's top edge (partial coverage); row 1
        // sits fully inside the frame bar.
        assert!(b.alpha[w / 2] > 0, "the notdef frame's fringe inks");
        assert_eq!(b.alpha[w + w / 2], 255, "the notdef frame inks");
    }

    #[test]
    fn the_bowl_counters_read_empty() {
        // O, o, 0, 8: the ring centers are hollow (the hole contour
        // subtracts under the nonzero rule).
        for (ch, cx, cy) in [('O', 380, 360), ('o', 240, 250), ('0', 320, 360)] {
            let g = lookup(ch).expect("glyph");
            let scale = 16.0 / 1000.0;
            let b = crate::raster::rasterize(&g.contours, scale);
            let px = (f64::from(cx) * f64::from(scale)).round() as i32 - b.x_off;
            let py = (f64::from(cy) * f64::from(scale)).round() as i32 - (b.h as i32 - b.y_top);
            let idx = py as usize * b.w as usize + px.clamp(0, b.w as i32 - 1) as usize;
            assert_eq!(b.alpha[idx], 0, "{ch}'s counter inks at the center");
        }
    }
}
