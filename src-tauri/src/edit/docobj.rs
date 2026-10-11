//! Edit 3 document-wide objects: watermark, header & footer. Contract: _docs/edit3-contract.md.
//! They are drawn by extra content streams per page (behind = first in /Contents, front = last), never by touching the page's own streams.
use super::build::{fill_of, Acc, Shared};
use super::content::{esc, fmt, inv, mul, tr, M};
use super::fonts::{embed_pc, win_ansi};
use super::whiteout::png_xobject;
use super::*;

/// Page geometry without analysing the page: displayed size and display (y up) -> user space.
pub(super) struct Geom {
    pub disp: [f64; 2],
    pub d2u: M,
}

pub(super) fn geom(doc: &Document, pid: ObjectId) -> Geom {
    let raw = page_box(doc, pid);
    let rot = inherited(doc, pid, b"Rotate").map(|o| num(o) as i64).unwrap_or(0).rem_euclid(360);
    let rot = if rot % 90 == 0 { rot } else { 0 };
    let disp = if rot == 90 || rot == 270 { [raw[3], raw[2]] } else { [raw[2], raw[3]] };
    Geom { disp, d2u: inv(display_matrix(raw, rot)).unwrap_or(content::ID) }
}

/// Pages ("" = all) as 0-based indices. Same syntax and rules as `parseRanges` in src/lib/split.ts: "1-3, 5", 1-based, inclusive,
/// blanks around tokens allowed, no empty token, no reversed range, every page inside 1..=pages.
/// The error is a key for the UI: `rangeErrFormat:<token>` or `rangeErr:<page>:<pages>`.
pub(super) fn parse_pages(text: &str, pages: u32) -> Result<BTreeSet<u32>, String> {
    if text.trim().is_empty() {
        return Ok((0..pages).collect());
    }
    let mut out = BTreeSet::new();
    for raw in text.split(',') {
        let tok = raw.trim();
        let bad = || format!("rangeErrFormat:{tok}");
        let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        let (a, b) = match tok.split_once('-') {
            Some((a, b)) if digits(a) && digits(b) => (a, Some(b)),
            None if digits(tok) => (tok, None),
            _ => return Err(bad()),
        };
        let from = a.parse::<u64>().unwrap_or(u64::MAX);
        let to = b.map(|b| b.parse::<u64>().unwrap_or(u64::MAX)).unwrap_or(from);
        if to < from {
            return Err(bad());
        }
        for p in [from, to] {
            if p < 1 || p > pages as u64 {
                return Err(format!("rangeErr:{p}:{pages}"));
            }
        }
        out.extend((from..=to).map(|p| p as u32 - 1));
    }
    Ok(out)
}

const MONTHS_ID: [&str; 12] = ["Januari", "Februari", "Maret", "April", "Mei", "Juni", "Juli", "Agustus", "September", "Oktober", "November", "Desember"];
const MONTHS_EN: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

/// Day number since 1970-01-01 -> (year, month 1-12, day).
fn civil(days: i64) -> (i64, usize, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + (m <= 2) as i64, m as usize, d)
}

/// "10 Oktober 2026" / "10 October 2026" for a day number.
pub(super) fn date_text(days: i64, lang: &str) -> String {
    let (y, m, d) = civil(days);
    format!("{d} {} {y}", if lang == "id" { MONTHS_ID[m - 1] } else { MONTHS_EN[m - 1] })
}

/// Today in the user's local time; `tz_min` = minutes east of UTC, sent by the frontend (no tz lookup in Rust).
pub(super) fn today(lang: &str, tz_min: i32) -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0) + tz_min as i64 * 60;
    date_text(secs.div_euclid(86_400), lang)
}

pub(super) struct WmPlan {
    pub w: WmE,
    pub pages: BTreeSet<u32>,
}

pub(super) struct HfPlan {
    pub h: HfE,
    pub pages: Vec<u32>, // included pages, ascending
    pub style: Fmt,      // resolved: concrete family and size
    pub total: i64,
    pub date: String,    // {date}: follows the UI language
    pub date_id: String, // {date_id}
    pub date_en: String, // {date_en}
    pub file: String,
}

