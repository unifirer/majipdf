// DEV-ONLY fake backend for the Edit PDF tool in a plain browser (no Tauri). Imported lazily from edit.ts
// behind `import.meta.env.DEV && !inTauri`; delete this file plus the `mock` branches in edit.ts to remove it.
// All text is made up. Magic inputs: "@" -> missing_chars (and bold on a direct paragraph without the PC font), "Ω" -> unsupported_chars, "OVERFLOW" -> overflow, "PUSH" -> cannot_push.
// White-out over a locked / PC-font paragraph -> whiteout_partial (unless cover_only).
import type { EditOpenReq, EditApplyReq, EditApplyRes, EditDoc, EditHistory, EditSaveReq, EditSaveRes, ObjReq, ObjRes, Obj, PageModel, Para, Rect, Run, SigItem, Style, Item, ObjEdit3, BatchRes, WmReq, HfReq, DocRes, DocObjs } from "./edit";
import { parseRanges } from "./split";

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const W = 595;
const H = 842;
const LOREM =
  "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat.";

const para = (id: number, y: number, text: string, o: Partial<Para> = {}): Para => {
  const p: Para = {
    id, bbox: [72, y, 451, 14], text, marker: null, align: "left", size: 11, pitch: 14, first_indent: 0, css_font: "Arial",
    bold: false, italic: false, color: "#000000", status: "direct", reason: null, edited: false, pc_font_used: false, rot: 0,
    style: { font: "orig", size: 11, color: "#000000", bold: false, italic: false, align: "left" }, runs: [], orig_font_name: "", ...o,
  };
  p.orig_font_name = p.css_font + "MT";
  p.style = { font: p.status === "direct" ? "orig" : "Arial", size: p.size, color: p.color, bold: p.bold, italic: p.italic, align: p.align };
  p.runs = [{ text, style: { ...p.style, font: p.css_font } }]; // like the real backend: a CSS family here, the frontend maps it to "orig"
  return p;
};
const fit = (p: Para) => {
  if (p.rot === 90 || p.rot === 270) return p; // sideways: the box is fixed in this mock
  const perLine = Math.max(8, Math.floor(p.bbox[2] / (p.size * 0.5)));
  p.bbox[3] = Math.max(1, Math.ceil(p.text.length / perLine)) * p.pitch;
  return p;
};
// Existing images and paths (Edit 3). `art` is mock-only: how the fake renderer draws the item. Path 40 is "tied to other content" (move -> cannot_move).
type Art = Item & { art: string; tied?: boolean };
const img = (id: number, bbox: Rect, art: string, locked: string | null = null): Art => ({ id, kind: "image", bbox, rot: 0, locked, art });
const line = (id: number, bbox: Rect, art = "line", tied = false): Art => ({ id, kind: "path", bbox, rot: 0, locked: null, art, tied });
const tableLines = (): Art[] => {
  const out: Art[] = [];
  [520, 550, 580, 610].forEach((y, k) => out.push(line(10 + k, [72, y - 0.25, 451, 0.5])));
  [72, 184.75, 297.5, 410.25, 523].forEach((x, k) => out.push(line(20 + k, [x - 0.25, 520, 0.5, 90])));
  for (let k = 0; k < 10; k++) out.push(line(30 + k, [72 + k * 24, 650, 12, 0.5], "dash")); // tiny pieces of one dashed rule
  out.push(line(40, [330, 700, 90, 0.5], "line", true));
  out.push(line(41, [72, 680, 120, 40], "shape"));
  return out;
};
const initial = (): PageModel[] => [
  {
    page: 0, scan: false, objs: [], wm_forced_front: false,
    items: [img(1, [430, 26, 90, 44], "logo"), img(2, [250, 738, 90, 40], "stamp", "form"), ...tableLines()],
    paras: [
      para(0, 80, "Laporan Contoh Kegiatan Tahunan", { align: "center", size: 18, pitch: 22, bold: true, bbox: [72, 80, 451, 22] }),
      para(1, 130, LOREM, { align: "justified", css_font: "Times New Roman" }),
      para(2, 215, LOREM.slice(40) + " " + LOREM.slice(0, 120), { align: "justified", css_font: "Times New Roman" }),
      para(3, 300, "Paragraf ini memakai font yang tidak bisa ditulisi, contoh untuk tombol font komputer.", { status: "pc_font", css_font: "Calibri" }),
      para(4, 345, "Teks miring tidak bisa disunting (rotated).", { status: "locked", reason: "rotated", italic: true }),
      para(6, 120, "Teks vertikal contoh yang dibaca dari bawah ke atas", { rot: 90, bbox: [566, 120, 26, 220], css_font: "Arial" }),
      para(7, 6, "Kotak di tepi atas", { bbox: [72, 6, 200, 14] }),
      para(8, 420, "Kotak di tepi kanan", { bbox: [440, 420, 150, 14], align: "right" }),
      para(9, 450, "Kotak di tepi kiri", { bbox: [2, 450, 150, 14] }),
      para(10, 400, "Vertikal tepi kiri dibaca dari atas ke bawah", { rot: 270, bbox: [3, 480, 26, 220] }),
      para(5, 790, "Halaman 1 dari 2 (footer berulang)", { status: "locked", reason: "form", size: 9, pitch: 11, align: "center" }),
    ].map(fit),
  },
  {
    page: 1, scan: false, objs: [], wm_forced_front: false,
    items: [img(1, [72, 330, 220, 140], "photo"), line(10, [72, 500, 451, 0.5]), line(11, [72, 520, 451, 0.5])],
    paras: [
      para(0, 80, "Lampiran", { size: 16, pitch: 20, bold: true, bbox: [72, 80, 451, 20] }),
      para(1, 120, LOREM.slice(0, 160), { marker: "1.", bbox: [96, 120, 427, 14] }),
      para(2, 175, LOREM.slice(60, 230), { marker: "2.", bbox: [96, 175, 427, 14] }),
      para(3, 240, "Struktur halaman ini tidak dikenali.", { status: "locked", reason: "structure" }),
      para(5, 818, "Kotak di tepi bawah halaman terakhir", { bbox: [72, 822, 300, 14] }),
      para(6, 600, "Vertikal tepi bawah kanan", { rot: 90, bbox: [566, 560, 26, 270] }),
      para(4, 790, "Halaman 2 dari 2 (footer berulang)", { status: "locked", reason: "form", size: 9, pitch: 11, align: "center" }),
    ].map(fit),
  },
];

