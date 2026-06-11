use air_rs::inbox::line_parser::{FrameEvent, LineParser, MAX_FRAME};
use serde_json::json;

fn frames(evs: Vec<FrameEvent>) -> Vec<serde_json::Value> {
    evs.into_iter().filter_map(|e| match e { FrameEvent::Frame(v) => Some(v), _ => None }).collect()
}

#[test]
fn splits_two_frames_in_one_chunk() {
    let mut p = LineParser::new();
    let out = p.feed(b"{\"type\":\"pong\"}\n{\"type\":\"gap\",\"after_seq\":3}\n");
    let fs = frames(out);
    assert_eq!(fs, vec![json!({"type":"pong"}), json!({"type":"gap","after_seq":3})]);
}

#[test]
fn reassembles_a_frame_split_across_chunks() {
    let mut p = LineParser::new();
    assert!(frames(p.feed(b"{\"type\":\"po")).is_empty());
    let fs = frames(p.feed(b"ng\"}\n"));
    assert_eq!(fs, vec![json!({"type":"pong"})]);
}

#[test]
fn skips_blank_lines() {
    let mut p = LineParser::new();
    let fs = frames(p.feed(b"\n   \n{\"type\":\"pong\"}\n"));
    assert_eq!(fs, vec![json!({"type":"pong"})]);
}

#[test]
fn bad_json_surfaces_a_parse_error_and_continues() {
    let mut p = LineParser::new();
    let out = p.feed(b"not json\n{\"type\":\"pong\"}\n");
    assert!(matches!(out[0], FrameEvent::ParseError(_)));
    assert_eq!(frames(out), vec![json!({"type":"pong"})]);
}

#[test]
fn over_ceiling_line_without_newline_errors_and_resets() {
    let mut p = LineParser::new();
    let big = vec![b'x'; MAX_FRAME + 1];
    let out = p.feed(&big);
    assert_eq!(out.len(), 1);
    assert!(matches!(out[0], FrameEvent::ParseError(_)));
    assert_eq!(frames(p.feed(b"{\"type\":\"pong\"}\n")), vec![json!({"type":"pong"})]);
}