/// What one page gets from the document-wide objects.
#[derive(Default)]
pub(super) struct DocAdd {
    pub behind: Vec<Vec<u8>>,
    pub front: Vec<Vec<u8>>,
    pub fonts: Vec<(String, ObjectId)>,
    pub xs: Vec<(String, ObjectId)>,
    pub gss: Vec<(String, ObjectId)>,
}

fn uniq(s: &str) -> String {
    s.chars().collect::<BTreeSet<_>>().into_iter().collect()
}

/// The embedded PC face for a family: its resource name and widths (1/1000 em per WinAnsi code).
fn face(doc: &mut Document, acc: &mut Acc, fam: &str, bold: bool, italic: bool, add: &mut DocAdd) -> Result<(String, [f64; 256]), BErr> {
    let st = acc.pcs.style_fam(fam, bold, italic, 1.0, "0 g").map_err(fail)?;
    let fi = st.font.trim_start_matches("MjF").parse::<usize>().unwrap_or(1) - 1;
    let id = *acc.pc_objs.entry(fi).or_insert_with(|| embed_pc(doc, &acc.pcs.faces[fi].1));
    if !add.fonts.iter().any(|f| f.0 == st.font) {
        add.fonts.push((st.font.clone(), id));
    }
    Ok((st.font, acc.pcs.faces[fi].1.widths))
}

/// Ascent of a PC face in em (what a baseline hangs below the top margin).
fn ascent(acc: &Acc, font: &str) -> f64 {
    let fi = font.trim_start_matches("MjF").parse::<usize>().unwrap_or(1) - 1;
    let pc = &acc.pcs.faces[fi].1;
    ttf_parser::Face::parse(&pc.bytes, 0).map(|f| f.ascender() as f64 / f.units_per_em() as f64).unwrap_or(0.9)
}

/// WinAnsi codes and the advance in em of a text; characters the face can't show are reported.
fn codes(widths: &[f64; 256], text: &str) -> Result<(Vec<u8>, f64), BErr> {
    let (mut out, mut w, mut bad) = (vec![], 0.0, String::new());
    for c in text.chars() {
        match win_ansi(c).filter(|&k| widths[k as usize] > 0.0) {
            Some(k) => {
                out.push(k);
                w += widths[k as usize] / 1000.0;
            }
            None => bad.push(c),
        }
    }
    if bad.is_empty() {
        Ok((out, w))
    } else {
        Err(BErr::Unsupported(uniq(&bad)))
    }
}

/// One shared ExtGState object per opacity value.
fn gs_for(doc: &mut Document, acc: &mut Acc, add: &mut DocAdd, opacity: f64) -> String {
    let key = (opacity * 1000.0).round() as i64;
    let (name, id) = match acc.gs.get(&key) {
        Some(v) => v.clone(),
        None => {
            let name = format!("MjGs{}", acc.gs.len() + 1);
            let o = Object::Real((key as f64 / 1000.0) as f32);
            let mut d = Dictionary::new();
            d.set("Type", "ExtGState");
            d.set("ca", o.clone());
            d.set("CA", o);
            let id = doc.add_object(d);
            acc.gs.insert(key, (name.clone(), id));
            (name, id)
        }
    };
    if !add.gss.iter().any(|g| g.0 == name) {
        add.gss.push((name.clone(), id));
    }
    name
}

fn mtx(m: M) -> String {
    format!("{} {} {} {} {} {}", fmt(m[0]), fmt(m[1]), fmt(m[2]), fmt(m[3]), fmt(m[4]), fmt(m[5]))
}

/// Shrink factor (<= 1) so a w x h box rotated by `angle` (radians) fits inside the page with a 4 % margin.
fn fit(w: f64, h: f64, angle: f64, disp: [f64; 2]) -> f64 {
    let (c, s) = (angle.cos().abs(), angle.sin().abs());
    let (bw, bh) = (w * c + h * s, w * s + h * c);
    (0.96 * disp[0] / bw.max(1e-6)).min(0.96 * disp[1] / bh.max(1e-6)).min(1.0)
}

