//! Security round 2 regressions (ported from the adversarial verifier's cases): every case must either leave no trace of the secret in
//! the saved file or fail closed (partial, an error, or a refused save). "Ok and saved, secret still there" is the failure.
use super::*;
use lopdf::dictionary;

fn rect(r: [i64; 4]) -> Object {
    r.iter().map(|v| Object::Integer(*v)).collect::<Vec<_>>().into()
}

fn sref(s: &str) -> Object {
    Object::string_literal(s)
}

fn mbox() -> Vec<Object> {
    vec![Object::Integer(0), Object::Integer(0), Object::Integer(612), Object::Integer(792)]
}

type Cat = Vec<(String, Object)>;

/// Like `mk`, with the catalog entries decided by the builder and a Helvetica that has widths (so text is measurable).
fn mkc(build: impl FnOnce(&mut Document, ObjectId, ObjectId) -> (Vec<ObjectId>, Cat)) -> Session {
    let mut doc = Document::with_version("1.5");
    let pages = doc.new_object_id();
    let font = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding", "FirstChar" => 0, "LastChar" => 255, "Widths" => vec![Object::Integer(600); 256] });
    let (ids, extra) = build(&mut doc, pages, font);
    doc.objects.insert(pages, dictionary! { "Type" => "Pages", "Kids" => ids.iter().map(|i| Object::Reference(*i)).collect::<Vec<_>>(), "Count" => ids.len() as i64 }.into());
    let mut cat = dictionary! { "Type" => "Catalog", "Pages" => pages };
    for (k, v) in extra {
        cat.set(k, v);
    }
    let cat = doc.add_object(cat);
    doc.trailer.set("Root", cat);
    let mut raw = Vec::new();
    doc.save_to(&mut raw).unwrap();
    let p = std::env::temp_dir().join(format!("majipdf_sec2_{}.pdf", stamp()));
    std::fs::write(&p, &raw).unwrap();
    let s = Session::open(&p, None, None).unwrap().0;
    let _ = std::fs::remove_file(&p);
    s
}

fn walk_strings(o: &Object, out: &mut Vec<Vec<u8>>) {
    match o {
        Object::String(b, _) | Object::Name(b) => out.push(b.clone()),
        Object::Array(a) => a.iter().for_each(|x| walk_strings(x, out)),
        Object::Dictionary(d) => d.iter().for_each(|(k, v)| {
            out.push(k.clone());
            walk_strings(v, out)
        }),
        Object::Stream(s) => {
            s.dict.iter().for_each(|(k, v)| {
                out.push(k.clone());
                walk_strings(v, out)
            });
            out.push(s.content.clone());
            if let Ok(p) = fonts::plain(s) {
                out.push(p);
            }
        }
        _ => {}
    }
}

/// Where the needle (plain or as UTF-16) is in the file: raw bytes, or any string / name / stream (decoded) of any object.
fn found(bytes: &[u8], needle: &[u8]) -> bool {
    let has = |h: &[u8], n: &[u8]| !n.is_empty() && h.windows(n.len()).any(|w| w == n);
    let u16: Vec<u8> = needle.iter().flat_map(|b| [0u8, *b]).collect();
    if has(bytes, needle) || has(bytes, &u16) {
        return true;
    }
    Document::load_mem(bytes).is_ok_and(|d| {
        d.objects.values().any(|o| {
            let mut v = vec![];
            walk_strings(o, &mut v);
            v.iter().any(|x| has(x, needle) || has(x, &u16))
        })
    })
}

fn save_bytes(s: &mut Session) -> Result<Vec<u8>, String> {
    let dir = std::env::temp_dir().join(format!("majipdf_sec2_save_{}", stamp()));
    std::fs::create_dir_all(&dir).unwrap();
    let r = s.save(&dir).map(|r| std::fs::read(&r.output).unwrap());
    let _ = std::fs::remove_dir_all(&dir);
    r
}

