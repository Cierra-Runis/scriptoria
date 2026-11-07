//! Writes a variable TrueType font (`glyf` + `gvar`, one weight axis) from glyph outlines given at
//! a few points of the axis (masters).
//!
//! The outlines go in as they are: every contour keeps its place, its start point and its
//! direction, and contours may overlap (each glyph gets the `OVERLAP_SIMPLE` flag, so renderers fill
//! it with the non-zero rule). A tool reading the font finds a glyph's contours in the order they
//! were given. Cubic segments become quadratic splines, converted for all masters of a segment
//! together so that the masters keep the same points; the points between two consecutive
//! off-curve points are left implied, as TrueType allows.
//!
//! Each glyph may have its own masters. Between two neighboring masters the font interpolates
//! linearly in the axis' normalized coordinate (which is linear in the axis value on either side of
//! the default): every master other than the default gets a variation whose region rises from the
//! master below it to its own position and falls to the master above. The default master (at the
//! axis default) is the glyph itself. The variations form a hierarchy (the ends first, then each
//! master between two placed ones), storing at each master only what interpolating the coarser
//! ones misses, and nothing for a contour that misses by less than the source's tolerance: a
//! glyph pays for its intermediate masters only in the strokes that need them.
//!
//! Coordinates are font units with y up. TrueType draws outer contours clockwise; nothing here
//! changes the direction of a contour.

use kurbo::{BezPath, CubicBez, PathEl, Point, QuadSpline};
use write_fonts::tables::cmap::Cmap;
use write_fonts::tables::fvar::{AxisInstanceArrays, Fvar, InstanceRecord, VariationAxisRecord};
use write_fonts::read::tables::glyf::CurvePoint;
use write_fonts::tables::glyf::{Bbox, Contour, GlyfLocaBuilder, SimpleGlyph};
use write_fonts::tables::gvar::iup::iup_delta_optimize;
use write_fonts::tables::gvar::{GlyphDeltas, GlyphVariations, Gvar, Tent};
use write_fonts::tables::head::{Flags, Head, MacStyle};
use write_fonts::tables::hhea::Hhea;
use write_fonts::tables::hmtx::{Hmtx, LongMetric};
use write_fonts::tables::maxp::Maxp;
use write_fonts::tables::name::{Name, NameRecord};
use write_fonts::tables::os2::Os2;
use write_fonts::tables::post::Post;
use write_fonts::tables::stat::{AxisRecord, AxisValue, AxisValueTableFlags, Stat};
use write_fonts::types::{F2Dot14, FWord, Fixed, GlyphId, LongDateTime, NameId, Tag, UfWord};
use write_fonts::{FontBuilder, OffsetMarker};

/// A font to write.
#[derive(Clone, Debug)]
pub struct FontSource {
    pub family: String,
    pub units_per_em: u16,
    pub ascender: i16,
    pub descender: i16,
    /// The weight axis (`wght`) in its user values.
    pub axis: Axis,
    /// Named instances: (name, weight).
    pub instances: Vec<(String, f64)>,
    /// The glyphs, in glyph-id order after `.notdef` (which is added).
    pub glyphs: Vec<Glyph>,
    /// Largest distance (font units) of a quadratic spline from the cubic it replaces.
    pub accuracy: f64,
    /// How far (font units) a contour may stray from a master before it keeps its own deltas
    /// there (see [`build`]): 0 keeps every master exact, up to rounding.
    pub tolerance: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Axis {
    pub min: f64,
    pub default: f64,
    pub max: f64,
}

impl Axis {
    /// The normalized coordinate of a weight: −1 at `min`, 0 at `default`, 1 at `max`, linear in
    /// between on either side.
    pub fn normalize(&self, value: f64) -> f64 {
        if value < self.default {
            -(self.default - value) / (self.default - self.min)
        } else if value > self.default {
            (value - self.default) / (self.max - self.default)
        } else {
            0.0
        }
    }
}

#[derive(Clone, Debug)]
pub struct Glyph {
    pub name: String,
    pub chars: Vec<char>,
    /// Advance width (font units), the same at every weight.
    pub advance: f64,
    /// (weight, outline) by increasing weight, one at the axis default. The outlines must be
    /// compatible: the same contours with the same sequence of segments (lines, quadratic or cubic
    /// curves), each contour starting with a move and ending with a close.
    pub masters: Vec<(f64, BezPath)>,
}

#[derive(Debug)]
pub enum Error {
    /// A glyph lacks a master at the axis default or at one of its ends, has a master outside the
    /// axis, or masters out of
    /// order.
    Masters(String),
    /// The masters of a glyph are not compatible (contour or segment structure differs).
    Incompatible(String),
    /// A glyph's points do not fit 16-bit coordinates.
    OutOfRange(String),
    /// A table could not be built.
    Table(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Masters(s) => write!(f, "masters: {s}"),
            Error::Incompatible(s) => write!(f, "incompatible masters: {s}"),
            Error::OutOfRange(s) => write!(f, "coordinates out of range: {s}"),
            Error::Table(s) => write!(f, "table: {s}"),
        }
    }
}

