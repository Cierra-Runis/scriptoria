//! A handwriting font project: the font's settings, its glyphs, and for each glyph the strokes it
//! is written with, in writing order, each placed in the em.
//!
//! Strokes are copied into the project (their outline as written and their skeleton, both in the
//! coordinates of the page they came from), so a project depends on no other file: documents
//! written in may move, change or go. A stroke's placement maps its page coordinates to font units
//! (y up). Where a stroke was written is not kept; a page's strokes already used are recognized by
//! their outlines ([`fingerprint`]).
//!
//! On disk a project is a folder:
//!
//! ```text
//! project.toml         format version, font settings, the glyph list (name, characters, advance,
//!                      file), the practice sheets made
//! glyphs/<file>.svg    one per glyph with strokes: each stroke a <g class="stroke"> whose
//!                      transform is its placement, holding its outline and its skeleton
//! parts/<file>.svg     one per part (strokes kept to put into glyphs), as a glyph's
//! ```
//!
//! The glyph files open in a browser, the glyph upright. See [`svg`] for their details.

mod skeleton;
mod style;
mod svg;

pub use skeleton::Node;

use std::path::{Path, PathBuf};

use kurbo::{Affine, BezPath, CubicBez, Point, Rect, Shape};
use serde::{Deserialize, Serialize};

/// The format version this crate writes and reads.
pub const FORMAT: u32 = 1;

/// The largest side a full-width glyph placed after a reference may have, as a fraction of the em:
/// neighbors do not touch.
pub const FULL_WIDTH_LIMIT: f64 = 0.96;

#[derive(Clone, Debug, PartialEq)]
pub struct Project {
    pub font: FontInfo,
    /// The glyphs, in font order.
    pub glyphs: Vec<Glyph>,
    /// The practice sheets made for it, to find their pages again when written.
    pub templates: Vec<Template>,
    /// Strokes kept to be put into glyphs again (radicals, components), by name.
    pub parts: Vec<Part>,
}

/// Strokes kept to be put into other glyphs (a radical, a component), placed as they were in the
/// glyph they were taken from.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub name: String,
    pub strokes: Vec<GlyphStroke>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FontInfo {
    pub family: String,
    pub units_per_em: u16,
    pub ascender: i16,
    pub descender: i16,
    /// The stroke radius scale of the thinnest weight (`wght` 100).
    #[serde(default = "default_thinnest")]
    pub thinnest: f64,
    /// The weight (`wght`) the strokes as written are: the boldest (900) by default. Radii are
    /// linear in the weight, from `thinnest` at 100 to the strokes as written here, and beyond.
    #[serde(default = "default_written_weight")]
    pub written_weight: f64,
    /// How large the hand writes: page units (pt) per em. New strokes are placed at this scale, and
    /// it sets their width in the font, which does not change with a glyph's size (see
    /// [`FontInfo::pen_scale`]); practice sheets' squares are the em at this size.
    pub pt_per_em: f64,
    /// The installed font whose glyphs show where characters sit and how big they are (see
    /// [`Glyph::fit`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<Reference>,
    /// How closely glyphs take the reference's size: 0 keeps the size written, 1 takes the
    /// reference's. Positions always follow the reference.
    #[serde(default = "default_fit_strength")]
    pub fit_strength: f64,
    /// How far glyphs placed after the reference take its proportions (width to height): 0 keeps
    /// the hand's, 1 takes the reference's.
    #[serde(default = "default_aspect_strength")]
    pub aspect_strength: f64,
    /// The family name in Latin letters, for software that wants one; `family` is then its
    /// Chinese name.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub english_name: String,
    /// The font's version (1.000 …).
    #[serde(default = "default_version")]
    pub version: f64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub copyright: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub designer: String,
    /// The terms the font may be used under.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub license: String,
    /// Extra space between lines (font units).
    #[serde(default)]
    pub line_gap: i16,
    /// The heights of lowercase and capital letters (font units; 0 for unknown).
    #[serde(default)]
    pub x_height: i16,
    #[serde(default)]
    pub cap_height: i16,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reference {
    pub file: PathBuf,
    /// The face, in a collection.
    #[serde(default)]
    pub face: u32,
}

fn default_written_weight() -> f64 {
    900.0
}

fn default_thinnest() -> f64 {
    0.3
}

fn default_fit_strength() -> f64 {
    1.0
}

fn default_version() -> f64 {
    1.0
}

fn default_aspect_strength() -> f64 {
    0.5
}

impl Default for FontInfo {
    fn default() -> Self {
        FontInfo {
            family: "Untitled".into(),
            units_per_em: 1000,
            ascender: 880,
            descender: -120,
            thinnest: default_thinnest(),
            written_weight: default_written_weight(),
            pt_per_em: DEFAULT_PT_PER_EM,
            reference: None,
            fit_strength: default_fit_strength(),
            aspect_strength: default_aspect_strength(),
            english_name: String::new(),
            version: default_version(),
            copyright: String::new(),
            designer: String::new(),
            license: String::new(),
            line_gap: 0,
            x_height: 0,
            cap_height: 0,
        }
    }
}

impl FontInfo {
    /// The scale strokes from freely written pages are placed at: font units per page unit, at
    /// [`FontInfo::pt_per_em`] (their [`GlyphStroke::pen`]). Also how closely outlines follow the
    /// strokes is set at this scale, for every glyph alike.
    pub fn pen_scale(&self) -> f64 {
        self.units_per_em as f64 / self.pt_per_em
    }
}

/// Points per em of a new project: the sample's handwriting (Goodnotes, `test/test.pdf`) is as large
/// as the reference font's characters at about 3.1 pt per em (`examples/handwriting-size.rs`).
pub const DEFAULT_PT_PER_EM: f64 = 3.1;

/// The cells across a practice sheet's page (as many rows as an A4-shaped page holds): each cell
/// a small part of the page, as handwriting is in a notebook.
pub const TEMPLATE_COLUMNS: usize = 24;

/// A practice sheet made for the project: its characters, a cell each, laid out by its grid (see
/// the template crate). Its id is on each page, so that written pages can be found again.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Template {
    pub id: u32,
    pub chars: String,
    pub grid: scriptoria_template::Grid,
}