// ---- history: a snapshot per step (the mock has no byte-level undo) ----
// Edit 3: `pages[i].items` holds only the images and paths (with a mock-only `art`); paragraph items are derived by mdl().
type Doc = { wm: WmReq | null; hfs: HfReq[] };
let pages: PageModel[] = initial();
let doc: Doc = { wm: null, hfs: [] };
const snap = () => JSON.stringify({ pages, doc });
let snaps: string[] = [snap()];
let cur = 0;
let nextId = 100;

const push = () => {
  snaps = snaps.slice(0, cur + 1);
  snaps.push(snap());
  cur++;
};
const restore = (s: string) => ({ pages, doc } = JSON.parse(s) as { pages: PageModel[]; doc: Doc });
const hist = (): EditHistory => ({
  can_undo: cur > 0,
  can_redo: cur < snaps.length - 1,
  count: cur,
  changed_pages: pages.map((p, i) => i).filter((i) => JSON.stringify((JSON.parse(snaps[0]).pages as PageModel[])[i]) !== JSON.stringify(pages[i])),
});

// What the backend reports for a page: paragraph items (same ids) plus the stored images/paths; forced-front flag for the scan-like page 2.
const mdl = (i: number): PageModel => {
  const p = pages[i];
  const paraItems: Item[] = p.paras.map((x) => ({ id: x.id, kind: "para", bbox: [...x.bbox] as Rect, rot: x.rot, locked: x.status === "locked" ? (x.reason ?? "structure") : null }));
  return { ...structuredClone(p), items: [...paraItems, ...structuredClone(p.items)], wm_forced_front: !!doc.wm && doc.wm.layer === "behind" && i === 1 };
};

export async function open(_r: EditOpenReq): Promise<EditDoc> {
  await sleep(500);
  pages = initial();
  doc = { wm: null, hfs: [] };
  snaps = [snap()];
  cur = 0;
  return { pages: 2, sizes: [[W, H], [W, H]] };
}
export async function page(i: number): Promise<PageModel> {
  await sleep(60);
  return mdl(i);
}