/// The contract of secure removal. Returns true when the white-out went through (and the saved file is clean).
fn secure(name: &str, mut s: Session, r: [f64; 4], needles: &[&[u8]]) -> bool {
    if !can_run() {
        return false;
    }
    for n in needles {
        assert!(found(&s.orig, n), "{name}: fixture holds {}", String::from_utf8_lossy(n));
    }
    let mut q = req("whiteout", 0, None, r);
    q.cover_only = Some(false);
    match s.apply_obj(&q) {
        Ok(ObjRes::Ok { .. }) => match save_bytes(&mut s) {
            Ok(b) => {
                for n in needles {
                    assert!(!found(&b, n), "{name}: LEAK of {}", String::from_utf8_lossy(n));
                }
                println!("{name}: removed and clean");
                true
            }
            Err(e) => {
                println!("{name}: save refused ({e}): fail closed");
                false
            }
        },
        Ok(ObjRes::WhiteoutPartial { .. }) => {
            println!("{name}: partial: fail closed");
            false
        }
        Ok(o) => panic!("{name}: {o:?}"),
        Err(e) => {
            println!("{name}: error ({e}): fail closed");
            false
        }
    }
}

// ---------- L1 / D2 / D3: content filters ----------

fn hexenc(b: &[u8]) -> Vec<u8> {
    let mut v: Vec<u8> = b.iter().flat_map(|x| format!("{:02X}", x).into_bytes()).collect();
    v.push(b'>');
    v
}

fn runlen(b: &[u8]) -> Vec<u8> {
    let mut v = vec![];
    for c in b.chunks(100) {
        v.push((c.len() - 1) as u8);
        v.extend(c);
    }
    v.push(128);
    v
}

#[test]
fn sec2_l1_content_filters() {
    if !can_run() {
        return;
    }
    let mut plain = b"BT /F1 20 Tf 72 700 Td (FILTSECRET) Tj ET".to_vec();
    plain.extend(std::iter::repeat(b' ').take(300)); // long enough for lopdf to really compress it
    for label in ["asciihex", "runlength", "hex+flate array", "flate+predictor1", "lzw (unsupported)"] {
        let p = plain.clone();
        let s = mkc(|doc, pages, font| {
            let st = match label {
                "asciihex" => Stream::new(dictionary! { "Filter" => "ASCIIHexDecode" }, hexenc(&p)),
                "runlength" => Stream::new(dictionary! { "Filter" => "RunLengthDecode" }, runlen(&p)),
                "hex+flate array" => {
                    let mut z = Stream::new(Dictionary::new(), p.clone());
                    z.compress().unwrap();
                    Stream::new(dictionary! { "Filter" => vec![Object::from("ASCIIHexDecode"), Object::from("FlateDecode")] }, hexenc(&z.content))
                }
                "flate+predictor1" => {
                    let mut z = Stream::new(Dictionary::new(), p.clone());
                    z.compress().unwrap();
                    Stream::new(dictionary! { "Filter" => "FlateDecode", "DecodeParms" => dictionary! { "Predictor" => 1 } }, z.content)
                }
                // content we cannot decode: the secret is in the file, as bytes the engine must not treat as content
                _ => Stream::new(dictionary! { "Filter" => "LZWDecode" }, p.clone()),
            };
            let cid = doc.add_object(st);
            (vec![doc.add_object(dictionary! { "Type" => "Page", "Parent" => pages, "MediaBox" => mbox(), "Contents" => cid, "Resources" => dictionary! { "Font" => dictionary! { "F1" => font } } })], vec![])
        });
        let went = secure(&format!("L1 {label}"), s, [60.0, 60.0, 300.0, 50.0], &[b"FILTSECRET"]);
        // the four decodable chains are really removed; the LZW page is refused, not destroyed
        assert_eq!(went, label != "lzw (unsupported)", "{label}");
    }
}