impl std::error::Error for Error {}

/// One glyph's TrueType points at each of its masters (same structure), with its contours' ends.
struct Points {
    masters: Vec<Vec<CurvePoint>>,
    ends: Vec<usize>,
}

/// The font file.
pub fn build(source: &FontSource) -> Result<Vec<u8>, Error> {
    let axis = source.axis;
    let wght = Tag::new(b"wght");

    // .notdef: an empty rectangle, half an em wide.
    let notdef = notdef_points(source);
    let mut all_points: Vec<Points> = vec![notdef];
    let mut glyph_masters: Vec<Vec<f64>> = vec![vec![axis.default]];
    for g in &source.glyphs {
        check_masters(g, &axis)?;
        all_points.push(glyph_points(g, source.accuracy)?);
        glyph_masters.push(g.masters.iter().map(|m| m.0).collect());
    }
    let advances: Vec<f64> = std::iter::once(source.units_per_em as f64 / 2.0).chain(source.glyphs.iter().map(|g| g.advance)).collect();
    let names: Vec<&str> = std::iter::once(".notdef").chain(source.glyphs.iter().map(|g| g.name.as_str())).collect();

    // glyf, loca and gvar.
    let mut builder = GlyfLocaBuilder::new();
    let mut variations = Vec::new();
    let mut font_bbox: Option<Bbox> = None;
    let mut metrics = Vec::new();
    let (mut max_points, mut max_contours) = (0u16, 0u16);
    for (gid, (points, weights)) in all_points.iter().zip(&glyph_masters).enumerate() {
        let default = weights.iter().position(|&w| w == axis.default).expect("checked");
        let glyph = simple_glyph(&points.masters[default], &points.ends);
        builder.add_glyph(&glyph).map_err(|e| Error::Table(format!("glyf: {e:?}")))?;
        font_bbox = Some(font_bbox.map_or(glyph.bbox, |b| b.union(glyph.bbox)));
        metrics.push(LongMetric { advance: advances[gid].round() as u16, side_bearing: glyph.bbox.x_min });
        max_points = max_points.max(points.masters[default].len() as u16);
        max_contours = max_contours.max(points.ends.len() as u16);
        variations.push(GlyphVariations::new(GlyphId::new(gid as u32), glyph_deltas(points, weights, default, &axis, source.tolerance)?));
    }
    let (glyf, loca, loca_format) = builder.build();
    let gvar = Gvar::new(variations, 1).map_err(|e| Error::Table(format!("gvar: {e:?}")))?;
    let bbox = font_bbox.unwrap_or_default();

    // Names: the usual ones, then the axis and the instances from 256.
    let family = &source.family;
    let postscript: String = family.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    let mut name_records = vec![
        (1, family.clone()),
        (2, "Regular".to_string()),
        (3, format!("{family};Regular")),
        (4, format!("{family} Regular")),
        (5, "Version 1.000".to_string()),
        (6, format!("{postscript}-Regular")),
        (25, postscript.clone()),
        (256, "Weight".to_string()),
    ];
    for (i, (name, _)) in source.instances.iter().enumerate() {
        name_records.push((257 + i as u16, name.clone()));
    }
    let name = Name::new(
        name_records
            .iter()
            .map(|(id, text)| NameRecord::new(3, 1, 0x409, NameId::new(*id), OffsetMarker::new(text.clone())))
            .collect(),
    );

    let fvar = Fvar::new(AxisInstanceArrays::new(
        vec![VariationAxisRecord {
            axis_tag: wght,
            min_value: Fixed::from_f64(axis.min),
            default_value: Fixed::from_f64(axis.default),
            max_value: Fixed::from_f64(axis.max),
            flags: 0,
            axis_name_id: NameId::new(256),
        }],
        source
            .instances
            .iter()
            .enumerate()
            .map(|(i, (_, value))| InstanceRecord {
                subfamily_name_id: NameId::new(257 + i as u16),
                flags: 0,
                coordinates: vec![Fixed::from_f64(*value)],
                post_script_name_id: None,
            })
            .collect(),
    ));
    let stat = Stat::new(
        vec![AxisRecord::new(wght, NameId::new(256), 0)],
        source
            .instances
            .iter()
            .enumerate()
            .map(|(i, (_, value))| {
                let flags = if *value == axis.default { AxisValueTableFlags::ELIDABLE_AXIS_VALUE_NAME } else { AxisValueTableFlags::empty() };
                AxisValue::format_1(0, flags, NameId::new(257 + i as u16), Fixed::from_f64(*value))
            })
            .collect(),
        NameId::new(2),
    );

    let head = Head::new(
        Fixed::from_f64(1.0),
        0,
        Flags::BASELINE_AT_Y_0 | Flags::LSB_AT_X_0,
        source.units_per_em,
        LongDateTime::new(0),
        LongDateTime::new(0),
        bbox.x_min,
        bbox.y_min,
        bbox.x_max,
        bbox.y_max,
        MacStyle::empty(),
        8,
        loca_format as i16,
    );
    let advance_max = metrics.iter().map(|m| m.advance).max().unwrap_or(0);
    let hhea = Hhea::new(
        FWord::new(source.ascender),
        FWord::new(source.descender),
        FWord::new(0),
        UfWord::new(advance_max),
        FWord::new(metrics.iter().map(|m| m.side_bearing).min().unwrap_or(0)),
        FWord::new(0),
        FWord::new(bbox.x_max),
        1,
        0,
        0,
        metrics.len() as u16,
    );
    let num_glyphs = metrics.len() as u16;
    let mut maxp = Maxp::new(num_glyphs);
    maxp.max_points = Some(max_points);
    maxp.max_contours = Some(max_contours);
    maxp.max_composite_points = Some(0);
    maxp.max_composite_contours = Some(0);
    maxp.max_zones = Some(1);
    maxp.max_twilight_points = Some(0);
    maxp.max_storage = Some(0);
    maxp.max_function_defs = Some(0);
    maxp.max_instruction_defs = Some(0);
    maxp.max_stack_elements = Some(0);
    maxp.max_size_of_instructions = Some(0);
    maxp.max_component_elements = Some(0);
    maxp.max_component_depth = Some(0);
    let hmtx = Hmtx::new(metrics.clone(), Vec::new());
    let mappings = source.glyphs.iter().enumerate().flat_map(|(i, g)| g.chars.iter().map(move |&c| (c, GlyphId::new(i as u32 + 1))));
    let cmap = Cmap::from_mappings(mappings).map_err(|e| Error::Table(format!("cmap: {e:?}")))?;
    let chars: Vec<u32> = source.glyphs.iter().flat_map(|g| g.chars.iter().map(|&c| c as u32)).collect();
    let os2 = Os2 {
        x_avg_char_width: (metrics.iter().map(|m| m.advance as f64).sum::<f64>() / metrics.len().max(1) as f64).round() as i16,
        us_weight_class: axis.default.round() as u16,
        fs_selection: write_fonts::tables::os2::SelectionFlags::REGULAR | write_fonts::tables::os2::SelectionFlags::USE_TYPO_METRICS,
        us_first_char_index: chars.iter().copied().min().unwrap_or(0).min(0xFFFF) as u16,
        us_last_char_index: chars.iter().copied().max().unwrap_or(0).min(0xFFFF) as u16,
        s_typo_ascender: source.ascender,
        s_typo_descender: source.descender,
        s_typo_line_gap: 0,
        us_win_ascent: bbox.y_max.max(source.ascender).max(0) as u16,
        us_win_descent: (-bbox.y_min.min(source.descender)).max(0) as u16,
        ach_vend_id: Tag::new(b"NONE"),
        ul_code_page_range_1: Some(1),
        ul_code_page_range_2: Some(0),
        sx_height: Some(0),
        s_cap_height: Some(0),
        us_default_char: Some(0),
        us_break_char: Some(32),
        us_max_context: Some(0),
        ..Default::default()
    };
    let post = Post { version: write_fonts::types::Version16Dot16::VERSION_2_0, ..Post::new_v2(names.iter().copied()) };

    let mut font = FontBuilder::new();
    let add = |r: Result<&mut FontBuilder, write_fonts::BuilderError>| r.map(|_| ()).map_err(|e| Error::Table(format!("{e:?}")));
    add(font.add_table(&head))?;
    add(font.add_table(&hhea))?;
    add(font.add_table(&maxp))?;
    add(font.add_table(&os2))?;
    add(font.add_table(&hmtx))?;
    add(font.add_table(&cmap))?;
    add(font.add_table(&name))?;
    add(font.add_table(&post))?;
    add(font.add_table(&glyf))?;
    add(font.add_table(&loca))?;
    add(font.add_table(&fvar))?;
    add(font.add_table(&gvar))?;
    add(font.add_table(&stat))?;
    Ok(font.build())
}