// ---- library ----
const FONTS = ["Arial", "Times New Roman", "Calibri", "Tahoma", "Verdana", "Courier New", "Bookman Old Style"];
export const fonts = async () => [...FONTS];
const svgUrl = (inner: string, w: number, h: number) => "data:image/svg+xml;utf8," + encodeURIComponent(`<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}">${inner}</svg>`);
const sigArt = (c: string, v: number) =>
  svgUrl(`<path d="M12 ${78 + v} C40 ${10 + v} 60 ${100 - v} 84 ${52 + v} S130 ${20 + v} 150 ${70 + v} S200 ${90 - v} 214 ${40 + v} S262 ${30 + v} 288 ${60 + v}" fill="none" stroke="${c}" stroke-width="4" stroke-linecap="round"/><path d="M20 ${100} L280 ${96 - v}" stroke="${c}" stroke-width="2.5" fill="none" stroke-linecap="round"/>`, 300, 120);
const stampArt = svgUrl(`<rect x="8" y="8" width="224" height="224" rx="20" fill="none" stroke="#C00000" stroke-width="8"/><text x="120" y="132" font-family="Arial" font-weight="700" font-size="44" fill="#C00000" text-anchor="middle">DITERIMA</text>`, 240, 240);
let lib: SigItem[] = [
  { id: "a1b2c3d4e5f60001", name: "Tanda tangan saya", kind: "signature", w: 300, h: 120, data_url: sigArt("#1F3864", 0) },
  { id: "a1b2c3d4e5f60002", name: "Paraf", kind: "signature", w: 300, h: 120, data_url: sigArt("#111111", 14) },
  { id: "a1b2c3d4e5f60003", name: "Stempel diterima", kind: "stamp", w: 240, h: 240, data_url: stampArt },
];
const assetUrl = new Map(lib.map((x) => [x.id, x.data_url]));
const addItem = (name: string, kind: SigItem["kind"], data_url: string, w: number, h: number) => {
  const it: SigItem = { id: "m" + (nextId++).toString(16).padStart(15, "0"), name, kind, w, h, data_url };
  lib.push(it);
  assetUrl.set(it.id, data_url);
  return it;
};
export const sigList = async (): Promise<SigItem[]> => structuredClone(lib);
export async function sigImport(path: string, kind: SigItem["kind"]): Promise<SigItem> {
  await sleep(500);
  const name = path.split(/[\\/]/).pop()!.replace(/\.\w+$/, "");
  return structuredClone(kind === "stamp" ? addItem(name, kind, stampArt, 240, 240) : addItem(name, kind, sigArt("#2a5f2a", 8), 300, 120));
}
export async function sigAddDrawn(dataUrl: string, name: string): Promise<SigItem> {
  await sleep(250);
  const img = new Image();
  img.src = dataUrl;
  await img.decode().catch(() => {});
  return structuredClone(addItem(name, "signature", dataUrl, img.naturalWidth || 600, img.naturalHeight || 240));
}
export async function sigRename(id: string, name: string) {
  await sleep(100);
  const it = lib.find((x) => x.id === id);
  if (it) it.name = name;
}
export async function sigClear() {
  await sleep(100);
  lib = [];
}
export async function sigDelete(id: string) {
  await sleep(100);
  lib = lib.filter((x) => x.id !== id);
}

