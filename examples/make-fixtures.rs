//! Builds the crates' test data from `test/test.pdf`. The crates' tests read only these files, so
//! they do not change when the PDF does; rerun this to pick other strokes.
//!
//! - `crates/skeleton/tests/data/strokes.svg`: single strokes, one or a few per situation (tags);
//! - `crates/skeleton/tests/data/characters.svg`: multi-stroke characters, strokes in writing order;
//! - `crates/skeleton/tests/data/drawing.svg`: a few strokes from drawings (smoke test only);
//! - `crates/outline/tests/data/{strokes,characters}.svg`: the skeletons of the same strokes, as
//!   recovered now.
//! - `crates/font/tests/data/glyphs.svg`: the characters as font glyphs, their outlines at the
//!   masters each needs (font units, `wght` values; see `scriptoria::font`).
//!
//! `cargo run --release --example make-fixtures`

mod common;

use std::path::Path;

use scriptoria_ink::kurbo::{Affine, Rect};
use scriptoria_ink::{InkPage, Stroke, StrokeShape};
use scriptoria_testkit::data::{self, Group, Skeleton, SkeletonPiece, StrokeKind};

/// A single stroke: page, index in writing order, tags (situations it exercises).
struct Pick(usize, usize, &'static str);

const STROKES: &[Pick] = &[
    // Ordinary strokes of characters.
    Pick(2, 1617, "straight"),
    Pick(2, 1621, "corner"),
    Pick(2, 1628, "sweep"),
    Pick(2, 345, "hook"),
    Pick(2, 1914, "short"),
    Pick(0, 20, "arc"),
    Pick(2, 1901, "loop"),
    Pick(2, 1956, "loop"),
    Pick(2, 1961, "loop zigzag"),
    Pick(2, 1963, "tight-turn"),
    // Constant-width lines (a monoline pen).
    Pick(2, 189, "line"),
    Pick(2, 1545, "line curve"),
    Pick(2, 1554, "line straight"),
    // Situations that were hard to fit.
    Pick(2, 337, "stutter pen-down"),
    Pick(2, 468, "hook pen-down"),
    Pick(2, 1361, "hairpin"),
    Pick(2, 822, "crescent fast"),
    Pick(2, 1399, "short-thick"),
    Pick(2, 252, "short-thick"),
    Pick(0, 1153, "tip multi-marker"),
    Pick(0, 1420, "tip"),
    Pick(2, 5, "dot"),
    Pick(2, 8, "dot"),
];

const DRAWING: &[Pick] = &[Pick(2, 2, "drawing"), Pick(2, 3, "drawing"), Pick(2, 4, "drawing"), Pick(2, 145, "drawing")];

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let doc = common::test_pdf();
    let pages: Vec<InkPage> = (0..doc.page_count()).map(|i| doc.page(i).unwrap()).collect();

    let singles: Vec<Vec<(usize, &Stroke, &str)>> =
        STROKES.iter().map(|&Pick(page, index, tags)| vec![(page, &pages[page].strokes[index], tags)]).collect();
    let singles = Layout::grid(singles.into_iter().map(|s| (None, s)).collect());
    let characters = Layout::grid(
        common::CHARACTERS
            .iter()
            .map(|c| {
                let (name, page) = (c.0, c.1);
                let strokes: Vec<_> = common::strokes_in(&pages[page], c.rect()).into_iter().map(|s| (page, s, "")).collect();
                println!("{name}: {} strokes ({})", strokes.len(), strokes.iter().map(|(_, s, _)| s.order.to_string()).collect::<Vec<_>>().join(" "));
                (Some(name), strokes)
            })
            .collect(),
    );
    let drawing = Layout::grid(DRAWING.iter().map(|&Pick(page, index, tags)| (None, vec![(page, &pages[page].strokes[index], tags)])).collect());

    let skeleton_dir = root.join("crates/skeleton/tests/data");
    let outline_dir = root.join("crates/outline/tests/data");
    data::write_strokes(&skeleton_dir.join("strokes.svg"), "single strokes", &singles.strokes());
    data::write_strokes(&skeleton_dir.join("characters.svg"), "characters", &characters.strokes());
    data::write_strokes(&skeleton_dir.join("drawing.svg"), "drawing strokes", &drawing.strokes());
    data::write_skeletons(&outline_dir.join("strokes.svg"), "skeletons of single strokes", &singles.skeletons());
    data::write_skeletons(&outline_dir.join("characters.svg"), "skeletons of characters", &characters.skeletons());
    let layout = scriptoria::font::Layout::default();
    let glyphs: Vec<data::Glyph> = common::CHARACTERS
        .iter()
        .map(|c| {
            let g = scriptoria::font::glyph(c.char(), &common::strokes_in(&pages[c.1], c.rect()), c.rect(), &layout);
            println!("{}: {} masters", c.0, g.masters.len());
            data::Glyph { name: g.name, chars: g.chars, advance: g.advance, masters: g.masters }
        })
        .collect();
    data::write_glyphs(&root.join("crates/font/tests/data/glyphs.svg"), "glyphs at their masters (font units, wght)", &glyphs);
    for (page, s, tags) in singles.groups.iter().flat_map(|g| &g.items) {
        println!("{:<14} {tags:<20} {} subpaths, start ({:.2}, {:.2}), size {:.2}×{:.2}", id(*page, s.order), s.subpaths, s.start.x, s.start.y, s.bbox.width(), s.bbox.height());
    }
}

