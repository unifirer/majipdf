//! Secure removal beyond the page stream (review round 2): annotations and form fields under a removed area, tagged-PDF alternate
//! text, and resource bindings (inherited or shared) that would keep a removed image reachable. Everything here fails closed:
//! when the correspondence is unclear it strips more, or reports "partial", rather than leaving content behind.
use super::content;
use super::fonts::{deref, effective_resources, num};
use lopdf::{Dictionary, Document, Object, ObjectId};
use std::collections::{BTreeMap, HashMap, HashSet};

fn rect_of(doc: &Document, d: &Dictionary) -> Option<[f64; 4]> {
    let a = deref(doc, d.get(b"Rect").ok()?).as_array().ok()?;
    if a.len() != 4 {
        return None;
    }
    let v: Vec<f64> = a.iter().map(|o| num(deref(doc, o))).collect();
    Some([v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])])
}

/// The page's /Annots entries (reference or direct dictionary), in order.
fn annot_entries(doc: &Document, pid: ObjectId) -> Vec<Object> {
    doc.get_dictionary(pid).ok().and_then(|p| p.get(b"Annots").ok()).and_then(|o| deref(doc, o).as_array().ok()).cloned().unwrap_or_default()
}

fn entry_dict<'a>(doc: &'a Document, o: &'a Object) -> Option<&'a Dictionary> {
    deref(doc, o).as_dict().ok()
}

fn is_widget(d: &Dictionary) -> bool {
    d.get(b"Subtype").ok().and_then(|o| o.as_name().ok()) == Some(b"Widget")
}

// ---------------- form fields ----------------

fn parent_of(doc: &Document, id: ObjectId) -> Option<ObjectId> {
    doc.get_dictionary(id).ok()?.get(b"Parent").ok()?.as_reference().ok()
}

/// The terminal field of a widget: the widget itself when it carries /FT or /V (a merged field), else the nearest ancestor that does.
fn terminal(doc: &Document, w: ObjectId) -> ObjectId {
    let mut cur = w;
    for _ in 0..64 {
        let Ok(d) = doc.get_dictionary(cur) else { return w };
        if d.has(b"FT") || d.has(b"V") {
            return cur;
        }
        match parent_of(doc, cur) {
            Some(p) => cur = p,
            None => return w,
        }
    }
    w
}

fn kids(doc: &Document, id: ObjectId) -> Vec<ObjectId> {
    doc.get_dictionary(id).ok().and_then(|d| d.get(b"Kids").ok()).and_then(|o| deref(doc, o).as_array().ok()).map(|a| a.iter().filter_map(|o| o.as_reference().ok()).collect()).unwrap_or_default()
}

/// The widgets (leaves) under a field node; a node without kids is its own leaf.
/// Iterative with a visited set: field trees in hostile PDFs can be cyclic or very deep (recursion overflowed the stack).
fn leaves(doc: &Document, id: ObjectId, out: &mut Vec<ObjectId>, budget: &mut usize) {
    let mut seen = HashSet::new();
    let mut stack = vec![id];
    while let Some(n) = stack.pop() {
        if *budget == 0 {
            return;
        }
        if !seen.insert(n) {
            continue;
        }
        *budget -= 1;
        let k = kids(doc, n);
        if k.is_empty() {
            out.push(n);
        }
        stack.extend(k.into_iter().rev());
    }
}

/// Indices of annotations that lie wholly inside one of `rects` (user space x0 y0 x1 y1), and how many only overlap one partly.
/// A widget whose field also has widgets elsewhere (another page, outside the rect) counts as partial: the field value would survive.
/// An annotation without a usable /Rect counts as partly inside (it can not be placed).
pub fn find_annots(doc: &Document, pid: ObjectId, rects: &[[f64; 4]]) -> (HashSet<usize>, usize) {
    let (mut inside, mut partial) = (HashSet::new(), 0);
    let entries = annot_entries(doc, pid);
    for (i, e) in entries.iter().enumerate() {
        let Some(d) = entry_dict(doc, e) else { continue };
        if d.get(b"Subtype").ok().and_then(|o| o.as_name().ok()) == Some(b"Popup") {
            continue; // goes with its parent
        }
        let Some(r) = rect_of(doc, d) else {
            partial += 1;
            continue;
        };
        let overlaps = |w: &[f64; 4]| r[0] < w[2] && w[0] < r[2] && r[1] < w[3] && w[1] < r[3];
        let within = |w: &[f64; 4]| r[0] >= w[0] - 0.5 && r[1] >= w[1] - 0.5 && r[2] <= w[2] + 0.5 && r[3] <= w[3] + 0.5;
        if rects.iter().any(within) {
            inside.insert(i);
        } else if rects.iter().any(overlaps) {
            partial += 1;
        }
    }
    let ids_in: HashSet<ObjectId> = inside.iter().filter_map(|i| entries[*i].as_reference().ok()).collect();
    for i in inside.clone() {
        let Object::Reference(id) = &entries[i] else { continue };
        if !doc.get_dictionary(*id).is_ok_and(is_widget) {
            continue;
        }
        let (mut ls, mut budget) = (vec![], 100_000usize);
        leaves(doc, terminal(doc, *id), &mut ls, &mut budget);
        if !ls.iter().all(|l| ids_in.contains(l)) {
            inside.remove(&i);
            partial += 1;
        }
    }
    (inside, partial)
}

