//! The open project, shared by the views. Changes go through here: it marks the project modified,
//! tells the views which glyphs changed (each glyph has a revision number for their caches) and
//! keeps what changed for undo. It also holds the project's reference font.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use gpui::{Context, EventEmitter};
use scriptoria_ink::Stroke;
use scriptoria_project::{FontInfo, Glyph, Project, Reference, StrokeOrigin};
use scriptoria_reference::ReferenceFont;

/// How many changes can be undone.
const UNDO_LIMIT: usize = 200;

/// What a change replaced: kept for undo (and what an undo replaced, for redo).
enum Change {
    /// Glyphs, by index, as they were.
    Glyphs(Vec<(usize, Glyph)>),
    Font(FontInfo),
}

pub struct Document {
    pub project: Project,
    /// The project's folder, once opened or saved.
    pub folder: Option<PathBuf>,
    pub modified: bool,
    /// Different for every document made, for caches.
    id: u64,
    /// The font characters are placed after, if any.
    reference: Option<Arc<ReferenceFont>>,
    /// Per glyph, bumped whenever it changes.
    revisions: Vec<u64>,
    next_revision: u64,
    undo: Vec<Change>,
    redo: Vec<Change>,
}

#[derive(Clone, Debug)]
pub enum DocumentEvent {
    /// Glyphs were added, removed or reordered.
    GlyphList,
    /// One glyph changed.
    Glyph(usize),
    /// The font's settings changed.
    Font,
    /// The project was saved or replaced.
    Saved,
}

impl EventEmitter<DocumentEvent> for Document {}

impl Document {
    /// A project to work on. Its reference font is opened; a project without one gets the font
    /// installed here, if any (see [`crate::assign::system_reference`]).
    pub fn new(mut project: Project, folder: Option<PathBuf>) -> Self {
        let reference = match &project.font.reference {
            Some(r) => crate::assign::open_reference(r).ok(),
            None => crate::assign::system_reference().map(|(r, font)| {
                project.font.reference = Some(r);
                font
            }),
        };
        let revisions = vec![0; project.glyphs.len()];
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        Document { project, folder, modified: false, id, reference: reference.map(Arc::new), revisions, next_revision: 1, undo: Vec::new(), redo: Vec::new() }
    }

    /// Different for every document made (a project opened twice is two documents).
    pub fn id(&self) -> u64 {
        self.id
    }

    /// The glyph's revision: changes whenever the glyph does.
    pub fn revision(&self, glyph: usize) -> u64 {
        self.revisions.get(glyph).copied().unwrap_or(0)
    }

    pub fn reference(&self) -> Option<&Arc<ReferenceFont>> {
        self.reference.as_ref()
    }

