//! The open project, shared by the views. Changes go through here: it marks the project modified
//! and tells the views which glyphs changed (each glyph has a revision number for their caches).

use std::path::{Path, PathBuf};

use gpui::{Context, EventEmitter};
use scriptoria_project::Project;

pub struct Document {
    pub project: Project,
    /// The project's folder, once opened or saved.
    pub folder: Option<PathBuf>,
    pub modified: bool,
    /// Per glyph, bumped whenever it changes.
    revisions: Vec<u64>,
    next_revision: u64,
}

#[derive(Clone, Debug)]
pub enum DocumentEvent {
    /// Glyphs were added, removed or reordered.
    GlyphList,
    /// One glyph changed.
    Glyph(usize),
    /// The project was saved or replaced.
    Saved,
}

impl EventEmitter<DocumentEvent> for Document {}

impl Document {
    pub fn new(project: Project, folder: Option<PathBuf>) -> Self {
        let revisions = vec![0; project.glyphs.len()];
        Document { project, folder, modified: false, revisions, next_revision: 1 }
    }

    /// The glyph's revision: changes whenever the glyph does.
    pub fn revision(&self, glyph: usize) -> u64 {
        self.revisions.get(glyph).copied().unwrap_or(0)
    }

    /// The project's name for titles: its folder's name, or its family name before it is saved.
    pub fn title(&self) -> String {
        let name = self.folder.as_ref().and_then(|f| f.file_name()).map(|n| n.to_string_lossy().into_owned());
        let name = name.unwrap_or_else(|| self.project.font.family.clone());
        if self.modified { format!("{name} •") } else { name }
    }

    /// Adds an empty glyph for every new character of `text`; returns how many.
    pub fn add_chars(&mut self, text: &str, cx: &mut Context<Self>) -> usize {
        let added = self.project.add_chars(text);
        if added > 0 {
            self.sync_revisions();
            self.touch(cx);
            cx.emit(DocumentEvent::GlyphList);
        }
        added
    }

    /// Changes a glyph through `f`.
    pub fn update_glyph(&mut self, glyph: usize, f: impl FnOnce(&mut scriptoria_project::Glyph), cx: &mut Context<Self>) {
        let Some(g) = self.project.glyphs.get_mut(glyph) else { return };
        f(g);
        self.revisions[glyph] = self.next_revision;
        self.next_revision += 1;
        self.touch(cx);
        cx.emit(DocumentEvent::Glyph(glyph));
    }

    pub fn save(&mut self, folder: &Path, cx: &mut Context<Self>) -> Result<(), scriptoria_project::Error> {
        self.project.save(folder)?;
        self.folder = Some(folder.to_path_buf());
        self.modified = false;
        cx.emit(DocumentEvent::Saved);
        cx.notify();
        Ok(())
    }

    fn touch(&mut self, cx: &mut Context<Self>) {
        self.modified = true;
        cx.notify();
    }

    fn sync_revisions(&mut self) {
        while self.revisions.len() < self.project.glyphs.len() {
            self.revisions.push(self.next_revision);
            self.next_revision += 1;
        }
        self.revisions.truncate(self.project.glyphs.len());
    }
}