#[test]
fn sec2_d2_d3_capped_decoding() {
    use std::io::Write;
    let zeros = |n: usize| {
        let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        e.write_all(&vec![0u8; n]).unwrap();
        e.finish().unwrap()
    };
    // with DecodeParms
    let s = Stream::new(dictionary! { "Filter" => "FlateDecode", "DecodeParms" => dictionary! { "Predictor" => 1 } }, zeros(200_000));
    assert_eq!(fonts::plain_cap(&s, 10_000), Err(fonts::DecErr::OverCap));
    // two Flate layers: the inner result is capped too
    let inner = zeros(200_000);
    let mut twice = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    twice.write_all(&inner).unwrap();
    let s = Stream::new(dictionary! { "Filter" => vec![Object::from("FlateDecode"), Object::from("FlateDecode")] }, twice.finish().unwrap());
    assert_eq!(fonts::plain_cap(&s, 100_000), Err(fonts::DecErr::OverCap));
    // a predictor that would overflow in lopdf is refused before any decoding
    for parms in [dictionary! { "Predictor" => 15, "Colors" => i64::MAX }, dictionary! { "Predictor" => 15, "BitsPerComponent" => 7 }, dictionary! { "Predictor" => 15, "Columns" => 1_000_000 }, dictionary! { "Predictor" => 99 }] {
        let s = Stream::new(dictionary! { "Filter" => "FlateDecode", "DecodeParms" => parms }, zeros(100));
        assert_eq!(fonts::plain(&s), Err(fonts::DecErr::Unsupported));
    }
    // a valid PNG predictor (Up) round trips
    let raw: Vec<u8> = vec![2, 1, 2, 3, 2, 1, 1, 1];
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    e.write_all(&raw).unwrap();
    let s = Stream::new(dictionary! { "Filter" => "FlateDecode", "DecodeParms" => dictionary! { "Predictor" => 12, "Columns" => 3 } }, e.finish().unwrap());
    assert_eq!(fonts::plain(&s).unwrap(), vec![1, 2, 3, 2, 3, 4]);
}

// ---------- L2 / L3 / L4 / L5: annotations and form fields ----------