impl Project {
    /// A new practice sheet for characters, kept with the project. Its squares are the em as large
    /// as the font takes handwriting to be ([`FontInfo::pt_per_em`]), so that characters written
    /// in them come out as those written anywhere else; its baseline is the font's.
    pub fn new_template(&mut self, chars: &[char]) -> &Template {
        let id = self.templates.iter().map(|t| t.id + 1).max().unwrap_or(1);
        let em = (self.font.ascender - self.font.descender) as f64;
        let cell = em / self.font.pen_scale();
        let grid = scriptoria_template::Grid::portrait(cell, TEMPLATE_COLUMNS, -self.font.descender as f64 / em);
        self.templates.push(Template { id, chars: chars.iter().collect(), grid });
        self.templates.last().expect("just added")
    }
}

/// Where a character sits in the em and how big it is: a reference glyph's box and advance (font
/// units, y up).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub bounds: Rect,
    pub advance: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Glyph {
    pub name: String,
    pub chars: Vec<char>,
    /// Advance width (font units).
    pub advance: f64,
    /// In writing order.
    pub strokes: Vec<GlyphStroke>,
}

impl Glyph {
    /// An empty glyph for a character, an em wide, named as fonts usually do.
    pub fn for_char(c: char, info: &FontInfo) -> Self {
        Glyph { name: glyph_name(c), chars: vec![c], advance: info.units_per_em as f64, strokes: Vec::new() }
    }

    /// The strokes' placed outlines' bounding box (font units), if there are strokes.
    pub fn bounds(&self) -> Option<Rect> {
        self.strokes_bounds(0..self.strokes.len())
    }

    /// Some strokes' placed outlines' bounding box (font units), if there are any.
    pub fn strokes_bounds(&self, which: impl IntoIterator<Item = usize>) -> Option<Rect> {
        which.into_iter().filter_map(|i| self.strokes.get(i)).map(GlyphStroke::bounds).reduce(|a, b| a.union(b))
    }

    /// Moves strokes in the em: `t` (font units) applies after their placement.
    pub fn transform_strokes(&mut self, which: &[usize], t: Affine) {
        for &i in which {
            if let Some(s) = self.strokes.get_mut(i) {
                s.placement = t * s.placement;
            }
        }
    }

    pub fn remove_strokes(&mut self, which: &[usize]) {
        let mut i = 0;
        self.strokes.retain(|_| {
            i += 1;
            !which.contains(&(i - 1))
        });
    }

    /// Moves strokes one place earlier (or later) in writing order, each past the nearest stroke
    /// not being moved; strokes already first (last) stay. Returns where the strokes are now.
    pub fn reorder_strokes(&mut self, which: &[usize], earlier: bool) -> Vec<usize> {
        let n = self.strokes.len();
        let mut moving: Vec<bool> = (0..n).map(|i| which.contains(&i)).collect();
        // Swaps of neighbors (i, i + 1) where the one moving passes the other.
        let swaps: Box<dyn Iterator<Item = usize>> = if earlier { Box::new(0..n.saturating_sub(1)) } else { Box::new((0..n.saturating_sub(1)).rev()) };
        for i in swaps {
            let (ahead, behind) = if earlier { (i + 1, i) } else { (i, i + 1) };
            if moving[ahead] && !moving[behind] {
                self.strokes.swap(i, i + 1);
                moving.swap(i, i + 1);
            }
        }
        (0..n).filter(|&i| moving[i]).collect()
    }

