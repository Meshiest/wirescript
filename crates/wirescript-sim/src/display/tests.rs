use super::*;

const RED: Pixel = Some([255, 0, 0]);
const GREEN: Pixel = Some([0, 255, 0]);

#[test]
fn a_row_decodes_tags_blocks_and_spaces() {
    let row = " <color=\"FF0000\">██<color=\"00FF00\">█ ";
    assert_eq!(decode_row(row, 5), [None, RED, RED, GREEN, None]);
}

#[test]
fn a_tag_holds_across_a_gap() {
    let row = "<color=\"FF0000\">█ █";
    assert_eq!(decode_row(row, 3), [RED, None, RED]);
}

#[test]
fn short_tags_and_untagged_blocks() {
    assert_eq!(
        decode_row("█<color=\"F00\">█", 2),
        [Some([255, 255, 255]), RED]
    );
}

#[test]
fn a_short_row_pads_and_a_long_one_cuts() {
    assert_eq!(decode_row("", 2), [None, None]);
    assert_eq!(decode_row("<color=\"FF0000\">███", 2), [RED, RED]);
}

#[test]
fn displays_frame_each_tick_a_row_changed() {
    let mut d = Displays::default();
    d.add("s", 2, 1);
    d.settle(0);
    assert!(d.frames().is_empty(), "nothing changed yet");
    d.set_row("s", 0, "<color=\"FF0000\">█");
    d.settle(3);
    d.set_row("s", 0, "<color=\"FF0000\">█");
    d.settle(4);
    assert_eq!(d.frames().len(), 1, "an unchanged row adds no frame");
    assert_eq!(d.frames()[0].tick, 3);
    let s = d.get("s").unwrap();
    assert_eq!(s.pixels(), [RED, None]);
    assert_eq!(s.render_plain(3), ["[display s @3]", "FF0000 ......"]);
}
