//! Edit PDF backend: inline text editing by byte-level surgery on the original content streams (lopdf)
//! plus one appended stream per edited page. Contract: _docs/edit-contract.md. Pages are 0-based everywhere.
//! Edits are data and the output is a pure function of (original bytes, effective edits); undo/redo only moves a cursor.
mod build;
mod content;
mod docobj;
mod fonts;
mod layout;
pub mod sig;
mod whiteout;

use crate::compress::{self, place, run_gs, size_of, stamp, temp_root, GsErr};
use crate::job::JobGuard;
use crate::pdf::{self, Opened};
use base64::Engine;
use content::{zones, Analysis, OK};
use fonts::{deref, effective_resources, load_fonts, num, used_codes, Fnt};
#[cfg(test)]
use layout::{retype, typeset};
use layout::{find_paras, Align, Fail, PageIn, Para, PcSet, Seg};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream};
use pdfium_render::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

// ---------------- wire types ----------------

#[derive(Deserialize)]
pub struct EditOpenReq {
    path: String,
    password: Option<String>,
}

#[derive(Serialize, Debug)]
pub struct EditDoc {
    pages: u32,
    sizes: Vec<(f32, f32)>,
}

#[derive(Serialize, Clone, Debug)]
pub struct ParaOut {
    id: u32,
    bbox: [f64; 4],
    text: String,
    marker: Option<String>,
    align: Align,
    size: f64,
    pitch: f64,
    first_indent: f64,
    css_font: String,
    bold: bool,
    italic: bool,
    color: String,
    status: &'static str,
    reason: Option<&'static str>,
    removed: bool, // taken out with the Select tool; bbox is empty
    edited: bool,
    pc_font_used: bool,
    rot: i64, // clockwise angle at which the text appears on the displayed page
    style: Fmt,
    orig_font_name: String,
    runs: Vec<Run>,
}

/// Format of a paragraph or text box. `font` is "orig" or a PC family name.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Fmt {
    pub font: String,
    pub size: f64,
    pub color: String,
    pub bold: bool,
    pub italic: bool,
    pub align: Align,
}

/// A piece of text with its own format; a paragraph or text box is a sequence of runs.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Run {
    pub text: String,
    pub style: Fmt,
}

#[derive(Serialize, Clone, Debug)]
pub struct ObjOut {
    id: u32,
    kind: &'static str,
    rect: [f64; 4],
    rot: i64,
    text: Option<String>,
    style: Option<Fmt>,
    asset: Option<String>,
    orig_font_name: String,
    runs: Option<Vec<Run>>,
}

/// Anything the Select tool can pick: a paragraph, an image or a path (Edit 3).
#[derive(Serialize, Clone, Debug)]
pub struct ItemOut {
    id: u32,
    kind: &'static str,
    bbox: [f64; 4], // display pt, top-left origin
    rot: i64,
    locked: Option<&'static str>,
}

#[derive(Serialize, Clone, Debug)]
pub struct PageModel {
    page: u32,
    paras: Vec<ParaOut>,
    scan: bool,
    objs: Vec<ObjOut>,
    items: Vec<ItemOut>,
    wm_forced_front: bool, // a watermark "behind" is drawn in front on this page (scan)
}

#[derive(Deserialize)]
pub struct EditApplyReq {
    page: u32,
    para: u32,
    text: String,
    use_pc_font: bool,
    allow_overlap: bool,
    #[serde(default)]
    style: Option<Fmt>,
    #[serde(default)]
    runs: Option<Vec<Run>>,
}

#[derive(Deserialize)]
pub struct ObjReq {
    kind: String,
    page: u32,
    id: Option<u32>,
    rect: Option<[f64; 4]>,
    text: Option<String>,
    style: Option<Fmt>,
    rot: Option<i64>,
    asset: Option<String>,
    cover_only: Option<bool>,
    #[serde(default)]
    runs: Option<Vec<Run>>,
    #[serde(default)]
    use_pc_font: Option<bool>,
}

#[derive(Serialize, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ObjRes {
    Ok { page: PageModel, id: u32 },
    MissingChars { chars: String },
    UnsupportedChars { chars: String },
    WhiteoutPartial { chars: usize },
}

#[derive(Serialize, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EditApplyRes {
    Ok { page: PageModel },
    MissingChars { chars: String },
    UnsupportedChars { chars: String },
    Overflow { lines_over: u32 },
    CannotPush,
}

#[derive(Serialize, Debug, Default)]
pub struct EditHistory {
    can_undo: bool,
    can_redo: bool,
    count: u32,
    changed_pages: Vec<u32>,
}

#[derive(Deserialize)]
pub struct EditSaveReq {
    out_mode: String,
}

#[derive(Serialize, Debug)]
pub struct EditSaveRes {
    output: String,
    size: u64,
    seconds: f64,
}

// ---------------- Edit 3 wire types ----------------

/// What a Select-tool item is.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum IKind {
    Para,
    Image,
    Path,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
pub struct ItemRef {
    kind: IKind,
    id: u32,
}

/// One edit of the Select tool (display pt, y down).
#[derive(Deserialize, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ObjEdit3 {
    Move { page: u32, item: ItemRef, dx: f64, dy: f64 },
    Resize { page: u32, item: ItemRef, rect: [f64; 4] },
    Remove { page: u32, item: ItemRef },
}

#[derive(Deserialize, Debug)]
pub struct EditBatchReq {
    edits: Vec<ObjEdit3>,
}

#[derive(Serialize, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BatchRes {
    Ok { pages: Vec<PageModel> },
    CannotMove { page: u32, item: ItemRef },
}

fn d_margin() -> f64 {
    28.0
}
fn d_start() -> u32 {
    1
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct WmReq {
    id: Option<u32>,
    kind: String, // "text" | "image"
    text: Option<String>,
    style: Option<Fmt>,
    asset: Option<String>,
    scale: Option<f64>,
    opacity: f64,
    angle: f64,
    layer: String, // "behind" | "front"
    pages: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct HfReq {
    id: Option<u32>,
    pos: String, // tl tc tr bl bc br
    template: String,
    style: Fmt,
    #[serde(default = "d_margin")]
    margin_pt: f64,
    #[serde(default = "d_start")]
    start: u32,
    #[serde(default)]
    skip_first: bool,
    #[serde(default)]
    pages: String,
    #[serde(default)]
    lang: String,
    #[serde(default)]
    tz_min: i32,
}

#[derive(Serialize, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DocRes {
    Ok { id: u32, changed: &'static str },
    BadPages { reason: String },
    UnsupportedChars { chars: String },
    TooLong,
    BadText,
}

#[derive(Serialize, Debug)]
pub struct DocObjs {
    watermarks: Vec<WmReq>,
    header_footers: Vec<HfReq>,
}

// ---------------- helpers on the lopdf side ----------------

fn inherited<'a>(doc: &'a Document, page: ObjectId, key: &[u8]) -> Option<&'a Object> {
    let mut node = doc.get_dictionary(page).ok()?;
    for _ in 0..32 {
        if let Ok(o) = node.get(key) {
            return Some(deref(doc, o));
        }
        node = doc.get_dictionary(node.get(b"Parent").ok()?.as_reference().ok()?).ok()?;
    }
    None
}

/// [x origin, y origin, width, height] of the crop box (media box when absent).
fn page_box(doc: &Document, page: ObjectId) -> [f64; 4] {
    let get = |k: &[u8]| -> Option<[f64; 4]> {
        let a = inherited(doc, page, k)?.as_array().ok()?;
        (a.len() == 4).then(|| [num(deref(doc, &a[0])), num(deref(doc, &a[1])), num(deref(doc, &a[2])), num(deref(doc, &a[3]))])
    };
    let b = get(b"CropBox").or_else(|| get(b"MediaBox")).unwrap_or([0.0, 0.0, 612.0, 792.0]);
    [b[0].min(b[2]), b[1].min(b[3]), (b[2] - b[0]).abs(), (b[3] - b[1]).abs()]
}

/// Bounding box of an XObject in its own space (images: unit square; forms: BBox through Matrix).
fn xobj_box(doc: &Document, page: ObjectId, name: &str) -> Option<[f64; 4]> {
    let xo = deref(doc, effective_resources(doc, page)?.get(b"XObject").ok()?).as_dict().ok()?;
    let Object::Stream(s) = deref(doc, xo.get(name.as_bytes()).ok()?) else { return None };
    if s.dict.get(b"Subtype").ok().and_then(|o| o.as_name().ok()) == Some(b"Image") {
        return Some([0.0, 0.0, 1.0, 1.0]);
    }
    let a = deref(doc, s.dict.get(b"BBox").ok()?).as_array().ok()?;
    let v: Vec<f64> = a.iter().map(|o| num(deref(doc, o))).collect();
    if v.len() != 4 {
        return None;
    }
    let m: Vec<f64> = s.dict.get(b"Matrix").ok().and_then(|o| deref(doc, o).as_array().ok()).map(|a| a.iter().map(|o| num(deref(doc, o))).collect()).unwrap_or_default();
    if m.len() != 6 {
        return Some([v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])]);
    }
    let pts = [(v[0], v[1]), (v[2], v[1]), (v[2], v[3]), (v[0], v[3])].map(|(x, y)| (x * m[0] + y * m[2] + m[4], x * m[1] + y * m[3] + m[5]));
    Some([
        pts.iter().map(|p| p.0).fold(f64::MAX, f64::min),
        pts.iter().map(|p| p.1).fold(f64::MAX, f64::min),
        pts.iter().map(|p| p.0).fold(f64::MIN, f64::max),
        pts.iter().map(|p| p.1).fold(f64::MIN, f64::max),
    ])
}

fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ---------------- page analysis ----------------

/// An image or path of a page for the Select tool. `op` = the painting op (or Do), `lo` = its first construction op.
#[derive(Clone, Debug)]
pub struct PItem {
    kind: IKind,
    id: u32,
    op: usize, // usize::MAX: inside a form XObject (locked)
    lo: usize,
    bbox: [f64; 4], // analysis frame x0 y0 x1 y1
    locked: Option<&'static str>,
    name: Option<String>, // XObject name of a Do
    clip: bool,           // the path also sets the clip (W)
}

pub struct PageAn {
    idx: u32,
    pid: ObjectId,
    page: [f64; 4],
    scan: bool,
    #[cfg_attr(not(test), allow(dead_code))]
    lock: Option<&'static str>,
    #[cfg_attr(not(test), allow(dead_code))]
    pdfium_texts: usize, // pdfium text objects, for the 1:1 check
    joined: Vec<u8>,
    an: Analysis,
    fonts: BTreeMap<String, Fnt>,
    paras: Vec<Para>,
    foot_y: f64,
    head_y: f64,
    rinv: content::M, // work frame -> content space
    disp: [f64; 2],   // displayed page size (the work frame is turned so the dominant text is upright)
    to_disp: content::M, // work frame -> displayed frame
    q: i64, // extra turn of the work frame, degrees
    form_segs: Vec<Seg>,   // text inside form XObjects (work frame): can only be covered
    form_imgs: Vec<[f64; 4]>, // images inside form XObjects (work frame x0 y0 x1 y1)
    items: Vec<PItem>,
}

