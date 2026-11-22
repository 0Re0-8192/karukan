//! Wire-format cross-check helper (see tools in the repo notes):
//!   `wire enc`  prints hex of sample replies built by the encoder, one per line;
//!   `wire dec`  reads hex `Command` lines on stdin and prints the decoded input.
use karukan_mozc::input;
use karukan_mozc::output::{self, CandView, Frame, PreeditView};
use std::io::BufRead;

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("enc") => {
            let frame = Frame {
                preedit: Some(PreeditView {
                    text: "今日は".into(),
                    caret: 3,
                    attrs: vec![(0, 2, true)],
                }),
                cands: Some(CandView {
                    items: vec![
                        ("今日".into(), None),
                        ("きょう".into(), Some("ひらがな".into())),
                    ],
                    cursor: 0,
                    page: 0,
                    total_pages: 1,
                }),
            };
            println!(
                "{}",
                hex(&output::encode_command(
                    &[0x08, 0x03],
                    Some(9),
                    true,
                    None,
                    &frame
                ))
            );
            let suggest = Frame {
                preedit: Some(PreeditView {
                    text: "きょう".into(),
                    caret: 3,
                    attrs: vec![(0, 3, false)],
                }),
                cands: Some(CandView {
                    items: vec![("今日".into(), None), ("教".into(), None)],
                    cursor: 0,
                    page: 0,
                    total_pages: 1,
                }),
            };
            println!(
                "{}",
                hex(&output::encode_command(
                    &[0x08, 0x03],
                    Some(9),
                    true,
                    None,
                    &suggest
                ))
            );
            println!(
                "{}",
                hex(&output::encode_command(
                    &[0x08, 0x03],
                    Some(9),
                    true,
                    Some("今日"),
                    &Frame::default()
                ))
            );
        }
        Some("dec") => {
            for line in std::io::stdin().lock().lines() {
                let line = line.unwrap();
                let bytes: Vec<u8> = (0..line.len() / 2)
                    .map(|i| u8::from_str_radix(&line[2 * i..2 * i + 2], 16).unwrap())
                    .collect();
                match input::decode_command_input(&bytes) {
                    Some((_, i)) => {
                        let sym = i.key.as_ref().and_then(input::keysym);
                        println!("{i:?} keysym={sym:?}");
                    }
                    None => println!("DECODE-FAILED"),
                }
            }
        }
        _ => eprintln!("usage: wire enc|dec"),
    }
}
