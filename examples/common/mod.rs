#![allow(dead_code)] // each tool uses a different part of this module

//! Shared by the tools in `examples/`.

use scriptoria_ink::kurbo::Rect;
use scriptoria_ink::{InkDocument, InkPage, Stroke};
use scriptoria_skeleton::Skeleton;
use scriptoria_testkit::data;

/// The project's sample document, `test/test.pdf`.
pub fn test_pdf() -> InkDocument {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/test/test.pdf");
    InkDocument::open(std::fs::read(path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"))).unwrap()
}

/// A skeleton as test data (without id, tags or source), e.g. for its swept region.
pub fn skeleton_data(sk: &Skeleton) -> data::Skeleton {
    data::Skeleton {
        id: String::new(),
        tags: Vec::new(),
        source: None,
        pieces: sk
            .pieces
            .iter()
            .map(|p| data::SkeletonPiece { circles: p.samples.iter().map(|s| (s.p, s.r)).collect(), spine: p.spine.clone() })
            .collect(),
    }
}

/// A character of the sample: the character, page, rectangle (page points, y down) holding the
/// centers of its strokes.
pub struct Character(pub &'static str, pub usize, pub [f64; 4]);

impl Character {
    pub fn char(&self) -> char {
        self.0.chars().next().unwrap()
    }

    pub fn rect(&self) -> Rect {
        let [x0, y0, x1, y1] = self.2;
        Rect::new(x0, y0, x1, y1)
    }
}

/// The strokes whose centers lie in a rectangle, in writing order.
pub fn strokes_in(page: &InkPage, rect: Rect) -> Vec<&Stroke> {
    page.strokes.iter().filter(|s| rect.contains(s.bbox.center())).collect()
}

/// Characters of the sample, for the crates' test data and the font.
pub const CHARACTERS: &[Character] = &[
    // Chinese.
    Character("世", 2, [212.9, 187.8, 217.9, 192.2]),
    Character("界", 2, [218.0, 187.8, 223.0, 192.3]),
    Character("你", 2, [214.6, 152.6, 218.25, 155.6]),
    Character("好", 2, [218.3, 152.6, 221.6, 155.6]),
    Character("啊", 2, [221.8, 152.6, 225.4, 155.6]),
    Character("吉", 2, [118.6, 62.3, 121.4, 65.2]),
    Character("敷", 2, [121.4, 62.3, 125.4, 65.2]),
    Character("佳", 2, [126.1, 62.3, 129.55, 65.3]),
    Character("助", 2, [129.55, 62.3, 133.5, 65.3]),
    // Kana.
    Character("あ", 2, [294.0, 209.3, 296.9, 212.1]),
    Character("き", 2, [296.9, 213.8, 299.4, 216.4]),
    Character("す", 2, [299.9, 217.4, 302.4, 220.3]),
    Character("ぬ", 2, [300.1, 225.7, 303.3, 228.5]),
    Character("の", 2, [307.2, 225.7, 309.8, 228.5]),
    Character("る", 2, [300.6, 228.8, 303.3, 231.9]),
    Character("れ", 2, [303.6, 228.8, 306.9, 231.9]),
    // Latin.
    Character("A", 0, [64.6, 7.6, 67.7, 10.6]),
    Character("B", 0, [69.6, 7.7, 72.6, 10.6]),
    Character("G", 0, [92.1, 7.8, 95.4, 11.0]),
    Character("K", 0, [79.4, 12.4, 82.2, 15.4]),
    Character("Q", 0, [73.6, 16.8, 77.0, 21.0]),
    Character("S", 0, [83.7, 17.8, 87.0, 21.3]),
    Character("Z", 0, [84.8, 23.4, 88.4, 26.6]),
];