fn id(page: usize, index: usize) -> String {
    format!("p{page}-{index}")
}

/// Groups of strokes moved onto a grid (one cell per group, relative positions kept), so the
/// files are pleasant to look at and coordinates stay small.
struct Layout<'a> {
    groups: Vec<Group<(usize, &'a Stroke, &'static str)>>,
    shift: Vec<Affine>,
}

impl<'a> Layout<'a> {
    fn grid(groups: Vec<(Option<&'static str>, Vec<(usize, &'a Stroke, &'static str)>)>) -> Self {
        let bounds: Vec<Rect> = groups.iter().map(|(_, items)| items.iter().map(|(_, s, _)| s.bbox).reduce(|a, b| a.union(b)).unwrap_or(Rect::ZERO)).collect();
        let cell = bounds.iter().map(|b| b.width().max(b.height())).fold(0.0, f64::max) + 1.0;
        let columns = (groups.len() as f64).sqrt().ceil().max(1.0) as usize;
        let shift = bounds
            .iter()
            .enumerate()
            .map(|(i, b)| {
                let (cx, cy) = ((i % columns) as f64 * cell, (i / columns) as f64 * cell);
                Affine::translate((cx - b.x0, cy - b.y0))
            })
            .collect();
        let groups = groups.into_iter().map(|(name, items)| Group { id: name.map(str::to_string), note: None, items }).collect();
        Self { groups, shift }
    }

    fn strokes(&self) -> Vec<Group<data::Stroke>> {
        self.groups
            .iter()
            .zip(&self.shift)
            .map(|(g, &shift)| Group {
                id: g.id.clone(),
                note: g.note.clone(),
                items: g
                    .items
                    .iter()
                    .map(|&(page, s, tags)| data::Stroke {
                        id: id(page, s.order),
                        tags: tags.split_whitespace().map(str::to_string).collect(),
                        source: Some(format!("test.pdf page {page} stroke {} at ({:.2}, {:.2})", s.order, s.bbox.x0, s.bbox.y0)),
                        kind: match s.shape {
                            StrokeShape::Outline { .. } => StrokeKind::Outline,
                            StrokeShape::Line { width } => StrokeKind::Line { width },
                        },
                        path: shift * s.path.clone(),
                    })
                    .collect(),
            })
            .collect()
    }

    fn skeletons(&self) -> Vec<Group<Skeleton>> {
        self.groups
            .iter()
            .zip(&self.shift)
            .map(|(g, &shift)| Group {
                id: g.id.clone(),
                note: g.note.clone(),
                items: g
                    .items
                    .iter()
                    .map(|&(page, s, tags)| {
                        let sk = scriptoria::skeleton(s);
                        Skeleton {
                            id: id(page, s.order),
                            tags: tags.split_whitespace().map(str::to_string).collect(),
                            source: Some(format!("skeleton of test.pdf page {page} stroke {}", s.order)),
                            pieces: sk
                                .pieces
                                .iter()
                                .map(|p| SkeletonPiece {
                                    circles: p.samples.iter().map(|c| (shift * c.p, c.r)).collect(),
                                    spine: p.spine.iter().map(|c| shift * *c).collect(),
                                })
                                .collect(),
                        }
                    })
                    .collect(),
            })
            .collect()
    }
}
