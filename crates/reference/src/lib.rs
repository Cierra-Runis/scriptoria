//! A reference font: an installed typeface whose glyphs show where each character sits in the em
//! and how big it is, so that handwritten characters can be placed alike (a 口 smaller than a 国,
//! a 一 in the middle, letters on the baseline).
//!
//! Only boxes, advances and outlines (for display) are read; nothing of the reference font goes
//! into the fonts made from handwriting. Collections (`.ttc`) are read face by face, and files are
//! memory-mapped, as CJK collections run to a hundred megabytes.

use std::path::{Path, PathBuf};

use kurbo::{BezPath, Rect, Shape};
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::raw::{FileRef, ReadError};
use skrifa::string::StringId;
use skrifa::{FontRef, MetadataProvider};

/// A face of a font file.
#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    pub index: u32,
    /// Family and style names (the typographic ones where given).
    pub family: String,
    pub style: String,
}

/// A character's glyph in the reference font, in the units of the font it is wanted for (y up,
/// the baseline at 0).
#[derive(Clone, Debug)]
pub struct Glyph {
    pub bounds: Rect,
    pub advance: f64,
    pub outline: BezPath,
}

#[derive(Debug)]
pub enum Error {
    Io(PathBuf, std::io::Error),
    Font(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(p, e) => write!(f, "{}: {e}", p.display()),
            Error::Font(e) => write!(f, "not a font: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<ReadError> for Error {
    fn from(e: ReadError) -> Self {
        Error::Font(e.to_string())
    }
}

enum Data {
    Mapped(memmap2::Mmap),
    Owned(Vec<u8>),
}

impl std::ops::Deref for Data {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Data::Mapped(m) => m,
            Data::Owned(v) => v,
        }
    }
}

pub struct ReferenceFont {
    data: Data,
    face: Face,
    units_per_em: f64,
}

impl ReferenceFont {
    /// A face of a font file (the default one, see [`default_face`], if not given).
    pub fn open(path: &Path, face: Option<u32>) -> Result<Self, Error> {
        let file = std::fs::File::open(path).map_err(|e| Error::Io(path.to_path_buf(), e))?;
        // SAFETY: the file is only read; a font file changing while it is open is not expected.
        let map = unsafe { memmap2::Mmap::map(&file) }.map_err(|e| Error::Io(path.to_path_buf(), e))?;
        Self::new(Data::Mapped(map), face)
    }

    /// A font in memory.
    pub fn from_bytes(bytes: Vec<u8>, face: Option<u32>) -> Result<Self, Error> {
        Self::new(Data::Owned(bytes), face)
    }

    fn new(data: Data, face: Option<u32>) -> Result<Self, Error> {
        let faces = faces_of(&data)?;
        let index = face.unwrap_or_else(|| default_face(&faces));
        let face = faces.into_iter().find(|f| f.index == index).ok_or_else(|| Error::Font(format!("no face {index}")))?;
        let units_per_em = FontRef::from_index(&data, face.index)?.head()?.units_per_em() as f64;
        Ok(ReferenceFont { data, face, units_per_em })
    }

    pub fn face(&self) -> &Face {
        &self.face
    }

    fn font(&self) -> FontRef<'_> {
        FontRef::from_index(&self.data, self.face.index).expect("checked when opened")
    }

    /// The glyph of a character, scaled to `units_per_em`; none if the font lacks it or it draws
    /// nothing (a space).
    pub fn glyph(&self, c: char, units_per_em: f64) -> Option<Glyph> {
        let font = self.font();
        let gid = font.charmap().map(c)?;
        let k = units_per_em / self.units_per_em;
        let mut pen = Pen::default();
        let settings = DrawSettings::unhinted(Size::unscaled(), LocationRef::default());
        font.outline_glyphs().get(gid)?.draw(settings, &mut pen).ok()?;
        if pen.0.elements().is_empty() {
            return None;
        }
        let outline = kurbo::Affine::scale(k) * pen.0;
        let advance = font.glyph_metrics(Size::unscaled(), LocationRef::default()).advance_width(gid)? as f64 * k;
        Some(Glyph { bounds: outline.bounding_box(), advance, outline })
    }
}