// ---- render ----
const esc = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
const boxLines = (text: string, size: number, w: number) => {
  const per = Math.max(4, Math.floor(w / (size * 0.5)));
  const out: string[] = [];
  for (const raw of text.split("\n")) {
    let line = "";
    for (const word of raw.split(" ")) {
      if (line && (line + " " + word).length > per) (out.push(line), (line = word));
      else line = line ? line + " " + word : word;
    }
    out.push(line);
  }
  return out;
};
const objSvg = (o: Obj) => {
  const [x, y, w, h] = o.rect;
  if (o.kind === "whiteout") return `<rect x="${x}" y="${y}" width="${w}" height="${h}" fill="#fff"/>`;
  if (o.kind === "image") return `<image href="${esc(assetUrl.get(o.asset ?? "") ?? "")}" x="${x}" y="${y}" width="${w}" height="${h}" preserveAspectRatio="none"/>`;
  const st = o.style!;
  const runs: Run[] = o.runs?.length ? o.runs : [{ text: o.text ?? "", style: st }];
  const big = Math.max(...runs.map((r) => r.style.size));
  const full = runs.map((r) => r.text).join("");
  const lines = boxLines(full, big, w);
  let idx = 0;
  const tx2 = lines
    .map((l, i) => {
      const at = Math.max(idx, full.indexOf(l, idx));
      idx = at + l.length;
      let pos = 0;
      const tsp = runs
        .map((r) => {
          const a = Math.max(at, pos);
          const b = Math.min(at + l.length, pos + r.text.length);
          const piece = b > a ? full.slice(a, b) : "";
          pos += r.text.length;
          return piece ? `<tspan font-size="${r.style.size}" fill="${r.style.color}" font-weight="${r.style.bold ? 700 : 400}" font-style="${r.style.italic ? "italic" : "normal"}" xml:space="preserve">${esc(piece)}</tspan>` : "";
        })
        .join("");
      return `<text x="${st.align === "center" ? x + w / 2 : st.align === "right" ? x + w : x}" y="${y + (i + 0.8) * big * 1.15}" text-anchor="${st.align === "center" ? "middle" : st.align === "right" ? "end" : "start"}">${tsp}</text>`;
    })
    .join("");
  if (o.runs?.length) return `<g font-family="${esc(st.font === "orig" ? "Arial" : st.font)}">${tx2}</g>`;
  const ax = st.align === "center" ? x + w / 2 : st.align === "right" ? x + w : x;
  const anchor = st.align === "center" ? "middle" : st.align === "right" ? "end" : "start";
  const fam = st.font === "orig" ? "Arial" : st.font;
  const tx = lines.map((l, i) => `<text x="${ax}" y="${y + (i + 0.8) * st.size * 1.15}" text-anchor="${anchor}">${esc(l)}</text>`).join("");
  return `<g font-family="${esc(fam)}" font-size="${st.size}" font-weight="${st.bold ? 700 : 400}" font-style="${st.italic ? "italic" : "normal"}" fill="${st.color}">${tx}</g>`;
};
const artSvg = (a: Art) => {
  const [x, y, w, h] = a.bbox;
  if (a.art === "logo") return `<g><rect x="${x}" y="${y}" width="${w}" height="${h}" rx="4" fill="#2f6f8f"/><text x="${x + w / 2}" y="${y + h / 2 + 5}" font-family="Arial" font-weight="700" font-size="16" fill="#fff" text-anchor="middle">LOGO</text></g>`;
  if (a.art === "stamp") return `<rect x="${x}" y="${y}" width="${w}" height="${h}" fill="none" stroke="#B45309" stroke-width="2"/><text x="${x + w / 2}" y="${y + h / 2 + 4}" font-family="Arial" font-size="11" fill="#B45309" text-anchor="middle">FORM STAMP</text>`;
  if (a.art === "photo") return `<rect x="${x}" y="${y}" width="${w}" height="${h}" fill="#bcd3c4"/><path d="M${x} ${y + h} L${x + w * 0.35} ${y + h * 0.35} L${x + w * 0.6} ${y + h * 0.75} L${x + w * 0.8} ${y + h * 0.5} L${x + w} ${y + h} Z" fill="#6f9a7e"/><circle cx="${x + w * 0.8}" cy="${y + h * 0.22}" r="${h * 0.1}" fill="#f2d16b"/>`;
  if (a.art === "shape") return `<rect x="${x}" y="${y}" width="${w}" height="${h}" fill="#E3EEF5" stroke="#35607f" stroke-width="1"/>`;
  return `<rect x="${x}" y="${y}" width="${Math.max(w, 0.5)}" height="${Math.max(h, 0.5)}" fill="${a.art === "dash" ? "#a0522d" : "#222"}"/>`;
};
const wmSvg = (w: WmReq, i: number) => {
  if (w.pages && !pageIn(w.pages, i)) return "";
  const cx = W / 2;
  const cy = H / 2;
  const a = (w.angle * Math.PI) / 180;
  if (w.kind === "image") {
    const iw = (W * (w.scale ?? 50)) / 100;
    const src = assetUrl.get(w.asset ?? "") ?? "";
    return `<g opacity="${w.opacity}" transform="rotate(${-w.angle} ${cx} ${cy})"><image href="${esc(src)}" x="${cx - iw / 2}" y="${cy - iw / 2}" width="${iw}" height="${iw}" preserveAspectRatio="xMidYMid meet"/></g>`;
  }
  const st = w.style!;
  const len = Math.max(1, (w.text ?? "").length);
  const lmax = 0.9 * Math.min(W / Math.max(Math.abs(Math.cos(a)), 0.01), H / Math.max(Math.abs(Math.sin(a)), 0.01));
  const fs = st.size > 0 ? st.size : Math.min(W * 0.15, lmax / (len * 0.66));
  return `<text x="${cx}" y="${cy + fs * 0.35}" transform="rotate(${-w.angle} ${cx} ${cy})" text-anchor="middle" font-family="${esc(st.font)}" font-size="${fs}" font-weight="${st.bold ? 700 : 400}" font-style="${st.italic ? "italic" : "normal"}" fill="${st.color}" opacity="${w.opacity}">${esc(w.text ?? "")}</text>`;
};
const pageIn = (ranges: string, i: number) => {
  const r = parseRanges(ranges, 2);
  return "groups" in r && r.groups.some((g) => i + 1 >= g.from && i + 1 <= g.to);
};
const hfSvg = (i: number) =>
  doc.hfs
    .map((h) => {
      const inc = [0, 1].filter((p) => !h.pages || pageIn(h.pages, p));
      const idx = inc.indexOf(i);
      if (idx < 0 || (h.skip_first && idx === 0)) return "";
      const date = new Date().toLocaleDateString(h.lang === "id" ? "id-ID" : "en-GB", { day: "numeric", month: "long", year: "numeric" });
      const txt = h.template.replaceAll("{page}", String(h.start + idx)).replaceAll("{total}", String(inc.length + h.start - 1)).replaceAll("{date}", date).replaceAll("{date_id}", new Date().toLocaleDateString("id-ID", { day: "numeric", month: "long", year: "numeric" })).replaceAll("{date_en}", new Date().toLocaleDateString("en-GB", { day: "numeric", month: "long", year: "numeric" })).replaceAll("{file}", "Laporan_Contoh");
      const x = h.pos[1] === "l" ? h.margin_pt : h.pos[1] === "c" ? W / 2 : W - h.margin_pt;
      const y = h.pos[0] === "t" ? h.margin_pt + h.style.size * 0.8 : H - h.margin_pt;
      const an = h.pos[1] === "l" ? "start" : h.pos[1] === "c" ? "middle" : "end";
      return `<text x="${x}" y="${y}" text-anchor="${an}" font-family="${esc(h.style.font)}" font-size="${h.style.size}" font-weight="${h.style.bold ? 700 : 400}" font-style="${h.style.italic ? "italic" : "normal"}" fill="${h.style.color}">${esc(txt)}</text>`;
    })
    .join("");
