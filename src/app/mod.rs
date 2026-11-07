//! The application's windows and views (GPUI): the main window ([`workspace`]) with the glyph
//! grid ([`font_view`]) and the handwritten pages ([`viewer`]), sharing the open project
//! ([`document`]).

pub mod document;
pub mod font_view;
pub mod mesh;
pub mod viewer;
pub mod workspace;

pub use workspace::{Workspace, init};
