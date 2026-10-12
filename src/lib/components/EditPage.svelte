<script lang="ts">
  import { onMount, tick, untrack } from "svelte";
  import { app } from "../state.svelte";
  import { t } from "../i18n";
  import { editPage, editRender, openEditor, closeEditor, applyEditor, usePcFont, selectObj, clearSel, moveObj, addWhiteout, coverOnly, dismissWo, startBox, openBoxEditor, placeArmed, selChanged, setTool, selectItems, toggleItem, isSelItem, moveItems, resizeItem, type Item, type Para, type Obj, type Rect, type Style } from "../edit";
  import { renderRuns, readRuns, offsetOf, setSel, textOf } from "../rich";

  // One page: lazy-rendered image, paragraph overlay and (when open here) the in-place editor. All text is plain; never {@html}.
  let { i, scale }: { i: number; scale: number } = $props();

  const e = app.edit;
  const size = $derived(e.doc!.sizes[i]);
  const dispW = $derived(size[0] * scale);
  const dispH = $derived(size[1] * scale);
  const model = $derived(e.models[i]);
  const ed = $derived(e.editing && e.editing.page === i ? e.editing : null);
  const edPara = $derived(ed?.kind === "para" ? model?.paras.find((p) => p.id === ed.id) : undefined);
  const edObj = $derived(ed?.kind === "box" && ed.id !== null ? model?.objs?.find((x) => x.id === ed.id) : undefined);
  // Geometry + look of the open editor, for a paragraph or a text box (box look follows the format panel live).
  const G = $derived.by(() => {
    if (!ed) return null;
    if (ed.kind === "para") {
      const p = edPara;
      return p ? { bbox: p.bbox, rot: p.rot, size: p.size, pitch: p.pitch, font: p.css_font, bold: p.bold, italic: p.italic, align: p.align, indent: p.first_indent } : null;
    }
    const st = ed.runs[0]?.style ?? ed.base;
    const bbox = edObj?.rect ?? ed.rect;
    if (!bbox) return null;
    const pitch = Math.max(st.size, ...ed.runs.map((r) => r.style.size)) * 1.15;
    const side = (edObj?.rot ?? ed.rot ?? 0) % 180 !== 0;
    const b: Rect = [bbox[0], bbox[1], side ? Math.max(bbox[2], pitch) : bbox[2], side ? bbox[3] : Math.max(bbox[3], pitch)];
    return { bbox: b, rot: edObj?.rot ?? ed.rot ?? 0, size: pitch / 1.15, pitch, font: st.font === "orig" ? "Arial" : st.font, bold: st.bold, italic: st.italic, align: st.align, indent: 0 };
  });
  const bar = $derived(ed?.bar && G ? { bbox: G.bbox, rot: G.rot } : e.woBar && e.woBar.page === i ? { bbox: e.woBar.rect, rot: 0 as const } : null);

  const dpr = typeof window !== "undefined" ? window.devicePixelRatio || 1 : 1;
  const rw = $derived(Math.min(2400, Math.max(200, Math.ceil((dispW * dpr) / 100) * 100)));
  const gen = $derived(e.allGen + (e.pgen[i] ?? 0));

  let host: HTMLDivElement;
  let near = $state(false);
  let img = $state("");
  let loadedKey = "";
  let ta = $state<HTMLDivElement>();
  let snap: Style[] = []; // styles the editor DOM was built from (span data-k)
  let built: object | null = null;
  let idle = $state(false);
  let idleT: ReturnType<typeof setTimeout> | 0 = 0;
  let tipEl = $state<HTMLDivElement>();
  let tipPos = $state({ left: 0, top: 0, ready: false });
  let barEl = $state<HTMLDivElement>();
  let barPos = $state({ left: 0, top: 0, ready: false });
  let askRevert = $state(false);
  const barW = 340;

  onMount(() => {
    const io = new IntersectionObserver((es) => (near = es.some((x) => x.isIntersecting)), { rootMargin: "1200px 0px" });
    io.observe(host);
    return () => io.disconnect();
  });

  // Load (and reload after an edit / undo / redo / zoom) only while near the viewport; far pages drop their image.
  $effect(() => {
    if (!near) {
      img = "";
      loadedKey = "";
      return;
    }
    const key = `${gen}|${rw}`;
    if (key === loadedKey) return;
    let dead = false;
    Promise.all([editPage(i), editRender(i, rw)])
      .then(([m, url]) => {
        if (dead) return;
        e.models[i] = m;
        img = url;
        loadedKey = key;
      })
      .catch(() => {}); // placeholder stays
    return () => (dead = true);
  });

  // Build the editor DOM from the runs (text nodes in styled spans, never HTML). Only programmatic changes rebuild it
  // (open, format change, zoom); typing never does, so the caret stays put. The selection is restored after a rebuild.
  const cssOf = (s: Style, base: string) =>
    `font-family:${fontOf(s.font === "orig" ? base : s.font)};font-weight:${s.bold ? 700 : 400};font-style:${s.italic ? "italic" : "normal"};font-size:${s.size * scale}px`;
  $effect(() => {
    if (!ta || !ed) return;
    ed.rev;
    scale;
    untrack(() => {
      if (!G) return;
      const first = built !== ed;
      const had = !first && document.activeElement === ta;
      built = ed;
      snap = ed!.runs.map((r) => r.style);
      renderRuns(ta!, ed!.runs, (s) => cssOf(s, G!.font));
      if (first) {
        ta!.focus();
        setSel(ta!, ed!.selS, ed!.selE);
      } else if (had) setSel(ta!, ed!.selS, ed!.selE);
    });
  });
  $effect(() => {
    if (!ed) built = null;
  });

  // Track the selection inside the editor for the format panel.
  $effect(() => {
    if (!ta) return;
    const on = () => {
      const s = getSelection();
      if (!s?.rangeCount || !ta || !s.anchorNode || !s.focusNode || !ta.contains(s.anchorNode) || !ta.contains(s.focusNode)) return;
      const r = s.getRangeAt(0);
      selChanged(offsetOf(ta, r.startContainer, r.startOffset), offsetOf(ta, r.endContainer, r.endOffset));
    };
    document.addEventListener("selectionchange", on);
    return () => document.removeEventListener("selectionchange", on);
  });

  // Typing hint after 2 s without a keystroke; hidden again by the next one and while a warning bar shows.
  function typed() {
    idle = false;
    clearTimeout(idleT);
    idleT = setTimeout(() => (idle = true), 2000);
  }
  $effect(() => {
    if (ed && !ed.bar) typed();
    else {
      clearTimeout(idleT);
      idle = false;
    }
    return () => clearTimeout(idleT);
  });
  $effect(() => {
    idle;
    if (!idle) tipPos.ready = false;
    else void tick().then(placeTip);
  });

  // A new bar (or none) always starts without the revert question.
  $effect(() => {
    ed?.bar;
    askRevert = false;
  });

  // Place the bar on the text's "below" side (rot 0 below, 270 right, 90 left, 180 above), flip to the opposite side when it
  // does not fit, else overlay it on the box; always clamped into the visible scroll area.
  function place() {
    if (!barEl || !bar) return;
    barPos = { ...locate(barEl, bar, !!ed?.bar), ready: true };
  }
  function placeTip() {
    if (!tipEl || !idle || !G) return;
    tipPos = { ...locate(tipEl, { bbox: G.bbox, rot: G.rot }, true), ready: true };
  }
  function locate(el: HTMLElement, bar: { bbox: Rect; rot: number }, useTa: boolean) {
    const h = host.getBoundingClientRect();
    const r0 = useTa ? ta?.getBoundingClientRect() : undefined;
    const sc = host.parentElement?.getBoundingClientRect();
    const R = r0 && r0.width > 0 ? { l: r0.left, t: r0.top, r: r0.right, b: r0.bottom } : { l: h.left + bar.bbox[0] * scale, t: h.top + bar.bbox[1] * scale, r: h.left + (bar.bbox[0] + bar.bbox[2]) * scale, b: h.top + (bar.bbox[1] + bar.bbox[3]) * scale };
    const V = { l: Math.max(sc?.left ?? 0, 0) + 8, t: Math.max(sc?.top ?? 0, 0) + 8, r: Math.min(sc?.right ?? innerWidth, innerWidth) - 8, b: Math.min(sc?.bottom ?? innerHeight, innerHeight) - 8 };
    const w = el.offsetWidth;
    const hh = el.offsetHeight;
    const clampX = (x: number) => Math.max(V.l, Math.min(x, V.r - w));
    const clampY = (y: number) => Math.max(V.t, Math.min(y, V.b - hh));
    const spots: Record<string, { left: number; top: number; ok: boolean }> = {
      below: { left: clampX(R.l), top: R.b + 8, ok: R.b + 8 + hh <= V.b },
      above: { left: clampX(R.l), top: R.t - 8 - hh, ok: R.t - 8 - hh >= V.t },
      right: { left: R.r + 8, top: clampY(R.t), ok: R.r + 8 + w <= V.r },
      left: { left: R.l - 8 - w, top: clampY(R.t), ok: R.l - 8 - w >= V.l },
    };
    const rot = bar.rot;
    const [pref, opp] = rot === 270 ? ["right", "left"] : rot === 90 ? ["left", "right"] : rot === 180 ? ["above", "below"] : ["below", "above"];
    const pick = spots[pref].ok ? spots[pref] : spots[opp].ok ? spots[opp] : { left: clampX(R.l), top: clampY(R.t), ok: true };
    return { left: clampX(pick.left), top: clampY(pick.top) };
  }
  $effect(() => {
    // re-place after every change that moves the box or resizes the bar
    ed?.bar;
    ed?.text;
    askRevert;
    scale;
    bar;
    if (!bar) {
      barPos.ready = false;
      return;
    }
    void tick().then(place);
  });
  $effect(() => {
    if (!bar && !idle) return;
    const sc = host.parentElement;
    const both = () => (place(), placeTip());
    sc?.addEventListener("scroll", both, { passive: true });
    window.addEventListener("resize", both);
    return () => {
      sc?.removeEventListener("scroll", both);
      window.removeEventListener("resize", both);
    };
  });

  const reasonKey = (p: Para) => (p.reason === "form" ? "editLockForm" : p.reason === "scan" ? "editLockScan" : "editLockOther");
  const tipUp = (p: Para) => p.bbox[1] + p.bbox[3] > size[1] - 150;
  const tipRight = (p: Para) => p.bbox[0] > size[0] / 2;
  const fontOf = (name: string) => `"${name.replace(/"/g, "")}", sans-serif`;

  // ---- placed objects: select / move / resize (one apply on release) ----
  const HANDLES = ["nw", "n", "ne", "e", "se", "s", "sw", "w"];
  const CURSOR: Record<string, string> = { nw: "nwse-resize", se: "nwse-resize", ne: "nesw-resize", sw: "nesw-resize", n: "ns-resize", s: "ns-resize", e: "ew-resize", w: "ew-resize" };
  const handlesOf = (o: Obj) => (o.kind !== "textbox" ? HANDLES : o.rot % 180 ? ["n", "s"] : ["w", "e"]); // a text box only changes its length
  type Drag = { id: number; mode: string; sx: number; sy: number; r0: Rect; r: Rect; moved: boolean };
  let drag = $state<Drag | null>(null);

  function resized(r0: Rect, h: string, dx: number, dy: number, o: Obj, lock: boolean): Rect {
    let [x, y] = r0;
    let x2 = x + r0[2];
    let y2 = y + r0[3];
    if (o.kind === "textbox") o.rot % 180 ? (dx = 0) : (dy = 0);
    if (h.includes("w")) x += dx;
    if (h.includes("e")) x2 += dx;
    if (h.includes("n")) y += dy;
    if (h.includes("s")) y2 += dy;
    const min = o.kind === "textbox" ? 24 : 8;
    if (x2 - x < min) h.includes("w") ? (x = x2 - min) : (x2 = x + min);
    if (y2 - y < min) h.includes("n") ? (y = y2 - min) : (y2 = y + min);
    let w = x2 - x;
    let hh = y2 - y;
    if (lock && o.kind === "image") {
      const ratio = r0[2] / r0[3];
      if (h.length === 2) {
        const k = Math.max(w / r0[2], hh / r0[3]);
        w = r0[2] * k;
        hh = r0[3] * k;
        x = h.includes("w") ? r0[0] + r0[2] - w : r0[0];
        y = h.includes("n") ? r0[1] + r0[3] - hh : r0[1];
      } else if (h === "e" || h === "w") {
        hh = w / ratio;
        y = r0[1] + (r0[3] - hh) / 2;
        x = h === "w" ? r0[0] + r0[2] - w : r0[0];
      } else {
        w = hh * ratio;
        x = r0[0] + (r0[2] - w) / 2;
        y = h === "n" ? r0[1] + r0[3] - hh : r0[1];
      }
    }
    return [x, y, w, hh];
  }
  const moved = (r0: Rect, dx: number, dy: number): Rect => [Math.max(0, Math.min(r0[0] + dx, size[0] - r0[2])), Math.max(0, Math.min(r0[1] + dy, size[1] - r0[3])), r0[2], r0[3]];

  function odown(ev: PointerEvent, o: Obj, mode: string) {
    if (ev.button !== 0 || e.editing) return;
    ev.stopPropagation();
    if (e.tool === "add" && o.kind === "textbox") return void openBoxEditor(i, o.id); // Add text: an existing box opens for editing
    selectObj(i, o.id);
    if (e.busy) return;
    (ev.currentTarget as HTMLElement).setPointerCapture(ev.pointerId);
    drag = { id: o.id, mode, sx: ev.clientX, sy: ev.clientY, r0: [...o.rect], r: [...o.rect], moved: false };
  }
  function omove(ev: PointerEvent, o: Obj) {
    if (!drag || drag.id !== o.id) return;
    const dx = (ev.clientX - drag.sx) / scale;
    const dy = (ev.clientY - drag.sy) / scale;
    if (!drag.moved && Math.hypot(dx, dy) * scale < 3) return;
    drag.moved = true;
    drag.r = drag.mode === "move" ? moved(drag.r0, dx, dy) : resized(drag.r0, drag.mode, dx, dy, o, !ev.shiftKey);
  }
  function oup(o: Obj) {
    const d = drag;
    drag = null;
    if (d?.moved && d.id === o.id) void moveObj(i, o, d.r);
  }
  const objLabel = (o: Obj) => (o.kind === "textbox" ? (o.text ?? "") : o.kind === "whiteout" ? t("editToolWo") : (e.sigs.find((s) => s.id === o.asset)?.name ?? t("editToolSig")));

  // ---- page-level pointer: white-out drag, add-text click, armed signature placement, deselect ----
  let wo = $state<Rect | null>(null);
  let woFrom: [number, number] | null = null;
  const at = (ev: MouseEvent): [number, number] => {
    const r = host.getBoundingClientRect();
    return [Math.max(0, Math.min((ev.clientX - r.left) / scale, size[0])), Math.max(0, Math.min((ev.clientY - r.top) / scale, size[1]))];
  };
  function hostdown(ev: PointerEvent) {
    const tg = ev.target as Element;
    if (ev.button !== 0 || (tg !== host && tg.tagName !== "IMG")) return;
    const p = at(ev);
    if (e.tool === "select") return void selectDown(ev, p);
    clearSel();
    if (e.tool === "wo") {
      if (e.editing || e.busy) return;
      host.setPointerCapture(ev.pointerId);
      woFrom = p;
      wo = [p[0], p[1], 0, 0];
    } else if (e.tool === "add") startBox(i, p[0], p[1]);
    else if (e.tool === "sig" && e.armed) void placeArmed(i, p[0], p[1]);
  }
  function hostmove(ev: PointerEvent) {
    if (e.tool === "select") return void selectMove(ev);
    if (!woFrom) return;
    const p = at(ev);
    wo = [Math.min(woFrom[0], p[0]), Math.min(woFrom[1], p[1]), Math.abs(p[0] - woFrom[0]), Math.abs(p[1] - woFrom[1])];
  }
  function hostup() {
    if (e.tool === "select") return void selectUp();
    const r = wo;
    wo = null;
    woFrom = null;
    if (r && r[2] >= 4 && r[3] >= 4) void addWhiteout(i, r);
  }
  // ---- Select tool: existing paragraphs, images, lines & shapes ----
  type IDrag = { mode: string; sx: number; sy: number; dx: number; dy: number; moved: boolean; u: Rect; r0: Rect; r: Rect; item: Item | null; collapse: Item | null };
  let hov = $state<Item | null>(null);
  let idrag = $state<IDrag | null>(null);
  let mq = $state<Rect | null>(null);
  let mqFrom: [number, number] | null = null;
  let mqShift = false;
  let lk = $state({ x: 0, y: 0 });
  let lkEl = $state<HTMLDivElement>();
  let lkPos = $state({ left: 0, top: 0 });
  const selHere = $derived(e.tool === "select" && e.isel?.page === i ? (model?.items ?? []).filter((it) => isSelItem(i, it)) : []);
  const oneImg = $derived(selHere.length === 1 && selHere[0].kind === "image" && !selHere[0].locked ? selHere[0] : null);
  const PX3 = 3; // px: lines and shapes are thin, so their outline and hit area are inflated by this much

  // Topmost (last in z) item under the point; a paragraph wins over lines and shapes, so table borders never steal cell clicks.
  function hitTest(p: [number, number]): Item | null {
    const tol = PX3 / scale;
    const hits = (model?.items ?? []).filter((it) => {
      const pad = it.kind === "path" ? tol : 0;
      return p[0] >= it.bbox[0] - pad && p[0] <= it.bbox[0] + it.bbox[2] + pad && p[1] >= it.bbox[1] - pad && p[1] <= it.bbox[1] + it.bbox[3] + pad;
    });
    const rest = hits.some((it) => it.kind === "para") ? hits.filter((it) => it.kind !== "path") : hits;
    return rest[rest.length - 1] ?? null;
  }
  function selectDown(ev: PointerEvent, p: [number, number]) {
    if (e.editing || e.busy) return;
    hov = null; // a stale hover would keep the pre-move outline
    const hit = hitTest(p);
    if (hit && !hit.locked) {
      host.setPointerCapture(ev.pointerId);
      let collapse: Item | null = null;
      if ((ev.shiftKey || ev.ctrlKey || ev.metaKey)) toggleItem(i, hit);
      else if (isSelItem(i, hit)) collapse = hit;
      else selectItems(i, [hit]);
      if (!isSelItem(i, hit)) return; // shift removed it: nothing to drag
      const sel = (model?.items ?? []).filter((it) => isSelItem(i, it));
      const x1 = Math.min(...sel.map((it) => it.bbox[0]));
      const y1 = Math.min(...sel.map((it) => it.bbox[1]));
      const x2 = Math.max(...sel.map((it) => it.bbox[0] + it.bbox[2]));
      const y2 = Math.max(...sel.map((it) => it.bbox[1] + it.bbox[3]));
      idrag = { mode: "move", sx: ev.clientX, sy: ev.clientY, dx: 0, dy: 0, moved: false, u: [x1, y1, x2, y2], r0: [x1, y1, x2 - x1, y2 - y1], r: [x1, y1, x2 - x1, y2 - y1], item: null, collapse };
      return;
    }
    if (!(ev.shiftKey || ev.ctrlKey || ev.metaKey)) selectItems(i, []);
    if (hit) return; // locked: nothing to do
    host.setPointerCapture(ev.pointerId);
    mqFrom = p;
    mqShift = ev.shiftKey || ev.ctrlKey || ev.metaKey;
    mq = [p[0], p[1], 0, 0];
  }
  function hdown(ev: PointerEvent, it: Item, mode: string) {
    if (ev.button !== 0 || e.busy) return;
    ev.stopPropagation();
    (ev.currentTarget as HTMLElement).setPointerCapture(ev.pointerId);
    idrag = { mode, sx: ev.clientX, sy: ev.clientY, dx: 0, dy: 0, moved: false, u: [0, 0, 0, 0], r0: [...it.bbox], r: [...it.bbox], item: it, collapse: null };
  }
  function selectMove(ev: PointerEvent) {
    const d = idrag;
    if (d) {
      const dx = (ev.clientX - d.sx) / scale;
      const dy = (ev.clientY - d.sy) / scale;
      if (!d.moved && Math.hypot(dx, dy) * scale < 3) return;
      d.moved = true;
      if (d.mode === "move") {
        d.dx = Math.max(-d.u[0], Math.min(dx, size[0] - d.u[2]));
        d.dy = Math.max(-d.u[1], Math.min(dy, size[1] - d.u[3]));
      } else d.r = resized(d.r0, d.mode, dx, dy, { kind: "image", rot: 0 } as Obj, !ev.shiftKey);
      return;
    }
    const p = at(ev);
    if (mqFrom) {
      mq = [Math.min(mqFrom[0], p[0]), Math.min(mqFrom[1], p[1]), Math.abs(p[0] - mqFrom[0]), Math.abs(p[1] - mqFrom[1])];
      return;
    }
    hov = (ev.target as Element).closest?.(".obj") ? null : hitTest(p);
    lk = { x: ev.clientX, y: ev.clientY };
  }
  function selectUp() {
    const d = idrag;
    idrag = null;
    if (d) {
      if (d.moved) void (d.mode === "move" ? moveItems(d.dx, d.dy) : resizeItem(i, d.item!, d.r));
      else if (d.collapse) selectItems(i, [d.collapse]);
      return;
    }
    const r = mq;
    mq = null;
    mqFrom = null;
    if (!r || r[2] < 3 || r[3] < 3) return;
    const inside = (model?.items ?? []).filter((it) => !it.locked && it.bbox[0] >= r[0] && it.bbox[1] >= r[1] && it.bbox[0] + it.bbox[2] <= r[0] + r[2] && it.bbox[1] + it.bbox[3] <= r[1] + r[3]);
    const keep = mqShift && e.isel?.page === i ? e.isel.items : [];
    selectItems(i, [...keep, ...inside.filter((it) => !keep.some((k) => k.kind === it.kind && k.id === it.id))]);
  }
  // Double-click a paragraph in the Select tool: switch to Edit text and open it.
  function hostdbl(ev: MouseEvent) {
    if (e.tool !== "select" || e.editing || (ev.target as Element).closest?.(".obj")) return;
    const it = hitTest(at(ev));
    const para = it?.kind === "para" && !it.locked ? model?.paras.find((x) => x.id === it.id) : undefined;
    if (!para || (para.status !== "direct" && para.status !== "pc_font")) return;
    setTool("text");
    openEditor(i, para.id, para.status === "pc_font");
  }
  const olBox = (b: Rect, kind: string, ox = 0, oy = 0) => {
    const pad = kind === "path" ? PX3 : 0;
    return `left:${(b[0] + ox) * scale - pad}px;top:${(b[1] + oy) * scale - pad}px;width:${b[2] * scale + 2 * pad}px;height:${b[3] * scale + 2 * pad}px`;
  };
  $effect(() => {
    // lock tooltip: beside the pointer, flipped and clamped so it is never clipped
    lk;
    if (!lkEl) return;
    const w = lkEl.offsetWidth;
    const h = lkEl.offsetHeight;
    lkPos = { left: Math.max(8, Math.min(lk.x + 14, innerWidth - w - 8)), top: lk.y + 20 + h > innerHeight - 8 ? Math.max(8, lk.y - 14 - h) : lk.y + 20 };
  });
  $effect(() => {
    if (e.tool !== "select") {
      hov = null;
      idrag = null;
      mq = null;
      mqFrom = null;
    }
  });

  // Add text and an armed signature place on a click: white-outs and images let it through.
  const through = $derived(e.tool === "add" || (e.tool === "sig" && !!e.armed));
  const cursor = $derived(e.tool === "select" ? (mq ? "crosshair" : idrag?.mode === "move" ? "grabbing" : hov ? (hov.locked ? "not-allowed" : "move") : "default") : e.tool === "wo" || e.tool === "add" || (e.tool === "sig" && e.armed) ? "crosshair" : "default");

  function onkeydown(ev: KeyboardEvent) {
    if (ev.key === "Escape") {
      ev.preventDefault();
      ev.stopPropagation();
      closeEditor();
      return;
    }
    typed();
    if (ev.key === "Enter" && (ev.ctrlKey || ev.metaKey)) {
      ev.preventDefault();
      void applyEditor();
    } else if (ev.key === "Enter" && e.editing?.kind !== "box") ev.preventDefault(); // paragraphs only; text boxes take line breaks
  }

  // DOM -> runs after every edit by the user.
  function oninput() {
    const ed2 = e.editing;
    if (!ed2 || !ta) return;
    ed2.runs = readRuns(ta, snap, ed2.base);
    ed2.text = textOf(ed2.runs);
    ed2.bar = null;
    typed();
  }
  // Plain text only; a paragraph takes no line breaks.
  function onpaste(ev: ClipboardEvent) {
    ev.preventDefault();
    let s = ev.clipboardData?.getData("text/plain") ?? "";
    if (e.editing?.kind !== "box") s = s.replace(/\s*[\r\n]+\s*/g, " ");
    document.execCommand("insertText", false, s);
  }

  // Dropped content is inserted as plain text only (like paste); dragging the editor text itself is off.
  function ondrop(ev: DragEvent) {
    ev.preventDefault();
    let s = ev.dataTransfer?.getData("text/plain") ?? "";
    if (!s || !ta) return;
    const r = document.caretRangeFromPoint?.(ev.clientX, ev.clientY);
    if (r && ta.contains(r.startContainer)) {
      const g = getSelection();
      g?.removeAllRanges();
      g?.addRange(r);
    }
    if (e.editing?.kind !== "box") s = s.replace(/\s*[\r\n]+\s*/g, " ");
    document.execCommand("insertText", false, s);
  }

  const btn = "min-height:32px;padding:0 12px;border-radius:8px;border:1px solid var(--line2);background:var(--surface);color:var(--text);font-size:14px;cursor:pointer;white-space:nowrap";
  const btnPrimary = "min-height:32px;padding:0 12px;border-radius:8px;border:0;background:var(--accent-fill);color:var(--accent-ink);font-size:14px;font-weight:600;cursor:pointer;white-space:nowrap";
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div
  bind:this={host}
  id="ep-{i}"
  role="group"
  aria-label={t("pageAria", { n: i + 1 })}
  onpointerdown={hostdown}
  onpointermove={hostmove}
  onpointerup={hostup}
  onpointercancel={hostup}
  onpointerleave={() => (hov = null)}
  ondblclick={hostdbl}
  style="cursor:{cursor};position:relative;flex-shrink:0;margin-inline:auto;width:{dispW}px;height:{dispH}px;background:var(--page);box-shadow:0 0 0 1px var(--pageedge),var(--shadow)"
