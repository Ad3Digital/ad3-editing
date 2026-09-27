// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. https://mozilla.org/MPL/2.0/
//! Font selection, shaping and outline extraction, independent of the GUI.
use crate::{
    Result,
    encoder::cancelled,
    media::SourceVersion,
    text_layout::{Baseline, Metrics},
};
use fontdb::{Database, Family, ID, Query, Source, Stretch, Style, Weight};
use harfrust::{Direction, Feature, ShapeOptions, ShaperData, ShaperInstance, UnicodeBuffer};
use skrifa::{
    FontRef, MetadataProvider, Tag,
    instance::Size,
    outline::{DrawSettings, OutlinePen},
    raw::TableProvider,
};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, atomic::AtomicBool},
};
use tiny_skia::{Path, PathBuilder};
use unicode_bidi::{BidiInfo, Level};
use unicode_script::{Script, UnicodeScript};
use unicode_segmentation::UnicodeSegmentation;

const FONT_BYTES: usize = 256 * 1024 * 1024;
const RUN_BYTES: usize = 64 * 1024 * 1024;
const MAX_POINTS: usize = 2_000_000;

#[derive(Clone, Debug, PartialEq)]
pub struct FontSpec {
    pub family: String,
    pub size: f64,
    pub weight: f64,
    pub style: Style,
    pub spacing: f64,
}
impl Default for FontSpec {
    fn default() -> Self {
        Self {
            family: "Inter".into(),
            size: 16.0,
            weight: 400.0,
            style: Style::Normal,
            spacing: 0.0,
        }
    }
}
pub struct Shaped {
    pub metrics: Metrics,
    pub path: Option<Path>,
}
struct Face {
    bytes: Arc<dyn AsRef<[u8]> + Send + Sync>,
    index: u32,
    shaper: ShaperData,
    version: Option<SourceVersion>,
}
impl Face {
    fn font(&self) -> Result<FontRef<'_>> {
        FontRef::from_index(self.bytes.as_ref().as_ref(), self.index).map_err(|e| e.to_string())
    }
    fn covers(&self, text: &str) -> bool {
        self.font().is_ok_and(|font| {
            text.chars()
                .all(|ch| ignorable(ch) || font.charmap().map(ch).is_some())
        })
    }
}
struct CachedRun {
    text: String,
    spec: FontSpec,
    baseline: Baseline,
    value: Arc<Shaped>,
    bytes: usize,
}
struct CandidateSet {
    key: (u16, u8),
    ids: Arc<Vec<ID>>,
}
#[derive(Default)]
pub struct FontBook {
    db: Option<Database>,
    bundled: Option<[ID; 2]>,
    versions: HashMap<ID, SourceVersion>,
    faces: VecDeque<(ID, Arc<Face>)>,
    face_bytes: usize,
    runs: VecDeque<CachedRun>,
    run_bytes: usize,
    fallbacks: HashMap<(ID, u64, u8, String), ID>,
    candidates: VecDeque<CandidateSet>,
    fallback_bytes: usize,
    read_budget: Option<u64>,
}
impl FontBook {
    pub(crate) fn begin_frame(&mut self) {
        self.read_budget = Some(256 * 1024 * 1024);
    }
    fn catalog(&mut self, cancel: Option<&AtomicBool>) -> Result<()> {
        cancelled(cancel)?;
        if self.db.is_none() {
            let mut db = Database::new();
            db.load_font_data(include_bytes!("../assets/fonts/Inter.ttf").to_vec());
            db.load_font_data(include_bytes!("../assets/fonts/Inter-Italic.ttf").to_vec());
            let normal = db
                .faces()
                .find(|f| f.style == Style::Normal)
                .ok_or("Bundled Inter normal face is invalid")?
                .id;
            let italic = db
                .faces()
                .find(|f| f.style == Style::Italic)
                .ok_or("Bundled Inter italic face is invalid")?
                .id;
            self.bundled = Some([normal, italic]);
            self.versions = crate::font_catalog::discover(&mut db, cancel)?;
            cancelled(cancel)?;
            self.db = Some(db);
        }
        Ok(())
    }
    fn face(&mut self, id: ID, cancel: Option<&AtomicBool>) -> Result<Arc<Face>> {
        cancelled(cancel)?;
        if let Some(index) = self.faces.iter().position(|(key, _)| *key == id) {
            let entry = self.faces.remove(index).unwrap();
            let face = entry.1.clone();
            self.faces.push_back(entry);
            return Ok(face);
        }
        let (source, index) = self
            .db
            .as_ref()
            .and_then(|db| db.face_source(id))
            .ok_or("Missing font face")?;
        let mut source_version = None;
        let bytes: Arc<dyn AsRef<[u8]> + Send + Sync> = match source {
            Source::Binary(bytes) => bytes,
            Source::File(path) => {
                let expected = self
                    .versions
                    .get(&id)
                    .cloned()
                    .ok_or("Missing catalogued font version")?;
                source_version = Some(expected.clone());
                if let Some((_, face)) = self
                    .faces
                    .iter()
                    .find(|(_, face)| face.version.as_ref() == Some(&expected))
                {
                    face.bytes.clone()
                } else {
                    let (data, version) = crate::font_catalog::read_font(
                        &path,
                        cancel,
                        self.read_budget.get_or_insert(256 * 1024 * 1024),
                    )?;
                    if expected != version {
                        return Err("Installed font changed; reopen the project".into());
                    }
                    Arc::new(data)
                }
            }
        };
        let font =
            FontRef::from_index(bytes.as_ref().as_ref(), index).map_err(|e| e.to_string())?;
        let shaper = ShaperData::new(&font);
        let len = bytes.as_ref().as_ref().len();
        let face = Arc::new(Face {
            version: source_version,
            bytes,
            index,
            shaper,
        });
        while self.face_bytes + len > FONT_BYTES || self.faces.len() >= 128 {
            let Some((_, old)) = self.faces.pop_front() else {
                break;
            };
            self.face_bytes -= old.bytes.as_ref().as_ref().len();
        }
        self.face_bytes += len;
        self.faces.push_back((id, face.clone()));
        Ok(face)
    }
    fn primary(&self, spec: &FontSpec) -> Result<ID> {
        let names = family_names(&spec.family)?;
        let db = self.db.as_ref().ok_or("Font catalog unavailable")?;
        let bundled = self
            .bundled
            .map(|ids| ids[usize::from(spec.style != Style::Normal)]);
        let query = |family| {
            db.query(&Query {
                families: &[family],
                weight: Weight(spec.weight.round() as u16),
                stretch: Stretch::Normal,
                style: spec.style,
            })
        };
        for name in &names {
            // Embedded Inter stays stable across host font installations.
            if name.eq_ignore_ascii_case("Inter")
                && let Some(id) = bundled
            {
                return Ok(id);
            }
            let family = match name.to_ascii_lowercase().as_str() {
                "serif" => Family::Serif,
                "sans-serif" => Family::SansSerif,
                "monospace" => Family::Monospace,
                "cursive" => Family::Cursive,
                "fantasy" => Family::Fantasy,
                _ => Family::Name(name),
            };
            if let Some(id) = query(family) {
                return Ok(id);
            }
        }
        query(Family::Serif)
            .or(bundled)
            .ok_or_else(|| "No usable font face".into())
    }
    #[allow(clippy::too_many_arguments)]
    fn fallback(
        &mut self,
        primary: ID,
        text: &str,
        spec: &FontSpec,
        probes: &mut usize,
        budget: &mut usize,
        cancel: Option<&AtomicBool>,
    ) -> Result<ID> {
        cancelled(cancel)?;
        let key = (
            primary,
            spec.weight.to_bits(),
            style_key(spec.style),
            text.to_owned(),
        );
        if let Some(id) = self.fallbacks.get(&key) {
            return Ok(*id);
        }
        let id = self.find_fallback(primary, text, spec, probes, budget, cancel)?;
        if self.fallback_bytes + text.len() > 4 * 1024 * 1024 || self.fallbacks.len() >= 4096 {
            self.fallbacks.clear();
            self.fallback_bytes = 0;
        }
        self.fallback_bytes += text.len();
        self.fallbacks.insert(key, id);
        Ok(id)
    }
    #[allow(clippy::too_many_arguments)]
    fn find_fallback(
        &mut self,
        primary: ID,
        text: &str,
        spec: &FontSpec,
        probes: &mut usize,
        budget: &mut usize,
        cancel: Option<&AtomicBool>,
    ) -> Result<ID> {
        cancelled(cancel)?;
        *probes += 1;
        *budget = budget
            .checked_sub(1)
            .ok_or("Text font fallback work limit exceeded")?;
        if *probes > 8192 {
            return Err("Text fallback exceeds 8192 font probes per word".into());
        }
        if self.face(primary, cancel)?.covers(text) {
            return Ok(primary);
        }
        let ids = self.candidate_ids(spec, cancel)?;
        let mut unsupported_style = false;
        for id in ids.iter().copied().filter(|id| *id != primary) {
            cancelled(cancel)?;
            *probes += 1;
            *budget = budget
                .checked_sub(1)
                .ok_or("Text font fallback work limit exceeded")?;
            if *probes > 8192 {
                return Err("Text fallback exceeds 8192 font probes per word".into());
            }
            let face = self.face(id, cancel)?;
            if face.covers(text) {
                if self.require_outline_style(id, &face.font()?, spec).is_ok() {
                    return Ok(id);
                }
                unsupported_style = true;
            }
        }
        if unsupported_style {
            return Err("Synthetic fallback font styling is not ported yet".into());
        }
        Ok(primary) // Its .notdef outline is the final missing-glyph representation.
    }
    fn candidate_ids(
        &mut self,
        spec: &FontSpec,
        cancel: Option<&AtomicBool>,
    ) -> Result<Arc<Vec<ID>>> {
        let key = (spec.weight.round() as u16, style_key(spec.style));
        if let Some(set) = self.candidates.iter().find(|set| set.key == key) {
            return Ok(set.ids.clone());
        }
        let db = self.db.as_ref().ok_or("Font catalog unavailable")?;
        let mut seen = HashSet::new();
        let mut ids = Vec::new();
        for face in db.faces() {
            cancelled(cancel)?;
            let Some((name, _)) = face.families.first() else {
                continue;
            };
            if !seen.insert(name.as_str()) {
                continue;
            }
            if let Some(id) = db.query(&Query {
                families: &[Family::Name(name)],
                weight: Weight(key.0),
                stretch: Stretch::Normal,
                style: spec.style,
            }) {
                ids.push(id);
            }
        }
        let ids = Arc::new(ids);
        if self.candidates.len() >= 32 {
            self.candidates.pop_front();
        }
        self.candidates.push_back(CandidateSet {
            key,
            ids: ids.clone(),
        });
        Ok(ids)
    }
    pub fn shape(
        &mut self,
        text: &str,
        spec: &FontSpec,
        baseline: Baseline,
        cancel: Option<&AtomicBool>,
    ) -> Result<Arc<Shaped>> {
        self.begin_frame();
        self.shape_bounded(text, spec, baseline, cancel, &mut 8192)
    }
    pub(crate) fn shape_bounded(
        &mut self,
        text: &str,
        spec: &FontSpec,
        baseline: Baseline,
        cancel: Option<&AtomicBool>,
        budget: &mut usize,
    ) -> Result<Arc<Shaped>> {
        cancelled(cancel)?;
        if text.len() > 1024 * 1024
            || spec.family.len() > 4096
            || !spec.size.is_finite()
            || !(0.0..=8192.0).contains(&spec.size)
            || !spec.spacing.is_finite()
            || !spec.weight.is_finite()
            || !(1.0..=1000.0).contains(&spec.weight)
        {
            return Err("Invalid or excessive font request".into());
        }
        if text.chars().count() > 100_000 {
            return Err("A shaped word accepts at most 100000 characters".into());
        }
        // Sequence-aware emoji/variation selection needs its own font matcher.
        // Reject it until ported instead of silently changing presentation.
        if text
            .chars()
            .any(|ch| matches!(ch,'\u{200d}'|'\u{fe00}'..='\u{fe0f}'|'\u{e0100}'..='\u{e01ef}'))
        {
            return Err("Variation-selector and ZWJ font selection is not ported yet".into());
        }
        if let Some(i) = self
            .runs
            .iter()
            .position(|r| r.text == text && r.spec == *spec && r.baseline == baseline)
        {
            let entry = self.runs.remove(i).unwrap();
            let value = entry.value.clone();
            self.runs.push_back(entry);
            return Ok(value);
        }
        self.catalog(cancel)?;
        let primary = self.primary(spec)?;
        let primary_face = self.face(primary, cancel)?;
        let primary_font = primary_face.font()?;
        self.require_outline_style(primary, &primary_font, spec)?;
        let location = primary_font
            .axes()
            .location(&[("wght", spec.weight as f32), ("opsz", spec.size as f32)]);
        let primary_metrics = primary_font.metrics(Size::new(spec.size as f32), &location);
        let em = normalized_em_ascent(
            &primary_font,
            spec.size,
            primary_metrics.ascent as f64,
            -primary_metrics.descent as f64,
        );
        let baseline_offset = match baseline {
            Baseline::Top => em,
            Baseline::Middle => em - spec.size / 2.0,
            Baseline::Bottom => em - spec.size,
            Baseline::Alphabetic => 0.0,
        };
        let mut ascent = (primary_metrics.ascent as f64).round();
        let mut descent = (-primary_metrics.descent as f64).round();
        let mut pen = Pen {
            path: PathBuilder::new(),
            x: 0.0,
            y: baseline_offset as f32,
            points: 0,
            overflow: false,
        };
        let mut advance = 0.0f64;
        // Canvas normalizes tab/carriage-return/form-feed to spaces before shaping.
        let normalized = text.replace(['\t', '\r', '\n', '\x0c'], " ");
        let mut probes = 0;
        // Original renderTokens uses a default Canvas context in an LTR host;
        // the JSX API has no direction property. Keep that paragraph level.
        let bidi = BidiInfo::new(&normalized, Some(Level::ltr()));
        for paragraph in &bidi.paragraphs {
            let (levels, runs) = bidi.visual_runs(paragraph, paragraph.range.clone());
            for run in runs {
                let rtl = levels[run.start].is_rtl();
                let value = &normalized[run.clone()];
                let mut groups: Vec<(ID, Script, usize, usize)> = Vec::new();
                for (at, cluster) in value.grapheme_indices(true) {
                    let id = self.fallback(primary, cluster, spec, &mut probes, budget, cancel)?;
                    let script = cluster
                        .chars()
                        .map(|ch| ch.script())
                        .find(|s| !matches!(s, Script::Common | Script::Inherited))
                        .unwrap_or_else(|| {
                            groups.last().map_or_else(
                                || {
                                    value
                                        .chars()
                                        .map(|ch| ch.script())
                                        .find(|s| !matches!(s, Script::Common | Script::Inherited))
                                        .unwrap_or(Script::Common)
                                },
                                |g| g.1,
                            )
                        });
                    if let Some(last) = groups.last_mut()
                        && last.0 == id
                        && (last.1 == script || script == Script::Common)
                    {
                        last.3 = at + cluster.len();
                    } else {
                        groups.push((id, script, at, at + cluster.len()));
                    }
                }
                if rtl {
                    groups.reverse();
                }
                for (id, script, start, end) in groups {
                    cancelled(cancel)?;
                    let face = self.face(id, cancel)?;
                    let font = face.font()?;
                    self.require_outline_style(id, &font, spec)?;
                    let settings = [("wght", spec.weight as f32), ("opsz", spec.size as f32)];
                    let location = font.axes().location(&settings);
                    let metrics = font.metrics(Size::new(spec.size as f32), &location);
                    ascent = ascent.max((metrics.ascent as f64).round());
                    descent = descent.max((-metrics.descent as f64).round());
                    let instance = ShaperInstance::from_variations(&font, settings);
                    let shaper = face.shaper.shaper(&font).instance(Some(&instance)).build();
                    let mut buffer = UnicodeBuffer::new();
                    buffer.push_str(&value[start..end]);
                    buffer.set_direction(if rtl {
                        Direction::RightToLeft
                    } else {
                        Direction::LeftToRight
                    });
                    if let Ok(script) = script.short_name().parse() {
                        buffer.set_script(script);
                    }
                    buffer.guess_segment_properties();
                    let features = [
                        Feature::new(Tag::new(b"liga"), 0, ..),
                        Feature::new(Tag::new(b"clig"), 0, ..),
                    ];
                    let options = ShapeOptions::new()
                        .scale(Some((spec.size * 64.0).round() as i32))
                        .features(if spec.spacing == 0.0 { &[] } else { &features });
                    let shaped = shaper.shape(buffer, options);
                    if shaped.glyph_infos().len() > 100_000 {
                        return Err("Shaped word exceeds 100000 glyphs".into());
                    }
                    let outlines = font.outline_glyphs();
                    for (i, (info, position)) in shaped
                        .glyph_infos()
                        .iter()
                        .zip(shaped.glyph_positions())
                        .enumerate()
                    {
                        cancelled(cancel)?;
                        let glyph_id = skrifa::GlyphId::new(info.glyph_id);
                        if font.color_glyphs().get(glyph_id).is_some() {
                            return Err("Color glyph painting is not ported yet".into());
                        }
                        pen.x = (advance + position.x_offset as f64 / 64.0) as f32;
                        pen.y = (baseline_offset - position.y_offset as f64 / 64.0) as f32;
                        if let Some(glyph) = outlines.get(glyph_id) {
                            glyph
                                .draw(
                                    DrawSettings::unhinted(Size::new(spec.size as f32), &location),
                                    &mut pen,
                                )
                                .map_err(|e| e.to_string())?;
                        } else {
                            return Err(
                                "Bitmap, SVG or missing glyph outlines are not ported yet".into()
                            );
                        }
                        if pen.overflow {
                            return Err("Text outlines exceed two million points".into());
                        }
                        advance += position.x_advance as f64 / 64.0;
                        if shaped
                            .glyph_infos()
                            .get(i + 1)
                            .is_none_or(|next| next.cluster != info.cluster)
                        {
                            advance += spec.spacing;
                        }
                        if !advance.is_finite() || !(advance as f32).is_finite() {
                            return Err("Text advance exceeds raster coordinate range".into());
                        }
                    }
                }
            }
        }
        if !advance.is_finite() {
            return Err("Text advance overflows".into());
        }
        let path = pen.path.finish();
        let bounds = path.as_ref().and_then(Path::compute_tight_bounds);
        let result = Arc::new(Shaped {
            metrics: Metrics {
                width: advance,
                ascent: ascent - baseline_offset,
                descent: descent + baseline_offset,
                left: bounds.map_or(0.0, |b| -f64::from(b.left())),
                right: bounds.map_or(0.0, |b| f64::from(b.right())),
                ink_ascent: bounds.map_or(-baseline_offset, |b| -f64::from(b.top())),
                ink_descent: bounds.map_or(baseline_offset, |b| f64::from(b.bottom())),
            },
            path,
        });
        let bytes = text.len()
            + spec.family.len()
            + result
                .path
                .as_ref()
                .map_or(0, |p| p.points().len() * 16 + p.verbs().len())
            + 256;
        if bytes <= RUN_BYTES {
            while self.run_bytes + bytes > RUN_BYTES || self.runs.len() >= 2048 {
                let Some(old) = self.runs.pop_front() else {
                    break;
                };
                self.run_bytes -= old.bytes;
            }
            self.run_bytes += bytes;
            self.runs.push_back(CachedRun {
                text: text.into(),
                spec: spec.clone(),
                baseline,
                value: result.clone(),
                bytes,
            });
        }
        Ok(result)
    }