fn annot_session(kind: &str) -> Session {
    let inside = [100, 600, 200, 640];
    mkc(|doc, pages, _| {
        let mut an: Vec<Object> = vec![];
        let mut cat: Cat = vec![];
        let af = |d: Dictionary| ("AcroForm".to_string(), Object::from(d));
        match kind {
            "parentV" => {
                let parent = doc.new_object_id();
                let w = doc.add_object(dictionary! { "Type" => "Annot", "Subtype" => "Widget", "Parent" => parent, "Rect" => rect(inside) });
                doc.objects.insert(parent, dictionary! { "FT" => "Tx", "T" => sref("p"), "V" => sref("PARENTSECRET"), "Kids" => vec![Object::Reference(w)] }.into());
                an.push(w.into());
                cat.push(af(dictionary! { "Fields" => vec![Object::Reference(parent)], "CO" => vec![Object::Reference(parent)] }));
            }
            "CO" => {
                let w = doc.add_object(dictionary! { "Type" => "Annot", "Subtype" => "Widget", "FT" => "Tx", "T" => sref("f"), "V" => sref("COSECRET"), "Rect" => rect(inside) });
                an.push(w.into());
                cat.push(af(dictionary! { "Fields" => vec![Object::Reference(w)], "CO" => vec![Object::Reference(w)] }));
            }
            "fieldsIndirect" => {
                let w = doc.add_object(dictionary! { "Type" => "Annot", "Subtype" => "Widget", "FT" => "Tx", "T" => sref("f"), "V" => sref("FIELDSSECRET"), "Rect" => rect(inside) });
                an.push(w.into());
                let arr = doc.add_object(vec![Object::Reference(w)]);
                cat.push(af(dictionary! { "Fields" => arr }));
            }
            "acroformIndirect" => {
                let w = doc.add_object(dictionary! { "Type" => "Annot", "Subtype" => "Widget", "FT" => "Tx", "T" => sref("f"), "V" => sref("AFINDSECRET"), "Rect" => rect(inside) });
                an.push(w.into());
                let a = doc.add_object(dictionary! { "Fields" => vec![Object::Reference(w)] });
                cat.push(("AcroForm".to_string(), a.into()));
            }
            "twoWidgetsOneField" => {
                // the second widget is outside the white-out: the value would survive there
                let parent = doc.new_object_id();
                let w1 = doc.add_object(dictionary! { "Type" => "Annot", "Subtype" => "Widget", "Parent" => parent, "Rect" => rect(inside) });
                let w2 = doc.add_object(dictionary! { "Type" => "Annot", "Subtype" => "Widget", "Parent" => parent, "Rect" => rect([400, 100, 500, 130]) });
                doc.objects.insert(parent, dictionary! { "FT" => "Tx", "T" => sref("p"), "V" => sref("TWOSECRET"), "Kids" => vec![Object::Reference(w1), Object::Reference(w2)] }.into());
                an.push(w1.into());
                an.push(w2.into());
                cat.push(af(dictionary! { "Fields" => vec![Object::Reference(parent)] }));
            }
            "irt" => {
                let fa = doc.add_object(dictionary! { "Type" => "Annot", "Subtype" => "FreeText", "Contents" => sref("FREESECRET"), "Rect" => rect(inside) });
                let reply = doc.add_object(dictionary! { "Type" => "Annot", "Subtype" => "Text", "IRT" => fa, "RT" => "R", "Contents" => sref("REPLYSECRET"), "Rect" => rect([400, 100, 420, 120]) });
                let reply2 = doc.add_object(dictionary! { "Type" => "Annot", "Subtype" => "Text", "IRT" => reply, "Contents" => sref("REPLY2SECRET"), "Rect" => rect([400, 200, 420, 220]) });
                an.push(fa.into());
                an.push(reply.into());
                an.push(reply2.into());
            }
            "popup" => {
                let pop = doc.new_object_id();
                let a = doc.add_object(dictionary! { "Type" => "Annot", "Subtype" => "Text", "Contents" => sref("PARSECRET"), "Popup" => pop, "Rect" => rect(inside) });
                doc.objects.insert(pop, dictionary! { "Type" => "Annot", "Subtype" => "Popup", "Parent" => a, "Contents" => sref("POPSECRET"), "Rect" => rect([300, 300, 400, 400]) }.into());
                an.push(a.into());
                an.push(pop.into());
            }
            "direct" => an.push(dictionary! { "Type" => "Annot", "Subtype" => "Text", "Contents" => sref("DIRECTSECRET"), "Rect" => rect(inside) }.into()),
            "ap" => {
                let ap = doc.add_object(Stream::new(dictionary! { "Type" => "XObject", "Subtype" => "Form", "BBox" => rect([0, 0, 100, 40]) }, b"BT (APSECRET) Tj ET".to_vec()));
                an.push(doc.add_object(dictionary! { "Type" => "Annot", "Subtype" => "Stamp", "AP" => dictionary! { "N" => ap }, "Rect" => rect(inside) }).into());
            }
            "link" => an.push(doc.add_object(dictionary! { "Type" => "Annot", "Subtype" => "Link", "A" => dictionary! { "S" => "URI", "URI" => sref("http://LINKSECRET.example") }, "Rect" => rect(inside) }).into()),
            "partial" => an.push(doc.add_object(dictionary! { "Type" => "Annot", "Subtype" => "FreeText", "Contents" => sref("PARTSECRET"), "Rect" => rect([150, 600, 300, 640]) }).into()),
            "norect" => an.push(doc.add_object(dictionary! { "Type" => "Annot", "Subtype" => "Text", "Contents" => sref("NORECTSECRET") }).into()),
            "badrect" => an.push(doc.add_object(dictionary! { "Type" => "Annot", "Subtype" => "Text", "Contents" => sref("BADRECTSECRET"), "Rect" => vec![Object::Integer(100), Object::Integer(600)] }).into()),
            _ => {}
        }
        let cid = doc.add_object(Stream::new(Dictionary::new(), b"0 g 10 10 5 5 re f".to_vec()));
        let p = doc.add_object(dictionary! { "Type" => "Page", "Parent" => pages, "MediaBox" => mbox(), "Contents" => cid, "Annots" => an });
        (vec![p], cat)
    })
}

#[test]
fn sec2_annotations_and_fields() {
    if !can_run() {
        return;
    }
    let w = [90.0, 140.0, 130.0, 60.0];
    for (kind, secrets) in [
        ("parentV", vec!["PARENTSECRET"]),
        ("CO", vec!["COSECRET"]),
        ("fieldsIndirect", vec!["FIELDSSECRET"]),
        ("acroformIndirect", vec!["AFINDSECRET"]),
        ("irt", vec!["FREESECRET", "REPLYSECRET", "REPLY2SECRET"]),
        ("popup", vec!["PARSECRET", "POPSECRET"]),
        ("direct", vec!["DIRECTSECRET"]),
        ("ap", vec!["APSECRET"]),
        ("link", vec!["LINKSECRET"]),
        ("partial", vec!["PARTSECRET"]),
        ("norect", vec!["NORECTSECRET"]),
        ("badrect", vec!["BADRECTSECRET"]),
        ("twoWidgetsOneField", vec!["TWOSECRET"]),
    ] {
        let s = annot_session(kind);
        let n: Vec<&[u8]> = secrets.iter().map(|s| s.as_bytes()).collect();
        let went = secure(&format!("annot {kind}"), s, w, &n);
        // the ones that can be taken out cleanly must be
        let must = ["parentV", "CO", "fieldsIndirect", "acroformIndirect", "irt", "popup", "direct", "ap", "link"];
        assert!(went || !must.contains(&kind), "{kind} should be removable");
    }
}

