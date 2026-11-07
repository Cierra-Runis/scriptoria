//! Prints the raw structure of a test stroke's subpaths, how each was analyzed, and the recovered
//! circles.
//!
//! `cargo run -p scriptoria-skeleton --example dump -- <data file> <stroke id> [first subpath] [last subpath]`
//! (data file: `strokes.svg`, `characters.svg` or `drawing.svg` in `tests/data/`).

use kurbo::{ParamCurve, PathSeg, Point};
use scriptoria_skeleton::{Circle, SubpathShape, analyze_subpath, fit_circle, recover_line, recover_outline, split_segments};
use scriptoria_testkit::data::{StrokeKind, read_strokes};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data").join(&args[1]);
    let groups = read_strokes(&file);
    let stroke = groups.iter().flat_map(|g| &g.items).find(|s| s.id == args[2]).unwrap_or_else(|| panic!("no stroke {} in {}", args[2], args[1]));
    let subs = split_segments(&stroke.path);
    let from = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);
    let to = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(subs.len().saturating_sub(1));
    let sk = match stroke.kind {
        StrokeKind::Outline => recover_outline(&stroke.path),
        StrokeKind::Line { width } => recover_line(&stroke.path, width / 2.0),
    };
    println!("{} ({}): {} subpaths, {} circles, breaks {}, fallbacks {}", stroke.id, stroke.tags.join(" "), subs.len(), sk.circle_count(), sk.quality.breaks, sk.quality.fallbacks);
    for (i, s) in sk.samples().enumerate() {
        println!("  circle {i}: ({:.4}, {:.4}) r {:.4}", s.p.x, s.p.y, s.r);
    }
    let show = |c: Option<Circle>, pts: &[Point]| match c {
        Some(c) => format!("({:.4}, {:.4}) r {:.4} rms {:.3}", c.c.x, c.c.y, c.r, c.relative_rms(pts)),
        None => "no fit".into(),
    };
    for k in from..=to.min(subs.len().saturating_sub(1)) {
        println!("subpath {k}:");
        for seg in &subs[k] {
            let (a, b) = (seg.start(), seg.end());
            let kind = match seg {
                PathSeg::Line(_) if (b - a).hypot() < 1e-3 => "L0",
                PathSeg::Line(_) => "L ",
                _ => "C ",
            };
            println!("   {kind} ({:.4}, {:.4}) → ({:.4}, {:.4})  len {:.4}", a.x, a.y, b.x, b.y, (b - a).hypot());
        }
        match analyze_subpath(&subs[k]) {
            SubpathShape::Capsule { start, end, sides } => {
                println!("   capsule: start {} pts {}", start.len(), show(fit_circle(&start), &start));
                println!("            end   {} pts {}", end.len(), show(fit_circle(&end), &end));
                match sides {
                    Some((f, b)) => {
                        for (name, s) in [("fwd", f), ("bwd", b)] {
                            let (a, z) = (s.start(), s.end());
                            println!("            {name} side ({:.4}, {:.4}) → ({:.4}, {:.4})", a.x, a.y, z.x, z.y);
                        }
                    }
                    None => println!("            no sides"),
                }
            }
            SubpathShape::Blob(p) => println!("   blob: {} pts {}", p.len(), show(fit_circle(&p), &p)),
        }
    }
}