fn check_masters(g: &Glyph, axis: &Axis) -> Result<(), Error> {
    let weights: Vec<f64> = g.masters.iter().map(|m| m.0).collect();
    if !weights.contains(&axis.default) {
        return Err(Error::Masters(format!("{} has no master at the default {}", g.name, axis.default)));
    }
    if weights.first() != Some(&axis.min) || weights.last() != Some(&axis.max) {
        return Err(Error::Masters(format!("{}: masters {weights:?} do not reach both ends of the axis", g.name)));
    }
    if weights.iter().any(|&w| w < axis.min || w > axis.max) || weights.windows(2).any(|w| w[0] >= w[1]) {
        return Err(Error::Masters(format!("{}: masters {weights:?} out of order or outside {}..{}", g.name, axis.min, axis.max)));
    }
    Ok(())
}

/// A rectangle with a hole, the usual `.notdef`.
fn notdef_points(source: &FontSource) -> Points {
    let w = source.units_per_em as i16 / 2;
    let (top, bottom) = (source.ascender, 0i16);
    let m = (w / 10).max(1);
    let rect = |x0: i16, y0: i16, x1: i16, y1: i16, clockwise: bool| {
        let mut pts = vec![(x0, y0), (x0, y1), (x1, y1), (x1, y0)];
        if !clockwise {
            pts.reverse();
        }
        pts.into_iter().map(|(x, y)| CurvePoint::on_curve(x, y)).collect::<Vec<_>>()
    };
    let mut points = rect(m, bottom, w - m, top, true);
    points.extend(rect(2 * m, bottom + m, w - 2 * m, top - m, false));
    Points { masters: vec![points], ends: vec![3, 7] }
}