export async function render(i: number, widthPx: number): Promise<string> {
  await sleep(250);
  const k = widthPx / W;
  const bars = pages[i].paras
    .map((p) => {
      const lines = Math.round(p.bbox[3] / p.pitch);
      return Array.from({ length: lines }, (_, l) => {
        const w = l === lines - 1 ? p.bbox[2] * 0.55 : p.bbox[2];
        const x = p.align === "center" ? p.bbox[0] + (p.bbox[2] - w) / 2 : p.bbox[0];
        const fill = p.status === "locked" ? "#c9c9cf" : p.color.toLowerCase() === "#000000" ? "#9a9aa3" : p.color;
        return `<rect x="${x}" y="${p.bbox[1] + l * p.pitch + p.pitch * 0.25}" width="${w}" height="${p.size * 0.55}" fill="${fill}"/>`;
      }).join("");
    })
    .join("");
  const objs = pages[i].objs.map(objSvg).join("");
  const arts = (pages[i].items as Art[]).map(artSvg).join("");
  const wm = doc.wm ? wmSvg(doc.wm, i) : "";
  const front = !!doc.wm && (doc.wm.layer === "front" || i === 1); // page 2 pretends to be a scan: always in front
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${widthPx}" height="${Math.round(H * k)}" viewBox="0 0 ${W} ${H}"><rect width="100%" height="100%" fill="#fff"/>${front ? "" : wm}${bars}${arts}${objs}${front ? wm : ""}${hfSvg(i)}</svg>`;
  return "data:image/svg+xml;utf8," + encodeURIComponent(svg);
}