>
  {#if img}<img src={img} alt="" draggable="false" style="position:absolute;inset:0;width:100%;height:100%;display:block;user-select:none" />{/if}

  <div class="paras" class:off={e.tool !== "text"}>
  {#each (model?.paras ?? []).filter((x) => !x.removed) as p (p.id)}
    {@const box = `left:${p.bbox[0] * scale}px;top:${p.bbox[1] * scale}px;width:${p.bbox[2] * scale}px;height:${p.bbox[3] * scale}px`}
    {@const tip = `${tipUp(p) ? "bottom:100%" : "top:100%"};${tipRight(p) ? "right:0" : "left:0"}`}
    {#if p.status === "direct"}
      <button class="para direct" class:edited={p.edited} class:hide={ed?.id === p.id} style={box} aria-label={p.text} onclick={() => openEditor(i, p.id)}></button>
    {:else if p.status === "pc_font"}
      <!-- svelte-ignore a11y_no_noninteractive_tabindex (keyboard focus reveals the tooltip and its button) -->
      <div class="para lock" class:edited={p.edited} class:hide={ed?.id === p.id} style={box} role="note" tabindex="0" aria-label={t("editPcHint")}>
        <i class="ph ph-lock-simple badge"></i>
        <div class="tip" style={tip}>
          <div class="bubble">
            <div>{t("editPcHint")}</div>
            <button style={btnPrimary} onclick={() => openEditor(i, p.id, true)}>{t("editPcBtn")}</button>
          </div>
        </div>
      </div>
    {:else}
      <!-- svelte-ignore a11y_no_noninteractive_tabindex (keyboard focus reveals the tooltip) -->
      <div class="para lock" style={box} role="note" tabindex="0" aria-label={t(reasonKey(p))}>
        <i class="ph ph-lock-simple badge"></i>
        <div class="tip" style={tip}><div class="bubble">{t(reasonKey(p))}</div></div>
      </div>
    {/if}
  {/each}
  </div>

  {#each model?.objs ?? [] as o (o.id)}
    {@const sel = e.sel?.page === i && e.sel.id === o.id}
    {@const r = drag?.id === o.id ? drag.r : o.rect}
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <div
      class="obj"
      class:sel
      class:hide={ed?.id === o.id && ed.kind === "box"}
      class:through={through && o.kind !== "textbox"}
      role="button"
      tabindex="0"
      aria-pressed={sel}
      aria-label={objLabel(o)}
      style="left:{r[0] * scale}px;top:{r[1] * scale}px;width:{r[2] * scale}px;height:{r[3] * scale}px;cursor:{drag?.id === o.id ? 'grabbing' : 'move'}"
      onpointerdown={(ev) => odown(ev, o, "move")}
      onpointermove={(ev) => omove(ev, o)}
      onpointerup={() => oup(o)}
      onpointercancel={() => (drag = null)}
      ondblclick={() => o.kind === "textbox" && openBoxEditor(i, o.id)}
      onfocus={() => selectObj(i, o.id)}
    >
      {#if sel && !ed}
        {#each handlesOf(o) as h (h)}
          <span
            class="handle"
            aria-hidden="true"
            style="cursor:{CURSOR[h]};left:{h.includes('w') ? '-7px' : h.includes('e') ? 'calc(100% - 7px)' : 'calc(50% - 7px)'};top:{h.includes('n') ? '-7px' : h.includes('s') ? 'calc(100% - 7px)' : 'calc(50% - 7px)'}"
            onpointerdown={(ev) => odown(ev, o, h)}
          ></span>
        {/each}
      {/if}
    </div>
  {/each}

  {#if e.tool === "select"}
    <div class="items" aria-hidden="true">
      {#if hov && !idrag && !isSelItem(i, hov)}<div class="ol hov" class:lk={!!hov.locked} style={olBox(hov.bbox, hov.kind)}></div>{/if}
      {#each selHere as it (it.kind + ":" + it.id)}
        {@const resizing = !!idrag?.item && idrag.item.id === it.id && idrag.item.kind === it.kind}
        {@const mv = idrag?.mode === "move"}
        <div class="ol sel" style={resizing ? olBox(idrag!.r, it.kind) : olBox(it.bbox, it.kind, mv ? idrag!.dx : 0, mv ? idrag!.dy : 0)}>
          {#if oneImg && it === oneImg && !ed}
            {#each HANDLES as h (h)}
              <span
                class="handle"
                aria-hidden="true"
                style="cursor:{CURSOR[h]};left:{h.includes('w') ? '-7px' : h.includes('e') ? 'calc(100% - 7px)' : 'calc(50% - 7px)'};top:{h.includes('n') ? '-7px' : h.includes('s') ? 'calc(100% - 7px)' : 'calc(50% - 7px)'}"
                onpointerdown={(ev) => hdown(ev, it, h)}
              ></span>
            {/each}
          {/if}
        </div>
      {/each}
      {#if mq}<div class="ol mq" style="left:{mq[0] * scale}px;top:{mq[1] * scale}px;width:{mq[2] * scale}px;height:{mq[3] * scale}px"></div>{/if}
    </div>
  {/if}

  {#if e.tool === "select" && hov?.locked && !idrag}
    <div
      bind:this={lkEl}
      role="tooltip"
      style="position:fixed;z-index:30;pointer-events:none;left:{lkPos.left}px;top:{lkPos.top}px;box-sizing:border-box;width:max-content;max-width:min(300px,calc(100vw - 16px));display:flex;gap:8px;align-items:flex-start;padding:6px 10px;border-radius:8px;background:var(--surface);color:var(--text);border:1px solid var(--line2);font-size:13.5px;line-height:1.35;box-shadow:var(--shadow)"
    >
      <i class="ph ph-lock-simple" style="font-size:16px;margin-top:1px;flex-shrink:0"></i>{t("editLockedItem")}
    </div>
  {/if}

  {#if wo}
    <div style="position:absolute;z-index:3;pointer-events:none;box-sizing:border-box;left:{wo[0] * scale}px;top:{wo[1] * scale}px;width:{wo[2] * scale}px;height:{wo[3] * scale}px;background:#fff;border:1.5px dashed #444"></div>
  {/if}

  {#if ed && G}
    {@const p = G}
    {@const side = p.rot === 90 || p.rot === 270}
    {@const tw = (side ? p.bbox[3] : p.bbox[2]) * scale}
    {@const th = (side ? p.bbox[2] : p.bbox[3]) * scale}
    <div
      data-editor
      style="position:absolute;z-index:5;left:{p.bbox[0] * scale}px;top:{p.bbox[1] * scale}px;width:{ed.kind === "box" ? p.bbox[2] * scale : Math.max(p.bbox[2] * scale, 300)}px"
    >
      <!-- sideways text: the editor is laid out in the text's own frame (line length x stacking extent) and turned over the box -->
      <div
        style={p.rot
          ? `position:absolute;left:${(p.bbox[2] * scale - tw) / 2}px;top:${(p.bbox[3] * scale - th) / 2}px;width:${tw}px;transform:rotate(${p.rot}deg);transform-origin:${tw / 2}px ${th / 2}px`
          : ""}
      >
      <div
        bind:this={ta}
        contenteditable={ed.applying ? "false" : "plaintext-only"}
        role="textbox"
        aria-multiline={ed.kind === "box"}
        tabindex="0"
        {oninput}
        {onkeydown}
        {onpaste}
        {ondrop}
        ondragstart={(ev) => ev.preventDefault()}
        spellcheck="false"
        aria-label={t("editName")}
        style="display:block;box-sizing:border-box;margin:0;padding:0;border:0;white-space:pre-wrap;overflow-wrap:anywhere;outline:2px solid var(--accent);outline-offset:2px;background:#FFF1E6;color:#000;caret-color:#000;width:{tw}px;min-height:{th}px;font-family:{fontOf(p.font)};font-size:{p.size * scale}px;line-height:{p.pitch * scale}px;text-align:{p.align === 'justified' ? 'justify' : p.align};text-indent:{p.indent * scale}px;opacity:{ed.applying ? 0.7 : 1}"
      ></div>
      </div>
    </div>
  {/if}
  {#if idle && ed && !ed.bar}
    <!-- Fixed layer, placed like the warning bar -->
    <div
      bind:this={tipEl}
      role="status"
      style="position:fixed;z-index:30;pointer-events:none;left:{tipPos.left}px;top:{tipPos.top}px;visibility:{tipPos.ready ? 'visible' : 'hidden'};box-sizing:border-box;width:max-content;max-width:min(340px,calc(100vw - 16px));padding:6px 10px;border-radius:8px;background:var(--surface);color:var(--text2);border:1px solid var(--line2);font-size:13.5px;line-height:1.35;box-shadow:var(--shadow);text-indent:0;text-align:left;font-weight:400;font-style:normal;text-align-last:auto"
    >
      {t(ed.kind === "box" ? "editTipBox" : "editTipPara")}
    </div>
  {/if}
  {#if bar}
    {@const b = ed?.bar}
    <!-- Fixed layer: never clipped by the page or the scroller; placed beside the text's "below" side, flipped / clamped in place() -->
    <div
      data-editor
      bind:this={barEl}
      role="alert"
      style="position:fixed;z-index:30;left:{barPos.left}px;top:{barPos.top}px;visibility:{barPos.ready ? 'visible' : 'hidden'};box-sizing:border-box;width:{barW}px;max-width:calc(100vw - 16px);padding:10px 12px;border-radius:10px;background:{!b || b.kind === 'missing' || b.kind === 'cannot_push' || b.kind === 'overflow' ? 'var(--warn-soft)' : 'var(--err-soft)'};color:var(--text);font-size:14px;line-height:1.4;box-shadow:var(--shadow);display:flex;flex-direction:column;gap:8px;overflow-wrap:anywhere;text-align:left;text-indent:0;font-weight:400;font-style:normal;text-align-last:auto"
    >
      {#if !b}
        <div>{e.woBar?.batch ? t("editRmPartial") : t("editWoPartial")}</div>
        {#if e.woBar?.annots}<div>{t("editWoAnnots", { n: e.woBar.annots })}</div>{/if}
        <div style="display:flex;gap:8px;flex-wrap:wrap">
          <button style={btnPrimary} onclick={coverOnly}>{t("editWoCover")}</button>
          <button style={btn} onclick={dismissWo}>{e.woBar?.batch ? t("cancel") : t("editWoUndo")}</button>
        </div>
      {:else}
      <div>
        {#if b.kind === "missing"}{t("editMissing", { c: b.chars })}
        {:else if b.kind === "unsupported"}{t("editUnsupported", { c: b.chars })}
        {:else if b.kind === "overflow"}{t("editOverflow", { n: b.n })}
        {:else if b.kind === "cannot_push"}{t("editCannotPush")}
        {:else}{b.text}{/if}
      </div>
      {#if askRevert}
        <div>{t("editRevertAsk")}</div>
        <div style="display:flex;gap:8px;flex-wrap:wrap">
          <button style={btnPrimary} onclick={closeEditor}>{t("editRevert")}</button>
          <button style={btn} onclick={() => (askRevert = false)}>{t("cancel")}</button>
        </div>
      {:else}
        <div style="display:flex;gap:8px;flex-wrap:wrap">
          {#if b.kind === "missing"}
            <button style={btnPrimary} onclick={usePcFont}>{t("editUsePc")}</button>
          {:else if b.kind === "cannot_push" || b.kind === "overflow"}
            <button style={btnPrimary} onclick={() => applyEditor(true)}>{t("editKeepOverlap")}</button>
          {/if}
          {#if b.kind !== "unsupported" && b.kind !== "error"}
            <button
              style={btn}
              onclick={async () => {
                if (e.editing) e.editing.bar = null;
                await tick();
                ta?.focus();
              }}>{t("editKeep")}</button
            >
          {/if}
          <button style={btn} onclick={() => (askRevert = true)}>{t("editRevert")}</button>
        </div>
      {/if}
      {/if}
    </div>
  {/if}
</div>

<style>
  .paras {
    position: absolute;
    inset: 0;
    pointer-events: none;
  }
  .paras .para {
    pointer-events: auto;
  }
  .paras.off .para {
    pointer-events: none;
  }
  .obj {
    position: absolute;
    z-index: 2;
    box-sizing: border-box;
    touch-action: none;
    border-radius: 1px;
  }
  .obj:hover {
    outline: 1.5px dashed var(--accent);
    outline-offset: 1px;
  }
  .obj.through {
    pointer-events: none;
  }
  .obj.sel {
    outline: 2px solid var(--accent);
    outline-offset: 0;
  }
  .items {
    position: absolute;
    inset: 0;
    z-index: 3;
    pointer-events: none;
  }
  .ol {
    position: absolute;
    box-sizing: border-box;
    border-radius: 1px;
  }
  .ol.hov {
    outline: 1.5px dashed var(--accent);
    outline-offset: 1px;
  }
  .ol.hov.lk {
    outline-color: var(--text3);
  }
  .ol.sel {
    outline: 2px solid var(--accent);
    background: color-mix(in srgb, var(--accent) 10%, transparent);
  }
  .ol.mq {
    border: 1.5px dashed var(--accent);
    background: color-mix(in srgb, var(--accent) 12%, transparent);
  }
  .items .handle {
    pointer-events: auto;
  }
  .obj.hide {
    visibility: hidden;
  }
  .obj:focus-visible {
    outline: 2px solid var(--focus);
    outline-offset: 2px;
  }
  .handle {
    position: absolute;
    width: 14px;
    height: 14px;
    box-sizing: border-box;
    border-radius: 4px;
    background: var(--surface);
    border: 2px solid var(--accent);
    touch-action: none;
  }
  .para {
    position: absolute;
    margin: 0;
    padding: 0;
    border: 0;
    background: transparent;
    border-radius: 2px;
    font: inherit;
  }
  .para.hide {
    visibility: hidden;
  }
  .direct {
    cursor: text;
  }
  .direct:hover,
  .direct:focus-visible,
  .lock:hover,
  .lock:focus-within {
    outline: 1.5px dashed var(--accent);
    outline-offset: 2px;
    background: color-mix(in srgb, var(--accent) 8%, transparent);
  }
  .lock {
    cursor: not-allowed;
  }
  .edited::before {
    content: "";
    position: absolute;
    left: -8px;
    top: 0;
    bottom: 0;
    width: 3px;
    border-radius: 2px;
    background: var(--accent);
  }
  .badge {
    display: none;
    position: absolute;
    top: -10px;
    right: -10px;
    width: 20px;
    height: 20px;
    border-radius: 50%;
    place-items: center;
    background: var(--surface);
    color: var(--text2);
    border: 1px solid var(--line2);
    font-size: 12px;
  }
  .lock:hover .badge,
  .lock:focus-within .badge {
    display: grid;
  }
  .tip {
    display: none;
    position: absolute;
    z-index: 6;
    padding: 6px 0;
    width: 270px;
    max-width: 70vw;
  }
  .lock:hover .tip,
  .lock:focus-within .tip {
    display: block;
  }
  .bubble {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 8px;
    padding: 10px 12px;
    border-radius: 10px;
    background: var(--surface);
    color: var(--text);
    border: 1px solid var(--line2);
    box-shadow: var(--shadow);
    font-size: 14px;
    line-height: 1.4;
    cursor: default;
    text-align: left;
    font-weight: 400;
  }
</style>
