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
    /// Stroke radius scales at the weight axis' minimum, default and maximum (wght 100, 400, 900).
    pub weight_scales: [f64; 3],
    /// How new strokes are placed: page units (pt) per em.
    pub pt_per_em: f64,
}

impl Default for FontInfo {
    fn default() -> Self {
        FontInfo { family: "Untitled".into(), units_per_em: 1000, ascender: 880, descender: -120, weight_scales: [0.3, 1.0, 3.0], pt_per_em: 4.5 }
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
        self.strokes.iter().map(|s| (s.placement * s.outline.path.clone()).bounding_box()).reduce(|a, b| a.union(b))
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

    #[test]
    fn initial_placement_puts_the_box_on_the_descender() {
        let info = FontInfo::default();
        let t = initial_placement(Rect::new(10.0, 20.0, 14.5, 24.5), &info);
        let (bottom_left, top_right) = (t * Point::new(10.0, 24.5), t * Point::new(14.5, 20.0));
        assert!((bottom_left - Point::new(0.0, -120.0)).hypot() < 1e-9);
        assert!((top_right - Point::new(1000.0, 880.0)).hypot() < 1e-9);
    }
}