pub struct Dom {
    font: String, // resource name
    size: f64,
    fill: String,
    css: String,
    base: String,
}

impl PageAn {
    /// The most common run of the page (font resource, size, colour), counted in characters.
    fn dominant(&self) -> Option<Dom> {
        let mut count: HashMap<(String, i64, String), usize> = HashMap::new();
        for ed in self.paras.iter().filter_map(|p| p.ed.as_ref()) {
            for c in ed.words.iter().flatten() {
                let st = &ed.styles[c.style];
                *count.entry((st.font.clone(), (st.size * 2.0).round() as i64, st.fill.clone())).or_default() += 1;
            }
        }
        let ((font, size2, fill), _) = count.into_iter().max_by_key(|(k, n)| (*n, k.0.clone()))?;
        let f = self.fonts.get(&font)?;
        Some(Dom { css: f.css_font(), base: f.base.clone(), font, size: size2 as f64 / 2.0, fill })
    }
}

/// Quarter turn (0/90/180/270) that makes the page's dominant text direction run left to right.
fn dominant_turn(an: &Analysis) -> i64 {
    let mut w = [0usize; 4];
    for s in an.shows.iter().filter(|s| !s.blank()) {
        let m = content::mul(s.tm, s.ctm);
        let k = if m[0].abs() >= m[1].abs() { if m[0] > 0.0 { 0 } else { 2 } } else if m[1] > 0.0 { 1 } else { 3 };
        w[k] += s.glyphs.len();
    }
    let best = (0..4).max_by_key(|&k| w[k]).unwrap_or(0);
    best as i64 * 90
}

/// User space -> upright display frame (crop origin at 0, y up) for a page rotated clockwise by `rot` degrees.
fn display_matrix(b: [f64; 4], rot: i64) -> content::M {
    let (w, h) = (b[2], b[3]);
    let r = match rot {
        90 => [0.0, -1.0, 1.0, 0.0, 0.0, w],
        180 => [-1.0, 0.0, 0.0, -1.0, w, h],
        270 => [0.0, 1.0, -1.0, 0.0, h, 0.0],
        _ => content::ID,
    };
    content::mul(content::tr(-b[0], -b[1]), r)
}

fn walk(o: &PdfPageObject, form: bool, direct: &mut Vec<String>, forms: &mut Vec<Seg>, imgs: &mut Vec<[f64; 4]>, paths: &mut Vec<[f64; 4]>) {
    if let Some(t) = o.as_text_object() {
        if !form {
            direct.push(t.text());
            return;
        }
        let text = t.text();
        if text.trim().is_empty() {
            return;
        }
        let Ok(q) = t.bounds().map(|q| q.to_rect()) else { return };
        let base = t.matrix().map(|m| m.f() as f64).unwrap_or(q.bottom().value as f64);
        forms.push(Seg {
            show: None,
            x0: q.left().value as f64,
            x1: q.right().value as f64,
            base,
            size: t.scaled_font_size().value as f64,
            text,
            class: 2,
            reason: Some("form"),
            fam: layout::family(&t.font().name()),
        });
    } else if form && (o.as_image_object().is_some() || o.as_path_object().is_some()) {
        if let Ok(q) = o.bounds().map(|q| q.to_rect()) {
            let b = [q.left().value as f64, q.bottom().value as f64, q.right().value as f64, q.top().value as f64];
            if o.as_image_object().is_some() { imgs.push(b) } else { paths.push(b) }
        }
    } else if let Some(x) = o.as_x_object_form_object() {
        for c in x.iter() {
            walk(&c, true, direct, forms, imgs, paths);
        }
    }
}

fn xobj_is_image(doc: &Document, page: ObjectId, name: &str) -> bool {
    let Some(xo) = effective_resources(doc, page).and_then(|r| r.get(b"XObject").ok()).and_then(|o| deref(doc, o).as_dict().ok()) else { return false };
    matches!(xo.get(name.as_bytes()).ok().map(|o| deref(doc, o)), Some(Object::Stream(s)) if s.dict.get(b"Subtype").ok().and_then(|o| o.as_name().ok()) == Some(b"Image"))
}

/// Images and paths of a page in op order (ids are per kind and stable: they only depend on the original content),
/// then the ones inside form XObjects, which are locked.
fn page_items(doc: &Document, pid: ObjectId, joined: &[u8], an: &Analysis, form_imgs: &[[f64; 4]], form_paths: &[[f64; 4]], lock_all: Option<&'static str>) -> Vec<PItem> {
    let (mut ni, mut np) = (0u32, 0u32);
    let mut out: Vec<PItem> = vec![];
    for (i, o) in an.ops.iter().enumerate() {
        let name = content::op_name(joined, o);
        let (kind, nm) = match o.k {
            OK::Do => match o.bbox.and(do_name(joined, o)) {
                Some(n) if xobj_is_image(doc, pid, &n) => (IKind::Image, Some(n)),
                _ => continue,
            },
            OK::Paint if name == "BI" => (IKind::Image, None),
            OK::Paint if name != "sh" && o.bbox.is_some() => (IKind::Path, None),
            _ => continue,
        };
        let mut b = o.bbox.unwrap();
        let mut lo = i;
        let mut clip = false;
        if kind == IKind::Path {
            // construction ops since the last paint or n (a W among them makes the path a clip too)
            while lo > 0 && matches!(content::op_name(joined, &an.ops[lo - 1]), "m" | "l" | "c" | "v" | "y" | "h" | "re" | "W" | "W*") {
                lo -= 1;
                clip |= matches!(content::op_name(joined, &an.ops[lo]), "W" | "W*");
            }
            // a hairline still needs a target to hit
            for (a, z) in [(0, 2), (1, 3)] {
                if b[z] - b[a] < 1.0 {
                    let m = (b[a] + b[z]) / 2.0;
                    (b[a], b[z]) = (m - 0.5, m + 0.5);
                }
            }
        }
        let id = if kind == IKind::Image { ni += 1; ni - 1 } else { np += 1; np - 1 };
        out.push(PItem { kind, id, op: i, lo, bbox: b, locked: lock_all, name: nm, clip });
    }
    for b in form_imgs {
        out.push(PItem { kind: IKind::Image, id: ni, op: usize::MAX, lo: usize::MAX, bbox: *b, locked: Some("form"), name: None, clip: false });
        ni += 1;
    }
    for b in form_paths {
        out.push(PItem { kind: IKind::Path, id: np, op: usize::MAX, lo: usize::MAX, bbox: *b, locked: Some("form"), name: None, clip: false });
        np += 1;
    }
    out
}

fn do_name(joined: &[u8], o: &content::Op) -> Option<String> {
    let t = String::from_utf8_lossy(&joined[o.s..o.e]).to_string();
    Some(t.trim().strip_prefix('/')?.split_whitespace().next()?.to_string())
}

fn analyse_page(doc: &Document, orig: &[u8], idx: u32) -> Result<PageAn, String> {
    let pid = *doc.get_pages().get(&(idx + 1)).ok_or("bad page")?;
    let raw_box = page_box(doc, pid);
    let rotate = inherited(doc, pid, b"Rotate").map(|o| num(o) as i64).unwrap_or(0);
    let rot = rotate.rem_euclid(360);
    let mut rmat = display_matrix(raw_box, if rot % 90 == 0 { rot } else { 0 });
    let disp = if rot == 90 || rot == 270 { [raw_box[3], raw_box[2]] } else { [raw_box[2], raw_box[3]] };
    let mut page = [0.0, 0.0, disp[0], disp[1]];
    let mut to_disp = content::ID;
    let fonts = load_fonts(doc, pid);
    let joined = doc.get_page_content(pid).map_err(|e| e.to_string())?;
    let mut an = content::analyse_with(&joined, &fonts, &|n| xobj_box(doc, pid, n), rmat);
    let q = dominant_turn(&an);
    if q != 0 {
        let qm = display_matrix([0.0, 0.0, disp[0], disp[1]], q);
        rmat = content::mul(rmat, qm);
        to_disp = content::inv(qm).unwrap_or(content::ID);
        if q == 90 || q == 270 {
            page = [0.0, 0.0, disp[1], disp[0]];
        }
        an = content::analyse_with(&joined, &fonts, &|n| xobj_box(doc, pid, n), rmat);
    }
    // pdfium side: text objects for the 1:1 check and undecodable codes, form text, image coverage
    let (direct, forms, form_imgs, form_paths, coverage) = {
        let p = pdf::pdfium()?;
        let d = p.load_pdf_from_byte_vec(orig.to_vec(), None).map_err(|e| e.to_string())?;
        let pg = d.pages().get(idx as PdfPageIndex).map_err(|e| e.to_string())?;
        let (mut direct, mut forms, mut imgs, mut paths) = (vec![], vec![], vec![], vec![]);
        for o in pg.objects().iter() {
            walk(&o, false, &mut direct, &mut forms, &mut imgs, &mut paths);
        }
        (direct, forms, imgs, paths, pdf::image_coverage(&pg))
    };
    // form text comes from pdfium in user space: into the display frame
    let forms: Vec<Seg> = forms
        .into_iter()
        .map(|mut s| {
            let (y0, y1) = (s.base - 0.22 * s.size, s.base + 0.78 * s.size);
            let pts = [(s.x0, y0), (s.x1, y0), (s.x1, y1), (s.x0, y1)].map(|(x, y)| (x * rmat[0] + y * rmat[2] + rmat[4], x * rmat[1] + y * rmat[3] + rmat[5]));
            s.x0 = pts.iter().map(|p| p.0).fold(f64::MAX, f64::min);
            s.x1 = pts.iter().map(|p| p.0).fold(f64::MIN, f64::max);
            s.base = pts.iter().map(|p| p.1).fold(f64::MAX, f64::min) + 0.22 * s.size;
            s
        })
        .collect();
    let to_work = |b: [f64; 4]| -> [f64; 4] {
        let pts = [(b[0], b[1]), (b[2], b[1]), (b[2], b[3]), (b[0], b[3])].map(|(x, y)| (x * rmat[0] + y * rmat[2] + rmat[4], x * rmat[1] + y * rmat[3] + rmat[5]));
        [pts.iter().map(|p| p.0).fold(f64::MAX, f64::min), pts.iter().map(|p| p.1).fold(f64::MAX, f64::min), pts.iter().map(|p| p.0).fold(f64::MIN, f64::max), pts.iter().map(|p| p.1).fold(f64::MIN, f64::max)]
    };
    let form_imgs: Vec<[f64; 4]> = form_imgs.into_iter().map(to_work).collect();
    let form_paths: Vec<[f64; 4]> = form_paths.into_iter().map(to_work).collect();
    let form_segs = forms.clone();
    let lock_all = if rot % 90 != 0 {
        Some("rotated")
    } else if direct.len() != an.shows.len() {
        Some("structure")
    } else {
        None
    };
    if lock_all.is_none() {
        for (s, t) in an.shows.iter_mut().zip(direct.iter()) {
            if s.glyphs.iter().any(|g| g.text.is_none()) {
                let cs: Vec<char> = t.chars().collect();
                let ok = cs.len() == s.glyphs.len();
                for (i, g) in s.glyphs.iter_mut().enumerate() {
                    if g.text.is_none() {
                        g.text = Some(if ok { cs[i].to_string() } else { "\u{fffd}".into() });
                    }
                }
            }
        }
    }
    let (mut paras, pitch) = find_paras(&PageIn { shows: &an.shows, fonts: &fonts, forms, page });
    if let Some(r) = lock_all {
        for p in paras.iter_mut() {
            p.locked = Some(r);
            p.ed = None;
        }
    }
    let (foot_y, head_y) = zones(&an, page, pitch);
    let items = page_items(doc, pid, &joined, &an, &form_imgs, &form_paths, lock_all);
    Ok(PageAn { idx, items, form_segs, form_imgs, pid, page, lock: lock_all, pdfium_texts: direct.len(), scan: coverage >= pdf::IMAGE_DOMINANT_PCT, joined, an, fonts, paras, foot_y, head_y, rinv: content::inv(rmat).unwrap_or(content::ID), disp, to_disp, q })
}