// ---- edits ----
export async function apply(r: EditApplyReq): Promise<EditApplyRes> {
  await sleep(300);
  const target = pages[r.page].paras.find((x) => x.id === r.para)!;
  if (r.text.includes("@") && !r.use_pc_font) return { kind: "missing_chars", chars: "@" };
  if (r.runs?.some((x) => x.style.bold) && !target.bold && !r.use_pc_font && target.status === "direct") return { kind: "missing_chars", chars: "B" };
  if (r.text.includes("Ω")) return { kind: "unsupported_chars", chars: "Ω" };
  if (r.text.includes("OVERFLOW") && !r.allow_overlap) return { kind: "overflow", lines_over: 3 };
  if (r.text.includes("PUSH") && !r.allow_overlap) return { kind: "cannot_push" };
  const p = pages[r.page].paras.find((x) => x.id === r.para)!;
  p.text = r.text;
  p.edited = true;
  if (r.runs?.length) {
    const s0 = r.runs[0].style;
    const ratio = s0.size / p.size;
    Object.assign(p, { size: s0.size, pitch: p.pitch * ratio, color: s0.color, bold: s0.bold, italic: s0.italic, align: s0.align, style: { ...s0 }, runs: structuredClone(r.runs) });
  } else if (r.style) {
    const ratio = r.style.size / p.size;
    Object.assign(p, { size: r.style.size, pitch: p.pitch * ratio, color: r.style.color, bold: r.style.bold, italic: r.style.italic, align: r.style.align, style: { ...r.style } });
    if (r.style.font !== "orig") (p.css_font = r.style.font, (p.pc_font_used = true));
  }
  fit(p);
  push();
  return { kind: "ok", page: mdl(r.page) };
}
const overlaps = (a: Rect, b: Rect) => a[0] < b[0] + b[2] && b[0] < a[0] + a[2] && a[1] < b[1] + b[3] && b[1] < a[1] + a[3];
export async function applyObj(q: ObjReq): Promise<ObjRes> {
  await sleep(250);
  const pg = pages[q.page];
  const at = q.id === null ? -1 : pg.objs.findIndex((x) => x.id === q.id);
  if (q.kind === "delete") {
    if (at >= 0) pg.objs.splice(at, 1);
    push();
    return { kind: "ok", page: mdl(q.page), id: q.id ?? -1 };
  }
  const rect: Rect = [...q.rect!];
  const o: Obj = { id: at >= 0 ? pg.objs[at].id : nextId++, kind: q.kind, rect, rot: q.rot ?? 0, text: q.text, style: q.style ? { ...q.style } : undefined, asset: q.asset };
  if (q.kind === "textbox") {
    if (q.runs?.length) (o.runs = structuredClone(q.runs), (o.text = q.runs.map((x) => x.text).join("")), (o.style = { ...q.runs[0].style }));
    else o.runs = [{ text: q.text!, style: { ...q.style! } }];
    if (q.text!.includes("@") && q.style!.font === "orig") return { kind: "missing_chars", chars: "@" };
    if (q.text!.includes("Ω")) return { kind: "unsupported_chars", chars: "Ω" };
    const big = Math.max(...o.runs!.map((x) => x.style.size));
    o.rect[3] = boxLines(o.text!, big, rect[2]).length * big * 1.15; // height comes from the backend
  }
  if (q.kind === "whiteout" && !q.cover_only && pg.paras.some((p) => p.status !== "direct" && overlaps(rect, p.bbox))) return { kind: "whiteout_partial", chars: 12 };
  if (at >= 0) pg.objs[at] = o;
  else pg.objs.push(o);
  push();
  return { kind: "ok", page: mdl(q.page), id: o.id };
}
export async function undo(): Promise<EditHistory> {
  await sleep(100);
  if (cur > 0) restore(snaps[--cur]);
  return hist();
}
export async function redo(): Promise<EditHistory> {
  await sleep(100);
  if (cur < snaps.length - 1) restore(snaps[++cur]);
  return hist();
}
export const history = async (): Promise<EditHistory> => hist();
export async function save(_r: EditSaveReq): Promise<EditSaveRes> {
  log("save");
  await sleep(1500);
  return { output: "C:/mock/Laporan_Contoh_edited.pdf", size: 210 * 1024, seconds: 2 };
}
export async function close(): Promise<void> {}

