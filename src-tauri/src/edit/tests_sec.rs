//! Security review round 2: regression tests on synthetic PDFs (no `_samples/` needed; they skip when pdfium or a PC font is missing).
use super::*;
use lopdf::dictionary;

/// A document built by hand: `build` adds the objects and returns the page ids (pages must use `pages` as /Parent).
fn mk(build: impl FnOnce(&mut Document, ObjectId, ObjectId) -> Vec<ObjectId>, pages_extra: Vec<(&str, Object)>, cat_extra: Vec<(&str, Object)>) -> Session {
    let mut doc = Document::with_version("1.5");
    let pages = doc.new_object_id();
    let font = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding" });
    let ids = build(&mut doc, pages, font);
    let mut pd = dictionary! { "Type" => "Pages", "Kids" => ids.iter().map(|i| Object::Reference(*i)).collect::<Vec<_>>(), "Count" => ids.len() as i64 };
    for (k, v) in pages_extra {
        pd.set(k, v);
    }
    doc.objects.insert(pages, Object::Dictionary(pd));
    let mut cat = dictionary! { "Type" => "Catalog", "Pages" => pages };
    for (k, v) in cat_extra {
        cat.set(k, v);
    }
    let cat = doc.add_object(cat);
    doc.trailer.set("Root", cat);
    let p = std::env::temp_dir().join(format!("majipdf_sec_{}.pdf", stamp()));
    doc.save(&p).unwrap();
    let s = Session::open(&p, None, None).unwrap().0;
    let _ = std::fs::remove_file(&p);
    s
}

fn gray_img(doc: &mut Document, w: i64, h: i64, bpc: i64, data: Vec<u8>, extra: Vec<(&str, Object)>) -> ObjectId {
    let mut d = dictionary! { "Type" => "XObject", "Subtype" => "Image", "Width" => w, "Height" => h, "ColorSpace" => "DeviceGray", "BitsPerComponent" => bpc };
    for (k, v) in extra {
        d.set(k, v);
    }
    doc.add_object(Stream::new(d, data))
}

fn page(doc: &mut Document, pages: ObjectId, content: &str, res: Option<Dictionary>, extra: Vec<(&str, Object)>) -> ObjectId {
    let cid = doc.add_object(Stream::new(Dictionary::new(), content.as_bytes().to_vec()));
    let mut d = dictionary! { "Type" => "Page", "Parent" => pages, "MediaBox" => vec![Object::Integer(0), Object::Integer(0), Object::Integer(612), Object::Integer(792)], "Contents" => cid };
    if let Some(r) = res {
        d.set("Resources", Object::Dictionary(r));
    }
    for (k, v) in extra {
        d.set(k, v);
    }
    doc.add_object(d)
}

/// A distinctive gray picture: no pixel is white.
fn pixels(n: usize) -> Vec<u8> {
    (0..n * n).map(|i| 0x20 + (i % 8) as u8 * 8).collect()
}

/// True when any stream of the file holds `needle`, decoded or raw.
fn leaked(bytes: &[u8], needle: &[u8]) -> bool {
    if bytes.windows(needle.len()).any(|w| w == needle) {
        return true;
    }
    let d = Document::load_mem(bytes).unwrap();
    d.objects.values().any(|o| matches!(o, Object::Stream(s) if fonts::plain(s).is_ok_and(|v| v.windows(needle.len()).any(|w| w == needle))))
}

fn wo(s: &mut Session, r: [f64; 4], cover: bool) -> ObjRes {
    let mut q = req("whiteout", 0, None, r);
    q.cover_only = Some(cover);
    s.apply_obj(&q).unwrap()
}

const IMG_CONTENT: &str = "q 200 0 0 200 100 500 cm /Im1 Do Q";
// display rect over that image (page 792 pt high): y = 792 - 700 = 92
const OVER_IMG: [f64; 4] = [90.0, 82.0, 230.0, 220.0];