// ---------- L7: structure tree ----------

fn struct_session(kind: &str) -> Session {
    mkc(|doc, pages, font| {
        let pid = doc.new_object_id();
        let cid = doc.add_object(Stream::new(Dictionary::new(), b"/P <</MCID 0>> BDC 0 g 10 10 5 5 re f EMC".to_vec()));
        doc.objects.insert(pid, dictionary! { "Type" => "Page", "Parent" => pages, "MediaBox" => mbox(), "Contents" => cid, "Resources" => dictionary! { "Font" => dictionary! { "F1" => font } } }.into());
        let sroot = doc.new_object_id();
        let mut direct_root = false;
        let k: Object = match kind {
            "directElem" => vec![Object::from(dictionary! { "Type" => "StructElem", "S" => "Figure", "Pg" => pid, "Alt" => sref("DIRECTALT"), "K" => 0 })].into(),
            "kIndirectArray" => {
                let e = doc.add_object(dictionary! { "Type" => "StructElem", "S" => "Figure", "Pg" => pid, "Alt" => sref("INDARRALT"), "K" => 0 });
                doc.add_object(vec![Object::Reference(e)]).into()
            }
            "kIndirectElem" => doc.add_object(dictionary! { "Type" => "StructElem", "S" => "Figure", "Pg" => pid, "Alt" => sref("INDELEMALT"), "K" => 0 }).into(),
            "container" => {
                let leaf = doc.add_object(dictionary! { "Type" => "StructElem", "S" => "P", "Pg" => pid, "K" => 0 });
                let sect = doc.add_object(dictionary! { "Type" => "StructElem", "S" => "Sect", "Alt" => sref("CONTAINERALT"), "ActualText" => sref("CONTAINERACT"), "K" => vec![Object::Reference(leaf)] });
                vec![Object::Reference(sect)].into()
            }
            "mcr" => {
                let e = doc.add_object(dictionary! { "Type" => "StructElem", "S" => "Span", "ActualText" => sref("MCRALT"), "K" => dictionary! { "Type" => "MCR", "Pg" => pid, "MCID" => 0 } });
                vec![Object::Reference(e)].into()
            }
            "inherit" => {
                let child = doc.add_object(dictionary! { "Type" => "StructElem", "S" => "Span", "E" => sref("CHILDEXP"), "K" => 0 });
                let e = doc.add_object(dictionary! { "Type" => "StructElem", "S" => "Div", "Pg" => pid, "K" => vec![Object::Reference(child)] });
                vec![Object::Reference(e)].into()
            }
            "directRoot" => {
                direct_root = true;
                doc.add_object(dictionary! { "Type" => "StructElem", "S" => "Figure", "Pg" => pid, "Alt" => sref("DIRROOTALT"), "K" => 0 }).into()
            }
            "otherPageKid" => {
                let e = doc.add_object(dictionary! { "Type" => "StructElem", "S" => "Figure", "Pg" => pages, "Alt" => sref("OTHERPGALT"), "K" => vec![Object::from(dictionary! { "Type" => "MCR", "Pg" => pid, "MCID" => 0 })] });
                vec![Object::Reference(e)].into()
            }
            "objrDirect" => {
                let e = doc.add_object(dictionary! { "Type" => "StructElem", "S" => "Link", "Alt" => sref("OBJRALT"), "K" => vec![Object::from(dictionary! { "Type" => "OBJR", "Pg" => pid, "Obj" => pages })] });
                vec![Object::Reference(e)].into()
            }
            "directNested" => {
                // a direct element inside a direct element
                let inner = dictionary! { "Type" => "StructElem", "S" => "Span", "Pg" => pid, "ActualText" => sref("NESTEDACT"), "K" => 0 };
                vec![Object::from(dictionary! { "Type" => "StructElem", "S" => "Div", "Alt" => sref("NESTEDOUTER"), "K" => vec![Object::from(inner)] })].into()
            }
            _ => Object::Null,
        };
        let sr = dictionary! { "Type" => "StructTreeRoot", "K" => k };
        let mut cat: Cat = vec![];
        if direct_root {
            cat.push(("StructTreeRoot".to_string(), sr.into()));
        } else {
            doc.objects.insert(sroot, sr.into());
            cat.push(("StructTreeRoot".to_string(), sroot.into()));
        }
        (vec![pid], cat)
    })
}