// ---- Edit 3: existing items, watermark, header & footer ----
export async function applyBatch(edits: ObjEdit3[]): Promise<BatchRes> {
  log("batch:" + edits.map((x) => `${x.kind}${x.item.kind}${x.item.id}`).join(","));
  await sleep(250);
  for (const ed of edits) {
    const it = ed.item.kind === "para" ? undefined : (pages[ed.page].items as Art[]).find((x) => x.kind === ed.item.kind && x.id === ed.item.id);
    if (ed.kind === "move" && it?.tied) return { kind: "cannot_move", page: ed.page, item: ed.item };
  }
  const touched = new Set<number>();
  for (const ed of edits) {
    const pg = pages[ed.page];
    touched.add(ed.page);
    if (ed.item.kind === "para") {
      const at = pg.paras.findIndex((x) => x.id === ed.item.id);
      if (at < 0) continue;
      if (ed.kind === "move") (pg.paras[at].bbox[0] += ed.dx), (pg.paras[at].bbox[1] += ed.dy);
      else if (ed.kind === "remove") pg.paras.splice(at, 1);
      continue;
    }
    const at = pg.items.findIndex((x) => x.kind === ed.item.kind && x.id === ed.item.id);
    if (at < 0) continue;
    if (ed.kind === "move") (pg.items[at].bbox[0] += ed.dx), (pg.items[at].bbox[1] += ed.dy);
    else if (ed.kind === "resize") pg.items[at].bbox = [...ed.rect] as Rect;
    else pg.items.splice(at, 1);
  }
  push();
  return { kind: "ok", pages: [...touched].map(mdl) };
}

const badPages = (ranges: string): DocRes | null => {
  if (!ranges.trim()) return null;
  const r = parseRanges(ranges, 2);
  return "error" in r ? { kind: "bad_pages", reason: `Bad page range "${ranges}"` } : null;
};
// Call order log for checks: window.__mockCalls
const log = (n: string) => ((globalThis as unknown as { __mockCalls: string[] }).__mockCalls ??= []).push(n);
export async function watermark(r: WmReq): Promise<DocRes> {
  log("watermark:" + (r.text ?? ""));
  await sleep(250);
  const bad = badPages(r.pages);
  if (bad) return bad;
  if (r.text?.includes("Ω")) return { kind: "unsupported_chars", chars: "Ω" };
  const id = doc.wm?.id ?? nextId++;
  doc.wm = { ...structuredClone(r), id };
  push();
  return { kind: "ok", id, changed: "all" };
}
export async function headerFooter(r: HfReq): Promise<DocRes> {
  await sleep(250);
  const bad = badPages(r.pages);
  if (bad) return bad;
  if (r.template.includes("Ω")) return { kind: "unsupported_chars", chars: "Ω" };
  const at = r.id === null ? -1 : doc.hfs.findIndex((x) => x.id === r.id);
  const id = at >= 0 ? doc.hfs[at].id! : nextId++;
  if (at >= 0) doc.hfs[at] = { ...structuredClone(r), id };
  else doc.hfs.push({ ...structuredClone(r), id });
  push();
  return { kind: "ok", id, changed: "all" };
}
export async function docObjs(): Promise<DocObjs> {
  return structuredClone({ watermarks: doc.wm ? [doc.wm] : [], header_footers: doc.hfs });
}
export async function deleteDocObj(id: number): Promise<void> {
  await sleep(150);
  if (doc.wm?.id === id) doc.wm = null;
  doc.hfs = doc.hfs.filter((x) => x.id !== id);
  push();
}
