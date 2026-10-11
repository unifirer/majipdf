<script lang="ts">
  import { app } from "../state.svelte";
  import { t } from "../i18n";
  import { chooseImage } from "../tauri";
  import { wmChanged, removeWm, rangeErr, sigImport, refreshSigs } from "../edit";
  import Seg from "./Seg.svelte";
  import Switch from "./Switch.svelte";
  import StyleFields from "./StyleFields.svelte";

  // Watermark section of the options panel (one watermark managed here). Every change previews 300 ms later (edit.ts).
  const e = app.edit;
  const w = e.wm;
  const QUICK = ["DRAFT", "RAHASIA", "SALINAN", "KONSEP", "CONFIDENTIAL", "COPY"];
  const rErr = $derived(rangeErr(w.allPages, w.ranges));
  let impErr = $state("");
  let impBusy = $state(false);

  const set = (p: Partial<typeof w>) => {
    Object.assign(w, p);
    wmChanged();
  };
  async function importImg() {
    impBusy = true;
    impErr = "";
    try {
      const p = await chooseImage();
      if (!p) return;
      const it = await sigImport(p, "stamp");
      await refreshSigs();
      set({ asset: it.id });
    } catch (x) {
      impErr = String(x);
    } finally {
      impBusy = false;
    }
  }
  const angleIn = (ev: Event) => {
    const v = parseFloat((ev.currentTarget as HTMLInputElement).value);
    if (Number.isFinite(v)) set({ angle: Math.max(-90, Math.min(90, Math.round(v))) });
  };

  const lbl = "font-size:13px;font-weight:700;color:var(--text3);letter-spacing:.04em;text-transform:uppercase";
  const field = "height:40px;box-sizing:border-box;border-radius:10px;border:1px solid var(--line2);background:var(--surface);color:var(--text);font:inherit;padding:0 10px;min-width:0";
  const row = "display:flex;flex-direction:column;gap:6px";
  const btn = "display:inline-flex;align-items:center;gap:6px;min-height:40px;padding:0 14px;border-radius:10px;border:1px solid var(--line2);background:var(--surface);color:var(--text);font-size:15px;cursor:pointer;white-space:nowrap";
</script>