    fn require_outline_style(&self, id: ID, font: &FontRef<'_>, spec: &FontSpec) -> Result<()> {
        let info = self
            .db
            .as_ref()
            .and_then(|db| db.face(id))
            .ok_or("Missing font metadata")?;
        if spec.style != Style::Normal && info.style == Style::Normal {
            return Err("Synthetic italic/oblique font styling is not ported yet".into());
        }
        if spec.weight >= 600.0
            && info.weight.0 < 600
            && font.axes().get_by_tag(Tag::new(b"wght")).is_none()
        {
            return Err("Synthetic bold font styling is not ported yet".into());
        }
        Ok(())
    }
}

fn style_key(style: Style) -> u8 {
    match style {
        Style::Normal => 0,
        Style::Italic => 1,
        Style::Oblique => 2,
    }
}
fn ignorable(ch: char) -> bool {
    ch.is_control()
        || matches!(
            ch,
            '\u{200c}' | '\u{200d}' | '\u{fe0e}' | '\u{fe0f}' | '\u{2060}'
        )
}
fn normalized_em_ascent(font: &FontRef<'_>, size: f64, ascent: f64, descent: f64) -> f64 {
    let (a, d) = font
        .os2()
        .ok()
        .map(|o| (o.s_typo_ascender() as f64, -o.s_typo_descender() as f64))
        .filter(|(a, d)| *a > 0.0 && *a + *d > 0.0 && *d >= 0.0)
        .unwrap_or((ascent, descent));
    if a + d > 0.0 {
        ((size * a / (a + d)) * 64.0).round() / 64.0
    } else {
        size * 0.8
    }
}
pub fn family_names(value: &str) -> Result<Vec<String>> {
    let mut output = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    for ch in value.chars() {
        match (quote, ch) {
            (None, '\'' | '"') => quote = Some(ch),
            (Some(q), c) if q == c => quote = None,
            (None, ',') => {
                if !current.trim().is_empty() {
                    output.push(current.trim().to_owned());
                }
                current.clear();
            }
            (_, c) => current.push(c),
        }
    }
    if quote.is_some() {
        return Err("Unclosed font family quote".into());
    }
    if !current.trim().is_empty() {
        output.push(current.trim().to_owned());
    }
    Ok(output)
}
struct Pen {
    path: PathBuilder,
    x: f32,
    y: f32,
    points: usize,
    overflow: bool,
}
impl Pen {
    fn allow(&mut self, n: usize) -> bool {
        self.points += n;
        self.overflow |= self.points > MAX_POINTS;
        !self.overflow
    }
}
impl OutlinePen for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        if self.allow(1) {
            self.path.move_to(self.x + x, self.y - y);
        }
    }
    fn line_to(&mut self, x: f32, y: f32) {
        if self.allow(1) {
            self.path.line_to(self.x + x, self.y - y);
        }
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        if self.allow(2) {
            self.path
                .quad_to(self.x + cx, self.y - cy, self.x + x, self.y - y);
        }
    }
    fn curve_to(&mut self, cx: f32, cy: f32, dx: f32, dy: f32, x: f32, y: f32) {
        if self.allow(3) {
            self.path.cubic_to(
                self.x + cx,
                self.y - cy,
                self.x + dx,
                self.y - dy,
                self.x + x,
                self.y - y,
            );
        }
    }
    fn close(&mut self) {
        if !self.overflow {
            self.path.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cached_fallback_observes_cancellation_and_async_word_work_can_stop() {
        use std::sync::atomic::Ordering;
        let mut book = FontBook::default();
        let spec = FontSpec::default();
        book.shape("A", &spec, Baseline::Top, None).unwrap();
        let primary = book.primary(&spec).unwrap();
        assert!(
            book.fallback(
                primary,
                "A",
                &spec,
                &mut 0,
                &mut 8192,
                Some(&AtomicBool::new(true))
            )
            .err()
            .unwrap()
            .contains("cancelled")
        );
        let cancel = Arc::new(AtomicBool::new(false));
        let signal = cancel.clone();
        let worker = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(1));
            signal.store(true, Ordering::Relaxed);
        });
        let result = book.shape(&"A".repeat(100_000), &spec, Baseline::Top, Some(&cancel));
        worker.join().unwrap();
        assert!(result.err().unwrap().contains("cancelled"));
    }
    #[test]
    fn faces_share_file_bytes_and_evicted_reads_obey_the_frame_io_budget() {
        let mut book = FontBook::default();
        book.catalog(None).unwrap();
        let path = std::env::temp_dir().join(format!(
            "ad3-font-cache-{}-{}.ttf",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let bytes = include_bytes!("../assets/fonts/Inter.ttf");
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .and_then(|mut file| {
                use std::io::Write;
                file.write_all(bytes)
            })
            .unwrap();
        let version = SourceVersion::read(&path).unwrap();
        let mut info = book
            .db
            .as_ref()
            .unwrap()
            .face(book.bundled.unwrap()[0])
            .unwrap()
            .clone();
        info.source = Source::File(path.clone());
        let a = book.db.as_mut().unwrap().push_face_info(info.clone());
        let b = book.db.as_mut().unwrap().push_face_info(info);
        book.versions.insert(a, version.clone());
        book.versions.insert(b, version);
        book.read_budget = Some(bytes.len() as u64);
        let first = book.face(a, None).unwrap();
        let second = book.face(b, None).unwrap();
        assert!(Arc::ptr_eq(&first.bytes, &second.bytes));
        assert_eq!(book.read_budget, Some(0));
        book.faces.clear();
        book.face_bytes = 0;
        assert!(book.face(a, None).err().unwrap().contains("byte budget"));
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn bundled_variable_inter_precedes_installed_namesakes_at_every_weight() {
        let mut book = FontBook::default();
        book.catalog(None).unwrap();
        let normal = book.primary(&FontSpec::default()).unwrap();
        let mut installed = book.db.as_ref().unwrap().face(normal).unwrap().clone();
        installed.weight = Weight(700);
        book.db.as_mut().unwrap().push_face_info(installed);
        assert_eq!(
            book.primary(&FontSpec {
                weight: 700.0,
                ..Default::default()
            })
            .unwrap(),
            normal
        );
    }
    #[test]
    fn fallback_cache_reuses_positive_probes_and_a_frame_budget_stops_new_work() {
        let mut book = FontBook::default();
        let spec = FontSpec::default();
        let mut budget = 1;
        book.shape_bounded("AAAA", &spec, Baseline::Top, None, &mut budget)
            .unwrap();
        assert_eq!(budget, 0);
        assert!(
            book.shape_bounded("B", &spec, Baseline::Top, None, &mut budget)
                .err()
                .unwrap()
                .contains("work limit")
        );
        let mut budget = 1;
        book.shape_bounded("B", &spec, Baseline::Top, None, &mut budget)
            .unwrap();
        assert_eq!(budget, 0);
    }
    #[test]
    fn fallback_cache_separates_weights_of_the_same_variable_primary_face() {
        let mut book = FontBook::default();
        let spec = FontSpec::default();
        book.shape("A", &spec, Baseline::Top, None).unwrap();
        let count = book.fallbacks.len();
        book.shape(
            "A",
            &FontSpec {
                weight: 700.0,
                ..spec
            },
            Baseline::Top,
            None,
        )
        .unwrap();
        assert_eq!(book.fallbacks.len(), count + 1);
    }
}
