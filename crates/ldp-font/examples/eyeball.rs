//! ASCII-render one line of text through ldp-font — the eyeball gate.

use ldp_font::LION_SANS;

/// Render one line at one pixel size, mapping coverage to glyphs of
/// ASCII ink so the letterforms are legible in a terminal.
fn render(text: &str, px: u32) {
    let run = LION_SANS.layout(text, px, 400);
    println!(
        "=== {text:?} @ {px}px (w={}, trunc={}) ===",
        run.width, run.truncated
    );
    let Some(ink) = run.ink else {
        println!("(no ink)");
        return;
    };
    let cols = ink.width() as usize;
    let rows = ink.height() as usize;
    let mut grid = vec![b'.'; cols * rows];
    for placed in &run.placed {
        let bitmap = LION_SANS.bitmap(placed.c, px);
        if bitmap.w == 0 {
            continue;
        }
        let bx = placed.x + bitmap.x_off - ink.min_x;
        let by = ink.top - bitmap.y_top;
        for row in 0..bitmap.h as i32 {
            for col in 0..bitmap.w as i32 {
                let gx = bx + col;
                let gy = by + row;
                if gx < 0 || gy < 0 || gx >= cols as i32 || gy >= rows as i32 {
                    continue;
                }
                let coverage = bitmap.alpha[row as usize * bitmap.w as usize + col as usize];
                let ch = match coverage {
                    0 => b'.',
                    1..=51 => b' ',
                    52..=102 => b'-',
                    103..=153 => b'+',
                    154..=204 => b'*',
                    _ => b'#',
                };
                grid[gy as usize * cols + gx as usize] = ch;
            }
        }
    }
    for row in 0..rows {
        println!(
            "{}",
            String::from_utf8_lossy(&grid[row * cols..(row + 1) * cols])
        );
    }
}

fn main() {
    for text in [
        "HAMBURGVFONTS",
        "hamburg v fonts",
        "0123456789",
        "Lion Display Server",
        "The quick brown fox",
        "Image Viewer 2.0",
    ] {
        render(text, 24);
        println!();
    }
}