#[test]
fn sec2_l7_structure_tree() {
    if !can_run() {
        return;
    }
    for (kind, secrets) in [
        ("directElem", vec!["DIRECTALT"]),
        ("kIndirectArray", vec!["INDARRALT"]),
        ("kIndirectElem", vec!["INDELEMALT"]),
        ("container", vec!["CONTAINERALT", "CONTAINERACT"]),
        ("mcr", vec!["MCRALT"]),
        ("inherit", vec!["CHILDEXP"]),
        ("directRoot", vec!["DIRROOTALT"]),
        ("otherPageKid", vec!["OTHERPGALT"]),
        ("objrDirect", vec!["OBJRALT"]),
        ("directNested", vec!["NESTEDACT", "NESTEDOUTER"]),
    ] {
        let n: Vec<&[u8]> = secrets.iter().map(|s| s.as_bytes()).collect();
        assert!(secure(&format!("struct {kind}"), struct_session(kind), [5.0, 770.0, 20.0, 20.0], &n), "{kind} is removable");
    }
}

// ---------- L6: unused alias ----------

#[test]
fn sec2_l6_unused_alias() {
    if !can_run() {
        return;
    }
    let px: Vec<u8> = (0..64).map(|i| 0x21 + ((i * 7 + i / 3) % 90) as u8).collect();
    for (label, inherited) in [("local", false), ("inherited", true)] {
        let s = mkc(|doc, pages, _| {
            let img = doc.add_object(Stream::new(dictionary! { "Type" => "XObject", "Subtype" => "Image", "Width" => 8, "Height" => 8, "ColorSpace" => "DeviceGray", "BitsPerComponent" => 8 }, px.clone()));
            let res = dictionary! { "XObject" => dictionary! { "Im1" => img, "Alias" => img } };
            let cid = doc.add_object(Stream::new(Dictionary::new(), b"q 200 0 0 200 100 500 cm /Im1 Do Q".to_vec()));
            let mut p = dictionary! { "Type" => "Page", "Parent" => pages, "MediaBox" => mbox(), "Contents" => cid };
            p.set("Resources", Object::Dictionary(res.clone()));
            let pid = doc.add_object(p);
            (vec![pid], vec![])
        });
        // the inherited variant is built by hand (the builder cannot reach the /Pages node)
        let s = if inherited {
            let mut doc = Document::load_mem(&s.orig).unwrap();
            let pid = doc.get_pages()[&1];
            let res = doc.get_dictionary(pid).ok().and_then(|p| p.get(b"Resources").ok().cloned());
            let (pages_id, _) = doc.get_dictionary(pid).unwrap().get(b"Parent").unwrap().as_reference().map(|r| (r, ())).unwrap();
            if let Ok(Object::Dictionary(p)) = doc.get_object_mut(pid) {
                p.remove(b"Resources");
            }
            if let (Some(res), Ok(Object::Dictionary(pd))) = (res, doc.get_object_mut(pages_id)) {
                pd.set("Resources", res);
            }
            let mut raw = Vec::new();
            doc.save_to(&mut raw).unwrap();
            let p = std::env::temp_dir().join(format!("majipdf_sec2_{}.pdf", stamp()));
            std::fs::write(&p, &raw).unwrap();
            let s = Session::open(&p, None, None).unwrap().0;
            let _ = std::fs::remove_file(&p);
            s
        } else {
            s
        };
        assert!(secure(&format!("alias {label}"), s, [90.0, 82.0, 230.0, 220.0], &[&px]), "{label}");
    }
}

// ---------- D1: many text objects ----------