<section aria-label={t("wmTitle")} style="display:flex;flex-direction:column;gap:16px">
  <div style="display:flex;align-items:center;justify-content:space-between;gap:8px">
    <div style={lbl}>{t("wmTitle")}</div>
    <button style="{btn};min-height:34px;padding:0 12px;font-size:14px;font-weight:600" onclick={() => (e.panel = null)}>{t("editDone")}</button>
  </div>

  <Seg label={t("wmTitle")} value={w.kind} options={[{ v: "text", label: t("wmText") }, { v: "image", label: t("wmImage") }]} onpick={(v) => set({ kind: v })} />

  {#if w.kind === "text"}
    <div style={row}>
      <label for="wm-text" style="font-weight:600">{t("wmText")}</label>
      <input id="wm-text" type="text" maxlength="60" value={w.text} oninput={(ev) => set({ text: ev.currentTarget.value })} style="{field};height:44px;padding:0 12px" />
      <select aria-label={t("wmQuick")} style={field} value="" onchange={(ev) => (set({ text: ev.currentTarget.value }), (ev.currentTarget.value = ""))}>
        <option value="" disabled>{t("wmQuick")}</option>
        {#each QUICK as q (q)}<option value={q}>{q}</option>{/each}
      </select>
    </div>
    <StyleFields s={w} onpatch={(p) => set(p)} />
    <div style={row}>
      <Switch on={w.auto} label={t("wmAuto")} onchange={(v) => set({ auto: v })} />
      {#if !w.auto}
        <label style="display:flex;align-items:center;gap:10px;font-weight:600">
          <input type="range" min="12" max="300" step="1" value={w.size} oninput={(ev) => set({ size: +ev.currentTarget.value })} aria-label={t("wmSize")} style="flex:1;min-width:0;accent-color:var(--accent)" />
          <span style="min-width:52px;text-align:right;font-variant-numeric:tabular-nums">{w.size} pt</span>
        </label>
      {/if}
    </div>
  {:else}
    <div style={row}>
      <span style="font-weight:600">{t("wmImage")}</span>
      {#if e.sigs.length}
        <ul style="list-style:none;margin:0;padding:0;display:grid;grid-template-columns:repeat(auto-fill,minmax(76px,1fr));gap:8px">
          {#each e.sigs as it (it.id)}
            <li>
              <button
                onclick={() => set({ asset: it.id })}
                aria-pressed={w.asset === it.id}
                title={it.name}
                style="width:100%;display:grid;place-items:center;height:60px;padding:4px;box-sizing:border-box;border-radius:10px;cursor:pointer;background:var(--page);border:2px solid {w.asset === it.id ? 'var(--accent)' : 'var(--line)'}"
                ><img src={it.data_url} alt={it.name} draggable="false" style="max-width:100%;max-height:100%;object-fit:contain" /></button
              >
            </li>
          {/each}
        </ul>
      {:else}
        <div style="color:var(--text2)">{t("sigEmpty")}</div>
      {/if}
      <button style="{btn};align-self:flex-start" onclick={importImg} disabled={impBusy}><i class="ph ph-image" style="font-size:17px"></i>{t("sigImport")}</button>
      {#if impErr}<div role="alert" style="padding:8px 10px;border-radius:10px;background:var(--err-soft);font-size:14px;overflow-wrap:anywhere">{impErr}</div>{/if}
    </div>
    <label style={row}>
      <span style="font-weight:600">{t("wmSize")}</span>
      <span style="display:flex;align-items:center;gap:10px">
        <input type="range" min="10" max="100" step="1" value={w.scale} oninput={(ev) => set({ scale: +ev.currentTarget.value })} style="flex:1;min-width:0;accent-color:var(--accent)" />
        <span style="min-width:44px;text-align:right;font-variant-numeric:tabular-nums">{w.scale} %</span>
      </span>
    </label>
  {/if}

  <label style={row}>
    <span style="font-weight:600">{t("wmOpacity")}</span>
    <span style="display:flex;align-items:center;gap:10px">
      <input type="range" min="5" max="100" step="1" value={w.opacity} oninput={(ev) => set({ opacity: +ev.currentTarget.value })} style="flex:1;min-width:0;accent-color:var(--accent)" />
      <span style="min-width:44px;text-align:right;font-variant-numeric:tabular-nums">{w.opacity} %</span>
    </span>
  </label>

  <div style={row}>
    <span style="font-weight:600">{t("wmAngle")}</span>
    <div style="display:flex;gap:8px;align-items:center">
      <div style="flex:1;min-width:0">
        <Seg label={t("wmAngle")} value={w.angle === 45 ? 45 : w.angle === 0 ? 0 : -1} options={[{ v: 45, label: t("wmDiagonal") }, { v: 0, label: t("wmStraight") }]} onpick={(v) => set({ angle: v })} />
      </div>
      <input type="number" min="-90" max="90" step="1" aria-label={t("wmAngle")} value={w.angle} oninput={angleIn} style="{field};width:64px;flex-shrink:0;padding:0 8px" />
      <span aria-hidden="true">°</span>
    </div>
  </div>

  <div style={row}>
    <span style="font-weight:600">{t("wmLayer")}</span>
    <Seg label={t("wmLayer")} value={w.layer} options={[{ v: "behind", label: t("wmBehind") }, { v: "front", label: t("wmFront") }]} onpick={(v) => set({ layer: v })} />
  </div>

  <div style={row}>
    <span style="font-weight:600">{t("wmPages")}</span>
    <Seg label={t("wmPages")} value={w.allPages} options={[{ v: true, label: t("wmAll") }, { v: false, label: t("wmRange") }]} onpick={(v) => set({ allPages: v })} />
    {#if !w.allPages}
      <input
        type="text"
        aria-label={t("wmRange")}
        placeholder="1-3, 5"
        value={w.ranges}
        aria-invalid={!!rErr}
        oninput={(ev) => set({ ranges: ev.currentTarget.value })}
        style="{field};height:44px;padding:0 12px;border-color:{rErr ? 'var(--err)' : 'var(--line2)'}"
      />
      {#if rErr}<div role="alert" style="color:var(--err);font-size:14px;overflow-wrap:anywhere">{rErr}</div>
      {:else}<div style="color:var(--text2);font-size:14px">{t("rangesHint")}</div>{/if}
    {/if}
  </div>

  {#if w.err}<div role="alert" style="padding:10px 12px;border-radius:12px;background:var(--err-soft);font-size:14.5px;overflow-wrap:anywhere">{w.err}</div>{/if}

  {#if w.id !== null}
    <button style="{btn};color:var(--err);align-self:flex-start" onclick={removeWm} disabled={e.busy}><i class="ph ph-trash" style="font-size:16px"></i>{t("wmRemove")}</button>
  {/if}
</section>

<style>
  button:disabled {
    opacity: 0.5;
    cursor: not-allowed !important;
  }
</style>