    /// The ink's box (font units) as the font draws it at the strokes' own weight: see
    /// [`GlyphStroke::ink_bounds`].
    pub fn ink_bounds(&self) -> Option<Rect> {
        self.strokes.iter().map(GlyphStroke::ink_bounds).reduce(|a, b| a.union(b))
    }

    fn all(&self) -> Vec<usize> {
        (0..self.strokes.len()).collect()
    }

    /// Moves all strokes sideways to have `left` before their ink, and makes the advance leave
    /// `right` after it.
    pub fn set_side_bearings(&mut self, left: f64, right: f64) {
        if let Some(b) = self.ink_bounds() {
            self.transform_strokes(&self.all(), Affine::translate((left - b.x0, 0.0)));
            self.advance = (left + b.width() + right).round();
        }
    }

    /// Moves all strokes sideways so that the left and right side bearings are equal.
    pub fn center(&mut self) {
        if let Some(b) = self.ink_bounds() {
            let dx = ((self.advance - b.width()) / 2.0 - b.x0).round();
            self.transform_strokes(&self.all(), Affine::translate((dx, 0.0)));
        }
    }

    /// Places the glyph as a full-width (CJK) character: an em wide, its strokes scaled so that the
    /// larger side of their box is `face` (a fraction) of the em, centered in the em square
    /// (descender to ascender).
    pub fn fit_full_width(&mut self, info: &FontInfo, face: f64) {
        let em = info.units_per_em as f64;
        self.advance = em;
        let longest = |b: Rect| b.width().max(b.height());
        if !self.scale_ink_to(longest, face * em) {
            return;
        }
        let b = self.ink_bounds().expect("strokes");
        let center = Point::new(em / 2.0, (info.ascender as f64 + info.descender as f64) / 2.0);
        self.transform_strokes(&self.all(), Affine::translate(center - b.center()));
    }

    /// Scales all strokes about their box's center until `measure` of the ink's box is `target`.
    /// The pen's width does not scale with the strokes, so a few rounds are taken. False without
    /// strokes.
    fn scale_ink_to(&mut self, measure: impl Fn(Rect) -> f64, target: f64) -> bool {
        for _ in 0..8 {
            let Some(b) = self.ink_bounds() else { return false };
            let k = target / measure(b).max(1e-9);
            if (k - 1.0).abs() < 1e-6 {
                break;
            }
            self.transform_strokes(&self.all(), scale_about(b.center(), k));
        }
        true
    }

    /// Places the glyph as the reference places its character (`frame`):
    /// - its proportions: drawn towards the reference's as far as the font's `aspect_strength`
    ///   goes (0 keeps the hand's), but for very long or very flat characters (一, 丨);
    /// - its size: the area of its box made the reference's, or for very long or very flat
    ///   characters the long side; `strength` (0 to 1) goes from the size written to the reference's;
    /// - across: a full-width reference (an em wide) has the box centered on the reference's, and
    ///   the advance an em, and it stays within [`FULL_WIDTH_LIMIT`] of the em; otherwise the
    ///   reference's side bearings around the box make the advance;
    /// - up and down: a reference sitting on the baseline (letters, digits) has the box sit there
    ///   too; otherwise the box is centered on the reference's.
    pub fn fit(&mut self, frame: &Frame, info: &FontInfo, strength: f64) {
        let Some(b) = self.ink_bounds() else { return };
        let r = frame.bounds;
        let em = info.units_per_em as f64;
        let long = |w: f64, h: f64| w.max(h) > 2.5 * w.min(h);
        // The hand's proportions drawn towards the reference's, as far as `aspect_strength` goes (the
        // area kept).
        let s = info.aspect_strength.clamp(0.0, 1.0);
        if s > 0.0 && !long(b.width(), b.height()) && !long(r.width(), r.height()) && b.area() > 0.0 && r.area() > 0.0 {
            let (aspect, reference) = (b.width() / b.height(), r.width() / r.height());
            let kx = (aspect.powf(1.0 - s) * reference.powf(s) / aspect).sqrt();
            let c = b.center().to_vec2();
            self.transform_strokes(&self.all(), Affine::translate(c) * Affine::scale_non_uniform(kx, 1.0 / kx) * Affine::translate(-c));
        }
        let Some(b) = self.ink_bounds() else { return };
        let wide = b.width() >= b.height();
        let measure = move |b: Rect| match (long(b.width(), b.height()) || long(r.width(), r.height()), wide) {
            (true, true) => b.width(),
            (true, false) => b.height(),
            (false, _) => b.area().sqrt(),
        };
        // From the size written towards the reference's, as far as `strength` goes.
        let (now, wanted) = (measure(b), measure(r));
        self.scale_ink_to(measure, now * (wanted / now.max(1e-9)).max(1e-3).powf(strength.clamp(0.0, 1.0)));
        let full_width = (frame.advance - em).abs() <= 0.02 * em;
        // A wide hand kept in proportion may outgrow the em: full-width characters stay inside it,
        // clear of their neighbors.
        let longest = |b: Rect| b.width().max(b.height());
        if full_width && longest(self.ink_bounds().expect("strokes")) > FULL_WIDTH_LIMIT * em {
            self.scale_ink_to(longest, FULL_WIDTH_LIMIT * em);
        }
        let b = self.ink_bounds().expect("strokes");
        let on_baseline = r.y0.abs() <= 0.02 * em;
        let dx = if full_width { r.center().x - b.center().x } else { r.x0 - b.x0 };
        let dy = if on_baseline { r.y0 - b.y0 } else { r.center().y - b.center().y };
        self.transform_strokes(&self.all(), Affine::translate((dx, dy)));
        self.advance = if full_width { frame.advance } else { (r.x0 + b.width() + frame.advance - r.x1).round() };
    }
}