/// Removes refs to `gone` from every array, and the keys that point at a removed object (/Popup, /IRT, /Parent, /Obj), everywhere.
fn scrub_refs(o: &mut Object, gone: &HashSet<ObjectId>, depth: u32) {
    if depth > 12 {
        return;
    }
    match o {
        Object::Array(a) => {
            a.retain(|x| !matches!(x, Object::Reference(id) if gone.contains(id)));
            a.iter_mut().for_each(|x| scrub_refs(x, gone, depth + 1));
        }
        Object::Dictionary(d) => scrub_dict(d, gone, depth),
        Object::Stream(s) => scrub_dict(&mut s.dict, gone, depth),
        _ => {}
    }
}

fn scrub_dict(d: &mut Dictionary, gone: &HashSet<ObjectId>, depth: u32) {
    for k in [&b"Popup"[..], b"IRT", b"Parent", b"Obj"] {
        if matches!(d.get(k), Ok(Object::Reference(id)) if gone.contains(id)) {
            d.remove(k);
        }
    }
    let keys: Vec<Vec<u8>> = d.iter().map(|(k, _)| k.clone()).collect();
    for k in keys {
        if let Ok(v) = d.get_mut(&k) {
            scrub_refs(v, gone, depth + 1);
        }
    }
}

/// Takes the annotations out of the page: replies (/IRT, recursively) and pop-ups with them; for a widget, its whole terminal field
/// (every widget of it is going) from /Fields, /Kids and /CO. Every other reference to a removed object is dropped, and the values
/// (/V /DV /Contents /RC) are cleared on the removed nodes. Returns the ids of everything removed.
pub fn remove_annots(doc: &mut Document, pid: ObjectId, idx: &HashSet<usize>) -> HashSet<ObjectId> {
    let entries = annot_entries(doc, pid);
    let mut drop_idx: HashSet<usize> = idx.clone();
    let mut gone: HashSet<ObjectId> = HashSet::new();
    for i in idx {
        if let Some(Object::Reference(id)) = entries.get(*i) {
            gone.insert(*id);
        }
    }
    // replies, to a fixed point (a reply to a reply)
    loop {
        let mut changed = false;
        for (i, e) in entries.iter().enumerate() {
            if drop_idx.contains(&i) {
                continue;
            }
            let reply = entry_dict(doc, e).and_then(|d| d.get(b"IRT").ok()).and_then(|o| o.as_reference().ok()).is_some_and(|r| gone.contains(&r));
            if reply {
                drop_idx.insert(i);
                if let Object::Reference(id) = e {
                    gone.insert(*id);
                }
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    // pop-ups, and the terminal fields of removed widgets with their whole subtree
    let seeds: Vec<ObjectId> = gone.iter().copied().collect();
    for id in seeds {
        let Ok(d) = doc.get_dictionary(id) else { continue };
        if let Ok(Object::Reference(p)) = d.get(b"Popup") {
            gone.insert(*p);
        }
        if is_widget(d) {
            let t = terminal(doc, id);
            let mut stack = vec![t];
            while let Some(n) = stack.pop() {
                if gone.insert(n) || n == t {
                    stack.extend(kids(doc, n));
                }
                if stack.len() > 100_000 {
                    break;
                }
            }
        }
    }
    // an intermediate field node that has no kids left goes too
    loop {
        let parents: HashSet<ObjectId> = gone.iter().filter_map(|g| parent_of(doc, *g)).filter(|p| !gone.contains(p)).collect();
        let empty: Vec<ObjectId> = parents.into_iter().filter(|p| kids(doc, *p).iter().all(|k| gone.contains(k))).filter(|p| !kids(doc, *p).is_empty()).collect();
        if empty.is_empty() {
            break;
        }
        gone.extend(empty);
    }
    // the page's own array first (direct annotations go by index), then every other reference
    let kept: Vec<Object> = entries.iter().enumerate().filter(|(i, e)| !drop_idx.contains(i) && !matches!(e, Object::Reference(id) if gone.contains(id))).map(|(_, e)| e.clone()).collect();
    if let Ok(Object::Dictionary(pd)) = doc.get_object_mut(pid) {
        pd.set("Annots", Object::Array(kept));
    }
    for id in &gone {
        if let Some(Object::Dictionary(d)) = doc.objects.get_mut(id) {
            for k in [&b"V"[..], b"DV", b"Contents", b"RC"] {
                d.remove(k);
            }
        }
    }
    let ids: Vec<ObjectId> = doc.objects.keys().copied().filter(|i| !gone.contains(i)).collect();
    for id in ids {
        if let Some(o) = doc.objects.get_mut(&id) {
            scrub_refs(o, &gone, 0);
        }
    }
    doc.trailer.iter_mut().for_each(|(_, v)| scrub_refs(v, &gone, 0));
    gone
}

// ---------------- structure tree ----------------

const ALT_KEYS: [&[u8]; 3] = [b"ActualText", b"Alt", b"E"];

#[derive(Clone, Debug)]
enum Step {
    Key(Vec<u8>),
    Idx(usize),
}

/// A structure element: the indirect object that holds it and the path from there (a direct element sits inside its parent's object).
type Spot = (ObjectId, Vec<Step>);

fn pg_of(d: &Dictionary) -> Option<ObjectId> {
    d.get(b"Pg").ok().and_then(|o| o.as_reference().ok())
}

fn dict_at<'a>(doc: &'a Document, spot: &Spot) -> Option<&'a Dictionary> {
    let mut cur: &Object = doc.objects.get(&spot.0)?;
    for st in &spot.1 {
        cur = match (st, cur) {
            (Step::Key(k), Object::Dictionary(d)) => d.get(k).ok()?,
            (Step::Key(k), Object::Stream(s)) => s.dict.get(k).ok()?,
            (Step::Idx(i), Object::Array(a)) => a.get(*i)?,
            _ => return None,
        };
    }
    match cur {
        Object::Dictionary(d) => Some(d),
        Object::Stream(s) => Some(&s.dict),
        _ => None,
    }
}

fn dict_at_mut<'a>(doc: &'a mut Document, spot: &Spot) -> Option<&'a mut Dictionary> {
    let mut cur: &mut Object = doc.objects.get_mut(&spot.0)?;
    for st in &spot.1 {
        cur = match (st, cur) {
            (Step::Key(k), Object::Dictionary(d)) => d.get_mut(k).ok()?,
            (Step::Key(k), Object::Stream(s)) => s.dict.get_mut(k).ok()?,
            (Step::Idx(i), Object::Array(a)) => a.get_mut(*i)?,
            _ => return None,
        };
    }
    match cur {
        Object::Dictionary(d) => Some(d),
        Object::Stream(s) => Some(&mut s.dict),
        _ => None,
    }
}