fn img_doc(content: &str, name: &str, img_extra: impl FnOnce(&mut Document) -> Vec<(&'static str, Object)>) -> Session {
    mk(
        |doc, pages, _| {
            let extra = img_extra(doc);
            let img = gray_img(doc, 8, 8, 8, pixels(8), extra);
            let res = dictionary! { "XObject" => dictionary! { name => img } };
            vec![page(doc, pages, content, Some(res), vec![])]
        },
        vec![],
        vec![],
    )
}

// A1: an image bound in the parent /Pages resources is gone from the file after a white-out
#[test]
fn sec_a1_inherited_resources() {
    if !can_run() {
        return;
    }
    // object ids are 1 = pages, 2 = font, 3 = image in this builder; the inherited dictionary names image 3
    let mut s = mk(
        |doc, pages, _| {
            let img = gray_img(doc, 8, 8, 8, pixels(8), vec![]);
            assert_eq!(img, (3, 0));
            vec![page(doc, pages, IMG_CONTENT, None, vec![])]
        },
        vec![("Resources", Object::Dictionary(dictionary! { "XObject" => dictionary! { "Im1" => Object::Reference((3, 0)) } }))],
        vec![],
    );
    let before = leaked(&s.orig, &pixels(8));
    assert!(before);
    let r = wo(&mut s, OVER_IMG, false);
    assert!(matches!(r, ObjRes::Ok { .. }), "{r:?}");
    let b = s.built().unwrap().bytes.clone();
    println!("A1 inherited image: original pixels in the file before {before}, after {}", leaked(&b, &pixels(8)));
    assert!(!leaked(&b, &pixels(8)));
    // select-removal of the same image
    let mut s2 = mk(
        |doc, pages, _| {
            let img = gray_img(doc, 8, 8, 8, pixels(8), vec![]);
            assert_eq!(img, (3, 0));
            vec![page(doc, pages, IMG_CONTENT, None, vec![])]
        },
        vec![("Resources", Object::Dictionary(dictionary! { "XObject" => dictionary! { "Im1" => Object::Reference((3, 0)) } }))],
        vec![],
    );
    batch_ok(&mut s2, vec![rm(0, IKind::Image, 0)]);
    assert!(!leaked(&s2.built().unwrap().bytes, &pixels(8)));
}

// A2: inline images and placed signatures can't be cleaned pixel by pixel: whiteout_partial, then cover only
#[test]
fn sec_a2_inline_and_placed_images() {
    if !can_run() {
        return;
    }
    let mut s = mk(|doc, pages, _| vec![page(doc, pages, "q 100 0 0 100 100 600 cm BI /W 2 /H 2 /BPC 8 /CS /G ID abcd EI Q", None, vec![])], vec![], vec![]);
    let r = wo(&mut s, [90.0, 80.0, 130.0, 130.0], false);
    println!("A2 inline: {}", matches!(r, ObjRes::WhiteoutPartial { .. }));
    assert!(matches!(r, ObjRes::WhiteoutPartial { .. }));
    assert!(matches!(wo(&mut s, [90.0, 80.0, 130.0, 130.0], true), ObjRes::Ok { .. }));
    // a placed image under a later white-out
    let dir = std::env::temp_dir().join(format!("majipdf_sec_lib_{}", stamp()));
    let lib = sig::Lib { dir: dir.clone() };
    let item = lib.add_drawn(&data_url(&alpha_png()), "x").unwrap();
    let mut s = synth_session(&[("", 0)]);
    s.lib_dir = Some(dir.clone());
    let mut q = req("image", 0, None, [100.0, 100.0, 90.0, 60.0]);
    q.asset = Some(item.id.clone());
    new_id(s.apply_obj(&q).unwrap());
    let r = wo(&mut s, [90.0, 90.0, 120.0, 80.0], false);
    println!("A2 placed image: {}", matches!(r, ObjRes::WhiteoutPartial { .. }));
    assert!(matches!(r, ObjRes::WhiteoutPartial { .. }));
    assert!(matches!(wo(&mut s, [90.0, 90.0, 120.0, 80.0], true), ObjRes::Ok { .. }));
    let _ = std::fs::remove_dir_all(&dir);
}

// A3: a white-out over a moved image uses where the image is now
#[test]
fn sec_a3_moved_image() {
    if !can_run() {
        return;
    }
    let mut s = img_doc(IMG_CONTENT, "Im1", |_| vec![]);
    batch_ok(&mut s, vec![mv(0, IKind::Image, 0, 0.0, 300.0)]); // 300 pt down
    let r = wo(&mut s, [90.0, 382.0, 230.0, 220.0], false);
    assert!(matches!(r, ObjRes::Ok { .. }), "{r:?}");
    let b = s.built().unwrap().bytes.clone();
    println!("A3 moved then white-out: original pixels left {}", leaked(&b, &pixels(8)));
    assert!(!leaked(&b, &pixels(8)));
    // resized: the same
    let mut s = img_doc(IMG_CONTENT, "Im1", |_| vec![]);
    let bb = s.model(0).unwrap().items.iter().find(|i| i.kind == "image").unwrap().bbox;
    s.apply_batch(&EditBatchReq { edits: vec![ObjEdit3::Resize { page: 0, item: ItemRef { kind: IKind::Image, id: 0 }, rect: [bb[0] + 300.0, bb[1], 80.0, 80.0] }] }).unwrap();
    assert!(matches!(wo(&mut s, [390.0, bb[1] - 5.0, 100.0, 100.0], false), ObjRes::Ok { .. }));
    assert!(!leaked(&s.built().unwrap().bytes, &pixels(8)));
}

// A4: soft masks: 8-bit is sanitised, 16-bit and broken sizes fail closed, bad dimensions don't panic
#[test]
fn sec_a4_soft_masks() {
    if !can_run() {
        return;
    }
    let mask8: Vec<u8> = (0..64).map(|i| if i % 2 == 0 { 255 } else { 128 }).collect();
    let mut s = img_doc(IMG_CONTENT, "Im1", |doc| {
        let m = gray_img(doc, 8, 8, 8, mask8.clone(), vec![]);
        vec![("SMask", Object::Reference(m))]
    });
    assert!(matches!(wo(&mut s, OVER_IMG, false), ObjRes::Ok { .. }));
    let b = s.built().unwrap().bytes.clone();
    assert!(!leaked(&b, &pixels(8)) && !leaked(&b, &mask8), "mask replaced too");
    for (label, w, h, bpc, len) in [("16-bit", 8i64, 8i64, 16i64, 128usize), ("short data", 8, 8, 8, 10), ("zero width", 0, 8, 8, 64), ("negative height", 8, -8, 8, 64)] {
        let mut s = img_doc(IMG_CONTENT, "Im1", |doc| {
            let m = gray_img(doc, w, h, bpc, vec![7u8; len], vec![]);
            vec![("SMask", Object::Reference(m))]
        });
        let r = wo(&mut s, OVER_IMG, false);
        println!("A4 {label}: {}", matches!(r, ObjRes::WhiteoutPartial { .. }));
        assert!(matches!(r, ObjRes::WhiteoutPartial { .. }), "{label}: {r:?}");
        assert!(matches!(wo(&mut s, OVER_IMG, true), ObjRes::Ok { .. }));
    }
    // /Mask (colour key) also fails closed
    let mut s = img_doc(IMG_CONTENT, "Im1", |_| vec![("Mask", Object::Array(vec![Object::Integer(0), Object::Integer(10)]))]);
    assert!(matches!(wo(&mut s, OVER_IMG, false), ObjRes::WhiteoutPartial { .. }));
}

// A5: #xx escapes in names
#[test]
fn sec_a5_escaped_names() {
    if !can_run() {
        return;
    }
    assert_eq!(content::decode_name(b"I#6d1"), "Im1");
    assert_eq!(content::decode_name(b"A#"), "A#");
    assert_eq!(content::decode_name(b"A#zz"), "A#zz");
    let mut s = img_doc("q 200 0 0 200 100 500 cm /I#6d1 Do Q", "Im1", |_| vec![]);
    assert_eq!(items_of(&s.model(0).unwrap(), "image").len(), 1);
    let r = wo(&mut s, OVER_IMG, false);
    assert!(matches!(r, ObjRes::Ok { .. }), "{r:?}");
    println!("A5 escaped name: original pixels left {}", leaked(&s.built().unwrap().bytes, &pixels(8)));
    assert!(!leaked(&s.built().unwrap().bytes, &pixels(8)));
    // a name that resolves to nothing: fail closed
    let mut s = img_doc("q 200 0 0 200 100 500 cm /Nope Do Q", "Im1", |_| vec![]);
    assert!(matches!(wo(&mut s, OVER_IMG, false), ObjRes::WhiteoutPartial { .. }));
}

fn annot_doc(rect: [i64; 4]) -> Session {
    mk(
        |doc, pages, font| {
            let widget = doc.add_object(dictionary! { "Type" => "Annot", "Subtype" => "Widget", "FT" => "Tx", "T" => "f1", "V" => Object::string_literal("SECRETVALUE"), "Rect" => rect.iter().map(|v| Object::Integer(*v)).collect::<Vec<_>>() });
            let link = doc.add_object(dictionary! { "Type" => "Annot", "Subtype" => "Link", "Rect" => vec![Object::Integer(400), Object::Integer(100), Object::Integer(450), Object::Integer(120)] });
            let _ = font;
            let p = page(doc, pages, "0 g 10 10 5 5 re f", None, vec![("Annots", Object::Array(vec![Object::Reference(widget), Object::Reference(link)]))]);
            doc.add_object(dictionary! { "Marker" => Object::string_literal("x") });
            vec![p]
        },
        vec![],
        vec![("AcroForm", Object::Dictionary(dictionary! { "Fields" => vec![Object::Reference((3, 0)), Object::Reference((4, 0))] }))],
    )
}

// A6: annotations under a white-out go; a partly covered one makes it partial
#[test]
fn sec_a6_annotations() {
    if !can_run() {
        return;
    }
    // widget Rect 100,600 .. 200,640 = display y 152..192; link elsewhere stays
    let mut s = annot_doc([100, 600, 200, 640]);
    let r = wo(&mut s, [90.0, 140.0, 130.0, 60.0], false);
    assert!(matches!(r, ObjRes::Ok { .. }), "{r:?}");
    let b = s.built().unwrap().bytes.clone();
    let d = Document::load_mem(&b).unwrap();
    let annots = d.get_dictionary(d.get_pages()[&1]).unwrap().get(b"Annots").unwrap().as_array().unwrap().len();
    let fields = d.get_dictionary(d.trailer.get(b"Root").unwrap().as_reference().unwrap()).unwrap().get(b"AcroForm").map(|o| deref(&d, o).as_dict().unwrap().get(b"Fields").unwrap().as_array().unwrap().len()).unwrap();
    println!("A6 annotations after: page {annots} (was 2), AcroForm fields {fields} (was 2), value in file {}", leaked(&b, b"SECRETVALUE"));
    assert_eq!((annots, fields), (1, 1));
    assert!(!leaked(&b, b"SECRETVALUE"));
    // partly covered
    let mut s = annot_doc([100, 600, 200, 640]);
    let r = wo(&mut s, [150.0, 140.0, 130.0, 60.0], false);
    println!("A6 partly covered: {r:?}");
    assert!(matches!(r, ObjRes::WhiteoutPartial { annots: 1, .. }));
    assert!(matches!(wo(&mut s, [150.0, 140.0, 130.0, 60.0], true), ObjRes::Ok { .. }));
    let b = s.built().unwrap().bytes.clone();
    assert!(leaked(&b, b"SECRETVALUE"), "cover only leaves the annotation");
    // Select removal of a paragraph over a widget: same rule through remove_partial
    // (needs a paragraph: skipped here; the shared find_annots path is covered above)
}

fn alt_doc() -> Session {
    mk(
        |doc, pages, font| {
            let p = page(
                doc,
                pages,
                "/Span <</ActualText (INLINESECRET) /MCID 0>> BDC BT /F1 12 Tf 72 700 Td (Hello) Tj ET EMC /P /Pr BDC 0 g 10 10 5 5 re f EMC",
                Some(dictionary! { "Font" => dictionary! { "F1" => font }, "Properties" => dictionary! { "Pr" => dictionary! { "ActualText" => Object::string_literal("PROPSECRET"), "MCID" => 1 } } }),
                vec![],
            );
            let elem = doc.add_object(dictionary! { "Type" => "StructElem", "S" => "Figure", "Pg" => p, "Alt" => Object::string_literal("ALTSECRET"), "ActualText" => Object::string_literal("ACTUALSECRET"), "K" => 0 });
            let root = doc.add_object(dictionary! { "Type" => "StructTreeRoot", "K" => vec![Object::Reference(elem)] });
            let _ = root;
            vec![p]
        },
        vec![],
        vec![("StructTreeRoot", Object::Reference((6, 0)))],
    )
}

// A7 + A8: alternate text is stripped on a destructive edit; an orphan copy goes with the prune
#[test]
fn sec_a7_alt_text_and_a8_prune() {
    if !can_run() {
        return;
    }
    let mut s = alt_doc();
    for k in [&b"ALTSECRET"[..], b"ACTUALSECRET", b"INLINESECRET", b"PROPSECRET"] {
        assert!(leaked(&s.orig, k), "fixture holds {}", String::from_utf8_lossy(k));
    }
    let r = wo(&mut s, [10.0, 770.0, 20.0, 12.0], false);
    assert!(matches!(r, ObjRes::Ok { .. }), "{r:?}");
    let b = s.built().unwrap().bytes.clone();
    let left: Vec<bool> = [&b"ALTSECRET"[..], b"ACTUALSECRET", b"INLINESECRET", b"PROPSECRET"].iter().map(|k| leaked(&b, k)).collect();
    println!("A7 alternate text left (struct /Alt, struct /ActualText, inline BDC, /Properties): {left:?}");
    assert!(left.iter().all(|x| !x));
    // a cover-only white-out is not destructive: nothing is stripped
    let mut s = alt_doc();
    wo(&mut s, [10.0, 770.0, 20.0, 12.0], true);
    assert!(leaked(&s.built().unwrap().bytes, b"ALTSECRET"));
    // A8: an unreachable object holding an earlier copy
    let mut doc = Document::load_mem(&alt_doc().orig).unwrap();
    doc.add_object(Stream::new(Dictionary::new(), b"ORPHANSECRET".to_vec()));
    let mut raw = Vec::new();
    doc.save_to(&mut raw).unwrap();
    let p = std::env::temp_dir().join(format!("majipdf_sec_orphan_{}.pdf", stamp()));
    std::fs::write(&p, &raw).unwrap();
    let mut s = Session::open(&p, None, None).unwrap().0;
    let _ = std::fs::remove_file(&p);
    assert!(leaked(&s.orig, b"ORPHANSECRET"));
    wo(&mut s, [10.0, 770.0, 20.0, 12.0], false);
    println!("A8 orphan object after a text-only white-out: {}", leaked(&s.built().unwrap().bytes, b"ORPHANSECRET"));
    assert!(!leaked(&s.built().unwrap().bytes, b"ORPHANSECRET"));
}

// A9: the save check catches an original image under a white-out
#[test]
fn sec_a9_verify_removed() {
    if !can_run() {
        return;
    }
    let mut s = img_doc(IMG_CONTENT, "Im1", |_| vec![]);
    let wos = [(0u32, OVER_IMG)];
    let none = HashSet::new();
    assert!(verify_removed(&s.orig, &wos, &none, &HashSet::new()).is_err(), "the original image is under the rect");
    assert!(matches!(wo(&mut s, OVER_IMG, false), ObjRes::Ok { .. }));
    let b = s.built().unwrap();
    assert!(verify_removed(&b.bytes, &wos, &b.ok_images, &HashSet::new()).is_ok());
    assert!(verify_removed(&b.bytes, &wos, &none, &HashSet::new()).is_err(), "not in the allowed set");
    // annotation under the rect
    let a = annot_doc([100, 600, 200, 640]);
    assert!(verify_removed(&a.orig, &[(0, [90.0, 140.0, 130.0, 60.0])], &none, &HashSet::new()).is_err());
    // alternate text left on a destructive page
    let t = alt_doc();
    assert!(verify_removed(&t.orig, &[], &none, &HashSet::from([0])).is_err());
    // a save of a good edit passes the whole check
    let dir = std::env::temp_dir().join(format!("majipdf_sec_save_{}", stamp()));
    std::fs::create_dir_all(&dir).unwrap();
    assert!(s.save(&dir).is_ok());
    let _ = std::fs::remove_dir_all(&dir);
}

// B1: budgets and effective graphics state
#[test]
fn sec_b1_state_budgets() {
    if !can_run() {
        return;
    }
    let mut c = String::new();
    for _ in 0..100_000 {
        c.push_str("/G gs\n");
    }
    c.push_str("BT /F1 12 Tf 72 700 Td (Hi) Tj ET");
    let t = std::time::Instant::now();
    let mut s = synth_session(&[(c.as_str(), 0)]);
    let m = s.model(0).unwrap();
    println!("B1 100k repeated gs: model in {} ms, items {}", t.elapsed().as_millis(), m.items.len());
    assert!(t.elapsed().as_secs() < 20);
    // distinct names beyond the budget lock the page
    let mut c = String::new();
    for i in 0..40 {
        c.push_str(&format!("/G{i} gs\n"));
    }
    c.push_str("10 10 5 5 re f");
    let mut s = synth_session(&[(c.as_str(), 0)]);
    let pa = s.page(0).unwrap();
    println!("B1 40 distinct states: page broken {}", pa.broken);
    assert!(pa.broken);
    assert!(items_of(&s.model(0).unwrap(), "path").iter().all(|i| i.locked == Some("structure")));
    assert!(matches!(s.apply_batch(&EditBatchReq { edits: vec![mv(0, IKind::Path, 0, 1.0, 1.0)] }).unwrap(), BatchRes::CannotMove { .. }));
    // an op budget
    let c = "q Q ".repeat(600_000);
    let mut s = synth_session(&[(c.as_str(), 0)]);
    assert!(s.page(0).unwrap().broken);
}

// B2, B3: panics on multi-byte text and malformed CMaps
#[test]
fn sec_b2_b3_no_panics() {
    // 3 ASCII zeros then a 0xFF byte inside a destination token
    let mut cm = b"1 beginbfchar <0003> <000".to_vec();
    cm.push(0xff);
    cm.extend(b"> endbfchar 1 beginbfrange <0010> <0012> <00".to_vec());
    cm.push(0xff);
    cm.extend(b"> endbfrange <00 <0041");
    let m = fonts::parse_tounicode(&cm);
    println!("B3 malformed CMap parsed without a panic ({} entries)", m.len());
}

// B4: the inflate cap
#[test]
fn sec_b4_decompression_cap() {
    use std::io::Write;
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    enc.write_all(&vec![0u8; 100_000]).unwrap();
    let z = enc.finish().unwrap();
    let st = Stream::new(dictionary! { "Filter" => "FlateDecode" }, z);
    assert!(fonts::plain_cap(&st, 1000).is_err(), "over the cap");
    assert_eq!(fonts::plain_cap(&st, 200_000).ok().map(|v| v.len()), Some(100_000));
    let plain = Stream::new(Dictionary::new(), vec![1u8; 50]);
    assert!(fonts::plain_cap(&plain, 10).is_err());
    println!("B4 flate bomb stopped at the cap");
}

// B5: signature import from a huge PDF page stays bounded
#[test]
fn sec_b5_import_bounded() {
    if !can_run() {
        return;
    }
    let mut doc = Document::with_version("1.5");
    let pages = doc.new_object_id();
    let cid = doc.add_object(Stream::new(Dictionary::new(), b"0 0 0 rg 100 100 8000 8000 re f".to_vec()));
    let p = doc.add_object(dictionary! { "Type" => "Page", "Parent" => pages, "MediaBox" => vec![Object::Integer(0), Object::Integer(0), Object::Integer(14400), Object::Integer(14400)], "Contents" => cid });
    doc.objects.insert(pages, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => vec![Object::Reference(p)], "Count" => 1 }));
    let cat = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
    doc.trailer.set("Root", cat);
    let dir = std::env::temp_dir().join(format!("majipdf_sec_big_{}", stamp()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("big.pdf");
    doc.save(&f).unwrap();
    let img = sig::load_source(&f).unwrap();
    println!("B5 14400 pt page rendered at {}x{}", img.width(), img.height());
    assert!(img.width().max(img.height()) <= 4000);
    let item = sig::Lib { dir: dir.join("lib") }.import(&f, "stamp").unwrap();
    assert!(item.w.max(item.h) <= 1200);
    let _ = std::fs::remove_dir_all(&dir);
}

// C-1: names we add never replace names the file already has (save, reopen, add another)
#[test]
fn sec_c1_resource_names() {
    if !can_run() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("majipdf_sec_names_{}", stamp()));
    let lib = sig::Lib { dir: dir.join("lib") };
    let a = lib.add_drawn(&data_url(&alpha_png()), "a").unwrap();
    let mut b2 = image::RgbaImage::new(30, 20);
    b2.pixels_mut().for_each(|p| *p = image::Rgba([10, 10, 200, 255]));
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(b2).write_to(&mut png, image::ImageFormat::Png).unwrap();
    let b = lib.add_drawn(&data_url(&png.into_inner()), "b").unwrap();
    let mut s = synth_session(&[("", 0)]);
    s.lib_dir = Some(lib.dir.clone());
    let mut q = req("image", 0, None, [100.0, 100.0, 90.0, 60.0]);
    q.asset = Some(a.id.clone());
    new_id(s.apply_obj(&q).unwrap());
    let out = dir.join("one.pdf");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(&out, s.built().unwrap().bytes.clone()).unwrap();
    let mut s2 = Session::open(&out, None, None).unwrap().0;
    s2.lib_dir = Some(lib.dir.clone());
    let mut q = req("image", 0, None, [300.0, 100.0, 90.0, 60.0]);
    q.asset = Some(b.id.clone());
    new_id(s2.apply_obj(&q).unwrap());
    let bytes = s2.built().unwrap().bytes.clone();
    let d = Document::load_mem(&bytes).unwrap();
    let pid = d.get_pages()[&1];
    let xo = effective_resources(&d, pid).unwrap().get(b"XObject").map(|o| deref(&d, o).as_dict().unwrap().clone()).unwrap();
    let mut dims: Vec<(i64, i64)> = xo.iter().filter(|(k, _)| k.starts_with(b"Mj")).filter_map(|(_, v)| if let Object::Stream(st) = deref(&d, v) { Some((st.dict.get(b"Width").unwrap().as_i64().unwrap(), st.dict.get(b"Height").unwrap().as_i64().unwrap())) } else { None }).collect();
    dims.sort();
    println!("C1 after save, reopen, add another: our images {dims:?}");
    assert_eq!(dims, vec![(30, 20), (60, 40)]);
    // the first signature's Do still names the first image
    let content = content(&bytes, 0);
    assert_eq!(String::from_utf8_lossy(&content).matches(" Do").count(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

// sig_clear
#[test]
fn sec_sig_clear() {
    let dir = std::env::temp_dir().join(format!("majipdf_sec_clear_{}", stamp()));
    let lib = sig::Lib { dir: dir.clone() };
    let it = lib.add_drawn(&data_url(&alpha_png()), "x").unwrap();
    let _ = lib.list();
    assert!(dir.join("index.json").exists() && lib.png(&it.id).is_some());
    lib.clear().unwrap();
    println!("sig_clear: items {}, files left {}", lib.list().len(), std::fs::read_dir(&dir).unwrap().count());
    assert!(lib.list().is_empty() && lib.png(&it.id).is_none() && std::fs::read_dir(&dir).unwrap().count() == 0);
    lib.clear().unwrap(); // clearing an empty or missing folder is fine
    let _ = std::fs::remove_dir_all(&dir);
}


#[path = "tests_sec2.rs"]
mod sec2;