use skrifa::raw::TableProvider as _;

/// The faces of a font file.
pub fn faces(path: &Path) -> Result<Vec<Face>, Error> {
    let file = std::fs::File::open(path).map_err(|e| Error::Io(path.to_path_buf(), e))?;
    // SAFETY: as in `ReferenceFont::open`.
    let map = unsafe { memmap2::Mmap::map(&file) }.map_err(|e| Error::Io(path.to_path_buf(), e))?;
    faces_of(&map)
}

fn faces_of(data: &[u8]) -> Result<Vec<Face>, Error> {
    let fonts: Vec<FontRef> = match FileRef::new(data)? {
        FileRef::Font(f) => vec![f],
        FileRef::Collection(c) => (0..c.len()).map(|i| c.get(i)).collect::<Result<_, _>>()?,
    };
    Ok(fonts
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let name = |preferred: StringId, fallback: StringId| {
                let s = f.localized_strings(preferred).english_or_first().or_else(|| f.localized_strings(fallback).english_or_first());
                s.map(|s| s.to_string()).unwrap_or_default()
            };
            Face {
                index: i as u32,
                family: name(StringId::TYPOGRAPHIC_FAMILY_NAME, StringId::FAMILY_NAME),
                style: name(StringId::TYPOGRAPHIC_SUBFAMILY_NAME, StringId::SUBFAMILY_NAME),
            }
        })
        .collect())
}

/// The face to use when none is named: a regular Simplified Chinese one if there is (CJK
/// collections hold every region and weight), else a regular one, else the first.
pub fn default_face(faces: &[Face]) -> u32 {
    let regular = |f: &&Face| f.style.eq_ignore_ascii_case("regular");
    let chinese = |f: &&Face| [" SC", " CN", "SC ", "Simplified"].iter().any(|t| f.family.contains(t));
    faces
        .iter()
        .filter(regular)
        .find(chinese)
        .or_else(|| faces.iter().find(regular))
        .or(faces.first())
        .map_or(0, |f| f.index)
}

/// An installed font that suits Chinese and Japanese handwriting, if one of the usual ones is
/// found: Source Han Sans or Noto Sans CJK.
pub fn system_default() -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(windows) = std::env::var_os("WINDIR") {
        dirs.push(PathBuf::from(windows).join("Fonts"));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local).join("Microsoft/Windows/Fonts"));
    }
    dirs.extend(["/Library/Fonts", "/System/Library/Fonts", "/usr/share/fonts/opentype/noto", "/usr/share/fonts/noto-cjk"].map(PathBuf::from));
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join("Library/Fonts"));
    }
    const NAMES: [&str; 6] = [
        "SourceHanSans.ttc",
        "SourceHanSansSC-Regular.otf",
        "SourceHanSans-Regular.ttc",
        "NotoSansCJK-Regular.ttc",
        "NotoSansCJKsc-Regular.otf",
        "NotoSansSC-Regular.otf",
    ];
    NAMES.iter().flat_map(|n| dirs.iter().map(move |d| d.join(n))).find(|p| p.is_file())
}

/// Collects a drawn glyph (y up, as in the font).
#[derive(Default)]
struct Pen(BezPath);

impl OutlinePen for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.move_to((x as f64, y as f64));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.line_to((x as f64, y as f64));
    }
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.0.quad_to((cx0 as f64, cy0 as f64), (x as f64, y as f64));
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.0.curve_to((cx0 as f64, cy0 as f64), (cx1 as f64, cy1 as f64), (x as f64, y as f64));
    }
    fn close(&mut self) {
        self.0.close_path();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(index: u32, family: &str, style: &str) -> Face {
        Face { index, family: family.into(), style: style.into() }
    }

    #[test]
    fn regular_simplified_chinese_faces_are_preferred() {
        let faces = [face(0, "Source Han Sans", "Bold"), face(1, "Source Han Sans J", "Regular"), face(2, "Source Han Sans SC", "Regular")];
        assert_eq!(default_face(&faces), 2);
        assert_eq!(default_face(&faces[..2]), 1);
        assert_eq!(default_face(&faces[..1]), 0);
        assert_eq!(default_face(&[]), 0);
    }
}