// ---------------- edits and building ----------------

/// Paragraph edit (Edit 1 + format).
#[derive(Clone, Debug, PartialEq)]
struct Edit {
    page: u32,
    para: u32,
    text: String,
    use_pc: bool,
    allow_overlap: bool,
    style: Option<Fmt>,
    runs: Option<Vec<Run>>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum ObjKind {
    TextBox,
    WhiteOut,
    Image,
}

/// Text box, white-out or image placed on a page. `rect` is the display bbox (pt, top-left origin) as requested.
#[derive(Clone, Debug, PartialEq)]
struct ObjE {
    page: u32,
    id: u32,
    kind: ObjKind,
    rect: [f64; 4],
    text: String,
    style: Fmt,
    rot: i64,
    asset: String,
    cover_only: bool,
    runs: Option<Vec<Run>>,
    use_pc: bool,
}

/// Select-tool state of one paragraph, image or path: moved (dx, dy in display pt, y down; images keep the new box instead) or removed.
/// An image's `rect` is where it sits before any push (pushes by paragraph edits still apply on top).
#[derive(Clone, Debug, PartialEq)]
struct MvE {
    page: u32,
    kind: IKind,
    id: u32,
    dx: f64,
    dy: f64,
    rect: Option<[f64; 4]>,
    removed: bool,
}

impl MvE {
    fn new(page: u32, kind: IKind, id: u32) -> MvE {
        MvE { page, kind, id, dx: 0.0, dy: 0.0, rect: None, removed: false }
    }
    fn is_identity(&self) -> bool {
        self.dx.abs() < 1e-9 && self.dy.abs() < 1e-9 && self.rect.is_none() && !self.removed
    }
}

/// Watermark over the whole document (or its page ranges).
#[derive(Clone, Debug, PartialEq)]
struct WmE {
    id: u32,
    image: bool,
    text: String,
    style: Fmt,
    asset: String,
    scale: f64,
    opacity: f64,
    angle: f64,
    front: bool,
    pages: String,
}

/// Header or footer text with placeholders.
#[derive(Clone, Debug, PartialEq)]
struct HfE {
    id: u32,
    pos: String,
    template: String,
    style: Fmt,
    margin_pt: f64,
    start: u32,
    skip_first: bool,
    pages: String,
    lang: String,
    tz_min: i32,
}

/// One entry of the effective edit list; list order is z-order.
#[derive(Clone, Debug, PartialEq)]
enum Item {
    Para(Edit),
    Obj(ObjE),
    Mv(MvE),
    Wm(WmE),
    Hf(HfE),
}

impl From<Edit> for Item {
    fn from(e: Edit) -> Item {
        Item::Para(e)
    }
}

impl Item {
    /// u32::MAX for the document-wide items (watermark, header/footer).
    fn page(&self) -> u32 {
        match self {
            Item::Para(e) => e.page,
            Item::Obj(o) => o.page,
            Item::Mv(m) => m.page,
            Item::Wm(_) | Item::Hf(_) => u32::MAX,
        }
    }
}

/// History entries: setting an item, or deleting an object.
#[derive(Clone, Debug)]
enum Act {
    Set(Item),
    Del(u32, u32), // page (u32::MAX = document-wide), id
    Batch(Vec<Act>), // one undo step
}

#[derive(Debug)]
enum BErr {
    Missing(String),
    Unsupported(String),
    Overflow(u32),
    CannotPush,
    Partial(usize),
    CannotMove(u32, IKind, u32), // page, item
    BadPages(String),
    TooLong,
    Other(String),
}

impl From<String> for BErr {
    fn from(s: String) -> Self {
        BErr::Other(s)
    }
}

#[derive(Default, Clone)]
struct PageInfo {
    shift: Vec<f64>,           // per paragraph: how far it moved down, pt
    lines: HashMap<u32, usize>, // edited paragraph -> new line count
    pc: HashSet<u32>,          // paragraphs retyped in a PC font
    ext: HashMap<u32, (f64, f64)>, // edited paragraph -> (left, right) of its new lines
    objs: HashMap<u32, [f64; 4]>,  // text box -> display rect with its real typeset size
    removed: usize,            // glyphs removed by white-outs
    runs: HashMap<u32, Vec<Run>>, // edited paragraph -> its content as runs
    off: HashMap<u32, (f64, f64)>, // moved paragraph -> offset (analysis frame)
    gone: HashSet<u32>,        // removed paragraphs
    ishift: HashMap<(IKind, u32), f64>, // image / path -> how far a push moved it down, pt
}

struct Built {
    bytes: Vec<u8>,
    info: HashMap<u32, PageInfo>,
    pc_used: bool,
}

fn fail(f: Fail) -> BErr {
    match f {
        Fail::Missing(c) => BErr::Missing(c),
        Fail::Unsupported(c) => BErr::Unsupported(c),
        Fail::NoPcFont => BErr::Other("no_pc_font".into()),
    }
}

// ---------------- session ----------------

struct Session {
    orig: Vec<u8>,
    doc: Document,
    path: PathBuf,
    sizes: Vec<(f32, f32)>,
    pages: HashMap<u32, Arc<PageAn>>,
    hist: Vec<Act>,
    cursor: usize,
    assets: HashMap<String, Arc<Vec<u8>>>, // PNG bytes of placed images (kept even if the library item is deleted)
    next_id: u32,
    lib_dir: Option<PathBuf>, // tests only
    built: Option<Arc<Built>>,
    used: Option<Arc<HashMap<ObjectId, HashSet<u32>>>>,
    floor_override: Option<f64>, // tests only
    scan_cache: HashMap<u32, bool>,
    _guard: Option<JobGuard>,
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);

fn with_session<T>(f: impl FnOnce(&mut Session) -> Result<T, String>) -> Result<T, String> {
    let mut g = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    f(g.as_mut().ok_or("no edit session")?)
}

fn to_bytes(path: &Path, pw: Option<&str>, locked: bool) -> Result<(Vec<u8>, Document), String> {
    let raw = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    if let Ok(d) = Document::load_mem(&raw).map_err(|_| ()).and_then(|d| if locked { Err(()) } else { Ok(d) }) {
        if !d.is_encrypted() {
            return Ok((raw, d));
        }
    }
    // encrypted or damaged for lopdf: let pdfium write a clean, decrypted copy first
    let bytes = {
        let p = pdf::pdfium()?;
        // pdfium keeps the encryption when it re-saves a document, so the pages go into a fresh one
        let d = p.load_pdf_from_file(path, pw).map_err(|e| e.to_string())?;
        let mut out = p.create_new_pdf().map_err(|e| e.to_string())?;
        let n = d.pages().len();
        out.pages_mut().copy_page_range_from_document(&d, 0..=n.saturating_sub(1), 0).map_err(|e| e.to_string())?;
        out.save_to_bytes().map_err(|e| e.to_string())?
    };
    let d = Document::load_mem(&bytes).map_err(|e| format!("cannot parse the PDF: {e}"))?;
    Ok((bytes, d))
}

impl Session {
    fn open(path: &Path, pw: Option<&str>, guard: Option<JobGuard>) -> Result<(Session, EditDoc), String> {
        match pdf::open(path, pw) {
            Opened::Ok { .. } => {}
            Opened::NeedsPassword => return Err("password required or wrong".into()),
            Opened::Damaged => return Err("pdfium could not open the file".into()),
        }
        let (orig, doc) = to_bytes(path, pw, matches!(pdf::open(path, None), Opened::NeedsPassword))?;
        let sizes: Vec<(f32, f32)> = {
            let p = pdf::pdfium()?;
            let d = p.load_pdf_from_byte_vec(orig.clone(), None).map_err(|e| e.to_string())?;
            let v = d.pages().iter().map(|pg| (pg.width().value, pg.height().value)).collect();
            v
        };
        let edoc = EditDoc { pages: sizes.len() as u32, sizes: sizes.clone() };
        let s = Session { orig, doc, path: path.to_path_buf(), sizes, pages: HashMap::new(), hist: vec![], cursor: 0, assets: HashMap::new(), next_id: 0, lib_dir: None, built: None, used: None, floor_override: None, scan_cache: HashMap::new(), _guard: guard };
        Ok((s, edoc))
    }

    fn page(&mut self, idx: u32) -> Result<Arc<PageAn>, String> {
        if idx as usize >= self.sizes.len() {
            return Err("bad page".into());
        }
        if let Some(p) = self.pages.get(&idx) {
            return Ok(p.clone());
        }
        let pa = Arc::new(analyse_page(&self.doc, &self.orig, idx)?);
        self.pages.insert(idx, pa.clone());
        Ok(pa)
    }

    /// The applied history replayed: one entry per paragraph / object in z order; edits that restore the original vanish.
    fn effective(&mut self) -> Vec<Item> {
        let mut v: Vec<Item> = vec![];
        for a in &self.hist[..self.cursor] {
            replay(&mut v, a);
        }
        let keep: Vec<bool> = v
            .iter()
            .map(|it| match it {
                Item::Para(e) => e.use_pc || e.style.is_some() || e.runs.is_some() || self.page(e.page).ok().and_then(|p| p.paras.get(e.para as usize).map(|q| norm(&q.text) != e.text)).unwrap_or(true),
                Item::Mv(m) => !m.is_identity(),
                _ => true,
            })
            .collect();
        v.into_iter().zip(keep).filter(|x| x.1).map(|x| x.0).collect()
    }

