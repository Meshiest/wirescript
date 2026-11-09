use super::*;

fn display(font: &str, pitch: i32) -> DisplayInfo {
    DisplayInfo {
        name: "s".into(),
        width: 4,
        height: 2,
        pitch,
        font: font.into(),
        border: true,
        side: Side::Top,
        offset: [0, 0, 0],
    }
}

/// A `█` as drawn: (width, height) in grid units.
fn drawn(d: &DisplayInfo) -> (f32, f32) {
    let (line_height, _, width_scale) = d.text_geometry();
    let block = BlockMetrics::of(&d.font);
    let em = line_height * block.em_per_line_height;
    (block.advance * em * width_scale, block.height * em)
}

#[test]
fn a_block_is_one_pixel_wide_and_fill_pixels_tall() {
    for font in ["IosevkaTerm", "MonaspaceArgon"] {
        let fill = BlockMetrics::of(font).fill;
        for pitch in [2, 4, 10, 40] {
            let (w, h) = drawn(&display(font, pitch));
            let p = pitch as f32;
            assert!((w - p).abs() < 1e-4 * p, "{font} at {pitch}: {w} wide");
            assert!(
                (h - fill * p).abs() < 1e-4 * p,
                "{font} at {pitch}: {h} tall"
            );
        }
    }
}

#[test]
fn the_block_top_meets_the_strip_top() {
    for font in ["IosevkaTerm", "MonaspaceArgon"] {
        for pitch in [2, 4, 10, 40] {
            let (lh, off, _) = display(font, pitch).text_geometry();
            let block = BlockMetrics::of(font);
            let em = lh * block.em_per_line_height;
            assert!((off + block.rise * em).abs() < 1e-5, "{font} at {pitch}");
        }
    }
}

#[test]
fn iosevka_keeps_the_geometry_measured_right_in_game() {
    // The left display of the third display_geometry run: square pixels, no
    // seams. Its numbers must not move when another font is tuned.
    let (line_height, offset_y, width_scale) = display("IosevkaTerm", 4).text_geometry();
    assert!((line_height - 4.0 / (1.25 * 1.28)).abs() < 1e-6);
    assert!((width_scale - 1.25 / 0.5).abs() < 1e-6);
    assert!((offset_y + 0.217 * 1.28 * line_height).abs() < 1e-6);
}