fn scale_about(center: Point, k: f64) -> Affine {
    Affine::translate(center.to_vec2()) * Affine::scale(k) * Affine::translate(-center.to_vec2())
}

impl GlyphStroke {
    /// The ink's box (font units) at the stroke's own weight: its skeleton's circles where placed,
    /// their radii at the stroke's pen scale rather than scaled with the placement. Strokes
    /// without a skeleton give their outline's box.
    pub fn ink_bounds(&self) -> Rect {
        let circles = self.skeleton.iter().flat_map(|p| &p.circles);
        circles
            .map(|&(c, r)| {
                let c = self.placement * c;
                Rect::from_center_size(c, (2.0 * r * self.pen, 2.0 * r * self.pen))
            })
            .reduce(|a, b| a.union(b))
            .unwrap_or_else(|| self.bounds())
    }

    /// The placed outline's bounding box (font units).
    pub fn bounds(&self) -> Rect {
        let b = self.outline.path.bounding_box();
        let b = match self.outline.kind {
            OutlineKind::Filled => b,
            OutlineKind::Line { width } => b.inflate(width / 2.0, width / 2.0),
        };
        self.placement.transform_rect_bbox(b)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct GlyphStroke {
    /// The stroke as written (page coordinates).
    pub outline: Outline,
    /// Its skeleton (page coordinates): circles along a centerline, piece by piece.
    pub skeleton: Vec<SkeletonPiece>,
    /// Page coordinates to font units (y up).
    pub placement: Affine,
    /// Font units per page unit for the stroke's width: the scale it was first placed at. It does
    /// not change when the stroke is scaled later, so that strokes keep the width of the pen that
    /// wrote them, and glyphs scaled to balance one another still match in weight.
    pub pen: f64,
}

/// A stroke's outline as written, as a number: the same stroke of the same page gives the same
/// number however often it is read (in one run of the program: it is not kept), so that a page's
/// strokes already in glyphs are found again without noting where strokes came from.
pub fn fingerprint(path: &BezPath) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let mut point = |p: Point| (p.x.to_bits(), p.y.to_bits()).hash(&mut h);
    let mut tags = Vec::new();
    for el in path.elements() {
        match *el {
            kurbo::PathEl::MoveTo(p) => {
                tags.push(0u8);
                point(p);
            }
            kurbo::PathEl::LineTo(p) => {
                tags.push(1);
                point(p);
            }
            kurbo::PathEl::QuadTo(a, b) => {
                tags.push(2);
                point(a);
                point(b);
            }
            kurbo::PathEl::CurveTo(a, b, c) => {
                tags.push(3);
                point(a);
                point(b);
                point(c);
            }
            kurbo::PathEl::ClosePath => tags.push(4),
        }
    }
    tags.hash(&mut h);
    h.finish()
}

#[derive(Clone, Debug, PartialEq)]
pub struct Outline {
    pub kind: OutlineKind,
    pub path: BezPath,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OutlineKind {
    /// A filled outline (non-zero fill rule).
    Filled,
    /// A line of constant width with round caps and joins.
    Line { width: f64 },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SkeletonPiece {
    /// (center, radius) along the piece.
    pub circles: Vec<(Point, f64)>,
    /// `spine[i]` runs from circle i to circle i + 1.
    pub spine: Vec<CubicBez>,
}

/// A glyph's name: the character itself for ASCII letters and digits, `uniXXXX` (or `uXXXXX`
/// beyond the BMP) otherwise.
pub fn glyph_name(c: char) -> String {
    match c as u32 {
        _ if c.is_ascii_alphanumeric() => c.to_string(),
        u if u <= 0xFFFF => format!("uni{u:04X}"),
        u => format!("u{u:05X}"),
    }
}

/// The initial placement of strokes written inside `bounds` (page coordinates, y down): scaled by
/// the project's font units per page unit, y flipped, the box's left edge at x = 0 and its bottom
/// on the descender.
pub fn initial_placement(bounds: Rect, info: &FontInfo) -> Affine {
    let k = info.units_per_em as f64 / info.pt_per_em;
    Affine::new([k, 0.0, 0.0, -k, -bounds.x0 * k, bounds.y1 * k + info.descender as f64])
}

impl Project {
    pub fn new(family: impl Into<String>) -> Self {
        Project { font: FontInfo { family: family.into(), ..FontInfo::default() }, glyphs: Vec::new(), templates: Vec::new(), parts: Vec::new() }
    }

    /// The glyph mapped to a character.
    pub fn glyph_for(&self, c: char) -> Option<usize> {
        self.glyphs.iter().position(|g| g.chars.contains(&c))
    }

    /// Adds an empty glyph for every character of `text` without one yet (whitespace and
    /// control characters skipped), in order; returns how many were added.
    pub fn add_chars(&mut self, text: &str) -> usize {
        let mut have: std::collections::HashSet<char> = self.glyphs.iter().flat_map(|g| g.chars.iter().copied()).collect();
        let before = self.glyphs.len();
        for c in text.chars().filter(|c| !c.is_whitespace() && !c.is_control()) {
            if have.insert(c) {
                self.glyphs.push(Glyph::for_char(c, &self.font));
            }
        }
        self.glyphs.len() - before
    }

    pub fn load(dir: &Path) -> Result<Self, Error> {
        let text = std::fs::read_to_string(dir.join(PROJECT_FILE)).map_err(|e| Error::Io(dir.join(PROJECT_FILE), e))?;
        let file: ProjectFile = toml::from_str(&text).map_err(|e| Error::Toml(e.to_string()))?;
        if file.format != FORMAT {
            return Err(Error::Format(file.format));
        }
        let mut glyphs = Vec::with_capacity(file.glyph.len());
        for g in file.glyph {
            let strokes = match &g.file {
                Some(f) => {
                    let path = dir.join(f);
                    let text = std::fs::read_to_string(&path).map_err(|e| Error::Io(path.clone(), e))?;
                    svg::read_strokes(&text, file.font.pen_scale()).map_err(|e| Error::Svg(path, e))?
                }
                None => Vec::new(),
            };
            glyphs.push(Glyph { name: g.name, chars: g.chars.chars().collect(), advance: g.advance, strokes });
        }
        let mut parts = Vec::with_capacity(file.part.len());
        for p in file.part {
            let path = dir.join(&p.file);
            let text = std::fs::read_to_string(&path).map_err(|e| Error::Io(path.clone(), e))?;
            let strokes = svg::read_strokes(&text, file.font.pen_scale()).map_err(|e| Error::Svg(path, e))?;
            parts.push(Part { name: p.name, strokes });
        }
        Ok(Project { font: file.font, glyphs, templates: file.template, parts })
    }

    /// Writes the project into `dir` (created if needed): `project.toml` and a file per glyph
    /// with strokes. Glyph files no glyph uses any more are removed.
    pub fn save(&self, dir: &Path) -> Result<(), Error> {
        let glyph_dir = dir.join(GLYPH_DIR);
        std::fs::create_dir_all(&glyph_dir).map_err(|e| Error::Io(glyph_dir.clone(), e))?;
        let mut used = Vec::new();
        let mut entries = Vec::with_capacity(self.glyphs.len());
        for g in &self.glyphs {
            let file = (!g.strokes.is_empty()).then(|| format!("{GLYPH_DIR}/{}.svg", file_name(&g.name)));
            if let Some(f) = &file {
                write(&dir.join(f), &svg::write_strokes(g, &self.font))?;
                used.push(dir.join(f));
            }
            entries.push(GlyphEntry { name: g.name.clone(), chars: g.chars.iter().collect(), advance: g.advance, file });
        }
        let part_dir = dir.join(PART_DIR);
        let mut parts = Vec::with_capacity(self.parts.len());
        if !self.parts.is_empty() {
            std::fs::create_dir_all(&part_dir).map_err(|e| Error::Io(part_dir.clone(), e))?;
        }
        for (i, part) in self.parts.iter().enumerate() {
            let f = format!("{PART_DIR}/{}.svg", part_file_name(i, &part.name));
            let glyph = Glyph { name: part.name.clone(), chars: Vec::new(), advance: self.font.units_per_em as f64, strokes: part.strokes.clone() };
            write(&dir.join(&f), &svg::write_strokes(&glyph, &self.font))?;
            used.push(dir.join(&f));
            parts.push(PartEntry { name: part.name.clone(), file: f });
        }
        let file = ProjectFile { format: FORMAT, font: self.font.clone(), glyph: entries, template: self.templates.clone(), part: parts };
        write(&dir.join(PROJECT_FILE), &toml::to_string_pretty(&file).map_err(|e| Error::Toml(e.to_string()))?)?;
        for folder in [glyph_dir, part_dir].into_iter().filter(|d| d.is_dir()) {
            for entry in std::fs::read_dir(&folder).map_err(|e| Error::Io(folder.clone(), e))?.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "svg") && !used.contains(&path) {
                    std::fs::remove_file(&path).map_err(|e| Error::Io(path, e))?;
                }
            }
        }
        Ok(())
    }
}

const PROJECT_FILE: &str = "project.toml";
const GLYPH_DIR: &str = "glyphs";
const PART_DIR: &str = "parts";

/// A part's file name: its place in the list, then its name's letters and digits (others as code
/// points), so that any name makes a file name and no two parts share one.
fn part_file_name(index: usize, name: &str) -> String {
    let readable: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_string() } else { format!("u{:04X}", c as u32) }).collect();
    format!("{index:03}-{readable}")
}