struct Walk<'a> {
    doc: &'a Document,
    pids: &'a HashSet<ObjectId>,
    seen: HashSet<ObjectId>,
    budget: usize,
    out: Vec<Spot>,
}

impl Walk<'_> {
    /// True when the node (or something below it) belongs to one of the pages; elements that do are collected, ancestors included.
    fn visit(&mut self, node: &Object, holder: ObjectId, path: Vec<Step>, inh: Option<ObjectId>, depth: u32) -> bool {
        if depth > 300 || self.budget == 0 {
            return false;
        }
        self.budget -= 1;
        let (holder, path, obj) = match node {
            Object::Reference(id) => {
                if !self.seen.insert(*id) {
                    return false;
                }
                let Ok(o) = self.doc.get_object(*id) else { return false };
                (*id, vec![], o)
            }
            o => (holder, path, o),
        };
        match obj {
            Object::Array(a) => {
                let mut t = false;
                for (i, k) in a.iter().enumerate() {
                    let mut p = path.clone();
                    p.push(Step::Idx(i));
                    t |= self.visit(k, holder, p, inh, depth + 1);
                }
                t
            }
            Object::Dictionary(d) => {
                let ty = d.get(b"Type").ok().and_then(|o| o.as_name().ok());
                let pg = pg_of(d).or(inh);
                if matches!(ty, Some(b"MCR") | Some(b"OBJR")) {
                    return pg.is_some_and(|p| self.pids.contains(&p));
                }
                let mut touched = pg.is_some_and(|p| self.pids.contains(&p));
                if let Some(k) = d.get(b"K").ok() {
                    let mut p = path.clone();
                    p.push(Step::Key(b"K".to_vec()));
                    let k = k.clone();
                    touched |= self.visit(&k, holder, p, pg, depth + 1);
                }
                if touched && ty != Some(b"StructTreeRoot") && (d.has(b"S") || ty == Some(b"StructElem")) {
                    self.out.push((holder, path));
                }
                touched
            }
            _ => false,
        }
    }
}