    /// Scan flags (image-dominant pages) of the original, cached.
    fn scan_flags(&mut self, pages: &BTreeSet<u32>) -> Result<HashMap<u32, bool>, String> {
        let todo: Vec<u32> = pages.iter().copied().filter(|p| !self.scan_cache.contains_key(p)).collect();
        if !todo.is_empty() {
            let p = pdf::pdfium()?;
            let d = p.load_pdf_from_byte_vec(self.orig.clone(), None).map_err(|e| e.to_string())?;
            for pg in todo {
                let page = d.pages().get(pg as PdfPageIndex).map_err(|e| e.to_string())?;
                self.scan_cache.insert(pg, pdf::image_coverage(&page) >= pdf::IMAGE_DOMINANT_PCT);
            }
        }
        Ok(pages.iter().map(|p| (*p, self.scan_cache[p])).collect())
    }

    fn build<T: Into<Item> + Clone>(&mut self, edits: &[T]) -> Result<Built, BErr> {
        let items: Vec<Item> = edits.iter().cloned().map(Into::into).collect();
        if items.is_empty() {
            return Ok(Built { bytes: self.orig.clone(), info: HashMap::new(), pc_used: false });
        }
        if self.used.is_none() {
            self.used = Some(Arc::new(used_codes(&self.doc)));
        }
        let used = self.used.clone().unwrap();
        let n = self.sizes.len() as u32;
        // document-wide objects: which pages, numbering, default styles
        let stem = self.path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let (mut wms, mut hfs): (Vec<docobj::WmPlan>, Vec<docobj::HfPlan>) = (vec![], vec![]);
        for it in &items {
            match it {
                Item::Wm(w) => wms.push(docobj::WmPlan { w: w.clone(), pages: docobj::parse_pages(&w.pages, n).map_err(BErr::BadPages)? }),
                Item::Hf(h) => {
                    let pages: Vec<u32> = docobj::parse_pages(&h.pages, n).map_err(BErr::BadPages)?.into_iter().collect();
                    let Some(&first) = pages.first() else { continue };
                    let mut style = h.style.clone();
                    if style.size <= 0.0 || style.font.is_empty() || style.font == "orig" {
                        // the dominant body style of the first page in its PC family, 2 pt smaller (at least 8)
                        let dom = self.page(first)?.dominant();
                        let fams = fonts::available_families();
                        if style.font.is_empty() || style.font == "orig" {
                            style.font = dom.as_ref().map(|d| d.css.clone()).filter(|c| fams.contains(c)).unwrap_or_else(|| "Arial".into());
                        }
                        if style.size <= 0.0 {
                            style.size = dom.as_ref().map(|d| (d.size - 2.0).max(8.0)).unwrap_or(10.0);
                        }
                    }
                    let total = pages.len() as i64 + h.start as i64 - 1;
                    hfs.push(docobj::HfPlan { h: h.clone(), pages, style, total, date: docobj::today(&h.lang, h.tz_min), date_id: docobj::today("id", h.tz_min), date_en: docobj::today("en", h.tz_min), file: stem.clone() });
                }
                _ => {}
            }
        }
        let need: BTreeSet<u32> = wms.iter().filter(|w| !w.w.front).flat_map(|w| w.pages.iter().copied()).collect();
        let scans = if need.is_empty() { HashMap::new() } else { self.scan_flags(&need)? };
        let mut pages: BTreeSet<u32> = items.iter().map(Item::page).filter(|p| *p != u32::MAX).collect();
        pages.extend(wms.iter().flat_map(|w| w.pages.iter().copied()));
        pages.extend(hfs.iter().flat_map(|h| h.pages.iter().copied()));
        let mut doc = self.doc.clone();
        let pids = doc.get_pages();
        let mut acc = build::Acc::default();
        let mut info = HashMap::new();
        let assets = self.assets.clone();
        for pg in pages {
            let pid = *pids.get(&(pg + 1)).ok_or_else(|| BErr::Other("bad page".into()))?;
            let es: Vec<&Item> = items.iter().filter(|e| e.page() == pg).collect();
            let pa = if es.is_empty() { None } else { Some(self.page(pg)?) };
            let sh = build::Shared { used: &used, floor_override: self.floor_override, orig: &self.orig, assets: &assets };
            let g = docobj::geom(&doc, pid);
            let add = docobj::page_add(&mut doc, &mut acc, &sh, &g, pg, &wms, &hfs, &scans)?;
            match pa {
                Some(pa) => {
                    info.insert(pg, build::build_page(&mut doc, &pa, &es, &sh, &mut acc, &add)?);
                }
                None if !add.behind.is_empty() || !add.front.is_empty() => build::light_page(&mut doc, pid, &add, &mut acc),
                None => {}
            }
        }
        let live: HashSet<ObjectId> = doc.page_iter().flat_map(|p| doc.get_page_contents(p)).collect();
        for id in acc.old.into_iter().filter(|i| !live.contains(i)) {
            doc.objects.remove(&id);
        }
        if acc.prune {
            doc.prune_objects(); // the replaced image's original pixels must not stay in the file
        }
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).map_err(|e| BErr::Other(e.to_string()))?;
        Ok(Built { bytes, info, pc_used: !acc.pc_objs.is_empty() })
    }

    fn built(&mut self) -> Result<Arc<Built>, String> {
        if self.built.is_none() {
            let e = self.effective();
            let b = self.build(&e).map_err(|e| match e {
                BErr::Other(s) => s,
                _ => "cannot rebuild the edited document".to_string(),
            })?;
            self.built = Some(Arc::new(b));
        }
        Ok(self.built.clone().unwrap())
    }

    fn model(&mut self, idx: u32) -> Result<PageModel, String> {
        let pa = self.page(idx)?;
        let b = self.built()?;
        let items = self.effective();
        let info = b.info.get(&idx);
        let paras: Vec<ParaOut> = pa
            .paras
            .iter()
            .map(|p| {
                let e = items.iter().find_map(|x| match x {
                    Item::Para(e) if e.page == idx && e.para == p.id => Some(e),
                    _ => None,
                });
                let shift = info.and_then(|i| i.shift.get(p.id as usize)).copied().unwrap_or(0.0);
                let nl = info.and_then(|i| i.lines.get(&p.id)).copied().unwrap_or(p.nlines);
                let pitch = e.and_then(|e| e.style.as_ref()).filter(|st| st.size > 0.0 && p.size > 0.0).map(|st| p.pitch * st.size / p.size).unwrap_or(p.pitch);
                let gone = info.is_some_and(|i| i.gone.contains(&p.id));
                let (ox, oy) = info.and_then(|i| i.off.get(&p.id)).copied().unwrap_or((0.0, 0.0));
                let (top, bot) = (p.top - shift + oy, p.bot - shift + oy - (nl as f64 * pitch - p.nlines as f64 * p.pitch));
                let base = Fmt { font: build::base_font(p), size: p.size, color: p.color.clone(), bold: p.bold, italic: p.italic, align: p.align };
                ParaOut {
                    id: p.id,
                    bbox: {
                        let (l, r) = info.and_then(|i| i.ext.get(&p.id)).copied().unwrap_or((p.l, p.r));
                        let (l, r) = (l + ox, r + ox);
                        // work frame -> displayed page, top-left origin
                        let t = pa.to_disp;
                        let pts = [(l, bot), (r, bot), (r, top.max(bot)), (l, top.max(bot))].map(|(x, y)| (x * t[0] + y * t[2] + t[4], x * t[1] + y * t[3] + t[5]));
                        let (x0, x1) = (pts.iter().map(|p| p.0).fold(f64::MAX, f64::min), pts.iter().map(|p| p.0).fold(f64::MIN, f64::max));
                        let (y0, y1) = (pts.iter().map(|p| p.1).fold(f64::MAX, f64::min), pts.iter().map(|p| p.1).fold(f64::MIN, f64::max));
                        if gone { [0.0; 4] } else { [x0, pa.disp[1] - y1, x1 - x0, y1 - y0] }
                    },
                    text: e.map(|e| e.text.clone()).unwrap_or_else(|| p.text.clone()),
                    marker: p.marker.clone(),
                    align: e.and_then(|e| e.runs.as_ref().and_then(|r| r.first()).map(|r| r.style.align).or(e.style.as_ref().map(|s| s.align))).unwrap_or(p.align),
                    size: p.size,
                    pitch: p.pitch,
                    first_indent: p.first_indent,
                    css_font: p.css_font.clone(),
                    bold: p.bold,
                    italic: p.italic,
                    color: p.color.clone(),
                    status: if p.locked.is_some() || gone { "locked" } else if p.pc_font { "pc_font" } else { "direct" },
                    reason: if gone { Some("removed") } else { p.locked },
                    removed: gone,
                    edited: e.is_some(),
                    pc_font_used: info.is_some_and(|i| i.pc.contains(&p.id)),
                    rot: (360 - pa.q) % 360,
                    style: e.and_then(|e| e.style.clone()).unwrap_or(base),
                    orig_font_name: p.orig_font.clone(),
                    runs: match e.and_then(|e| e.runs.clone()) {
                        Some(r) => merge_runs(r),
                        None => info.and_then(|i| i.runs.get(&p.id)).cloned().unwrap_or_else(|| base_runs(&pa, p)),
                    },
                }
            })
            .collect();
        let dom_name = pa.dominant().map(|d| d.base).unwrap_or_default();
        let objs = items
            .iter()
            .filter_map(|x| match x {
                Item::Obj(o) if o.page == idx => Some(ObjOut {
                    id: o.id,
                    kind: match o.kind {
                        ObjKind::TextBox => "textbox",
                        ObjKind::WhiteOut => "whiteout",
                        ObjKind::Image => "image",
                    },
                    rect: info.and_then(|i| i.objs.get(&o.id)).copied().unwrap_or(o.rect),
                    rot: o.rot,
                    text: (o.kind == ObjKind::TextBox).then(|| o.text.clone()),
                    style: (o.kind == ObjKind::TextBox).then(|| o.style.clone()),
                    asset: (o.kind == ObjKind::Image).then(|| o.asset.clone()),
                    orig_font_name: dom_name.clone(),
                    runs: (o.kind == ObjKind::TextBox).then(|| o.runs.clone().map(merge_runs).unwrap_or_else(|| vec![Run { text: o.text.clone(), style: o.style.clone() }])),
                }),
                _ => None,
            })
            .collect();
        // Select-tool items: paragraphs as modelled above, then images and paths where the current state puts them
        // z-order: by the position of the item's first op in the content stream (last = on top); form content and locked text sit at the bottom
        let mut sel: Vec<(usize, ItemOut)> = paras
            .iter()
            .zip(pa.paras.iter())
            .filter(|(p, _)| !p.removed)
            .map(|(p, q)| {
                let z = q.ed.as_ref().and_then(|e| e.shows.iter().map(|&i| pa.an.shows[i].op).min()).unwrap_or(0);
                (z, ItemOut { id: p.id, kind: "para", bbox: p.bbox, rot: p.rot, locked: if p.status == "locked" { p.reason } else { None } })
            })
            .collect();
        for pi in &pa.items {
            let mv = items.iter().find_map(|x| match x {
                Item::Mv(m) if m.page == idx && m.kind == pi.kind && m.id == pi.id => Some(m),
                _ => None,
            });
            if mv.is_some_and(|m| m.removed) {
                continue;
            }
            let tot = info.and_then(|i| i.ishift.get(&(pi.kind, pi.id))).copied().unwrap_or(0.0);
            let wb = mv.and_then(|m| m.rect).map(|r| build::rect_work(&pa, r)).unwrap_or(pi.bbox);
            let mut wb = [wb[0], wb[1] - tot, wb[2], wb[3] - tot];
            if let Some(m) = mv.filter(|m| m.rect.is_none()) {
                let (dx, dy) = disp_delta_to_work(&pa, m.dx, m.dy);
                wb = [wb[0] + dx, wb[1] + dy, wb[2] + dx, wb[3] + dy];
            }
            sel.push((if pi.op == usize::MAX { 0 } else { pi.op }, ItemOut { id: pi.id, kind: if pi.kind == IKind::Image { "image" } else { "path" }, bbox: disp_of_work(&pa, wb), rot: 0, locked: pi.locked }));
        }
        sel.sort_by_key(|x| x.0);
        let sel: Vec<ItemOut> = sel.into_iter().map(|x| x.1).collect();
        let n = self.sizes.len() as u32;
        let forced = pa.scan && items.iter().any(|x| matches!(x, Item::Wm(w) if !w.front && docobj::parse_pages(&w.pages, n).is_ok_and(|s| s.contains(&idx))));
        Ok(PageModel { page: idx, paras, scan: pa.scan, objs, items: sel, wm_forced_front: forced })
    }

