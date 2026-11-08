//! How alike a project's glyphs are: for the CJK characters written, the spread of their size (ink
//! box against the em), of their strokes' width, and of the slope of their horizontal strokes —
//! what makes handwriting look uneven as a font. Reads the project only.
//!
//! With `--try`, the same again after the project is (in memory only) placed after its reference
//! font (or `--reference <font file>`), its proportions 70% of the way to the reference's made as wide
//! as the hand writes, and its strokes' widths evened out.
//!
//! `cargo run --example consistency -- <project folder> [--try] [--reference <font file>]`

use scriptoria_project::Project;

fn spread(name: &str, mut v: Vec<f64>, unit: &str) {
    if v.is_empty() {
        return;
    }
    v.sort_by(f64::total_cmp);
    let q = |p: f64| v[((v.len() - 1) as f64 * p).round() as usize];
    println!("{name}: 中位 {:.2}{unit}，10%–90% {:.2}–{:.2}{unit}，最小 {:.2}，最大 {:.2}（{} 个）", q(0.5), q(0.1), q(0.9), v[0], v[v.len() - 1], v.len());
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let folder = args.first().expect("a project folder");
    let mut project = Project::load(std::path::Path::new(folder)).expect("a project");
    report(&project);
    if args.iter().any(|a| a == "--try") {
        let reference = match args.iter().position(|a| a == "--reference") {
            Some(i) => scriptoria_reference::ReferenceFont::open(std::path::Path::new(&args[i + 1]), None).expect("the font"),
            None => scriptoria::assign::open_reference(project.font.reference.as_ref().expect("a reference font")).expect("the reference"),
        };
        let (bias, _) = scriptoria::assign::estimate_aspect_bias(&project, &reference).expect("glyphs to measure");
        println!(
            "\n参考字体 {} {}，宽高比偏好 {bias:.2}（按自己的字估计），宽高比贴合 0.7，统一笔宽之后：",
            reference.face().family,
            reference.face().style
        );
        project.font.aspect_strength = 0.7;
        project.font.aspect_bias = bias;
        let info = project.font.clone();
        for g in &mut project.glyphs {
            if g.chars.first().is_some_and(|&c| scriptoria_template::is_square(c)) && !g.strokes.is_empty() {
                scriptoria::assign::fit(g, &info, &reference);
            }
        }
        let width = project.typical_width().unwrap();
        for g in &mut project.glyphs {
            g.even_widths(width, 0.7);
        }
        report(&project);
    }
}

fn report(project: &Project) {
    let font = &project.font;
    let em = (font.ascender - font.descender) as f64;
    let (mut heights, mut widths, mut pens, mut slopes, mut glyph_slopes) = (vec![], vec![], vec![], vec![], vec![]);
    for g in &project.glyphs {
        let Some(&c) = g.chars.first() else { continue };
        if !scriptoria_template::is_square(c) || g.strokes.is_empty() {
            continue;
        }
        if let Some(ink) = g.ink_bounds() {
            heights.push(ink.height() / em);
            widths.push(ink.width() / em);
        }
        let mut horizontal = vec![];
        for s in &g.strokes {
            let radii: Vec<f64> = s.skeleton.iter().flat_map(|p| p.circles.iter().map(|c| c.1)).collect();
            if !radii.is_empty() {
                pens.push(radii.iter().sum::<f64>() / radii.len() as f64 * s.pen);
            }
            // A stroke's direction: from its first circle to its last, placed.
            let circles: Vec<_> = s.skeleton.iter().flat_map(|p| p.circles.iter().map(|c| s.placement * c.0)).collect();
            if let (Some(a), Some(b)) = (circles.first(), circles.last()) {
                let d = *b - *a;
                let len = d.hypot();
                let angle = d.y.atan2(d.x).to_degrees();
                // Horizontal strokes: written left to right, within 25° of level, at least a fifth of an em long.
                if len > 0.2 * em && angle.abs() < 25.0 {
                    slopes.push(angle);
                    horizontal.push(angle);
                }
            }
        }
        if !horizontal.is_empty() {
            glyph_slopes.push(horizontal.iter().sum::<f64>() / horizontal.len() as f64);
        }
    }
    println!("{}：{} 个字形", font.family, project.glyphs.len());
    spread("汉字墨迹高（em）", heights, "");
    spread("汉字墨迹宽（em）", widths, "");
    spread("笔画平均半径（字体单位）", pens, "");
    spread("横画倾斜（每一横，度，正为右上）", slopes, "°");
    spread("横画倾斜（每字平均，度）", glyph_slopes, "°");
}