/// A glyph's points at each of its masters.
fn glyph_points(g: &Glyph, accuracy: f64) -> Result<Points, Error> {
    let paths: Vec<&BezPath> = g.masters.iter().map(|m| &m.1).collect();
    let elements: Vec<&[PathEl]> = paths.iter().map(|p| p.elements()).collect();
    let n = elements[0].len();
    if elements.iter().any(|e| e.len() != n) {
        return Err(Error::Incompatible(format!("{}: different numbers of path elements", g.name)));
    }
    let count = paths.len();
    let mut masters: Vec<Vec<(f64, f64, bool)>> = vec![Vec::new(); count];
    let mut ends = Vec::new();
    let mut last = vec![Point::ZERO; count];
    let mut start = vec![Point::ZERO; count];
    let mut contour_start = 0;
    for i in 0..n {
        let els: Vec<PathEl> = elements.iter().map(|e| e[i]).collect();
        match els[0] {
            PathEl::MoveTo(_) => {
                for (k, el) in els.iter().enumerate() {
                    let PathEl::MoveTo(p) = *el else { return Err(incompatible(g, i)) };
                    masters[k].push((p.x, p.y, true));
                    last[k] = p;
                    start[k] = p;
                }
                contour_start = masters[0].len() - 1;
            }
            PathEl::LineTo(_) => {
                for (k, el) in els.iter().enumerate() {
                    let PathEl::LineTo(p) = *el else { return Err(incompatible(g, i)) };
                    masters[k].push((p.x, p.y, true));
                    last[k] = p;
                }
            }
            PathEl::QuadTo(..) => {
                for (k, el) in els.iter().enumerate() {
                    let PathEl::QuadTo(c, p) = *el else { return Err(incompatible(g, i)) };
                    masters[k].push((c.x, c.y, false));
                    masters[k].push((p.x, p.y, true));
                    last[k] = p;
                }
            }
            PathEl::CurveTo(..) => {
                let mut cubics = Vec::with_capacity(count);
                for (k, el) in els.iter().enumerate() {
                    let PathEl::CurveTo(c1, c2, p) = *el else { return Err(incompatible(g, i)) };
                    cubics.push(CubicBez::new(last[k], c1, c2, p));
                    last[k] = p;
                }
                let splines = compatible_quadratics(&cubics, accuracy);
                for (k, spline) in splines.iter().enumerate() {
                    let pts = spline.points();
                    // Off-curve points (the on-curve ones between them are implied), then the end.
                    for c in &pts[1..pts.len() - 1] {
                        masters[k].push((c.x, c.y, false));
                    }
                    let p = pts[pts.len() - 1];
                    masters[k].push((p.x, p.y, true));
                }
            }
            PathEl::ClosePath => {
                if els.iter().any(|e| *e != PathEl::ClosePath) {
                    return Err(incompatible(g, i));
                }
                // A contour closes by itself: drop a last point that repeats the first, in all
                // masters alike.
                let repeats = (0..count).all(|k| {
                    let (x, y, on) = *masters[k].last().unwrap();
                    on && masters[k].len() > contour_start + 1 && (x - start[k].x).abs() < 1e-9 && (y - start[k].y).abs() < 1e-9
                });
                if repeats {
                    for m in &mut masters {
                        m.pop();
                    }
                }
                ends.push(masters[0].len() - 1);
            }
        }
    }
    let masters = masters
        .into_iter()
        .map(|pts| {
            pts.into_iter()
                .map(|(x, y, on)| {
                    let (x, y) = (x.round(), y.round());
                    if x.abs() > i16::MAX as f64 || y.abs() > i16::MAX as f64 {
                        return Err(Error::OutOfRange(g.name.clone()));
                    }
                    Ok(if on { CurvePoint::on_curve(x as i16, y as i16) } else { CurvePoint::off_curve(x as i16, y as i16) })
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Points { masters, ends })
}

fn incompatible(g: &Glyph, element: usize) -> Error {
    Error::Incompatible(format!("{}: path element {element} differs between masters", g.name))
}

/// Quadratic splines for the same cubic at several masters, all with the same number of pieces,
/// each within `accuracy` of its cubic.
fn compatible_quadratics(cubics: &[CubicBez], accuracy: f64) -> Vec<QuadSpline> {
    kurbo::cubics_to_quadratic_splines(cubics, accuracy)
        .or_else(|| kurbo::cubics_to_quadratic_splines(cubics, accuracy * 4.0))
        .unwrap_or_else(|| {
            // Degenerate cubics the conversion gives up on: one quadratic each, through the
            // midpoint of the handles (always compatible).
            cubics.iter().map(|c| QuadSpline::new(vec![c.p0, c.p1.midpoint(c.p2), c.p3])).collect()
        })
}

fn simple_glyph(points: &[CurvePoint], ends: &[usize]) -> SimpleGlyph {
    let mut contours = Vec::with_capacity(ends.len());
    let mut start = 0;
    for &end in ends {
        contours.push(Contour::from(points[start..=end].to_vec()));
        start = end + 1;
    }
    let mut glyph = SimpleGlyph { bbox: Bbox::default(), contours, instructions: Vec::new(), overlaps: true };
    glyph.recompute_bounding_box();
    glyph
}

/// The variations of a glyph, as a hierarchy over its masters: the default first, then the masters
/// at the axis' ends (each varying from the default), then the rest coarse to fine, each between
/// the two masters already placed around it ([`hierarchy`]). A master's region rises from one of
/// those to its own position and falls to the other, so the outlines at the placed masters stay
/// as they are, and its deltas are only what the straight interpolation between them misses.
///
/// A contour whose deltas there all stay within `tolerance` gets none (so a stroke that varies
/// linearly over a stretch costs nothing at the masters inside it), and deltas the renderer can
/// infer from their neighbors within half a unit are left out (IUP). Each master's deltas are
/// taken against what the coarser variations actually give, inferred deltas included, so the
/// glyph lies within the tolerance, plus rounding and inference, of every master: the errors do
/// not add up from level to level. The four phantom points (the metrics) do not vary.
fn glyph_deltas(points: &Points, weights: &[f64], default: usize, axis: &Axis, tolerance: f64) -> Result<Vec<GlyphDeltas>, Error> {
    use kurbo::{Point as P, Vec2 as V};
    let base = &points.masters[default];
    let at = |m: usize| -> Vec<V> { points.masters[m].iter().map(|p| V::new(p.x as f64, p.y as f64)).collect() };
    let coords: Vec<P> = base.iter().map(|p| P::new(p.x as f64, p.y as f64)).chain([P::ZERO; 4]).collect();
    let n: Vec<f64> = weights.iter().map(|&w| axis.normalize(w)).collect();
    let f2 = |x: f64| F2Dot14::from_f32(x as f32);
    // What the variations placed so far give at each placed master.
    let mut placed: Vec<Option<Vec<V>>> = vec![None; weights.len()];
    placed[default] = Some(at(default));
    let mut out = Vec::new();
    for (m, lower, upper) in hierarchy(&n, default) {
        let target = at(m);
        let (lo, hi) = (placed[lower].as_ref().unwrap(), placed[upper].as_ref().unwrap());
        let t = if n[upper] != n[lower] { (n[m] - n[lower]) / (n[upper] - n[lower]) } else { 0.0 };
        let before: Vec<V> = lo.iter().zip(hi).map(|(a, b)| *a + (*b - *a) * t).collect();
        let mut deltas: Vec<V> = target.iter().zip(&before).map(|(p, q)| V::new((p.x - q.x).round(), (p.y - q.y).round())).collect();
        let mut start = 0;
        for &end in &points.ends {
            let contour = &mut deltas[start..=end];
            if contour.iter().zip(&target[start..=end]).zip(&before[start..=end]).all(|((_, p), q)| (*p - *q).hypot() <= tolerance) {
                contour.iter_mut().for_each(|d| *d = V::ZERO);
            }
            start = end + 1;
        }
        if deltas.iter().all(|d| *d == V::ZERO) {
            placed[m] = Some(before);
            continue;
        }
        let full: Vec<V> = deltas.into_iter().chain([V::ZERO; 4]).collect();
        let optimized = iup_delta_optimize(full.clone(), coords.clone(), 0.5, &points.ends).map_err(|e| Error::Table(format!("iup: {e:?}")))?;
        let required: Vec<bool> = optimized.iter().map(|d| d.required).collect();
        let applied = infer_deltas(&full, &required, &coords, &points.ends);
        placed[m] = Some(before.iter().zip(&applied).map(|(q, d)| *q + *d).collect());
        let region = if lower == upper { (f2(n[m].min(0.0)), f2(n[m].max(0.0))) } else { (f2(n[lower]), f2(n[upper])) };
        out.push(GlyphDeltas::new(vec![Tent::new(f2(n[m]), Some(region))], optimized));
    }
    Ok(out)
}

/// The deltas a renderer applies when only the `required` ones are stored: the others inferred
/// from the stored ones around them in their contour (the gvar rules: for each coordinate, linear
/// between the two neighbors' coordinates in the default glyph, the nearer one's delta outside
/// that range, theirs if they coincide and agree, zero if they coincide and differ), a contour
/// without any stored deltas staying put. `deltas` and `coords` include the four phantom points
/// after the contours (each its own contour here).
fn infer_deltas(deltas: &[kurbo::Vec2], required: &[bool], coords: &[kurbo::Point], ends: &[usize]) -> Vec<kurbo::Vec2> {
    let mut out = vec![kurbo::Vec2::ZERO; ends.last().map_or(0, |e| e + 1)];
    let mut start = 0;
    for &end in ends {
        let refs: Vec<usize> = (start..=end).filter(|&i| required[i]).collect();
        for i in start..=end {
            let d = if required[i] {
                deltas[i]
            } else if refs.is_empty() {
                kurbo::Vec2::ZERO
            } else {
                // The stored points before and after i in the contour (cyclically).
                let before = *refs.iter().rev().find(|&&j| j < i).or(refs.last()).unwrap();
                let after = *refs.iter().find(|&&j| j > i).or(refs.first()).unwrap();
                let axis = |c: fn(kurbo::Point) -> f64, dv: fn(kurbo::Vec2) -> f64| {
                    let (c1, c2, ci) = (c(coords[before]), c(coords[after]), c(coords[i]));
                    let (d1, d2) = (dv(deltas[before]), dv(deltas[after]));
                    if c1 == c2 {
                        if d1 == d2 { d1 } else { 0.0 }
                    } else {
                        let (lo, hi, dlo, dhi) = if c1 < c2 { (c1, c2, d1, d2) } else { (c2, c1, d2, d1) };
                        if ci <= lo {
                            dlo
                        } else if ci >= hi {
                            dhi
                        } else {
                            dlo + (dhi - dlo) * (ci - lo) / (hi - lo)
                        }
                    }
                };
                kurbo::Vec2::new(axis(|p| p.x, |v| v.x), axis(|p| p.y, |v| v.y))
            };
            out[i] = d;
        }
        start = end + 1;
    }
    out
}

/// The order in which a glyph's masters get their variations, by their normalized positions `n`
/// (increasing), with the two masters each lies between when it is placed: the masters at the
/// ends first (each against the default, which is both of its neighbors then), then, while any is
/// left, the one nearest the middle of the widest stretch between placed masters, against that
/// stretch's ends. (For masters from halving the range, as many as needed, this places each
/// master after the two it halves.)
fn hierarchy(n: &[f64], default: usize) -> Vec<(usize, usize, usize)> {
    let last = n.len() - 1;
    let mut placed = vec![false; n.len()];
    placed[default] = true;
    let mut order = Vec::new();
    for end in [0, last] {
        if !placed[end] {
            placed[end] = true;
            order.push((end, default, default));
        }
    }
    loop {
        // Stretches between consecutive placed masters that hold unplaced ones.
        let anchors: Vec<usize> = (0..n.len()).filter(|&i| placed[i]).collect();
        let next = anchors
            .windows(2)
            .filter(|w| w[1] > w[0] + 1)
            .max_by(|a, b| (n[a[1]] - n[a[0]]).total_cmp(&(n[b[1]] - n[b[0]])))
            .map(|w| {
                let middle = 0.5 * (n[w[0]] + n[w[1]]);
                let m = (w[0] + 1..w[1]).min_by(|&i, &j| (n[i] - middle).abs().total_cmp(&(n[j] - middle).abs())).unwrap();
                (m, w[0], w[1])
            });
        let Some((m, lower, upper)) = next else { break };
        placed[m] = true;
        order.push((m, lower, upper));
    }
    order
}