    fn history(&mut self, touched: Vec<u32>) -> EditHistory {
        let mut pages: Vec<u32> = self.effective().iter().map(Item::page).chain(touched).collect();
        if pages.contains(&u32::MAX) {
            // a document-wide object changes every page
            pages = (0..self.sizes.len() as u32).collect();
        }
        pages.sort_unstable();
        pages.dedup();
        EditHistory { can_undo: self.cursor > 0, can_redo: self.cursor < self.hist.len(), count: self.effective().len() as u32, changed_pages: pages }
    }

    /// Pushes a history entry (clearing the redo tail). A format-only change of the same target right after the previous one replaces it,
    /// so a burst of live-preview changes is one undo step.
    fn push_act(&mut self, a: Act) {
        self.hist.truncate(self.cursor);
        let merge = self.cursor > 0
            && match (&self.hist[self.cursor - 1], &a) {
                (Act::Set(Item::Para(p)), Act::Set(Item::Para(q))) => p.page == q.page && p.para == q.para && p.text == q.text,
                (Act::Set(Item::Wm(p)), Act::Set(Item::Wm(q))) => p.id == q.id,
                (Act::Set(Item::Hf(p)), Act::Set(Item::Hf(q))) => p.id == q.id,
                (Act::Set(Item::Obj(p)), Act::Set(Item::Obj(q))) => p.id == q.id && p.kind == q.kind && p.text == q.text && p.rect == q.rect && p.rot == q.rot && p.asset == q.asset,
                _ => false,
            };
        if merge {
            self.hist[self.cursor - 1] = a;
        } else {
            self.hist.push(a);
            self.cursor += 1;
        }
    }

    fn apply(&mut self, req: &EditApplyReq) -> Result<EditApplyRes, String> {
        let pa = self.page(req.page)?;
        let para = pa.paras.get(req.para as usize).ok_or("no such paragraph")?;
        if para.locked.is_some() {
            return Err("locked".into());
        }
        let mut runs: Option<Vec<Run>> = req.runs.clone().map(|rs| rs.into_iter().map(|r| Run { style: norm_fmt(para, &r.style), ..r }).collect());
        let text = match &runs {
            Some(rs) => norm(&rs.iter().map(|r| r.text.as_str()).collect::<String>()),
            None => norm(&req.text),
        };
        if runs.as_ref().is_some_and(|rs| !req.use_pc_font && canon(rs) == canon(&base_runs(&pa, para))) {
            runs = None; // back to the original look
        }
        let runs = runs.map(|r| merge_runs(same_align(r)));
        // a style equal to the paragraph's own look is no style
        let style = req.style.clone().map(|st| norm_fmt(para, &st)).filter(|_| runs.is_none()).filter(|st| {
            st.font != build::base_font(para) || (st.size - para.size).abs() > 0.01 || !st.color.eq_ignore_ascii_case(&para.color) || st.bold != para.bold || st.italic != para.italic || st.align != para.align
        });
        let e = Edit { page: req.page, para: req.para, text, use_pc: req.use_pc_font, allow_overlap: req.allow_overlap, style, runs };
        let cur = self.effective();
        let at = cur.iter().position(|x| matches!(x, Item::Para(p) if p.page == e.page && p.para == e.para));
        let mut trial: Vec<Item> = cur.into_iter().filter(|x| !matches!(x, Item::Para(p) if p.page == e.page && p.para == e.para)).collect();
        let noop = !e.use_pc && e.style.is_none() && e.runs.is_none() && norm(&para.text) == e.text;
        if noop && at.is_none() {
            return Ok(EditApplyRes::Ok { page: self.model(req.page)? });
        }
        if !noop {
            // an existing edit keeps its slot in the list (z-order)
            trial.insert(at.map(|i| i.min(trial.len())).unwrap_or(trial.len()), Item::Para(e.clone()));
        }
        match self.build(&trial) {
            Ok(b) => {
                self.push_act(Act::Set(Item::Para(e)));
                self.built = Some(Arc::new(b));
                Ok(EditApplyRes::Ok { page: self.model(req.page)? })
            }
            Err(BErr::Missing(chars)) => Ok(EditApplyRes::MissingChars { chars }),
            Err(BErr::Unsupported(chars)) => Ok(EditApplyRes::UnsupportedChars { chars }),
            Err(BErr::Overflow(n)) => Ok(EditApplyRes::Overflow { lines_over: n }),
            Err(BErr::CannotPush) => Ok(EditApplyRes::CannotPush),
            Err(BErr::Partial(_)) | Err(BErr::CannotMove(..)) | Err(BErr::BadPages(_)) | Err(BErr::TooLong) => Err("unexpected result".into()),
            Err(BErr::Other(s)) => Err(s),
        }
    }

    fn apply_obj(&mut self, req: &ObjReq) -> Result<ObjRes, String> {
        let pa = self.page(req.page)?;
        let items = self.effective();
        let find = |id: u32| {
            items.iter().find_map(|x| match x {
                Item::Obj(o) if o.id == id => Some(o.clone()),
                _ => None,
            })
        };
        if req.kind == "delete" {
            let id = req.id.ok_or("no id")?;
            find(id).ok_or("no such object")?;
            self.push_act(Act::Del(req.page, id));
            self.built = None;
            self.built()?;
            return Ok(ObjRes::Ok { page: self.model(req.page)?, id });
        }
        let kind = match req.kind.as_str() {
            "textbox" => ObjKind::TextBox,
            "whiteout" => ObjKind::WhiteOut,
            "image" => ObjKind::Image,
            _ => return Err("bad kind".into()),
        };
        let prev = req.id.and_then(find);
        let id = match req.id {
            Some(id) => {
                self.next_id = self.next_id.max(id);
                id
            }
            None => {
                self.next_id += 1;
                self.next_id
            }
        };
        let mut rect = req.rect.or(prev.as_ref().map(|p| p.rect)).ok_or("no rect")?;
        let rot = req.rot.or(prev.as_ref().map(|p| p.rot)).unwrap_or((360 - pa.q) % 360).rem_euclid(360) / 90 * 90;
        let runs = req.runs.clone().filter(|r| !r.is_empty()).map(|r| merge_runs(same_align(r.into_iter().map(|x| Run { text: collapse_spaces(&x.text), ..x }).collect())));
        let text = match &runs {
            Some(r) => r.iter().map(|x| x.text.as_str()).collect(),
            None => req.text.clone().or(prev.as_ref().map(|p| p.text.clone())).unwrap_or_default().replace("\r\n", "\n"),
        };
        let runs = runs.or_else(|| if req.text.is_none() && req.style.is_none() { prev.as_ref().and_then(|p| p.runs.clone()) } else { None });
        let style = match runs.as_ref().and_then(|r| r.first()) {
            Some(r) => r.style.clone(),
            None => req.style.clone().or(prev.as_ref().map(|p| p.style.clone())).unwrap_or_else(|| default_fmt(&pa)),
        };
        let mut asset = req.asset.clone().or(prev.as_ref().map(|p| p.asset.clone())).unwrap_or_default();
        if kind == ObjKind::Image {
            let png = match self.assets.get(&asset) {
                Some(p) => p.clone(),
                None => {
                    let lib = sig::Lib { dir: match &self.lib_dir { Some(d) => d.clone(), None => sig::default_dir()? } };
                    let png = Arc::new(lib.png(&asset).ok_or("no such asset")?);
                    self.assets.insert(asset.clone(), png.clone());
                    png
                }
            };
            if rect[2] <= 0.0 || rect[3] <= 0.0 {
                // a click point: the image is centred on it, 150 pt wide (110 for a stamp)
                let (w, h) = image::load_from_memory(&png).map(|i| (i.width() as f64, i.height() as f64)).map_err(|e| e.to_string())?;
                let stamp = sig::Lib { dir: match &self.lib_dir { Some(d) => d.clone(), None => sig::default_dir()? } }.kind_of(&asset).as_deref() == Some("stamp");
                let tw = if stamp { 110.0 } else { 150.0 };
                rect = [rect[0] - tw / 2.0, rect[1] - tw * h / w / 2.0, tw, tw * h / w];
            }
        } else {
            asset.clear();
        }
        let o = ObjE { page: req.page, id, kind, rect, text, style, rot, asset, cover_only: req.cover_only.unwrap_or(prev.as_ref().is_some_and(|p| p.cover_only)), runs: if kind == ObjKind::TextBox { runs } else { None }, use_pc: req.use_pc_font.unwrap_or(prev.as_ref().is_some_and(|p| p.use_pc)) };
        if prev.as_ref() == Some(&o) {
            return Ok(ObjRes::Ok { page: self.model(req.page)?, id });
        }
        let mut trial = items.clone();
        match trial.iter().position(|x| matches!(x, Item::Obj(p) if p.id == id)) {
            Some(i) => trial[i] = Item::Obj(o.clone()),
            None => trial.push(Item::Obj(o.clone())),
        }
        match self.build(&trial) {
            Ok(b) => {
                self.push_act(Act::Set(Item::Obj(o)));
                self.built = Some(Arc::new(b));
                Ok(ObjRes::Ok { page: self.model(req.page)?, id })
            }
            Err(BErr::Missing(chars)) => Ok(ObjRes::MissingChars { chars }),
            Err(BErr::Unsupported(chars)) => Ok(ObjRes::UnsupportedChars { chars }),
            Err(BErr::Partial(chars)) => Ok(ObjRes::WhiteoutPartial { chars }),
            Err(BErr::Other(s)) => Err(s),
            Err(_) => Err("cannot place the object".into()),
        }
    }