/// Every structure element that belongs to one of `pids`, the whole tree walked: its own /Pg, an inherited /Pg, a marked-content
/// or object kid's /Pg, or a descendant that qualifies (so ancestors are included). Direct and indirect elements, indirect /K arrays.
fn struct_touched(doc: &Document, pids: &HashSet<ObjectId>) -> Vec<Spot> {
    let Some(root) = doc.trailer.get(b"Root").ok().and_then(|o| o.as_reference().ok()) else { return vec![] };
    let Some(sroot) = doc.get_dictionary(root).ok().and_then(|c| c.get(b"StructTreeRoot").ok()).cloned() else { return vec![] };
    let mut w = Walk { doc, pids, seen: HashSet::new(), budget: 3_000_000, out: vec![] };
    let path = if matches!(sroot, Object::Reference(_)) { vec![] } else { vec![Step::Key(b"StructTreeRoot".to_vec())] };
    w.visit(&sroot, root, path, None, 0);
    w.out
}

/// How many of those elements still carry alternate text. The same walk as the strip, so the check is never blind.
pub fn alt_left(doc: &Document, pids: &HashSet<ObjectId>) -> usize {
    struct_touched(doc, pids).iter().filter(|s| dict_at(doc, s).is_some_and(|d| ALT_KEYS.iter().any(|k| d.has(k)))).count()
}

/// Strips /ActualText, /Alt and /E from those elements.
pub fn strip_alt(doc: &mut Document, pids: &HashSet<ObjectId>) {
    for spot in struct_touched(doc, pids) {
        if let Some(d) = dict_at_mut(doc, &spot) {
            for k in ALT_KEYS {
                d.remove(k);
            }
        }
    }
}

/// Alternate-text keys out of the marked-content property lists of a page (the /Properties resources, direct or indirect).
pub fn strip_properties(doc: &mut Document, pid: ObjectId) {
    let Some(mut res) = effective_resources(doc, pid).cloned() else { return };
    let Some(props) = res.get(b"Properties").ok().map(|o| deref(doc, o).clone()) else { return };
    let Object::Dictionary(mut pd) = props else { return };
    let mut touched = false;
    let keys: Vec<Vec<u8>> = pd.iter().map(|(k, _)| k.clone()).collect();
    for k in keys {
        match pd.get(&k).cloned() {
            Ok(Object::Reference(id)) => {
                if let Ok(Object::Dictionary(d)) = doc.get_object_mut(id) {
                    for a in ALT_KEYS {
                        d.remove(a);
                    }
                }
            }
            Ok(Object::Dictionary(mut d)) => {
                for a in ALT_KEYS {
                    d.remove(a);
                }
                pd.set(k, Object::Dictionary(d));
                touched = true;
            }
            _ => {}
        }
    }
    if touched {
        res.set("Properties", Object::Dictionary(pd));
        if let Ok(Object::Dictionary(p)) = doc.get_object_mut(pid) {
            p.set("Resources", Object::Dictionary(res));
        }
    }
}

// ---------------- resources ----------------

/// Who holds a page's resource dictionary in the ORIGINAL document: the shared indirect object, else the page or ancestor node itself.
fn owner_key(doc: &Document, pid: ObjectId) -> Option<(ObjectId, bool)> {
    let mut node = pid;
    for _ in 0..32 {
        let d = doc.get_dictionary(node).ok()?;
        if let Ok(r) = d.get(b"Resources") {
            return Some(match r {
                Object::Reference(id) => (*id, false),
                _ => (node, node != pid),
            });
        }
        node = d.get(b"Parent").ok()?.as_reference().ok()?;
    }
    None
}