fn wm_bytes(doc: &mut Document, acc: &mut Acc, sh: &Shared, g: &Geom, w: &WmE, add: &mut DocAdd) -> Result<Option<Vec<u8>>, BErr> {
    let a = w.angle.to_radians();
    let (c, s) = (a.cos(), a.sin());
    let centre = tr(g.disp[0] / 2.0, g.disp[1] / 2.0);
    if !w.image && w.text.trim().is_empty() {
        return Ok(None);
    }
    let gs = gs_for(doc, acc, add, w.opacity);
    let mut b = format!("q\n/{gs} gs\n0 Tc 0 Tw 100 Tz 0 Ts 0 Tr\n").into_bytes();
    if w.image {
        let png = sh.assets.get(&w.asset).ok_or_else(|| BErr::Other("no such asset".into()))?;
        if !acc.asset_objs.contains_key(&w.asset) {
            let n = acc.asset_objs.len() + 1;
            let id = png_xobject(doc, png).map_err(BErr::Other)?;
            acc.asset_objs.insert(w.asset.clone(), (format!("MjSg{n}"), id));
        }
        let (name, id) = acc.asset_objs[&w.asset].clone();
        if !add.xs.iter().any(|x| x.0 == name) {
            add.xs.push((name.clone(), id));
        }
        let (iw, ih) = match acc.dims.get(&w.asset) {
            Some(d) => *d,
            None => {
                let d = image::ImageReader::new(std::io::Cursor::new(png.as_slice())).with_guessed_format().ok().and_then(|r| r.into_dimensions().ok()).ok_or_else(|| BErr::Other("bad image".into()))?;
                acc.dims.insert(w.asset.clone(), d);
                d
            }
        };
        let wd0 = w.scale / 100.0 * g.disp[0];
        let hd0 = wd0 * ih as f64 / iw.max(1) as f64;
        let k = fit(wd0, hd0, a, g.disp);
        let (wd, hd) = (wd0 * k, hd0 * k);
        let cm = mul(mul(mul([wd, 0.0, 0.0, hd, -wd / 2.0, -hd / 2.0], [c, s, -s, c, 0.0, 0.0]), centre), g.d2u);
        b.extend(format!("{} cm\n/{name} Do\nQ\n", mtx(cm)).into_bytes());
        return Ok(Some(b));
    }
    let text: String = w.text.split_whitespace().collect::<Vec<_>>().join(" ");
    let fam = if w.style.font == "orig" || w.style.font.is_empty() { "Arial" } else { w.style.font.as_str() };
    let (font, widths) = face(doc, acc, fam, w.style.bold, w.style.italic, add)?;
    let (cs, em) = codes(&widths, &text)?;
    let mut size = if w.style.size > 0.0 { w.style.size } else { 0.15 * g.disp[0] };
    size *= fit(em * size, size, a, g.disp);
    let tw = em * size;
    // the text box runs from 0.22 em below the baseline to 0.78 em above it; its centre goes to the page centre
    let o = (centre[4] - (tw / 2.0 * c - 0.28 * size * s), centre[5] - (tw / 2.0 * s + 0.28 * size * c));
    let tm = mul([c, s, -s, c, o.0, o.1], g.d2u);
    b.extend(format!("{}\nBT\n/{font} {} Tf\n{} Tm\n{} Tj\nET\nQ\n", fill_of(&w.style.color), fmt(size), mtx(tm), esc(&cs)).into_bytes());
    Ok(Some(b))
}