    // ---------------- Edit 3: Select tool, watermark, header & footer ----------------

    /// Display-frame shift (pt, y down) that pushes from paragraph edits currently apply to an image or path.
    fn push_shift(&mut self, page: u32, kind: IKind, id: u32) -> Result<(f64, f64), String> {
        let tot = self.built()?.info.get(&page).and_then(|i| i.ishift.get(&(kind, id))).copied().unwrap_or(0.0);
        let t = self.page(page)?.to_disp;
        Ok((-tot * t[2], tot * t[3]))
    }

    /// Moves, resizes and removals of existing paragraphs, images and paths: one undo step for the whole list.
    fn apply_batch(&mut self, req: &EditBatchReq) -> Result<BatchRes, String> {
        let mut cur = self.effective();
        let mut acts: Vec<Act> = vec![];
        let mut pages: BTreeSet<u32> = BTreeSet::new();
        for e in &req.edits {
            let (page, item) = match e {
                ObjEdit3::Move { page, item, .. } | ObjEdit3::Resize { page, item, .. } | ObjEdit3::Remove { page, item } => (*page, *item),
            };
            let pa = self.page(page)?;
            let (exists, locked, orig) = match item.kind {
                IKind::Para => match pa.paras.get(item.id as usize) {
                    Some(p) => (true, p.locked.is_some() || p.ed.is_none(), [0.0; 4]),
                    None => (false, false, [0.0; 4]),
                },
                k => match pa.items.iter().find(|x| x.kind == k && x.id == item.id) {
                    Some(x) => (true, x.locked.is_some(), x.bbox),
                    None => (false, false, [0.0; 4]),
                },
            };
            if !exists {
                return Err("no such item".into());
            }
            if locked {
                return Ok(BatchRes::CannotMove { page, item });
            }
            let same = |x: &Item| matches!(x, Item::Mv(m) if m.page == page && m.kind == item.kind && m.id == item.id);
            let mut mv = cur.iter().find(|x| same(x)).and_then(|x| if let Item::Mv(m) = x { Some(m.clone()) } else { None }).unwrap_or_else(|| MvE::new(page, item.kind, item.id));
            match e {
                ObjEdit3::Move { dx, dy, .. } if item.kind == IKind::Image => {
                    let b = mv.rect.unwrap_or_else(|| disp_of_work(&pa, orig));
                    mv.rect = Some([b[0] + dx, b[1] + dy, b[2], b[3]]);
                }
                ObjEdit3::Move { dx, dy, .. } => {
                    mv.dx += dx;
                    mv.dy += dy;
                }
                ObjEdit3::Resize { rect, .. } => {
                    if item.kind != IKind::Image {
                        return Err("only images can be resized".into());
                    }
                    let (sx, sy) = self.push_shift(page, item.kind, item.id)?;
                    mv.rect = Some([rect[0] - sx, rect[1] - sy, rect[2].max(1.0), rect[3].max(1.0)]);
                }
                ObjEdit3::Remove { .. } => mv.removed = true,
            }
            match cur.iter().position(same) {
                Some(i) => cur[i] = Item::Mv(mv.clone()),
                None => cur.push(Item::Mv(mv.clone())),
            }
            acts.push(Act::Set(Item::Mv(mv)));
            pages.insert(page);
        }
        if acts.is_empty() {
            return Ok(BatchRes::Ok { pages: vec![] });
        }
        match self.build(&cur) {
            Ok(b) => {
                self.push_act(Act::Batch(acts));
                self.built = Some(Arc::new(b));
                let pages = pages.into_iter().map(|p| self.model(p)).collect::<Result<Vec<_>, _>>()?;
                Ok(BatchRes::Ok { pages })
            }
            Err(BErr::CannotMove(page, kind, id)) => Ok(BatchRes::CannotMove { page, item: ItemRef { kind, id } }),
            Err(BErr::Other(s)) => Err(s),
            Err(_) => Err("cannot apply the change".into()),
        }
    }

    /// PNG bytes of a library asset, kept in the session so placed uses survive deleting the library item.
    fn ensure_asset(&mut self, asset: &str) -> Result<(), String> {
        if !self.assets.contains_key(asset) {
            let lib = sig::Lib { dir: match &self.lib_dir { Some(d) => d.clone(), None => sig::default_dir()? } };
            let png = lib.png(asset).ok_or("no such asset")?;
            self.assets.insert(asset.to_string(), Arc::new(png));
        }
        Ok(())
    }

    fn doc_id(&mut self, id: Option<u32>) -> u32 {
        match id {
            Some(id) => {
                self.next_id = self.next_id.max(id);
                id
            }
            None => {
                self.next_id += 1;
                self.next_id
            }
        }
    }

    /// Sets a watermark or header/footer (replacing the one with the same id) after a trial build.
    fn apply_doc(&mut self, it: Item, id: u32) -> Result<DocRes, String> {
        let mut trial = self.effective();
        let same = |x: &Item| match (x, &it) {
            (Item::Wm(a), Item::Wm(b)) => a.id == b.id,
            (Item::Hf(a), Item::Hf(b)) => a.id == b.id,
            _ => false,
        };
        match trial.iter().position(same) {
            Some(i) => trial[i] = it.clone(),
            None => trial.push(it.clone()),
        }
        match self.build(&trial) {
            Ok(b) => {
                self.push_act(Act::Set(it));
                self.built = Some(Arc::new(b));
                Ok(DocRes::Ok { id, changed: "all" })
            }
            Err(BErr::BadPages(reason)) => Ok(DocRes::BadPages { reason }),
            Err(BErr::TooLong) => Ok(DocRes::TooLong),
            Err(BErr::Unsupported(chars)) | Err(BErr::Missing(chars)) => Ok(DocRes::UnsupportedChars { chars }),
            Err(BErr::Other(s)) => Err(s),
            Err(_) => Err("cannot apply the change".into()),
        }
    }

    fn apply_wm(&mut self, req: &WmReq) -> Result<DocRes, String> {
        let image = match req.kind.as_str() {
            "text" => false,
            "image" => true,
            _ => return Err("bad kind".into()),
        };
        if !image && req.text.as_deref().is_none_or(|t| t.trim().is_empty()) {
            return Ok(DocRes::BadText);
        }
        let asset = req.asset.clone().unwrap_or_default();
        if image {
            self.ensure_asset(&asset)?;
        }
        let style = req.style.clone().unwrap_or(Fmt { font: "Arial".into(), size: 0.0, color: "#808080".into(), bold: false, italic: false, align: Align::Center });
        let id = self.doc_id(req.id);
        let w = WmE {
            id,
            image,
            text: req.text.clone().unwrap_or_default(),
            style,
            asset,
            scale: req.scale.unwrap_or(50.0).clamp(10.0, 100.0),
            opacity: req.opacity.clamp(0.05, 1.0),
            angle: req.angle.clamp(-90.0, 90.0),
            front: req.layer == "front",
            pages: req.pages.trim().to_string(),
        };
        self.apply_doc(Item::Wm(w), id)
    }

    fn apply_hf(&mut self, req: &HfReq) -> Result<DocRes, String> {
        if !["tl", "tc", "tr", "bl", "bc", "br"].contains(&req.pos.as_str()) {
            return Err("bad position".into());
        }
        let id = self.doc_id(req.id);
        let h = HfE {
            id,
            pos: req.pos.clone(),
            template: req.template.clone(),
            style: Fmt { color: if req.style.color.is_empty() { "#000000".into() } else { req.style.color.clone() }, ..req.style.clone() },
            margin_pt: req.margin_pt.clamp(0.0, 300.0),
            start: req.start,
            skip_first: req.skip_first,
            pages: req.pages.trim().to_string(),
            lang: req.lang.clone(),
            tz_min: req.tz_min,
        };
        self.apply_doc(Item::Hf(h), id)
    }

    fn doc_objs(&mut self) -> DocObjs {
        let (mut watermarks, mut header_footers) = (vec![], vec![]);
        for it in self.effective() {
            match it {
                Item::Wm(w) => watermarks.push(WmReq {
                    id: Some(w.id),
                    kind: if w.image { "image" } else { "text" }.into(),
                    text: (!w.image).then(|| w.text.clone()),
                    style: Some(w.style.clone()),
                    asset: w.image.then(|| w.asset.clone()),
                    scale: w.image.then_some(w.scale),
                    opacity: w.opacity,
                    angle: w.angle,
                    layer: if w.front { "front" } else { "behind" }.into(),
                    pages: w.pages.clone(),
                }),
                Item::Hf(h) => header_footers.push(HfReq { id: Some(h.id), pos: h.pos, template: h.template, style: h.style, margin_pt: h.margin_pt, start: h.start, skip_first: h.skip_first, pages: h.pages, lang: h.lang, tz_min: h.tz_min }),
                _ => {}
            }
        }
        DocObjs { watermarks, header_footers }
    }

    fn del_doc(&mut self, id: u32) -> Result<(), String> {
        if !self.effective().iter().any(|x| matches!(x, Item::Wm(w) if w.id == id) || matches!(x, Item::Hf(h) if h.id == id)) {
            return Err("no such object".into());
        }
        self.push_act(Act::Del(u32::MAX, id));
        self.built = None;
        self.built()?;
        Ok(())
    }

    fn step(&mut self, back: bool) -> Result<EditHistory, String> {
        let touched: Vec<u32> = self.effective().iter().map(Item::page).collect();
        let at = if back { self.cursor.checked_sub(1) } else { (self.cursor < self.hist.len()).then_some(self.cursor) };
        let Some(at) = at else { return Ok(self.history(vec![])) };
        let pages = act_pages(&self.hist[at]);
        self.cursor = if back { at } else { at + 1 };
        self.built = None;
        self.built()?;
        Ok(self.history(touched.into_iter().chain(pages).collect()))
    }

    fn render(&mut self, page: u32, width_px: u32) -> Result<String, String> {
        let b = self.built()?;
        let img = render_bytes(&b.bytes, page, width_px.clamp(200, 2400))?;
        let mut buf = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 85).encode_image(&image::DynamicImage::ImageRgba8(img).to_rgb8()).map_err(|e| e.to_string())?;
        Ok(format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(buf)))
    }
}

fn act_pages(a: &Act) -> Vec<u32> {
    match a {
        Act::Set(it) => vec![it.page()],
        Act::Del(p, _) => vec![*p],
        Act::Batch(v) => v.iter().flat_map(act_pages).collect(),
    }
}

