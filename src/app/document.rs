//! The open project, shared by the views. Changes go through here: it marks the project modified,
//! tells the views which glyphs changed (each glyph has a revision number for their caches) and
//! keeps what changed for undo. It also holds the project's reference font.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use gpui::{Context, EventEmitter};
use scriptoria_ink::Stroke;
use scriptoria_project::{FontInfo, Glyph, GlyphStroke, Part, Project, Reference, Template};
use scriptoria_reference::ReferenceFont;

/// How many changes can be undone.
const UNDO_LIMIT: usize = 200;

/// What a change replaced: kept for undo (and what an undo replaced, for redo).
enum Change {
    /// Glyphs, by index, as they were.
    Glyphs(Vec<(usize, Glyph)>),
    Font(FontInfo),
    /// Glyphs that were removed, by the index they had (ascending).
    Removed(Vec<(usize, Glyph)>),
    /// Glyphs that were added, by index (ascending).
    Added(Vec<usize>),
    /// The practice sheets, as they were.
    Templates(Vec<Template>),
    /// The parts, as they were.
    Parts(Vec<Part>),
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
    /// Bumped whenever anything changes.
    generation: u64,
    /// The outlines in glyphs (see [`Document::used_outlines`]), for a generation.
    used: std::cell::RefCell<Option<(u64, Arc<HashSet<u64>>)>>,
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
    /// Glyphs were added or removed: where each glyph that was at an index is now (none if it was
    /// removed). Followed by [`DocumentEvent::GlyphList`].
    Reindexed(Arc<[Option<usize>]>),
    /// The parts kept changed.
    Parts,
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
        Document { project, folder, modified: false, id, reference: reference.map(Arc::new), generation: 0, used: Default::default(), revisions, next_revision: 1, undo: Vec::new(), redo: Vec::new() }
    }

    /// Different for every document made (a project opened twice is two documents).
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Changes whenever the project does.
    pub fn generation(&self) -> u64 {
        self.generation
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

    /// Adds an empty glyph for every new character of `text`, at the end, as one step to undo;
    /// returns how many.
    pub fn add_chars(&mut self, text: &str, cx: &mut Context<Self>) -> usize {
        let before = self.project.glyphs.len();
        let added = self.project.add_chars(text);
        if added > 0 {
            self.sync_revisions();
            self.touch(cx);
            cx.emit(DocumentEvent::GlyphList);
            self.record(Change::Added((before..before + added).collect()));
        }
        added
    }

    /// Removes glyphs, as one step to undo; returns how many.
    pub fn remove_glyphs(&mut self, glyphs: &[usize], cx: &mut Context<Self>) -> usize {
        let mut glyphs: Vec<usize> = glyphs.iter().copied().filter(|&g| g < self.project.glyphs.len()).collect();
        glyphs.sort_unstable();
        glyphs.dedup();
        if glyphs.is_empty() {
            return 0;
        }
        let n = glyphs.len();
        let removed = self.take_glyphs(&glyphs, cx);
        self.record(Change::Removed(removed));
        n
    }

    /// Takes glyphs out (indices ascending); returns them with the indices they had.
    fn take_glyphs(&mut self, glyphs: &[usize], cx: &mut Context<Self>) -> Vec<(usize, Glyph)> {
        let n = self.project.glyphs.len();
        let gone: HashSet<usize> = glyphs.iter().copied().collect();
        let mut map = Vec::with_capacity(n);
        let mut kept = 0;
        for i in 0..n {
            map.push((!gone.contains(&i)).then(|| {
                kept += 1;
                kept - 1
            }));
        }
        let mut removed = Vec::with_capacity(glyphs.len());
        for &i in glyphs.iter().rev() {
            removed.push((i, self.project.glyphs.remove(i)));
            self.revisions.remove(i);
        }
        removed.reverse();
        self.reindexed(map, cx);
        removed
    }

    /// Puts glyphs back at the indices they had (ascending); returns those indices.
    fn put_glyphs(&mut self, glyphs: Vec<(usize, Glyph)>, cx: &mut Context<Self>) -> Vec<usize> {
        let n = self.project.glyphs.len();
        let at: HashSet<usize> = glyphs.iter().map(|(i, _)| *i).collect();
        // The glyphs there were go, in order, to the places left between those put back.
        let map: Vec<Option<usize>> = (0..n + at.len()).filter(|i| !at.contains(i)).map(Some).collect();
        let indices: Vec<usize> = glyphs.iter().map(|(i, _)| *i).collect();
        for (i, g) in glyphs {
            self.project.glyphs.insert(i, g);
            self.revisions.insert(i, self.next_revision);
            self.next_revision += 1;
        }
        self.reindexed(map, cx);
        indices
    }

    fn reindexed(&mut self, map: Vec<Option<usize>>, cx: &mut Context<Self>) {
        self.touch(cx);
        cx.emit(DocumentEvent::Reindexed(map.into()));
        cx.emit(DocumentEvent::GlyphList);
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

    /// Puts in all of the font's settings at once, as one step to undo; a reference font named
    /// anew is opened first (and the settings stay as they were if it does not open).
    pub fn replace_font(&mut self, info: FontInfo, cx: &mut Context<Self>) -> Result<(), scriptoria_reference::Error> {
        if info.reference != self.project.font.reference {
            self.reference = info.reference.as_ref().map(crate::assign::open_reference).transpose()?.map(Arc::new);
        }
        self.update_font(|f| *f = info, cx);
        Ok(())
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
            Change::Removed(glyphs) => Change::Added(self.put_glyphs(glyphs, cx)),
            Change::Parts(parts) => {
                let replaced = std::mem::replace(&mut self.project.parts, parts);
                self.touch(cx);
                cx.emit(DocumentEvent::Parts);
                Change::Parts(replaced)
            }
            Change::Templates(templates) => {
                let replaced = std::mem::replace(&mut self.project.templates, templates);
                self.touch(cx);
                cx.emit(DocumentEvent::GlyphList);
                Change::Templates(replaced)
            }
            Change::Added(glyphs) => Change::Removed(self.take_glyphs(&glyphs, cx)),
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

    /// Adds written strokes (from one page, in writing order) to the end of a glyph (see
    /// [`crate::assign::assign`]).
    pub fn assign(&mut self, glyph: usize, strokes: &[Stroke], cx: &mut Context<Self>) {
        let Some(before) = self.project.glyphs.get(glyph).cloned() else { return };
        let list: Vec<&Stroke> = strokes.iter().collect();
        crate::assign::assign(&mut self.project, glyph, &list, self.reference.as_deref());
        if self.project.glyphs[glyph] != before {
            self.changed(glyph, cx);
            self.record(Change::Glyphs(vec![(glyph, before)]));
        }
    }

    /// A new practice sheet for characters, kept with the project.
    pub fn new_template(&mut self, chars: &[char], cx: &mut Context<Self>) -> Template {
        let template = self.project.new_template(chars).clone();
        self.touch(cx);
        template
    }

    /// Keeps strokes as a part named `name` (in place of a part of that name), as a step to undo.
    pub fn save_part(&mut self, name: &str, strokes: Vec<GlyphStroke>, cx: &mut Context<Self>) {
        let before = self.project.parts.clone();
        let part = Part { name: name.to_string(), strokes };
        match self.project.parts.iter_mut().find(|p| p.name == name) {
            Some(p) => *p = part,
            None => self.project.parts.push(part),
        }
        self.parts_changed(before, cx);
    }

    /// Forgets a part, as a step to undo.
    pub fn remove_part(&mut self, name: &str, cx: &mut Context<Self>) {
        let before = self.project.parts.clone();
        self.project.parts.retain(|p| p.name != name);
        self.parts_changed(before, cx);
    }

    fn parts_changed(&mut self, before: Vec<Part>, cx: &mut Context<Self>) {
        if self.project.parts != before {
            self.record(Change::Parts(before));
            self.touch(cx);
            cx.emit(DocumentEvent::Parts);
        }
    }

    /// Drops a practice sheet that could not be written out (not a step to undo).
    pub fn discard_template(&mut self, id: u32, cx: &mut Context<Self>) {
        self.project.templates.retain(|t| t.id != id);
        self.touch(cx);
    }

    /// Forgets a practice sheet (pages written on it are not recognized any more), as a step to
    /// undo.
    pub fn remove_template(&mut self, id: u32, cx: &mut Context<Self>) {
        let before = self.project.templates.clone();
        self.project.templates.retain(|t| t.id != id);
        if self.project.templates != before {
            self.record(Change::Templates(before));
            self.touch(cx);
            cx.emit(DocumentEvent::GlyphList);
        }
    }

    /// Puts characters written on practice sheets into their glyphs (added if new), replacing the
    /// strokes they had, as one step to undo; returns how many glyphs had strokes before.
    pub fn write_cells(&mut self, written: Vec<crate::template::Written>, cx: &mut Context<Self>) -> usize {
        let text: String = written.iter().map(|w| w.c).collect();
        self.add_chars(&text, cx);
        // Each glyph once (a character written twice: the later one).
        let mut by_glyph: std::collections::BTreeMap<usize, crate::template::Written> = std::collections::BTreeMap::new();
        for w in written {
            if let Some(g) = self.project.glyph_for(w.c) {
                by_glyph.insert(g, w);
            }
        }
        let glyphs: Vec<usize> = by_glyph.keys().copied().collect();
        let replaced = glyphs.iter().filter(|&&g| !self.project.glyphs[g].strokes.is_empty()).count();
        // `update_glyphs` goes through the glyphs in the order given.
        let mut cells = by_glyph.into_values();
        self.update_glyphs(
            &glyphs,
            |g| {
                if let Some(w) = cells.next() {
                    g.strokes = w.strokes;
                    g.advance = w.advance;
                }
            },
            cx,
        );
        replaced
    }

    /// The outlines of the strokes in glyphs, as fingerprints (see
    /// [`scriptoria_project::fingerprint`]): a page's strokes among them are used. Kept until the
    /// project changes.
    pub fn used_outlines(&self) -> Arc<HashSet<u64>> {
        let mut used = self.used.borrow_mut();
        match &*used {
            Some((generation, set)) if *generation == self.generation => set.clone(),
            _ => {
                let set: Arc<HashSet<u64>> =
                    Arc::new(self.project.glyphs.iter().flat_map(|g| &g.strokes).map(|s| scriptoria_project::fingerprint(&s.outline.path)).collect());
                *used = Some((self.generation, set.clone()));
                set
            }
        }
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
        self.generation += 1;
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
