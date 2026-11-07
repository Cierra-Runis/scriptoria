//! A handwriting font project: the font's settings, its glyphs, and for each glyph the strokes it
//! is written with, in writing order, each placed in the em.
//!
//! Strokes are copied into the project (their outline as written and their skeleton, both in the
//! coordinates of the page they came from, with a note of where that was), so a project opens
//! without its source documents. A stroke's placement maps its page coordinates to font units
//! (y up).
//!
//! On disk a project is a folder:
//!
//! ```text
//! project.toml         format version, font settings, sources, the glyph list (name, characters,
//!                      advance, file)
//! glyphs/<file>.svg    one per glyph with strokes: each stroke a <g class="stroke"> whose
//!                      transform is its placement, holding its outline and its skeleton
//! ```
//!
//! The glyph files open in a browser, the glyph upright. See [`svg`] for their details.

mod svg;

use std::path::{Path, PathBuf};

use kurbo::{Affine, BezPath, CubicBez, Point, Rect, Shape};
use serde::{Deserialize, Serialize};

/// The format version this crate writes and reads.
pub const FORMAT: u32 = 1;

#[derive(Clone, Debug, PartialEq)]
pub struct Project {
    pub font: FontInfo,
    /// Documents strokes were taken from.
    pub sources: Vec<Source>,
    /// The glyphs, in font order.
    pub glyphs: Vec<Glyph>,
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
    /// How new strokes are placed: page units (pt) per em.
    pub pt_per_em: f64,
}

fn default_written_weight() -> f64 {
    900.0
}

fn default_thinnest() -> f64 {
    0.3
}

impl Default for FontInfo {
    fn default() -> Self {
        FontInfo { family: "Untitled".into(), units_per_em: 1000, ascender: 880, descender: -120, thinnest: default_thinnest(), written_weight: default_written_weight(), pt_per_em: 4.5 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Source {
    /// How strokes refer to it.
    pub id: String,
    /// The document as it was imported (absolute, or relative to the project folder).
    pub file: PathBuf,
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

    /// Moves all strokes sideways so that the left and right side bearings are equal.
    pub fn center(&mut self) {
        if let Some(b) = self.bounds() {
            let dx = ((self.advance - b.width()) / 2.0 - b.x0).round();
            self.transform_strokes(&(0..self.strokes.len()).collect::<Vec<_>>(), Affine::translate((dx, 0.0)));
        }
    }

    /// Places the glyph as a full-width (CJK) character: an em wide, its strokes scaled so that the
    /// larger side of their box is `face` (a fraction) of the em, centered in the em square
    /// (descender to ascender).
    pub fn fit_full_width(&mut self, info: &FontInfo, face: f64) {
        let em = info.units_per_em as f64;
        self.advance = em;
        let Some(b) = self.bounds() else { return };
        let k = face * em / b.width().max(b.height()).max(1e-9);
        let center = Point::new(em / 2.0, (info.ascender as f64 + info.descender as f64) / 2.0);
        let t = Affine::translate(center.to_vec2()) * Affine::scale(k) * Affine::translate(-b.center().to_vec2());
        self.transform_strokes(&(0..self.strokes.len()).collect::<Vec<_>>(), t);
    }
}

impl GlyphStroke {
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
    /// Where the stroke was written, if known.
    pub origin: Option<StrokeOrigin>,
    /// The stroke as written (page coordinates).
    pub outline: Outline,
    /// Its skeleton (page coordinates): circles along a centerline, piece by piece.
    pub skeleton: Vec<SkeletonPiece>,
    /// Page coordinates to font units (y up).
    pub placement: Affine,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StrokeOrigin {
    pub source: String,
    pub page: usize,
    /// The stroke's index on its page, in writing order.
    pub index: usize,
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
        Project { font: FontInfo { family: family.into(), ..FontInfo::default() }, sources: Vec::new(), glyphs: Vec::new() }
    }

    /// The glyph mapped to a character.
    pub fn glyph_for(&self, c: char) -> Option<usize> {
        self.glyphs.iter().position(|g| g.chars.contains(&c))
    }

    /// Adds an empty glyph for every character of `text` without one yet (whitespace and
    /// control characters skipped), in order; returns how many were added.
    pub fn add_chars(&mut self, text: &str) -> usize {
        let mut added = 0;
        for c in text.chars().filter(|c| !c.is_whitespace() && !c.is_control()) {
            if self.glyph_for(c).is_none() {
                self.glyphs.push(Glyph::for_char(c, &self.font));
                added += 1;
            }
        }
        added
    }

    /// The source with this file, added if new; returns its id.
    pub fn source_for(&mut self, file: &Path) -> String {
        if let Some(s) = self.sources.iter().find(|s| s.file == file) {
            return s.id.clone();
        }
        let id = format!("s{}", self.sources.len() + 1);
        self.sources.push(Source { id: id.clone(), file: file.to_path_buf() });
        id
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
                    svg::read_strokes(&text).map_err(|e| Error::Svg(path, e))?
                }
                None => Vec::new(),
            };
            glyphs.push(Glyph { name: g.name, chars: g.chars.chars().collect(), advance: g.advance, strokes });
        }
        Ok(Project { font: file.font, sources: file.source, glyphs })
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
        let file = ProjectFile { format: FORMAT, font: self.font.clone(), source: self.sources.clone(), glyph: entries };
        write(&dir.join(PROJECT_FILE), &toml::to_string_pretty(&file).map_err(|e| Error::Toml(e.to_string()))?)?;
        for entry in std::fs::read_dir(&glyph_dir).map_err(|e| Error::Io(glyph_dir.clone(), e))?.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "svg") && !used.contains(&path) {
                std::fs::remove_file(&path).map_err(|e| Error::Io(path, e))?;
            }
        }
        Ok(())
    }
}

const PROJECT_FILE: &str = "project.toml";
const GLYPH_DIR: &str = "glyphs";

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
    #[serde(default)]
    source: Vec<Source>,
    #[serde(default)]
    glyph: Vec<GlyphEntry>,
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
                origin: Some(StrokeOrigin { source: "s1".into(), page: 0, index: i }),
                outline: Outline { kind: OutlineKind::Filled, path: Rect::new(i as f64, 0.0, i as f64 + 1.0, 1.0).to_path(1e-3) },
                skeleton: Vec::new(),
                placement: Affine::IDENTITY,
            })
            .collect();
        Glyph { name: "x".into(), chars: vec!['x'], advance: 10.0, strokes }
    }

    fn order(g: &Glyph) -> Vec<usize> {
        g.strokes.iter().map(|s| s.origin.as_ref().unwrap().index).collect()
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
        let t = initial_placement(Rect::new(10.0, 20.0, 14.5, 24.5), &info);
        let (bottom_left, top_right) = (t * Point::new(10.0, 24.5), t * Point::new(14.5, 20.0));
        assert!((bottom_left - Point::new(0.0, -120.0)).hypot() < 1e-9);
        assert!((top_right - Point::new(1000.0, 880.0)).hypot() < 1e-9);
    }
}