fn hf_bytes(doc: &mut Document, acc: &mut Acc, g: &Geom, p: &HfPlan, pg: u32, add: &mut DocAdd) -> Result<Option<Vec<u8>>, BErr> {
    let Some(idx) = p.pages.iter().position(|&x| x == pg) else { return Ok(None) };
    if p.h.skip_first && idx == 0 {
        return Ok(None);
    }
    let text = p
        .h
        .template
        .replace("{page}", &(p.h.start as i64 + idx as i64).to_string())
        .replace("{total}", &p.total.to_string())
        .replace("{date_id}", &p.date_id)
        .replace("{date_en}", &p.date_en)
        .replace("{date}", &p.date)
        .replace("{file}", &p.file)
        .replace(['\r', '\n'], " ");
    if text.trim().is_empty() {
        return Ok(None);
    }
    let st = &p.style;
    let (font, widths) = face(doc, acc, &st.font, st.bold, st.italic, add)?;
    let (cs, em) = codes(&widths, &text)?;
    let m = p.h.margin_pt;
    // too wide for the space between the margins: shrink down to 5 pt, then give up
    let avail = (g.disp[0] - 2.0 * m).max(1.0);
    let size = if em * st.size > avail { (avail / em).min(st.size).max(5.0) } else { st.size };
    if em * size > avail + 0.01 {
        return Err(BErr::TooLong);
    }
    let tw = em * size;
    let x = match &p.h.pos[1..] {
        "l" => m,
        "c" => (g.disp[0] - tw) / 2.0,
        _ => g.disp[0] - m - tw,
    };
    let y = if p.h.pos.starts_with('t') { g.disp[1] - m - ascent(acc, &font) * size } else { m };
    let tm = mul(tr(x, y), g.d2u);
    Ok(Some(format!("q\n0 Tc 0 Tw 100 Tz 0 Ts 0 Tr\n{}\nBT\n/{font} {} Tf\n{} Tm\n{} Tj\nET\nQ\n", fill_of(&st.color), fmt(size), mtx(tm), esc(&cs)).into_bytes()))
}

/// All document-wide streams for page `pg`. A watermark "behind" goes in front on a scan (it would be hidden under the image).
#[allow(clippy::too_many_arguments)]
pub(super) fn page_add(doc: &mut Document, acc: &mut Acc, sh: &Shared, g: &Geom, pg: u32, wms: &[WmPlan], hfs: &[HfPlan], scans: &HashMap<u32, bool>) -> Result<DocAdd, BErr> {
    let mut add = DocAdd::default();
    for p in wms.iter().filter(|p| p.pages.contains(&pg)) {
        if let Some(b) = wm_bytes(doc, acc, sh, g, &p.w, &mut add)? {
            if p.w.front || scans.get(&pg).copied().unwrap_or(false) {
                add.front.push(b);
            } else {
                add.behind.push(b);
            }
        }
    }
    for p in hfs {
        if let Some(b) = hf_bytes(doc, acc, g, p, pg, &mut add)? {
            add.front.push(b);
        }
    }
    Ok(add)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_ranges_follow_split_ts() {
        let ok = |s: &str, n: u32| parse_pages(s, n).map(|v| v.into_iter().map(|x| x + 1).collect::<Vec<_>>());
        assert_eq!(ok("", 10), Ok((1..=10).collect()));
        assert_eq!(ok("1-3, 5, 8-10", 10), Ok(vec![1, 2, 3, 5, 8, 9, 10]));
        assert_eq!(ok(" 2 , 4-4 ", 10), Ok(vec![2, 4]));
        assert_eq!(ok("5, 1", 10), Ok(vec![1, 5]));
        assert_eq!(ok("0", 10), Err("rangeErr:0:10".into()));
        assert_eq!(ok("1-11", 10), Err("rangeErr:11:10".into()));
        assert_eq!(ok("3-1", 10), Err("rangeErrFormat:3-1".into()));
        assert_eq!(ok("a", 10), Err("rangeErrFormat:a".into()));
        assert_eq!(ok("1,,2", 10), Err("rangeErrFormat:".into()));
        assert_eq!(ok("1-2-3", 10), Err("rangeErrFormat:1-2-3".into()));
        assert_eq!(ok("-3", 10), Err("rangeErrFormat:-3".into()));
    }

    #[test]
    fn dates() {
        assert_eq!(date_text(20_736, "id"), "10 Oktober 2026");
        assert_eq!(date_text(20_736, "en"), "10 October 2026");
        assert_eq!(date_text(0, "en"), "1 January 1970");
        assert_eq!(date_text(19_782, "id"), "29 Februari 2024");
    }
}
