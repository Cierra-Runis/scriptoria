#![allow(dead_code)] // each tool uses a different part of this module

//! Shared by the tools in `examples/`.

use scriptoria_ink::InkDocument;
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
