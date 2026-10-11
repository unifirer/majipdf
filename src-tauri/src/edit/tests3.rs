//! Edit 3 tests (contract _docs/edit3-contract.md, "Tests"). Same rules as tests.rs: samples come from `_samples/` and are
//! skipped when absent; real-name samples print counts and booleans only, inserted words are made up. Synthetic documents
//! cover the document-wide objects without any sample (they skip themselves when pdfium or a PC font is missing).
use super::*;
use lopdf::dictionary;

fn e3_dir() -> PathBuf {
    let d = out_dir().join("e3");
    let _ = std::fs::create_dir_all(&d);
    d
}

fn can_run() -> bool {
    pdf::pdfium().is_ok() && !fonts::available_families().is_empty()
}

/// One entry per page: content stream and /Rotate. Helvetica, Letter.
fn synth(pages: &[(&str, i64)]) -> PathBuf {
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let font = doc.add_object(lopdf::dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding" });
    let mut kids = vec![];
    for (c, rot) in pages {
        let cid = doc.add_object(Stream::new(Dictionary::new(), c.as_bytes().to_vec()));
        let pid = doc.add_object(lopdf::dictionary! {
            "Type" => "Page", "Parent" => pages_id,
            "MediaBox" => vec![Object::Integer(0), Object::Integer(0), Object::Integer(612), Object::Integer(792)],
            "Resources" => lopdf::dictionary! { "Font" => lopdf::dictionary! { "F1" => font } },
            "Contents" => cid, "Rotate" => *rot,
        });
        kids.push(Object::Reference(pid));
    }
    doc.objects.insert(pages_id, Object::Dictionary(lopdf::dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => pages.len() as i64 }));
    let cat = doc.add_object(lopdf::dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", cat);
    let p = std::env::temp_dir().join(format!("majipdf_e3_{}.pdf", stamp()));
    doc.save(&p).unwrap();
    p
}

fn synth_session(pages: &[(&str, i64)]) -> Session {
    let p = synth(pages);
    let s = Session::open(&p, None, None).unwrap().0;
    let _ = std::fs::remove_file(&p);
    s
}

fn flat(t: &str) -> String {
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn wm_req(text: &str, layer: &str, pages: &str, angle: f64) -> WmReq {
    WmReq {
        id: None,
        kind: "text".into(),
        text: Some(text.into()),
        style: Some(Fmt { font: "Arial".into(), size: 0.0, color: "#808080".into(), bold: true, italic: false, align: Align::Center }),
        asset: None,
        scale: None,
        opacity: 0.3,
        angle,
        layer: layer.into(),
        pages: pages.into(),
    }
}

fn hf_req(pos: &str, template: &str) -> HfReq {
    HfReq {
        id: None,
        pos: pos.into(),
        template: template.into(),
        style: Fmt { font: "Arial".into(), size: 10.0, color: "#000000".into(), bold: false, italic: false, align: Align::Center },
        margin_pt: 28.0,
        start: 1,
        skip_first: false,
        pages: String::new(),
        lang: "id".into(),
        tz_min: 0,
    }
}

fn doc_ok(r: DocRes) -> u32 {
    match r {
        DocRes::Ok { id, .. } => id,
        o => panic!("expected ok, got {o:?}"),
    }
}

/// (min x, min y, max x, max y) in pixels of the dark pixels of an image.
fn ink_box(img: &RgbaImage) -> Option<(u32, u32, u32, u32)> {
    let mut b: Option<(u32, u32, u32, u32)> = None;
    for (x, y, p) in img.enumerate_pixels() {
        if p.0[..3].iter().any(|&c| c < 160) {
            b = Some(match b {
                None => (x, y, x, y),
                Some(o) => (o.0.min(x), o.1.min(y), o.2.max(x), o.3.max(y)),
            });
        }
    }
    b
}

fn ink_in(img: &RgbaImage, r: [f64; 4], dpi: f64) -> u64 {
    let k = dpi / 72.0;
    let (x0, y0, x1, y1) = ((r[0] * k) as i64, (r[1] * k) as i64, ((r[0] + r[2]) * k) as i64, ((r[1] + r[3]) * k) as i64);
    img.enumerate_pixels().filter(|(x, y, p)| (*x as i64) >= x0 && (*x as i64) < x1 && (*y as i64) >= y0 && (*y as i64) < y1 && p.0[..3].iter().any(|&c| c < 200)).count() as u64
}

/// Pixels that differ by more than 16 levels outside every rect (display pt, top-left origin) grown by `grow` px, and inside any of them.
fn diff_outside(a: &RgbaImage, b: &RgbaImage, rs: &[[f64; 4]], dpi: f64, grow: i64) -> (u64, u64) {
    let k = dpi / 72.0;
    let boxes: Vec<(i64, i64, i64, i64)> = rs.iter().map(|r| ((r[0] * k) as i64 - grow, (r[1] * k) as i64 - grow, ((r[0] + r[2]) * k) as i64 + grow, ((r[1] + r[3]) * k) as i64 + grow)).collect();
    let (mut out, mut inn) = (0, 0);
    for (x, y, p) in a.enumerate_pixels() {
        let q = b.get_pixel(x, y);
        if (0..3).any(|c| (p[c] as i32 - q[c] as i32).abs() > 16) {
            if boxes.iter().any(|bx| (x as i64) >= bx.0 && (x as i64) <= bx.2 && (y as i64) >= bx.1 && (y as i64) <= bx.3) {
                inn += 1;
            } else {
                out += 1;
            }
        }
    }
    (out, inn)
}

fn mv(page: u32, kind: IKind, id: u32, dx: f64, dy: f64) -> ObjEdit3 {
    ObjEdit3::Move { page, item: ItemRef { kind, id }, dx, dy }
}

fn rm(page: u32, kind: IKind, id: u32) -> ObjEdit3 {
    ObjEdit3::Remove { page, item: ItemRef { kind, id } }
}

fn batch_ok(s: &mut Session, edits: Vec<ObjEdit3>) -> Vec<PageModel> {
    match s.apply_batch(&EditBatchReq { edits }).unwrap() {
        BatchRes::Ok { pages } => pages,
        o => panic!("expected ok, got {o:?}"),
    }
}

fn items_of(m: &PageModel, kind: &str) -> Vec<ItemOut> {
    m.items.iter().filter(|i| i.kind == kind).cloned().collect()
}

fn count_in(text: &str, needle: &str) -> usize {
    flat(text).matches(needle).count()
}

// 1: items per page, stable ids
#[test]
fn e3_items_and_stable_ids() {
    for (prefix, label) in [(RK, "ringkasan"), ("table_BonBarangUmum", "bon"), ("table_systemgenerated-IKPA", "ikpa")] {
        let Some(mut s) = open(prefix) else { continue };
        let m = s.model(0).unwrap();
        let (np, ni, nth) = (items_of(&m, "para").len(), items_of(&m, "image").len(), items_of(&m, "path").len());
        let locked = m.items.iter().filter(|i| i.locked.is_some()).count();
        let again = reanalyse(&s.orig, 0);
        let sig = |p: &PageAn| p.items.iter().map(|i| (i.kind, i.id, i.op, i.bbox.map(|v| (v * 100.0).round() as i64))).collect::<Vec<_>>();
        let first = s.page(0).unwrap();
        println!("{label} p1: items para {np}, image {ni}, path {nth} (locked {locked}); ids stable {}", sig(&first) == sig(&again));
        assert_eq!(sig(&first), sig(&again));
        assert_eq!(np, m.paras.len());
        assert_eq!(ni + nth, first.items.len());
    }
}

/// First movable page-1 image that is not page-sized.
fn find_image(s: &mut Session) -> Option<ItemOut> {
    s.model(0).ok()?.items.into_iter().find(|i| i.kind == "image" && i.locked.is_none() && i.bbox[2] < 400.0 && i.bbox[3] < 400.0 && i.bbox[2] > 8.0)
}

// 2: image move / resize / remove on a logo
#[test]
fn e3_image_move_resize_remove() {
    let _g = gs_guard();
    for prefix in ["table_systemgenerated-IKPA", "signed_", "letter_Lampiran", RK] {
        let Some(mut s) = open(prefix) else { continue };
        let Some(logo) = find_image(&mut s) else { continue };
        let pa = s.page(0).unwrap();
        let before = shot(&s.orig, &pa, 100.0);
        let old = logo.bbox;
        let m = batch_ok(&mut s, vec![mv(0, IKind::Image, logo.id, 90.0, 60.0)]);
        let nb = items_of(&m[0], "image").iter().find(|i| i.id == logo.id).unwrap().bbox;
        let b = s.built().unwrap();
        let after = shot(&b.bytes, &pa, 100.0);
        let (out, inn) = diff_outside(&before, &after, &[old, nb], 100.0, 2);
        let moved = (nb[0] - old[0] - 90.0).abs() < 0.5 && (nb[1] - old[1] - 60.0).abs() < 0.5 && (nb[2] - old[2]).abs() < 0.5;
        after.save(e3_dir().join("logo_move.png")).unwrap();
        let clean = gs_clean(&b.bytes);
        println!("{prefix}: logo move: reported box moved {moved}, diff outside old+new {out}, inside {inn}, gs clean {clean}");
        assert!(moved && out == 0 && inn > 0 && clean);
        let r = [nb[0], nb[1], nb[2] / 2.0, nb[3] / 2.0];
        let res = s.apply_batch(&EditBatchReq { edits: vec![ObjEdit3::Resize { page: 0, item: ItemRef { kind: IKind::Image, id: logo.id }, rect: r }] }).unwrap();
        let BatchRes::Ok { pages } = res else { panic!("resize") };
        let rb = items_of(&pages[0], "image").iter().find(|i| i.id == logo.id).unwrap().bbox;
        let b2 = s.built().unwrap();
        let after2 = shot(&b2.bytes, &pa, 100.0);
        let (out2, _) = diff_outside(&before, &after2, &[old, nb], 100.0, 2);
        after2.save(e3_dir().join("logo_resize.png")).unwrap();
        println!("{prefix}: logo resize: box {:.1}x{:.1} (wanted {:.1}x{:.1}), diff outside {out2}", rb[2], rb[3], r[2], r[3]);
        assert!((rb[2] - r[2]).abs() < 0.5 && (rb[3] - r[3]).abs() < 0.5 && out2 == 0);
        s.step(true).unwrap();
        s.step(true).unwrap();
        let n_imgs = |bytes: &[u8]| -> usize { Document::load_mem(bytes).unwrap().objects.values().filter(|o| matches!(o, Object::Stream(st) if st.dict.get(b"Subtype").ok().and_then(|x| x.as_name().ok()) == Some(b"Image"))).count() };
        let imgs_before = n_imgs(&s.orig);
        let m = batch_ok(&mut s, vec![rm(0, IKind::Image, logo.id)]);
        assert!(!m[0].items.iter().any(|i| i.kind == "image" && i.id == logo.id), "removed id is gone from items");
        let b3 = s.built().unwrap();
        let after3 = shot(&b3.bytes, &pa, 100.0);
        let (out3, inn3) = diff_outside(&before, &after3, &[old], 100.0, 2);
        after3.save(e3_dir().join("logo_remove.png")).unwrap();
        let clean = gs_clean(&b3.bytes);
        println!("{prefix}: logo remove: image objects {imgs_before} -> {}, diff outside {out3}, inside {inn3}, gs clean {clean}", n_imgs(&b3.bytes));
        assert!(out3 == 0 && inn3 > 0 && clean);
        return;
    }
    println!("no sample with a movable logo: skipped");
}

// 3: paths (table borders) on bon p1, and a path inside BT
#[test]
fn e3_path_remove_move_and_bt() {
    if let Some(mut s) = open("table_BonBarangUmum") {
        let pa = s.page(0).unwrap();
        let m = s.model(0).unwrap();
        let thin = |i: &&ItemOut| i.locked.is_none() && (i.bbox[3] < 3.0 || i.bbox[2] < 3.0);
        let line = items_of(&m, "path").iter().filter(thin).cloned().max_by(|a, b| a.bbox[2].max(a.bbox[3]).total_cmp(&b.bbox[2].max(b.bbox[3])));
        if let Some(line) = line {
            let before = shot(&s.orig, &pa, 100.0);
            let m1 = batch_ok(&mut s, vec![rm(0, IKind::Path, line.id)]);
            let after = shot(&s.built().unwrap().bytes, &pa, 100.0);
            let (out, inn) = diff_outside(&before, &after, &[line.bbox], 100.0, 2);
            after.save(e3_dir().join("border_removed.png")).unwrap();
            println!("bon: paths {} -> {}; border remove: diff outside {out}, inside {inn}", items_of(&m, "path").len(), items_of(&m1[0], "path").len());
            assert!(out == 0 && inn > 0 && items_of(&m1[0], "path").len() == items_of(&m, "path").len() - 1);
            s.step(true).unwrap();
            let m2 = batch_ok(&mut s, vec![mv(0, IKind::Path, line.id, 0.0, 14.0)]);
            let nb = items_of(&m2[0], "path").iter().find(|i| i.id == line.id).unwrap().bbox;
            let after = shot(&s.built().unwrap().bytes, &pa, 100.0);
            let (out, inn) = diff_outside(&before, &after, &[line.bbox, nb], 100.0, 2);
            after.save(e3_dir().join("border_moved.png")).unwrap();
            println!("bon: border move: box y {:.1} -> {:.1}, diff outside old+new {out}, inside {inn}", line.bbox[1], nb[1]);
            assert!((nb[1] - line.bbox[1] - 14.0).abs() < 0.5 && out == 0 && inn > 0);
        } else {
            println!("bon: no thin path found");
        }
    }
    if can_run() {
        let mut s = synth_session(&[("BT /F1 12 Tf 50 700 Td 100 100 m 200 100 l S ET", 0)]);
        assert_eq!(items_of(&s.model(0).unwrap(), "path").len(), 1);
        let r = s.apply_batch(&EditBatchReq { edits: vec![mv(0, IKind::Path, 0, 5.0, 5.0)] }).unwrap();
        println!("path inside BT: cannot_move {}", matches!(r, BatchRes::CannotMove { .. }));
        assert!(matches!(r, BatchRes::CannotMove { page: 0, item: ItemRef { kind: IKind::Path, id: 0 } }));
        // a plain rectangle moves; the clip-only path is no item
        let mut s = synth_session(&[("0 0 1 rg 100 100 50 20 re f q 10 10 200 200 re W n 300 300 40 40 re S Q", 0)]);
        assert_eq!(items_of(&s.model(0).unwrap(), "path").len(), 2, "clip-only path is no item");
        let pa = s.page(0).unwrap();
        batch_ok(&mut s, vec![mv(0, IKind::Path, 0, 30.0, 0.0)]);
        let aft = shot(&s.built().unwrap().bytes, &pa, 72.0);
        let bef = shot(&s.orig, &pa, 72.0);
        let blue = |img: &RgbaImage, x: u32, y: u32| img.get_pixel(x, y).0[2] > 200 && img.get_pixel(x, y).0[0] < 80;
        println!("synthetic rect moved right by 30 pt: old spot blue {} -> {}, new spot blue {} -> {}", blue(&bef, 110, 682), blue(&aft, 110, 682), blue(&bef, 170, 682), blue(&aft, 170, 682));
        assert!(blue(&bef, 110, 682) && !blue(&aft, 110, 682) && blue(&aft, 170, 682));
        // removing the stroked square keeps the clip valid
        batch_ok(&mut s, vec![rm(0, IKind::Path, 1)]);
        assert!(gs_clean(&s.built().unwrap().bytes));
    }
}

// 4: paragraph move / remove on ringkasan
#[test]
fn e3_paragraph_move_remove() {
    let Some(mut s) = open(RK) else { return };
    let pa = s.page(0).unwrap();
    let m0 = s.model(0).unwrap();
    let h = pa.disp[1];
    let Some(p) = m0.paras.iter().find(|q| q.status == "direct" && q.text.split_whitespace().count() > 25 && q.bbox[1] + 2.0 * q.bbox[3] + 20.0 < h - 60.0) else {
        println!("no suitable paragraph");
        return;
    };
    let (id, bbox) = (p.id, p.bbox);
    let ptext = p.text.clone();
    let words: String = ptext.split_whitespace().take(3).collect::<Vec<_>>().join(" ");
    let before = shot(&s.orig, &pa, 100.0);
    let n_before = count_in(&text_of(&s.orig, 0), &words);
    let dy = bbox[3] + 12.0;
    let moved = batch_ok(&mut s, vec![mv(0, IKind::Para, id, 0.0, dy)]);
    let nb = items_of(&moved[0], "para").iter().find(|i| i.id == id).unwrap().bbox;
    let b = s.built().unwrap();
    let after = shot(&b.bytes, &pa, 100.0);
    let new_rect = [bbox[0], bbox[1] + dy, bbox[2], bbox[3]];
    let (ink_old_before, ink_old_after) = (ink_in(&before, bbox, 100.0), ink_in(&after, bbox, 100.0));
    let (ink_new_before, ink_new_after) = (ink_in(&before, new_rect, 100.0), ink_in(&after, new_rect, 100.0));
    let n_after = count_in(&text_of(&b.bytes, 0), &words);
    after.save(e3_dir().join("para_move.png")).unwrap();
    println!("para move: box y {:.1} -> {:.1}; words occur {n_before} -> {n_after}; ink old area {ink_old_before} -> {ink_old_after}; new area {ink_new_before} -> {ink_new_after}", bbox[1], nb[1]);
    assert!((nb[1] - bbox[1] - dy).abs() < 1.0 && n_after == n_before && ink_old_after * 50 <= ink_old_before.max(50) && ink_new_after > ink_new_before);
    // a move after a text edit carries the edited text
    s.step(true).unwrap();
    let extra = made_up_in(&ptext, 1);
    let r = s.apply(&EditApplyReq { page: 0, para: id, text: format!("{ptext} {extra}"), use_pc_font: false, allow_overlap: true, style: None, runs: None }).unwrap();
    assert!(matches!(r, EditApplyRes::Ok { .. }));
    batch_ok(&mut s, vec![mv(0, IKind::Para, id, 0.0, 6.0)]);
    let t = flat(&text_of(&s.built().unwrap().bytes, 0));
    println!("edited then moved: new word present {}", t.contains(&extra));
    assert!(t.contains(&extra));
    s.step(true).unwrap();
    s.step(true).unwrap();
    // remove: the words are gone, the rest stays where it was
    let m1 = batch_ok(&mut s, vec![rm(0, IKind::Para, id)]);
    assert!(!m1[0].items.iter().any(|i| i.kind == "para" && i.id == id));
    let b = s.built().unwrap();
    let after = shot(&b.bytes, &pa, 100.0);
    after.save(e3_dir().join("para_remove.png")).unwrap();
    let re = reanalyse(&b.bytes, 0);
    let (n, ok, _, _) = moved_ok(&pa, &re, id, 0.0);
    let gone = count_in(&text_of(&b.bytes, 0), &words);
    println!("para remove: words occur {n_before} -> {gone}; other paragraphs unchanged {ok}/{n}; ink left in old area {}", ink_in(&after, bbox, 100.0));
    assert!(gone < n_before && ok == n);
}

// 5 + 8: a batch is one undo step with exact bytes; a removed paragraph is not in the saved file
#[test]
fn e3_batch_undo_and_privacy() {
    let _g = gs_guard();
    let Some(mut s) = open(RK) else { return };
    let m = s.model(0).unwrap();
    let paras: Vec<&ParaOut> = m.paras.iter().filter(|q| q.status == "direct" && q.text.split_whitespace().count() > 12).take(3).collect();
    if paras.len() < 3 {
        return;
    }
    let ids: Vec<u32> = paras.iter().map(|p| p.id).collect();
    let removed_text = paras[1].text.clone();
    let orig = s.orig.clone();
    batch_ok(&mut s, vec![mv(0, IKind::Para, ids[0], 10.0, 0.0), rm(0, IKind::Para, ids[1]), mv(0, IKind::Para, ids[2], 0.0, 4.0)]);
    let h = s.history(vec![]);
    assert_eq!((h.count, h.can_undo), (3, true));
    let edited = s.built().unwrap().bytes.clone();
    s.step(true).unwrap();
    assert_eq!(s.built().unwrap().bytes, orig, "one undo reverts the whole batch, byte for byte");
    assert!(!s.history(vec![]).can_undo);
    s.step(false).unwrap();
    assert_eq!(s.built().unwrap().bytes, edited, "redo restores the same bytes");
    let dir = std::env::temp_dir().join(format!("majipdf_e3_save_{}", stamp()));
    std::fs::create_dir_all(&dir).unwrap();
    let r = s.save(&dir).unwrap();
    let out = std::fs::read(&r.output).unwrap();
    let want: String = removed_text.chars().filter(|c| !c.is_whitespace()).collect();
    let have: String = text_of(&out, 0).chars().filter(|c| !c.is_whitespace()).collect();
    let had: String = text_of(&orig, 0).chars().filter(|c| !c.is_whitespace()).collect();
    println!("saved: removed paragraph occurs {} time(s) (was {}), output {} bytes", have.matches(&want).count(), had.matches(&want).count(), r.size);
    assert!(have.matches(&want).count() < had.matches(&want).count());
    let _ = std::fs::remove_dir_all(&dir);
}

// 8b: a removed image is not in the saved file
#[test]
fn e3_removed_image_is_unrecoverable() {
    for prefix in ["signed_", "table_systemgenerated-IKPA", "letter_Lampiran", "scanned_bsc"] {
        let Some(mut s) = open(prefix) else { continue };
        let Some(logo) = find_image(&mut s) else { continue };
        let pa = s.page(0).unwrap();
        let pi = pa.items.iter().find(|i| i.kind == IKind::Image && i.id == logo.id).unwrap().clone();
        let Some(name) = pi.name.clone() else { continue };
        let pid = pa.pid;
        let xo = effective_resources(&s.doc, pid).unwrap().get(b"XObject").map(|o| deref(&s.doc, o).as_dict().unwrap().clone()).unwrap();
        let Object::Reference(oid) = xo.get(name.as_bytes()).unwrap().clone() else { continue };
        let Object::Stream(st) = s.doc.get_object(oid).unwrap().clone() else { continue };
        let shared = s.doc.page_iter().filter(|p| *p != pid).any(|p| effective_resources(&s.doc, p).and_then(|r| r.get(b"XObject").ok()).and_then(|o| deref(&s.doc, o).as_dict().ok()).is_some_and(|d| d.iter().any(|(_, v)| v.as_reference().ok() == Some(oid))));
        if shared || st.content.len() < 200 {
            continue;
        }
        batch_ok(&mut s, vec![rm(0, IKind::Image, logo.id)]);
        let out = s.built().unwrap().bytes.clone();
        let mid = st.content.len() / 2;
        let probe = &st.content[mid..mid + 64];
        let present = out.windows(64).any(|w| w == probe);
        let d = Document::load_mem(&out).unwrap();
        let thumb = d.get_dictionary(d.get_pages()[&1]).unwrap().has(b"Thumb");
        println!("{prefix}: removed image stream still in file {present}; object gone {}; /Thumb on page {thumb}", d.get_object(oid).is_err());
        assert!(!present && d.get_object(oid).is_err() && !thumb);
        return;
    }
    println!("no sample with an unshared image: skipped");
}

// 6: watermark on a synthetic document
#[test]
fn e3_watermark_synthetic() {
    if !can_run() {
        return;
    }
    let pgs = [("BT /F1 12 Tf 72 700 Td (One) Tj ET", 0), ("BT /F1 12 Tf 72 700 Td (Two) Tj ET", 0), ("BT /F1 12 Tf 72 700 Td (Three) Tj ET", 0)];
    let mut s = synth_session(&pgs);
    let orig = s.orig.clone();
    for (txt, why) in [("0", "rangeErr:0:3"), ("1-9", "rangeErr:9:3"), ("3-1", "rangeErrFormat:3-1"), ("1,,2", "rangeErrFormat:")] {
        match s.apply_wm(&wm_req("DRAFT", "behind", txt, 45.0)).unwrap() {
            DocRes::BadPages { reason } => assert_eq!(reason, why),
            o => panic!("{o:?}"),
        }
    }
    assert!(matches!(s.apply_wm(&wm_req("\u{3a9}", "behind", "", 0.0)).unwrap(), DocRes::UnsupportedChars { .. }));
    assert_eq!(s.effective().len(), 0, "failed applies leave nothing behind");
    let id = doc_ok(s.apply_wm(&wm_req("DRAFT", "behind", "2", 45.0)).unwrap());
    let b = s.built().unwrap();
    let (c0, c1, c2) = (content(&b.bytes, 0), content(&b.bytes, 1), content(&b.bytes, 2));
    let d = Document::load_mem(&b.bytes).unwrap();
    let pid = d.get_pages()[&2];
    let first = d.get_page_contents(pid)[0];
    let first_bytes = match d.get_object(first).unwrap() {
        Object::Stream(st) => st.get_plain_content().unwrap(),
        _ => vec![],
    };
    let gs = effective_resources(&d, pid).unwrap().get(b"ExtGState").map(|o| deref(&d, o).as_dict().unwrap().clone()).unwrap();
    let ca = gs.iter().filter_map(|(_, v)| deref(&d, v).as_dict().ok().and_then(|g| g.get(b"ca").ok().map(num))).next().unwrap();
    let text = flat(&text_of(&b.bytes, 1));
    let (bw, bh, inside) = {
        let p = pdf::pdfium().unwrap();
        let pd = p.load_pdf_from_byte_vec(b.bytes.clone(), None).unwrap();
        let pg = pd.pages().get(1).unwrap();
        let bb = pg.objects().iter().filter_map(|o| o.as_text_object().filter(|t| t.text().contains("DRAFT")).and_then(|_| o.bounds().ok())).next().unwrap();
        let inside = bb.left().value >= 0.0 && bb.bottom().value >= 0.0 && bb.right().value <= 612.0 && bb.top().value <= 792.0;
        ((bb.right().value - bb.left().value) as f64, (bb.top().value - bb.bottom().value) as f64, inside)
    };
    println!(
        "watermark pages '2': page 1 unchanged {}, page 3 unchanged {}, page 2 changed {}; first stream starts with q {}; ca {ca:.2}; text extractable {}; box {bw:.0}x{bh:.0} inside page {inside}",
        c0 == content(&orig, 0),
        c2 == content(&orig, 2),
        c1 != content(&orig, 1),
        first_bytes.starts_with(b"q"),
        text.contains("DRAFT")
    );
    assert!(c0 == content(&orig, 0) && c2 == content(&orig, 2) && c1 != content(&orig, 1) && first_bytes.starts_with(b"q") && (ca - 0.3).abs() < 0.01 && text.contains("DRAFT") && inside);
    assert!(bh > 0.5 * bw, "a 45 degree box is about as tall as wide");
    // front: the same watermark (same id) moves to the last stream
    let id2 = doc_ok(s.apply_wm(&WmReq { id: Some(id), ..wm_req("DRAFT", "front", "", 0.0) }).unwrap());
    assert_eq!(id, id2);
    let b = s.built().unwrap();
    let d = Document::load_mem(&b.bytes).unwrap();
    let ids = d.get_page_contents(d.get_pages()[&1]);
    let last = match d.get_object(*ids.last().unwrap()).unwrap() {
        Object::Stream(st) => String::from_utf8_lossy(&st.get_plain_content().unwrap()).to_string(),
        _ => String::new(),
    };
    println!("front watermark: last stream holds the text {}", last.contains("Tj"));
    assert!(last.contains("Tj"));
    let pa = s.page(0).unwrap();
    shot(&b.bytes, &pa, 60.0).save(e3_dir().join("wm_front_synth.png")).unwrap();
    // doc objects, undo / redo, delete
    let o = s.doc_objs();
    assert_eq!((o.watermarks.len(), o.header_footers.len(), o.watermarks[0].id), (1, 0, Some(id)));
    let edited = b.bytes.clone();
    s.step(true).unwrap();
    s.step(true).unwrap();
    assert_eq!(s.built().unwrap().bytes, orig, "undo restores the original bytes");
    s.step(false).unwrap();
    s.step(false).unwrap();
    assert_eq!(s.built().unwrap().bytes, edited, "redo restores the same bytes");
    s.del_doc(id).unwrap();
    assert_eq!(s.built().unwrap().bytes, orig);
    s.step(true).unwrap();
    assert_eq!(s.built().unwrap().bytes, edited, "undo of a delete");
}

// 6: image watermark has an SMask
#[test]
fn e3_watermark_image() {
    if !can_run() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("majipdf_e3_lib_{}", stamp()));
    let lib = sig::Lib { dir: dir.clone() };
    let item = lib.add_drawn(&data_url(&alpha_png()), "stamp").unwrap();
    let mut s = synth_session(&[("BT /F1 12 Tf 72 700 Td (One) Tj ET", 0), ("", 0)]);
    s.lib_dir = Some(dir.clone());
    let w = WmReq { kind: "image".into(), text: None, asset: Some(item.id.clone()), scale: Some(40.0), ..wm_req("", "front", "", 0.0) };
    doc_ok(s.apply_wm(&w).unwrap());
    lib.delete(&item.id).unwrap();
    let b = s.built().unwrap();
    let d = Document::load_mem(&b.bytes).unwrap();
    let mut found = 0;
    for pid in d.page_iter() {
        let xo = effective_resources(&d, pid).unwrap().get(b"XObject").map(|o| deref(&d, o).as_dict().unwrap().clone()).unwrap();
        found += xo.iter().filter(|(k, v)| k.starts_with(b"MjSg") && matches!(deref(&d, v), Object::Stream(st) if st.dict.has(b"SMask"))).count();
    }
    let img = shot(&b.bytes, &s.page(0).unwrap(), 50.0);
    let centre = img.get_pixel(img.width() / 2, img.height() / 2).0;
    println!("image watermark: pages with an SMask image {found}, centre pixel reddish {}", centre[0] > centre[1] + 40);
    assert_eq!(found, 2);
    assert!(centre[0] > centre[1] + 40);
    let _ = std::fs::remove_dir_all(&dir);
}

// 6b: scan pages force the watermark to the front
#[test]
fn e3_watermark_scan_forced_front() {
    let Some(mut s) = open("scanned_bsc") else { return };
    doc_ok(s.apply_wm(&wm_req("DRAFT", "behind", "1", 45.0)).unwrap());
    let m = s.model(0).unwrap();
    let b = s.built().unwrap();
    let d = Document::load_mem(&b.bytes).unwrap();
    let ids = d.get_page_contents(d.get_pages()[&1]);
    let txt = |i: ObjectId| match d.get_object(i).unwrap() {
        Object::Stream(st) => String::from_utf8_lossy(&st.get_plain_content().unwrap()).contains(" Tj"),
        _ => false,
    };
    println!("scanned_bsc: scan {}, forced front {}, last stream has the watermark {}, first stream has it {}", m.scan, m.wm_forced_front, txt(*ids.last().unwrap()), txt(ids[0]));
    assert!(m.wm_forced_front && txt(*ids.last().unwrap()) && !txt(ids[0]));
    let pa = s.page(0).unwrap();
    shot(&b.bytes, &pa, 60.0).save(e3_dir().join("wm_scan_front.png")).unwrap();
}

// 6c: pictures of a text watermark behind and in front on ringkasan
#[test]
fn e3_watermark_pictures() {
    let Some(mut s) = open(RK) else { return };
    let pa = s.page(0).unwrap();
    let id = doc_ok(s.apply_wm(&wm_req("DRAFT", "behind", "", 45.0)).unwrap());
    shot(&s.built().unwrap().bytes, &pa, 70.0).save(e3_dir().join("wm_behind.png")).unwrap();
    doc_ok(s.apply_wm(&WmReq { id: Some(id), ..wm_req("DRAFT", "front", "", 45.0) }).unwrap());
    shot(&s.built().unwrap().bytes, &pa, 70.0).save(e3_dir().join("wm_front.png")).unwrap();
}

// 7: header and footer on a synthetic document
#[test]
fn e3_header_footer_synthetic() {
    if !can_run() {
        return;
    }
    let body = |t: &str, at: &str| format!("BT /F1 12 Tf {at} Td ({t}) Tj ET");
    let (b1, b2, b4, b5) = (body("One", "72 700"), body("Two", "72 700"), body("Four", "72 700"), body("Five", "72 700"));
    // page 3 leaves a q and a scale open: the footer must not inherit them
    let b3 = format!("q 0.5 0 0 0.5 0 0 cm {}", body("Three", "144 1400"));
    let pgs = [(b1.as_str(), 0), (b2.as_str(), 0), (b3.as_str(), 0), (b4.as_str(), 0), (b5.as_str(), 0)];
    let mut s = synth_session(&pgs);
    let id = doc_ok(s.apply_hf(&HfReq { skip_first: true, ..hf_req("bc", "Halaman {page} dari {total}") }).unwrap());
    let bytes = s.built().unwrap().bytes.clone();
    let (t1, t2) = (flat(&text_of(&bytes, 0)), flat(&text_of(&bytes, 1)));
    println!("footer, skip first: page 1 has no footer {}, page 2 has 'Halaman 2 dari 5' {}", !t1.contains("Halaman"), t2.contains("Halaman 2 dari 5"));
    assert!(!t1.contains("Halaman") && t2.contains("Halaman 2 dari 5") && flat(&text_of(&bytes, 4)).contains("Halaman 5 dari 5"));
    let pa = s.page(2).unwrap();
    let img = render_bytes(&bytes, 2, (pa.disp[0] * 100.0 / 72.0) as u32).unwrap();
    img.save(e3_dir().join("footer_p3.png")).unwrap();
    let k = 100.0 / 72.0;
    let band_h = (60.0 * k) as u32;
    let band = RgbaImage::from_fn(img.width(), band_h, |x, y| *img.get_pixel(x, img.height() - band_h + y));
    let bx = ink_box(&band).unwrap();
    let cx = (bx.0 + bx.2) as f64 / 2.0 / k;
    println!("footer on the unbalanced page: centre x {cx:.1} pt (page centre 306)");
    assert!((cx - 306.0).abs() < 3.0);
    doc_ok(s.apply_hf(&HfReq { id: Some(id), start: 5, ..hf_req("bc", "Halaman {page} dari {total}") }).unwrap());
    let bytes = s.built().unwrap().bytes.clone();
    println!("start 5: page 2 shows 'Halaman 6 dari 9' {}", flat(&text_of(&bytes, 1)).contains("Halaman 6 dari 9"));
    assert!(flat(&text_of(&bytes, 1)).contains("Halaman 6 dari 9") && flat(&text_of(&bytes, 0)).contains("Halaman 5 dari 9"));
    doc_ok(s.apply_hf(&hf_req("tl", "{date} {file}")).unwrap());
    let t = flat(&text_of(&s.built().unwrap().bytes, 0));
    let today = docobj::today("id", 0);
    println!("date: today's Indonesian date is in the header {}, the footer is still there {}", t.contains(&today), t.contains("Halaman 5 dari 9"));
    assert!(t.contains(&today) && t.contains("Halaman 5 dari 9"));
    assert_eq!(s.history(vec![]).changed_pages, vec![0, 1, 2, 3, 4]);
    doc_ok(s.apply_hf(&HfReq { id: Some(id), pages: "2-3".into(), ..hf_req("bc", "{page}/{total}") }).unwrap());
    let bytes = s.built().unwrap().bytes.clone();
    println!("pages 2-3: page 2 '1/2' {}, page 3 '2/2' {}, page 4 none {}", flat(&text_of(&bytes, 1)).contains("1/2"), flat(&text_of(&bytes, 2)).contains("2/2"), !flat(&text_of(&bytes, 3)).contains('/'));
    assert!(flat(&text_of(&bytes, 1)).contains("1/2") && flat(&text_of(&bytes, 2)).contains("2/2") && !flat(&text_of(&bytes, 3)).contains('/'));
}

// 7: top-right on a rotated page lands in the displayed top-right corner
#[test]
fn e3_header_on_rotated_page() {
    if !can_run() {
        return;
    }
    let mut s = synth_session(&[("", 90), ("", 0)]);
    doc_ok(s.apply_hf(&hf_req("tr", "88")).unwrap());
    let bytes = s.built().unwrap().bytes.clone();
    let k = 100.0 / 72.0;
    for pg in 0..2u32 {
        let (w, h) = (s.sizes[pg as usize].0 as f64, s.sizes[pg as usize].1 as f64);
        let img = render_bytes(&bytes, pg, (w * k) as u32).unwrap();
        let b = ink_box(&img).unwrap();
        let (right, top) = (w - (b.2 + 1) as f64 / k, b.1 as f64 / k);
        img.save(e3_dir().join(format!("header_tr_p{}.png", pg + 1))).unwrap();
        println!("tr on page {} ({w:.0}x{h:.0}): gap to right edge {right:.1} pt, to top edge {top:.1} pt (margin 28)", pg + 1);
        assert!(right > 27.0 && right <= 30.0 && top > 27.0 && top <= 30.5);
    }
}

// 7b: pictures of footers on ringkasan
#[test]
fn e3_footer_pictures() {
    let Some(mut s) = open(RK) else { return };
    let n = s.sizes.len();
    doc_ok(s.apply_hf(&HfReq { skip_first: true, ..hf_req("bc", "Halaman {page} dari {total}") }).unwrap());
    doc_ok(s.apply_hf(&hf_req("tr", "{date}")).unwrap());
    let bytes = s.built().unwrap().bytes.clone();
    let t2 = flat(&text_of(&bytes, 1));
    println!("ringkasan footer: {n} pages, page 2 shows the numbering {}", t2.contains(&format!("Halaman 2 dari {n}")));
    assert!(t2.contains(&format!("Halaman 2 dari {n}")));
    let pa = s.page(1).unwrap();
    render_bytes(&bytes, 1, (pa.disp[0] * 70.0 / 72.0) as u32).unwrap().save(e3_dir().join("footer_rk_p2.png")).unwrap();
}

// inline image whose data holds " EI " must stay in one piece (move wraps it, remove deletes all of it)
#[test]
fn e3_inline_image_with_ei_in_data() {
    if !can_run() {
        return;
    }
    let mut c: Vec<u8> = b"q 100 0 0 100 200 500 cm BI /W 4 /H 4 /BPC 8 /CS /G ID ".to_vec();
    c.extend_from_slice(&[0x20, 0x45, 0x49, 0x20, 0x20, 0x45, 0x49, 0x0a, 0x20, 0x45, 0x49, 0x20, 0x10, 0x10, 0x10, 0x10]);
    c.extend_from_slice(b"
EI Q
0 0 1 rg 50 50 30 30 re f
");
    let cs = String::from_utf8(c).unwrap();
    let mut s = synth_session(&[(cs.as_str(), 0)]);
    let m = s.model(0).unwrap();
    assert_eq!(items_of(&m, "image").len(), 1);
    assert_eq!(items_of(&m, "path").len(), 1, "the square after the image is still its own path");
    let pa0 = s.page(0).unwrap();
    let before = shot(&s.orig, &pa0, 72.0);
    let old = items_of(&m, "image")[0].bbox;
    batch_ok(&mut s, vec![mv(0, IKind::Image, 0, 60.0, 40.0)]);
    let after = shot(&s.built().unwrap().bytes, &pa0, 72.0);
    let nb = items_of(&s.model(0).unwrap(), "image")[0].bbox;
    let (out, inn) = diff_outside(&before, &after, &[old, nb], 72.0, 2);
    println!("inline image move: diff outside {out}, inside {inn}");
    assert!(out == 0 && inn > 0 && after.get_pixel(65, 792 - 65).0[2] > 200);
    s.step(true).unwrap();
    batch_ok(&mut s, vec![rm(0, IKind::Image, 0)]);
    let o = s.built().unwrap().bytes.clone();
    let left = content(&o, 0).windows(2).any(|w| w == b"EI") || content(&o, 0).windows(2).any(|w| w == b"BI");
    let a2 = shot(&o, &pa0, 72.0);
    println!("inline image remove: no BI/EI bytes left {}, blue square kept {}", !left, a2.get_pixel(65, 792 - 65).0[2] > 200);
    assert!(!left && a2.get_pixel(65, 792 - 65).0[2] > 200);
    // without a known size the validated scan still finds the right EI
    let mut c: Vec<u8> = b"BI /W 2 /H 2 /BPC 8 /CS /Cs1 ID ".to_vec();
    c.extend_from_slice(b" EI    ");
    c.extend_from_slice(b"
EI
0 0 1 rg 50 50 30 30 re f
");
    let toks = analyse(&c, &BTreeMap::new(), &|_| None);
    assert_eq!(toks.ops.iter().filter(|o| o.k == OK::Paint).count(), 2, "image + square");
}

#[test]
fn e3_header_too_long_and_empty_watermark() {
    if !can_run() {
        return;
    }
    let mut s = synth_session(&[("", 0)]);
    let long = "W".repeat(70);
    // shrinks to fit
    doc_ok(s.apply_hf(&hf_req("bc", &long[..30])).unwrap());
    let img = render_bytes(&s.built().unwrap().bytes, 0, 612).unwrap();
    let b = ink_box(&img).unwrap();
    println!("30 x W at 10 pt: ink {}..{} of 612", b.0, b.2);
    assert!(b.0 >= 27 && b.2 <= 585);
    // 70 x W needs 5 pt: fits in 556 pt? 70 * 0.944 * 5 = 330, so it shrinks; a huge template does not
    doc_ok(s.apply_hf(&hf_req("bc", &long)).unwrap());
    let img = render_bytes(&s.built().unwrap().bytes, 0, 612).unwrap();
    let b = ink_box(&img).unwrap();
    assert!(b.0 >= 27 && b.2 <= 585);
    let huge = "W".repeat(400);
    assert!(matches!(s.apply_hf(&hf_req("bc", &huge)).unwrap(), DocRes::TooLong));
    assert!(matches!(s.apply_wm(&wm_req("  ", "behind", "", 0.0)).unwrap(), DocRes::BadText));
}

fn trailer_ids(bytes: &[u8]) -> Vec<Vec<u8>> {
    let d = Document::load_mem(bytes).unwrap();
    d.trailer.get(b"ID").ok().and_then(|o| o.as_array().ok()).map(|a| a.iter().filter_map(|o| if let Object::String(s, _) = o { Some(s.clone()) } else { None }).collect()).unwrap_or_default()
}

// every save gets a new ID[1]; ID[0] stays
#[test]
fn e3_save_renews_document_id() {
    let _g = gs_guard();
    if !can_run() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("majipdf_e3_id_{}", stamp()));
    std::fs::create_dir_all(&dir).unwrap();
    // gs path (PC font embedded) on a synthetic file that has no /ID, and the sample (non-gs) when present
    let mut s = synth_session(&[("BT /F1 12 Tf 72 700 Td (One) Tj ET", 0)]);
    doc_ok(s.apply_hf(&hf_req("bc", "{page}")).unwrap());
    let (a, b) = (std::fs::read(s.save(&dir).unwrap().output).unwrap(), std::fs::read(s.save(&dir).unwrap().output).unwrap());
    let (ia, ib) = (trailer_ids(&a), trailer_ids(&b));
    println!("synthetic (gs save): ids present {}, ID[1] differs between two saves {}", ia.len() == 2, ia.get(1) != ib.get(1));
    assert!(ia.len() == 2 && ib.len() == 2 && ia[1] != ib[1] && ia[1].len() == 16);
    if let Some(mut s) = open(RK) {
        let orig = trailer_ids(&s.orig);
        let m = s.model(0).unwrap();
        let p = m.paras.iter().find(|q| q.status == "direct").unwrap();
        batch_ok(&mut s, vec![rm(0, IKind::Para, p.id)]);
        let (a, b) = (std::fs::read(s.save(&dir).unwrap().output).unwrap(), std::fs::read(s.save(&dir).unwrap().output).unwrap());
        let (ia, ib) = (trailer_ids(&a), trailer_ids(&b));
        println!("ringkasan: input has ID {}, ID[0] kept {}, ID[1] new {}, two saves differ {}", orig.len() == 2, orig.first() == ia.first(), orig.get(1) != ia.get(1), ia.get(1) != ib.get(1));
        if orig.len() == 2 {
            assert!(orig[0] == ia[0] && orig[1] != ia[1]);
        }
        assert!(ia[1] != ib[1]);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn e3_date_placeholders() {
    if !can_run() {
        return;
    }
    let mut s = synth_session(&[("", 0)]);
    // +07:00 around midnight UTC is already the next day
    let tz = 7 * 60;
    doc_ok(s.apply_hf(&HfReq { tz_min: tz, lang: "en".into(), ..hf_req("tl", "{date_id}|{date_en}|{date}") }).unwrap());
    let t = flat(&text_of(&s.built().unwrap().bytes, 0));
    let (id, en) = (docobj::today("id", tz), docobj::today("en", tz));
    println!("date placeholders: id form {}, en form {}, {{date}} follows lang en {}", t.contains(&id), t.contains(&en), t.matches(&en).count() == 2);
    assert!(t.contains(&format!("{id}|{en}|{en}")));
    assert_eq!(docobj::date_text(20_737, "id"), "11 Oktober 2026");
    assert_eq!(docobj::date_text(20_737, "en"), "11 October 2026");
}
