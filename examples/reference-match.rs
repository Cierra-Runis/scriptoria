//! Which installed font is the best reference for a project's handwriting: for every CJK character
//! written (its strokes as written, before any fitting), its proportions (width to height) and its
//! size relative to the hand's typical size, against each font's same character. For each font:
//! how many of the characters it has; the hand's proportions against its, over all (bias) and
//! character by character once that is taken out (spread); the spread of relative sizes; and a
//! score, the two spreads together (smaller: the font's design relations are more the hand's).
//!
//! `cargo run --release --example reference-match -- <project folder> [font files…]`
//! (without font files: the Chinese fonts installed here)

use std::path::{Path, PathBuf};

use scriptoria_ink::kurbo::{Rect, Shape as _};
use scriptoria_project::Project;
use scriptoria_reference::ReferenceFont;

/// A character as written: its ink box in the page's units.
struct Written {
    c: char,
    ink: Rect,
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn candidates() -> Vec<PathBuf> {
    let system = PathBuf::from(r"C:\Windows\Fonts");
    let user = std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join(r"Microsoft\Windows\Fonts"));
    let names = [
        "SourceHanSans.ttc", "simkai.ttf", "STKAITI.TTF", "simfang.ttf", "STFANGSO.TTF", "simsun.ttc", "STSONG.TTF", "STZHONGS.TTF", "msyh.ttc",
        "simhei.ttf", "STXIHEI.TTF", "Deng.ttf", "STXINGKA.TTF", "STXINWEI.TTF", "STLITI.TTF", "SIMLI.TTF", "SIMYOU.TTF", "FZSTK.TTF", "FZYTK.TTF",
        "HarmonyOS_Sans_SC_Regular.ttf", "FZFWZhuZiMinchoR.TTF", "LXGWWenKai-Regular.ttf",
    ];
    names.iter().flat_map(|n| [Some(system.join(n)), user.as_ref().map(|u| u.join(n))]).flatten().filter(|p| p.is_file()).collect()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let project = Project::load(Path::new(args.first().expect("a project folder"))).expect("a project");
    let fonts: Vec<PathBuf> = if args.len() > 1 { args[1..].iter().map(PathBuf::from).collect() } else { candidates() };
    // The characters written, each from strokes placed together (written in one go).
    let written: Vec<Written> = project
        .glyphs
        .iter()
        .filter_map(|g| {
            let c = *g.chars.first()?;
            let first = g.strokes.first()?.placement;
            if !scriptoria_template::is_square(c) || g.strokes.iter().any(|s| s.placement != first) {
                return None;
            }
            let ink = g.strokes.iter().map(|s| s.outline.path.bounding_box()).reduce(|a, b| a.union(b))?;
            (ink.area() > 0.0).then_some(Written { c, ink })
        })
        .collect();
    println!("{} 个手写汉字\n", written.len());
    println!("{:<34} {:>6}  {:>16}  {:>10}  {:>10}  {:>6}", "字体", "覆盖", "宽高比偏差(整体)", "宽高比离散", "大小离散", "得分");
    let mut rows = Vec::new();
    for path in &fonts {
        let Ok(font) = ReferenceFont::open(path, None) else { continue };
        let pairs: Vec<(&Written, Rect)> = written.iter().filter_map(|w| font.glyph(w.c, 1000.0).map(|g| (w, g.bounds))).filter(|(_, b)| b.area() > 0.0).collect();
        if pairs.len() < written.len() / 2 {
            continue;
        }
        let aspect = |b: Rect| (b.width() / b.height()).ln();
        let diffs: Vec<f64> = pairs.iter().map(|(w, b)| aspect(w.ink) - aspect(*b)).collect();
        let bias = median(diffs.clone());
        let aspect_spread = median(diffs.iter().map(|d| (d - bias).abs()).collect());
        let hand_typical = median(pairs.iter().map(|(w, _)| w.ink.area().sqrt()).collect());
        let font_typical = median(pairs.iter().map(|(_, b)| b.area().sqrt()).collect());
        let size_spread = median(pairs.iter().map(|(w, b)| ((w.ink.area().sqrt() / hand_typical) / (b.area().sqrt() / font_typical)).ln().abs()).collect());
        let name = format!("{} {}", font.face().family, font.face().style);
        rows.push((aspect_spread + size_spread, name, pairs.len(), bias, aspect_spread, size_spread));
    }
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (score, name, n, bias, aspect, size) in rows {
        // Per cent: e^x − 1.
        let pc = |x: f64| (x.exp() - 1.0) * 100.0;
        println!("{name:<34} {n:>6}  {:>+15.0}%  {:>9.1}%  {:>9.1}%  {score:>6.3}", pc(bias), pc(aspect), pc(size));
    }
}