/// Applies one history entry to the effective list: an item replaces its predecessor in place (z-order), a new one goes on top.
fn replay(v: &mut Vec<Item>, a: &Act) {
    match a {
        Act::Set(it) => {
            let same = |x: &Item| match (x, it) {
                (Item::Para(p), Item::Para(q)) => p.page == q.page && p.para == q.para,
                (Item::Obj(p), Item::Obj(q)) => p.id == q.id,
                (Item::Mv(p), Item::Mv(q)) => p.page == q.page && p.kind == q.kind && p.id == q.id,
                (Item::Wm(p), Item::Wm(q)) => p.id == q.id,
                (Item::Hf(p), Item::Hf(q)) => p.id == q.id,
                _ => false,
            };
            match v.iter().position(same) {
                Some(i) => v[i] = it.clone(),
                None => v.push(it.clone()),
            }
        }
        Act::Del(_, id) => v.retain(|x| match x {
            Item::Obj(o) => o.id != *id,
            Item::Wm(w) => w.id != *id,
            Item::Hf(h) => h.id != *id,
            _ => true,
        }),
        Act::Batch(b) => b.iter().for_each(|a| replay(v, a)),
    }
}

/// Analysis-frame (y up) box -> displayed page, [x, y, w, h] with a top-left origin.
fn disp_of_work(pa: &PageAn, b: [f64; 4]) -> [f64; 4] {
    let t = pa.to_disp;
    let pts = [(b[0], b[1]), (b[2], b[1]), (b[2], b[3]), (b[0], b[3])].map(|(x, y)| (x * t[0] + y * t[2] + t[4], x * t[1] + y * t[3] + t[5]));
    let (x0, x1) = (pts.iter().map(|p| p.0).fold(f64::MAX, f64::min), pts.iter().map(|p| p.0).fold(f64::MIN, f64::max));
    let (y0, y1) = (pts.iter().map(|p| p.1).fold(f64::MAX, f64::min), pts.iter().map(|p| p.1).fold(f64::MIN, f64::max));
    [x0, pa.disp[1] - y1, x1 - x0, y1 - y0]
}

/// A move on the displayed page (pt, y down) as a move in the analysis frame (y up).
fn disp_delta_to_work(pa: &PageAn, dx: f64, dy: f64) -> (f64, f64) {
    let w = inv_m(pa.to_disp);
    (dx * w[0] - dy * w[2], dx * w[1] - dy * w[3])
}

/// Line breaks normalised, runs of blanks collapsed to one space (hard breaks stay).
fn collapse_spaces(t: &str) -> String {
    let mut out = String::new();
    for c in t.replace("\r\n", "\n").chars() {
        if (c == ' ' || c == '\t') && out.ends_with(' ') {
            continue;
        }
        out.push(if c == '\t' { ' ' } else { c });
    }
    out
}

/// Alignment belongs to the whole target: every run takes the first run's.
fn same_align(mut v: Vec<Run>) -> Vec<Run> {
    if let Some(a) = v.first().map(|r| r.style.align) {
        v.iter_mut().for_each(|r| r.style.align = a);
    }
    v
}

/// Runs with adjacent equal formats merged.
fn merge_runs(v: Vec<Run>) -> Vec<Run> {
    let mut out: Vec<Run> = vec![];
    for r in v {
        match out.last_mut() {
            Some(l) if l.style == r.style => l.text.push_str(&r.text),
            _ => out.push(r),
        }
    }
    out
}

/// Character sequence of runs with white space collapsed, for "is this still the original?" comparisons.
fn canon(runs: &[Run]) -> Vec<(char, Fmt)> {
    let mut out: Vec<(char, Fmt)> = vec![];
    for r in runs {
        let mut f = r.style.clone();
        f.color = f.color.to_lowercase();
        for c in r.text.chars() {
            if c.is_whitespace() {
                if out.last().is_some_and(|l| l.0 != ' ') && !out.is_empty() {
                    out.push((' ', f.clone()));
                }
            } else {
                out.push((c, f.clone()));
            }
        }
    }
    if out.last().is_some_and(|l| l.0 == ' ') {
        out.pop();
    }
    out
}

/// The format of a layout style: orig font resources are "orig", PC faces their family.
fn fmt_of_style(fonts: &BTreeMap<String, Fnt>, pcs: Option<&PcSet>, align: Align, st: &layout::Style) -> Fmt {
    let (font, bold, italic) = match st.font.strip_prefix("MjF").and_then(|n| n.parse::<usize>().ok()) {
        Some(i) => pcs.and_then(|p| p.faces.get(i.wrapping_sub(1))).map(|(k, _)| (layout::pc_family_name(&k.0), k.1, k.2)).unwrap_or(("Arial".into(), false, false)),
        None => {
            let f = fonts.get(&st.font);
            ("orig".into(), f.is_some_and(|f| f.bold), f.is_some_and(|f| f.italic))
        }
    };
    Fmt { font, size: (st.size * 100.0).round() / 100.0, color: content::fill_css(&st.fill), bold, italic, align }
}

/// The paragraph's own content as runs.
fn base_runs(pa: &PageAn, p: &Para) -> Vec<Run> {
    match &p.ed {
        Some(ed) => layout::runs_from_words(&ed.words, &ed.styles, &|st| fmt_of_style(&pa.fonts, None, p.align, st)),
        None => vec![Run { text: p.text.clone(), style: Fmt { font: build::base_font(p), size: p.size, color: p.color.clone(), bold: p.bold, italic: p.italic, align: p.align } }],
    }
}

/// A font equal to the CSS family of a directly editable paragraph's own font means "orig".
fn norm_fmt(p: &Para, f: &Fmt) -> Fmt {
    let mut f = f.clone();
    if !p.pc_font && f.font.eq_ignore_ascii_case(&p.css_font) {
        f.font = "orig".into();
    }
    f
}

/// Style of a new text box: the page's dominant run (Arial 11 black on an empty page).
fn default_fmt(pa: &PageAn) -> Fmt {
    let fams = fonts::available_families();
    match pa.dominant() {
        Some(d) => Fmt { font: if fams.contains(&d.css) { d.css } else { "Arial".into() }, size: d.size.clamp(6.0, 72.0), color: content::fill_css(&d.fill), bold: false, italic: false, align: Align::Left },
        None => Fmt { font: "Arial".into(), size: 11.0, color: "#000000".into(), bold: false, italic: false, align: Align::Left },
    }
}

fn mul_m(a: content::M, b: content::M) -> content::M {
    content::mul(a, b)
}

fn inv_m(a: content::M) -> content::M {
    content::inv(a).unwrap_or(content::ID)
}

fn render_bytes(bytes: &[u8], page: u32, width_px: u32) -> Result<image::RgbaImage, String> {
    let p = pdf::pdfium()?;
    let d = p.load_pdf_from_byte_vec(bytes.to_vec(), None).map_err(|e| e.to_string())?;
    let pg = d.pages().get(page as PdfPageIndex).map_err(|e| e.to_string())?;
    let img = pg.render_with_config(&PdfRenderConfig::new().set_target_width(width_px as i32)).map_err(|e| e.to_string())?.as_image().map_err(|e| e.to_string())?.to_rgba8();
    Ok(img)
}

// ---------------- save ----------------

const GS_FLAGS: [&str; 9] = [
    "-sDEVICE=pdfwrite",
    "-dPassThroughJPEGImages=true",
    "-dDownsampleColorImages=false",
    "-dDownsampleGrayImages=false",
    "-dDownsampleMonoImages=false",
    "-dAutoFilterColorImages=false",
    "-dAutoFilterGrayImages=false",
    "-dCompatibilityLevel=1.6",
    "-dNOPAUSE",
];

/// Whole-page 72 DPI comparison of two files; true when every page differs in at most 0.2 % of its pixels.
fn same_render(a: &Path, b: &Path) -> bool {
    let Ok(p) = pdf::pdfium() else { return false };
    let (Ok(da), Ok(db)) = (p.load_pdf_from_file(a, None), p.load_pdf_from_file(b, None)) else { return false };
    if da.pages().len() != db.pages().len() {
        return false;
    }
    let cfg = PdfRenderConfig::new().scale_page_by_factor(1.0);
    for i in 0..da.pages().len() {
        let (Ok(x), Ok(y)) = (da.pages().get(i), db.pages().get(i)) else { return false };
        let (Ok(x), Ok(y)) = (x.render_with_config(&cfg).and_then(|r| r.as_image()), y.render_with_config(&cfg).and_then(|r| r.as_image())) else { return false };
        let (x, y) = (x.to_rgba8(), y.to_rgba8());
        if x.dimensions() != y.dimensions() {
            return false;
        }
        let diff = x.pixels().zip(y.pixels()).filter(|(p, q)| (0..3).any(|c| (p[c] as i32 - q[c] as i32).abs() > 16)).count();
        if diff as f64 > 0.002 * (x.width() * x.height()) as f64 {
            return false;
        }
    }
    true
}

/// All text of one page of a PDF, by pdfium (empty when it can't be read).
fn text_of_page(bytes: &[u8], pg: u32) -> String {
    let Ok(p) = pdf::pdfium() else { return String::new() };
    let Ok(d) = p.load_pdf_from_byte_vec(bytes.to_vec(), None) else { return String::new() };
    d.pages().get(pg as PdfPageIndex).ok().and_then(|pg| pg.text().ok().map(|t| t.all())).unwrap_or_default()
}

fn chars_of(s: &str) -> HashMap<char, usize> {
    let mut m = HashMap::new();
    for c in s.chars().filter(|c| !c.is_whitespace()) {
        *m.entry(c).or_default() += 1;
    }
    m
}

/// Reopens the output: same page count, and every character of the new paragraphs is on its page.
/// A white-out to check on the output: page, display rect (top-left origin), user space -> display (y up) matrix, display height.
type WoCheck = (u32, [f64; 4], content::M, f64);

/// A removed paragraph to look for in the output: page, its text without blanks, how often that text occurs on the original page.
type GoneCheck = (u32, String, usize);

fn verify(out: &Path, pages: usize, edits: &[(u32, String)], wos: &[WoCheck], gone: &[GoneCheck]) -> Result<(), String> {
    let p = pdf::pdfium()?;
    let d = p.load_pdf_from_file(out, None).map_err(|_| "verify".to_string())?;
    if d.pages().len() as usize != pages {
        return Err("verify".into());
    }
    for pg in edits.iter().map(|e| e.0).collect::<HashSet<_>>() {
        let page = d.pages().get(pg as PdfPageIndex).map_err(|_| "verify".to_string())?;
        let text = page.text().map(|t| t.all()).map_err(|_| "verify".to_string())?;
        let have = chars_of(&text);
        let mut want: HashMap<char, usize> = HashMap::new();
        for (_, t) in edits.iter().filter(|e| e.0 == pg) {
            for (c, n) in chars_of(t) {
                *want.entry(c).or_default() += n;
            }
        }
        if want.iter().any(|(c, n)| have.get(c).copied().unwrap_or(0) < *n) {
            return Err("verify".into());
        }
    }
    // a removed paragraph must occur less often than it did
    for pg in gone.iter().map(|g| g.0).collect::<HashSet<_>>() {
        let page = d.pages().get(pg as PdfPageIndex).map_err(|_| "verify".to_string())?;
        let text: String = page.text().map(|t| t.all()).map_err(|_| "verify".to_string())?.chars().filter(|c| !c.is_whitespace()).collect();
        if gone.iter().any(|g| g.0 == pg && text.matches(g.1.as_str()).count() >= g.2) {
            return Err("verify".into());
        }
    }
    // no glyph centre may lie inside a white-out (1 pt inset: ink and advance boxes differ at the edges)
    for pg in wos.iter().map(|w| w.0).collect::<HashSet<_>>() {
        let page = d.pages().get(pg as PdfPageIndex).map_err(|_| "verify".to_string())?;
        let text = page.text().map_err(|_| "verify".to_string())?;
        for ch in text.chars().iter() {
            if ch.unicode_char().is_none_or(|c| c.is_whitespace()) {
                continue;
            }
            let Ok(b) = ch.tight_bounds() else { continue };
            let (cx, cy) = (((b.left().value + b.right().value) / 2.0) as f64, ((b.bottom().value + b.top().value) / 2.0) as f64);
            for (_, r, m, dh) in wos.iter().filter(|w| w.0 == pg) {
                let (x, y) = (cx * m[0] + cy * m[2] + m[4], dh - (cx * m[1] + cy * m[3] + m[5]));
                if x > r[0] + 1.0 && x < r[0] + r[2] - 1.0 && y > r[1] + 1.0 && y < r[1] + r[3] - 1.0 {
                    return Err("verify".into());
                }
            }
        }
    }
    Ok(())
}