/// An image removed from `edited` pages must not stay reachable through resources that are inherited from a /Pages node or shared
/// with other pages. Every other page that holds the same resources gets its own copy without the removed names (unless it draws
/// them, or draws something we can not classify), and an inherited dictionary is then dropped. `names`: edited page -> removed names.
pub fn detach_removed(orig: &Document, doc: &mut Document, names: &[(ObjectId, Vec<String>)]) {
    let mut by_key: BTreeMap<(ObjectId, bool), HashSet<String>> = BTreeMap::new();
    let edited: HashSet<ObjectId> = names.iter().map(|n| n.0).collect();
    for (pid, ns) in names {
        if let Some(k) = owner_key(orig, *pid) {
            by_key.entry(k).or_default().extend(ns.iter().cloned());
        }
    }
    if by_key.is_empty() {
        return;
    }
    let pages: Vec<ObjectId> = orig.page_iter().collect();
    let mut done: HashMap<ObjectId, Dictionary> = HashMap::new();
    for q in pages {
        if edited.contains(&q) {
            continue;
        }
        let Some(key) = owner_key(orig, q) else { continue };
        let Some(gone) = by_key.get(&key) else { continue };
        let Some(res) = effective_resources(orig, q).cloned() else { continue };
        let xo = res.get(b"XObject").ok().and_then(|o| deref(orig, o).as_dict().ok()).cloned().unwrap_or_default();
        let drawn = super::fonts::page_content(orig, q).map(|c| content::do_names(&c)).ok();
        let is_image = |n: &str| {
            xo.get(n.as_bytes()).ok().map(|o| deref(orig, o)).is_some_and(|o| matches!(o, Object::Stream(s) if s.dict.get(b"Subtype").ok().and_then(|t| t.as_name().ok()) == Some(b"Image")))
        };
        // drop a name only when the page's content is readable and draws nothing but images
        let may_drop = drawn.as_ref().is_some_and(|dn| dn.iter().all(|n| is_image(n)));
        let mut copy = res.clone();
        let mut xd = xo.clone();
        if may_drop {
            let ids: HashSet<ObjectId> = gone.iter().filter_map(|n| xo.get(n.as_bytes()).ok().and_then(|o| o.as_reference().ok())).collect();
            for (k, v) in xo.iter() {
                let n = String::from_utf8_lossy(k).to_string();
                let same = v.as_reference().ok().is_some_and(|id| ids.contains(&id));
                if (gone.contains(&n) || same) && !drawn.as_ref().unwrap().contains(&n) {
                    xd.remove(k);
                }
            }
        }
        copy.set("XObject", Object::Dictionary(xd));
        done.insert(q, copy);
    }
    for (q, res) in done {
        if let Ok(Object::Dictionary(pd)) = doc.get_object_mut(q) {
            pd.set("Resources", Object::Dictionary(res));
        }
    }
    // the holders that were inherited nodes no longer need (or may keep) their dictionary
    for ((holder, inherited), _) in by_key {
        if inherited {
            if let Ok(Object::Dictionary(d)) = doc.get_object_mut(holder) {
                d.remove(b"Resources");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{dictionary, Object};

    #[test]
    fn cyclic_field_tree_terminates() {
        // A <-> B as each other's /Kids and /Parent, plus a widget under A: used to overflow the stack.
        let mut doc = Document::with_version("1.7");
        let (a, b, w) = (doc.new_object_id(), doc.new_object_id(), doc.new_object_id());
        doc.objects.insert(a, Object::Dictionary(dictionary! { "FT" => "Tx", "Kids" => vec![Object::Reference(b), Object::Reference(w)], "Parent" => Object::Reference(b) }));
        doc.objects.insert(b, Object::Dictionary(dictionary! { "Kids" => vec![Object::Reference(a)], "Parent" => Object::Reference(a) }));
        doc.objects.insert(w, Object::Dictionary(dictionary! { "Subtype" => "Widget", "Parent" => Object::Reference(a) }));
        let (mut out, mut budget) = (vec![], 100_000usize);
        leaves(&doc, a, &mut out, &mut budget);
        assert_eq!(out, vec![w]);
        assert_eq!(terminal(&doc, w), a);
    }
}
