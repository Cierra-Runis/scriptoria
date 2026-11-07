//! The application's windows and views (GPUI): the main window ([`workspace`]) with the glyph
//! grid ([`font_view`]), the handwritten pages ([`viewer`]) and single glyphs ([`glyph_view`]),
//! sharing the open project ([`document`]).

pub mod document;
pub mod font_view;
pub mod glyph_view;
pub mod mesh;
pub mod viewer;
pub mod workspace;

pub use workspace::{Workspace, init};