/// The output with a fresh document id: ID[0] stays the input's (permanent id), ID[1] = 16 bytes of sha256(old ID[1], output hash, time).
/// Viewers like Acrobat key their caches (extracted text) on the id, so an edited file must not look like the original.
fn renew_id(bytes: &[u8], input: &Document) -> Result<Vec<u8>, String> {
    use sha2::{Digest, Sha256};
    let ids: Vec<Vec<u8>> = input.trailer.get(b"ID").ok().and_then(|o| o.as_array().ok()).map(|a| a.iter().filter_map(|o| if let Object::String(s, _) = o { Some(s.clone()) } else { None }).collect()).unwrap_or_default();
    let hash = Sha256::digest(bytes);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let mut h = Sha256::new();
    h.update(ids.get(1).cloned().unwrap_or_default());
    h.update(hash);
    h.update(nanos.to_le_bytes());
    let id1 = h.finalize()[..16].to_vec();
    let id0 = ids.first().cloned().unwrap_or_else(|| Sha256::digest([&hash[..], b"id0"].concat())[..16].to_vec());
    let mut doc = Document::load_mem(bytes).map_err(|e| e.to_string())?;
    doc.trailer.set("ID", Object::Array(vec![Object::String(id0, lopdf::StringFormat::Hexadecimal), Object::String(id1, lopdf::StringFormat::Hexadecimal)]));
    let mut out = Vec::new();
    doc.save_to(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

impl Session {
    fn save(&mut self, out_dir: &Path) -> Result<EditSaveRes, String> {
        let start = std::time::Instant::now();
        let edits = self.effective();
        if edits.is_empty() {
            return Err("nothing_to_save".into());
        }
        let b = self.built()?;
        // what the output must contain / must not contain; a later item that overlaps a text or a white-out takes it out of the check
        let mut words: Vec<(u32, String)> = vec![];
        let mut wo_checks: Vec<WoCheck> = vec![];
        let boxes: Vec<(Option<[f64; 4]>, Option<String>)> = {
            let mut v = vec![];
            for it in &edits {
                match it {
                    Item::Para(e) => {
                        let m = self.model(e.page)?;
                        v.push((m.paras.get(e.para as usize).map(|p| p.bbox), Some(e.text.clone())));
                    }
                    Item::Obj(o) if o.kind == ObjKind::TextBox => v.push((b.info.get(&o.page).and_then(|i| i.objs.get(&o.id)).copied(), Some(o.text.clone()))),
                    Item::Obj(o) => v.push((Some(o.rect), None)),
                    _ => v.push((None, None)),
                }
            }
            v
        };
        // paragraphs taken out with the Select tool
        let removed: HashSet<(u32, u32)> = edits.iter().filter_map(|x| if let Item::Mv(m) = x { (m.kind == IKind::Para && m.removed).then_some((m.page, m.id)) } else { None }).collect();
        let mut gone: Vec<GoneCheck> = vec![];
        for &(pg, id) in &removed {
            let pa = self.page(pg)?;
            let want: String = pa.paras.get(id as usize).map(|p| p.text.chars().filter(|c| !c.is_whitespace()).collect()).unwrap_or_default();
            let have: String = text_of_page(&self.orig, pg).chars().filter(|c| !c.is_whitespace()).collect();
            let n = if want.chars().count() >= 8 { have.matches(want.as_str()).count() } else { 0 };
            if n > 0 {
                gone.push((pg, want, n));
            }
        }
        let hit = |a: [f64; 4], c: [f64; 4]| a[0] < c[0] + c[2] && c[0] < a[0] + a[2] && a[1] < c[1] + c[3] && c[1] < a[1] + a[3];
        for (i, it) in edits.iter().enumerate() {
            if matches!(it, Item::Para(e) if removed.contains(&(e.page, e.para))) {
                continue;
            }
            let later_text = |r: [f64; 4]| edits.iter().enumerate().skip(i + 1).any(|(j, x)| x.page() == it.page() && boxes[j].1.is_some() && boxes[j].0.is_none_or(|bb| hit(bb, r)));
            let later_wo = |r: [f64; 4]| edits.iter().enumerate().skip(i + 1).any(|(j, x)| matches!(x, Item::Obj(o) if o.kind == ObjKind::WhiteOut) && x.page() == it.page() && boxes[j].0.is_some_and(|bb| hit(bb, r)));
            match it {
                Item::Obj(o) if o.kind == ObjKind::WhiteOut => {
                    if !o.cover_only && !later_text(o.rect) {
                        let pa = self.page(o.page)?;
                        wo_checks.push((o.page, o.rect, mul_m(inv_m(pa.rinv), pa.to_disp), pa.disp[1]));
                    }
                }
                Item::Obj(o) if o.kind == ObjKind::Image => {}
                _ => {
                    if let (Some(bb), Some(t)) = (boxes[i].0, boxes[i].1.clone()) {
                        if !later_wo(bb) {
                            words.push((it.page(), t));
                        }
                    }
                }
            }
        }
        let work = temp_root().join(stamp());
        std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
        let r = (|| {
            let (raw, tmp) = (work.join("raw.pdf"), work.join("out.pdf"));
            std::fs::write(&raw, &b.bytes).map_err(|e| e.to_string())?;
            let mut chosen = raw.clone();
            if b.pc_used {
                // subset the embedded PC fonts; keep the result only when it renders the same
                let gs = work.join("gs.pdf");
                let mut args: Vec<String> = GS_FLAGS.map(String::from).into();
                args.push("-dBATCH".into());
                args.push(format!("-sOutputFile={}", gs.display()));
                args.push(raw.display().to_string());
                match run_gs(&args, |_| {}) {
                    Ok(()) if size_of(&gs) > 0 && same_render(&raw, &gs) => chosen = gs,
                    Err(GsErr::Cancelled) => return Err("cancelled".to_string()),
                    _ => {}
                }
            }
            let pages = self.sizes.len();
            verify(&chosen, pages, &words, &wo_checks, &gone)?;
            let out = renew_id(&std::fs::read(&chosen).map_err(|e| e.to_string())?, &self.doc)?;
            std::fs::write(&tmp, out).map_err(|e| e.to_string())?;
            let stem = self.path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "output".into());
            let dest = place(&tmp, out_dir, &stem, "_edited", "pdf")?;
            Ok(EditSaveRes { output: dest.display().to_string(), size: size_of(&dest), seconds: start.elapsed().as_secs_f64() })
        })();
        let _ = std::fs::remove_dir_all(&work);
        r
    }
}

// ---------------- commands ----------------

#[tauri::command]
pub async fn edit_open(req: EditOpenReq) -> Result<EditDoc, String> {
    drop(SESSION.lock().unwrap_or_else(|e| e.into_inner()).take()); // replaces any earlier session and frees its job slot
    let guard = crate::job::begin()?;
    tauri::async_runtime::spawn_blocking(move || {
        let pw = req.password.as_deref().filter(|p| !p.is_empty());
        let (s, d) = Session::open(Path::new(&req.path), pw, Some(guard))?;
        *SESSION.lock().unwrap_or_else(|e| e.into_inner()) = Some(s);
        Ok(d)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn edit_page(page: u32) -> Result<PageModel, String> {
    tauri::async_runtime::spawn_blocking(move || with_session(|s| s.model(page))).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn edit_render(page: u32, width_px: u32) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || with_session(|s| s.render(page, width_px))).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn edit_apply(req: EditApplyReq) -> Result<EditApplyRes, String> {
    tauri::async_runtime::spawn_blocking(move || with_session(|s| s.apply(&req))).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn edit_apply_batch(req: EditBatchReq) -> Result<BatchRes, String> {
    tauri::async_runtime::spawn_blocking(move || with_session(|s| s.apply_batch(&req))).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn edit_watermark(req: WmReq) -> Result<DocRes, String> {
    tauri::async_runtime::spawn_blocking(move || with_session(|s| s.apply_wm(&req))).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn edit_header_footer(req: HfReq) -> Result<DocRes, String> {
    tauri::async_runtime::spawn_blocking(move || with_session(|s| s.apply_hf(&req))).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn edit_doc_objs() -> Result<DocObjs, String> {
    tauri::async_runtime::spawn_blocking(move || with_session(|s| Ok(s.doc_objs()))).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn edit_delete_doc_obj(id: u32) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || with_session(|s| s.del_doc(id).map(|_| serde_json::json!({ "kind": "ok" })))).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn edit_apply_obj(req: ObjReq) -> Result<ObjRes, String> {
    tauri::async_runtime::spawn_blocking(move || with_session(|s| s.apply_obj(&req))).await.map_err(|e| e.to_string())?
}

/// PC font families installed on this machine.
#[tauri::command]
pub fn edit_fonts() -> Vec<String> {
    fonts::available_families()
}

#[tauri::command]
pub async fn edit_undo() -> Result<EditHistory, String> {
    tauri::async_runtime::spawn_blocking(move || with_session(|s| s.step(true))).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn edit_redo() -> Result<EditHistory, String> {
    tauri::async_runtime::spawn_blocking(move || with_session(|s| s.step(false))).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn edit_history() -> Result<EditHistory, String> {
    tauri::async_runtime::spawn_blocking(move || with_session(|s| Ok(s.history(vec![])))).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn edit_save(app: tauri::AppHandle, req: EditSaveReq) -> Result<EditSaveRes, String> {
    tauri::async_runtime::spawn_blocking(move || {
        with_session(|s| {
            let dir = if req.out_mode == "folder" { compress::folder_dir(&app) } else { s.path.parent().map(Path::to_path_buf).unwrap_or_default() };
            s.save(&dir)
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// No-op when there is no session.
#[tauri::command]
pub fn edit_close() {
    drop(SESSION.lock().unwrap_or_else(|e| e.into_inner()).take());
}

#[cfg(test)]
mod tests;