/// Writes through a temporary file, so a failed write leaves the old file whole.
fn write(path: &Path, text: &str) -> Result<(), Error> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text).map_err(|e| Error::Io(tmp.clone(), e))?;
    std::fs::rename(&tmp, path).map_err(|e| Error::Io(path.to_path_buf(), e))
}

/// A glyph's file name: its name with an underscore after every capital letter (so names that
/// differ only in case get different files on case-insensitive file systems, as in UFO) and
/// characters files cannot hold replaced.
fn file_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for c in name.chars() {
        match c {
            'A'..='Z' => {
                out.push(c);
                out.push('_');
            }
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => out.push('_'),
            c if c.is_control() => out.push('_'),
            c => out.push(c),
        }
    }
    if out.starts_with('.') {
        out.insert(0, '_');
    }
    out
}

#[derive(Serialize, Deserialize)]
struct ProjectFile {
    format: u32,
    font: FontInfo,
    // Projects before 2026-11-08 also list `[[source]]`: the documents strokes came from (ignored).
    #[serde(default)]
    glyph: Vec<GlyphEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    template: Vec<Template>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    part: Vec<PartEntry>,
}

#[derive(Serialize, Deserialize)]
struct PartEntry {
    name: String,
    file: String,
}

#[derive(Serialize, Deserialize)]
struct GlyphEntry {
    name: String,
    chars: String,
    advance: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    file: Option<String>,
}

