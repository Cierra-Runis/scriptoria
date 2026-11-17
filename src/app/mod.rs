//! The application's windows and views (GPUI): the main window ([`workspace`]) with the glyph
//! grid ([`font_view`]), the handwritten pages ([`viewer`]), single glyphs ([`glyph_view`]) and
//! sample text ([`sample_view`]), sharing the open project ([`document`]) and the glyphs' outlines
//! ([`outlines`]), in the colors of [`theme`].

pub mod charset_picker;
pub mod document;
pub mod font_info;
pub mod font_view;
pub mod glyph_view;
pub mod mesh;
pub mod outlines;
pub mod part_uses;
pub mod sample_view;
pub mod theme;
pub mod viewer;
pub mod workspace;

pub use workspace::{Workspace, init};