#[test]
fn sec2_d1_many_text_objects() {
    if !can_run() {
        return;
    }
    for n in [4000usize, 20_000] {
        let mut c = String::from("BT /F1 4 Tf\n");
        for i in 0..n {
            c.push_str(&format!("1 0 0 1 {} {} Tm (a) Tj\n", 10 + (i % 150) * 4, 780 - (i / 150) * 5));
        }
        c.push_str("ET");
        let mut s = mkc(|doc, pages, font| {
            let cid = doc.add_object(Stream::new(Dictionary::new(), c.clone().into_bytes()));
            (vec![doc.add_object(dictionary! { "Type" => "Page", "Parent" => pages, "MediaBox" => mbox(), "Contents" => cid, "Resources" => dictionary! { "Font" => dictionary! { "F1" => font } } })], vec![])
        });
        let t = std::time::Instant::now();
        let m = s.model(0).unwrap();
        println!("D1 {n} single-glyph text objects: model in {:?}, {} items", t.elapsed(), m.items.len());
        assert!(t.elapsed().as_secs() < 30, "{n} objects took {:?}", t.elapsed());
    }
    // past the show budget the page is locked, not analysed
    let mut c = String::from("BT /F1 4 Tf\n");
    for i in 0..60_000 {
        c.push_str(&format!("1 0 0 1 {} {} Tm (a) Tj\n", 10 + (i % 150) * 4, 780 - (i / 150) * 1));
    }
    c.push_str("ET");
    let mut s = mkc(|doc, pages, font| {
        let cid = doc.add_object(Stream::new(Dictionary::new(), c.clone().into_bytes()));
        (vec![doc.add_object(dictionary! { "Type" => "Page", "Parent" => pages, "MediaBox" => mbox(), "Contents" => cid, "Resources" => dictionary! { "Font" => dictionary! { "F1" => font } } })], vec![])
    });
    let t = std::time::Instant::now();
    assert!(s.page(0).unwrap().broken);
    println!("D1 60k show ops: locked in {:?}", t.elapsed());
    assert!(t.elapsed().as_secs() < 30);
}

// ---------- D4 / O1 ----------

#[test]
fn sec2_d4_huge_image_and_o1_unmeasurable_font() {
    if !can_run() {
        return;
    }
    // an image that claims 12000 x 12000 pixels: not taken apart in memory, the white-out is partial
    let mut s = mkc(|doc, pages, _| {
        let img = doc.add_object(Stream::new(dictionary! { "Type" => "XObject", "Subtype" => "Image", "Width" => 12000, "Height" => 12000, "ColorSpace" => "DeviceGray", "BitsPerComponent" => 8 }, vec![9u8; 100]));
        let cid = doc.add_object(Stream::new(Dictionary::new(), b"q 200 0 0 200 100 500 cm /Im1 Do Q".to_vec()));
        (vec![doc.add_object(dictionary! { "Type" => "Page", "Parent" => pages, "MediaBox" => mbox(), "Contents" => cid, "Resources" => dictionary! { "XObject" => dictionary! { "Im1" => img } } })], vec![])
    });
    let t = std::time::Instant::now();
    let r = s.apply_obj(&req("whiteout", 0, None, [90.0, 82.0, 230.0, 220.0])).unwrap();
    println!("D4 12000x12000 image: {r:?} in {:?}", t.elapsed());
    assert!(matches!(r, ObjRes::WhiteoutPartial { .. }));
    // O1: a Type1 font with no /Widths: nothing is measurable, so removing text under the white-out cannot be proven
    let mut s = mkc(|doc, pages, _| {
        let font = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding" });
        let cid = doc.add_object(Stream::new(Dictionary::new(), b"BT /F1 20 Tf 72 700 Td (NOWIDTHSECRET) Tj ET".to_vec()));
        (vec![doc.add_object(dictionary! { "Type" => "Page", "Parent" => pages, "MediaBox" => mbox(), "Contents" => cid, "Resources" => dictionary! { "Font" => dictionary! { "F1" => font } } })], vec![])
    });
    let r = s.apply_obj(&req("whiteout", 0, None, [60.0, 60.0, 300.0, 50.0])).unwrap();
    println!("O1 text in a font without widths: {r:?}");
    assert!(matches!(r, ObjRes::WhiteoutPartial { .. }));
    // "cover only" still goes through
    let mut q = req("whiteout", 0, None, [60.0, 60.0, 300.0, 50.0]);
    q.cover_only = Some(true);
    assert!(matches!(s.apply_obj(&q).unwrap(), ObjRes::Ok { .. }));
}