#[derive(Debug)]
pub enum Error {
    Io(PathBuf, std::io::Error),
    Toml(String),
    Svg(PathBuf, String),
    /// A format version this crate does not read.
    Format(u32),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(p, e) => write!(f, "{}: {e}", p.display()),
            Error::Toml(e) => write!(f, "project.toml: {e}"),
            Error::Svg(p, e) => write!(f, "{}: {e}", p.display()),
            Error::Format(v) => write!(f, "project format {v} (this version reads {FORMAT})"),
        }
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyph_names() {
        assert_eq!(glyph_name('A'), "A");
        assert_eq!(glyph_name('世'), "uni4E16");
        assert_eq!(glyph_name('𠀀'), "u20000");
        assert_eq!(file_name("A"), "A_");
        assert_eq!(file_name("a"), "a");
        assert_eq!(file_name(".notdef"), "_.notdef");
    }

    #[test]
    fn characters_are_added_once() {
        let mut p = Project::new("x");
        assert_eq!(p.add_chars("世界 世"), 2);
        assert_eq!(p.add_chars("界A"), 1);
        assert_eq!(p.glyphs.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), ["uni4E16", "uni754C", "A"]);
        assert_eq!(p.glyph_for('A'), Some(2));
    }

    /// A glyph of `n` unit-square strokes side by side, each placed as is; stroke i's square is
    /// at x = i.
    fn squares(n: usize) -> Glyph {
        let strokes = (0..n)
            .map(|i| GlyphStroke {
                outline: Outline { kind: OutlineKind::Filled, path: Rect::new(i as f64, 0.0, i as f64 + 1.0, 1.0).to_path(1e-3) },
                skeleton: Vec::new(),
                placement: Affine::IDENTITY,
                pen: FontInfo::default().pen_scale(),
            })
            .collect();
        Glyph { name: "x".into(), chars: vec!['x'], advance: 10.0, strokes }
    }

    fn order(g: &Glyph) -> Vec<usize> {
        // Which square each is: where its outline starts.
        g.strokes.iter().map(|s| s.outline.path.bounding_box().x0 as usize).collect()
    }

    #[test]
    fn strokes_are_reordered_as_a_block() {
        let mut g = squares(5);
        assert_eq!(g.reorder_strokes(&[1, 3], true), [0, 2]);
        assert_eq!(order(&g), [1, 0, 3, 2, 4]);
        // The first stroke cannot move earlier; the other one closes up to it.
        assert_eq!(g.reorder_strokes(&[0, 2], true), [0, 1]);
        assert_eq!(order(&g), [1, 3, 0, 2, 4]);
        assert_eq!(g.reorder_strokes(&[3, 4], false), [3, 4]);
        assert_eq!(order(&g), [1, 3, 0, 2, 4]);
        assert_eq!(g.reorder_strokes(&[0], false), [1]);
        assert_eq!(order(&g), [3, 1, 0, 2, 4]);
    }

    #[test]
    fn strokes_are_removed_and_moved() {
        let mut g = squares(4);
        g.remove_strokes(&[0, 2]);
        assert_eq!(order(&g), [1, 3]);
        g.transform_strokes(&[1], Affine::translate((10.0, 5.0)));
        assert_eq!(g.strokes_bounds([1]), Some(Rect::new(13.0, 5.0, 14.0, 6.0)));
        assert_eq!(g.bounds(), Some(Rect::new(1.0, 0.0, 14.0, 6.0)));
    }

    #[test]
    fn ink_keeps_the_pens_width_when_scaled() {
        let info = FontInfo::default();
        let pen = info.pen_scale();
        let mut g = squares(1);
        g.strokes[0].skeleton = vec![SkeletonPiece { circles: vec![(Point::new(0.0, 0.0), 0.1), (Point::new(1.0, 0.0), 0.1)], spine: Vec::new() }];
        g.transform_strokes(&[0], Affine::scale(200.0));
        // The centers move with the placement, the radius is the pen's: 0.1 pt.
        let r = 0.1 * pen;
        let b = g.ink_bounds().unwrap();
        for (got, want) in [(b.x0, -r), (b.x1, 200.0 + r), (b.y0, -r), (b.y1, r)] {
            assert!((got - want).abs() < 1e-9, "{b:?}");
        }
    }

    /// A glyph of one box (no skeleton), its ink `w` × `h` at the origin.
    fn boxed(w: f64, h: f64) -> Glyph {
        let mut g = squares(1);
        g.strokes[0].outline.path = Rect::new(0.0, 0.0, w, h).to_path(1e-3);
        g
    }

    fn close(a: Rect, b: Rect) -> bool {
        [(a.x0, b.x0), (a.x1, b.x1), (a.y0, b.y0), (a.y1, b.y1)].iter().all(|(p, q)| (p - q).abs() < 1e-6)
    }

    #[test]
    fn glyphs_take_the_reference_place_and_size() {
        let info = FontInfo::default();
        // A full-width character: the area is the reference's, the box centered on it.
        let mut g = boxed(400.0, 350.0);
        g.fit(&Frame { bounds: Rect::new(100.0, -50.0, 900.0, 650.0), advance: 1000.0 }, &info, 1.0);
        assert!(close(g.bounds().unwrap(), Rect::new(100.0, -50.0, 900.0, 650.0)), "{:?}", g.bounds());
        assert_eq!(g.advance, 1000.0);
        // 一: by its long side, its own thickness kept in proportion.
        let mut g = boxed(300.0, 30.0);
        g.fit(&Frame { bounds: Rect::new(50.0, 350.0, 950.0, 430.0), advance: 1000.0 }, &info, 1.0);
        assert!(close(g.bounds().unwrap(), Rect::new(50.0, 345.0, 950.0, 435.0)), "{:?}", g.bounds());
        // A letter sits on the baseline, its advance the reference's side bearings around it.
        let mut g = boxed(100.0, 140.0);
        g.strokes[0].placement = Affine::translate((30.0, -60.0));
        g.fit(&Frame { bounds: Rect::new(40.0, 0.0, 540.0, 700.0), advance: 600.0 }, &info, 1.0);
        assert!(close(g.bounds().unwrap(), Rect::new(40.0, 0.0, 540.0, 700.0)), "{:?}", g.bounds());
        assert_eq!(g.advance, 600.0);
        // A wide hand with the reference's area would outgrow the em: it is kept inside.
        let mut g = boxed(1000.0, 450.0);
        g.fit(&Frame { bounds: Rect::new(100.0, -50.0, 900.0, 750.0), advance: 1000.0 }, &info, 1.0);
        let b = g.bounds().unwrap();
        assert!((b.width() - FULL_WIDTH_LIMIT * 1000.0).abs() < 0.01 && (b.center().x - 500.0).abs() < 1e-6, "{b:?}");
        // At strength 0 the size stays, only the place changes.
        let mut g = boxed(400.0, 350.0);
        g.fit(&Frame { bounds: Rect::new(100.0, -50.0, 900.0, 650.0), advance: 1000.0 }, &info, 0.0);
        assert!(close(g.bounds().unwrap(), Rect::new(300.0, 125.0, 700.0, 475.0)), "{:?}", g.bounds());
    }

    #[test]
    fn proportions_go_towards_the_references_as_far_as_asked() {
        // A hand twice as wide as high, a square reference: its proportions kept, met halfway, taken.
        for (strength, aspect) in [(0.0, 2.0), (0.5, 2f64.sqrt()), (1.0, 1.0)] {
            let info = FontInfo { aspect_strength: strength, ..FontInfo::default() };
            let mut g = boxed(400.0, 200.0);
            g.fit(&Frame { bounds: Rect::new(300.0, 100.0, 700.0, 500.0), advance: 1000.0 }, &info, 1.0);
            let b = g.bounds().unwrap();
            assert!((b.width() / b.height() - aspect).abs() < 1e-6, "{strength}: {b:?}");
            assert!((b.area() - 400.0 * 400.0).abs() < 1.0, "the reference's area: {b:?}");
        }
    }

    #[test]
    fn fitting_reaches_the_reference_though_the_pen_does_not_scale() {
        let info = FontInfo::default();
        let mut g = squares(1);
        // A stroke 1 pt long, 0.1 pt in radius, written at the project's size.
        g.strokes[0].skeleton = vec![SkeletonPiece { circles: vec![(Point::new(0.0, 0.0), 0.1), (Point::new(1.0, 1.0), 0.1)], spine: Vec::new() }];
        g.strokes[0].placement = Affine::scale(info.pen_scale());
        let frame = Frame { bounds: Rect::new(100.0, -50.0, 900.0, 750.0), advance: 1000.0 };
        g.fit(&frame, &info, 1.0);
        let b = g.ink_bounds().unwrap();
        assert!((b.area().sqrt() - frame.bounds.area().sqrt()).abs() < 0.01, "{b:?}");
        assert!((b.center() - frame.bounds.center()).hypot() < 1e-6, "{b:?}");
    }

    #[test]
    fn glyphs_are_centered_and_fitted() {
        let mut g = squares(2);
        g.center();
        assert_eq!(g.bounds(), Some(Rect::new(4.0, 0.0, 6.0, 1.0)));
        let info = FontInfo::default();
        g.fit_full_width(&info, 0.9);
        assert_eq!(g.advance, 1000.0);
        let b = g.bounds().unwrap();
        // 2 × 1 scaled to 900 × 450, centered in the em square.
        for (got, want) in [(b.x0, 50.0), (b.x1, 950.0), (b.y0, 155.0), (b.y1, 605.0)] {
            assert!((got - want).abs() < 1e-9, "{b:?}");
        }
    }

    #[test]
    fn initial_placement_puts_the_box_on_the_descender() {
        let info = FontInfo::default();
        let e = info.pt_per_em;
        let t = initial_placement(Rect::new(10.0, 20.0, 10.0 + e, 20.0 + e), &info);
        let (bottom_left, top_right) = (t * Point::new(10.0, 20.0 + e), t * Point::new(10.0 + e, 20.0));
        assert!((bottom_left - Point::new(0.0, -120.0)).hypot() < 1e-9);
        assert!((top_right - Point::new(1000.0, 880.0)).hypot() < 1e-9);
    }

    #[test]
    fn practice_squares_are_the_em_as_handwriting_is_written() {
        let mut p = Project::new("x");
        let grid = p.new_template(&['世']).grid;
        // The em of 1000 units at the project's points per em, the baseline 120 units up.
        assert!((grid.cell - DEFAULT_PT_PER_EM).abs() < 1e-9 && (grid.baseline - 0.12).abs() < 1e-9, "{grid:?}");
        assert_eq!(grid.columns(), TEMPLATE_COLUMNS);
        assert!(grid.rows() > TEMPLATE_COLUMNS, "a page standing up: {grid:?}");
    }
}