    /// Uses another reference font (or none), as a change to undo.
    pub fn set_reference(&mut self, reference: Option<Reference>, cx: &mut Context<Self>) -> Result<(), scriptoria_reference::Error> {
        self.reference = reference.as_ref().map(crate::assign::open_reference).transpose()?.map(Arc::new);
        self.update_font(|f| f.reference = reference, cx);
        Ok(())
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

    /// Changes a glyph through `f`, as one step to undo (none if `f` changes nothing).
    pub fn update_glyph(&mut self, glyph: usize, f: impl FnOnce(&mut Glyph), cx: &mut Context<Self>) {
        let mut f = Some(f);
        self.update_glyphs(&[glyph], |g| (f.take().expect("one glyph"))(g), cx);
    }

    /// Changes glyphs through `f`, all as one step to undo; returns how many changed.
    pub fn update_glyphs(&mut self, glyphs: &[usize], mut f: impl FnMut(&mut Glyph), cx: &mut Context<Self>) -> usize {
        let mut before = Vec::new();
        for &i in glyphs {
            let Some(g) = self.project.glyphs.get_mut(i) else { continue };
            let was = g.clone();
            f(g);
            if *g != was {
                before.push((i, was));
            }
        }
        let changed = before.len();
        if changed > 0 {
            for &(i, _) in &before {
                self.changed(i, cx);
            }
            self.record(Change::Glyphs(before));
        }
        changed
    }

    /// Places glyphs as the reference font places their characters (see
    /// [`crate::assign::fit`]), as one step to undo; returns how many the reference has.
    pub fn fit_to_reference(&mut self, glyphs: &[usize], cx: &mut Context<Self>) -> usize {
        let Some(reference) = self.reference.clone() else { return 0 };
        let info = self.project.font.clone();
        let mut found = 0;
        self.update_glyphs(
            glyphs,
            |g| {
                if !g.strokes.is_empty() && crate::assign::fit(g, &info, &reference) {
                    found += 1;
                }
            },
            cx,
        );
        found
    }

    /// Changes the font's settings through `f`, as one step to undo (none if `f` changes nothing).
    pub fn update_font(&mut self, f: impl FnOnce(&mut FontInfo), cx: &mut Context<Self>) {
        let before = self.project.font.clone();
        f(&mut self.project.font);
        if self.project.font != before {
            self.record(Change::Font(before));
            self.touch(cx);
            cx.emit(DocumentEvent::Font);
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Takes back the last change; false if there is none.
    pub fn undo(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(change) = self.undo.pop() else { return false };
        let undone = self.apply(change, cx);
        self.redo.push(undone);
        true
    }

    /// Makes the last undone change again; false if there is none.
    pub fn redo(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(change) = self.redo.pop() else { return false };
        let redone = self.apply(change, cx);
        self.undo.push(redone);
        true
    }

    /// Puts back what a change replaced; returns what that replaces in turn.
    fn apply(&mut self, change: Change, cx: &mut Context<Self>) -> Change {
        match change {
            Change::Glyphs(glyphs) => {
                let replaced = glyphs
                    .into_iter()
                    .map(|(i, g)| {
                        let was = std::mem::replace(&mut self.project.glyphs[i], g);
                        self.changed(i, cx);
                        (i, was)
                    })
                    .collect();
                Change::Glyphs(replaced)
            }
            Change::Font(font) => {
                if font.reference != self.project.font.reference {
                    self.reference = font.reference.as_ref().and_then(|r| crate::assign::open_reference(r).ok()).map(Arc::new);
                }
                let replaced = std::mem::replace(&mut self.project.font, font);
                self.touch(cx);
                cx.emit(DocumentEvent::Font);
                Change::Font(replaced)
            }
        }
    }

    /// Keeps what a change replaces, for undo.
    fn record(&mut self, change: Change) {
        if self.undo.len() == UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.undo.push(change);
        self.redo.clear();
    }

    /// Adds written strokes (from page `page` of source `source`, in writing order) to the end of a
    /// glyph (see [`crate::assign::assign`]).
    pub fn assign(&mut self, glyph: usize, source: &str, page: usize, strokes: &[Stroke], cx: &mut Context<Self>) {
        let Some(before) = self.project.glyphs.get(glyph).cloned() else { return };
        let list: Vec<(&Stroke, StrokeOrigin)> =
            strokes.iter().map(|s| (s, StrokeOrigin { source: source.to_string(), page, index: s.order })).collect();
        crate::assign::assign(&mut self.project, glyph, &list, self.reference.as_deref());
        if self.project.glyphs[glyph] != before {
            self.changed(glyph, cx);
            self.record(Change::Glyphs(vec![(glyph, before)]));
        }
    }

    /// The strokes of a source some glyph uses: (page, index).
    pub fn used_strokes(&self, source: &str) -> HashSet<(usize, usize)> {
        self.project
            .glyphs
            .iter()
            .flat_map(|g| &g.strokes)
            .filter_map(|s| s.origin.as_ref())
            .filter(|o| o.source == source)
            .map(|o| (o.page, o.index))
            .collect()
    }

    fn changed(&mut self, glyph: usize, cx: &mut Context<Self>) {
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
