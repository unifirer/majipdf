<script lang="ts">
  import { tick } from "svelte";
  import { app, HF_POS, type HfSpot } from "../state.svelte";
  import { t } from "../i18n";
  import { hfChanged, clearHf, rangeErr, HF_PRESETS, presetOf, type HfPos } from "../edit";
  import Seg from "./Seg.svelte";
  import Switch from "./Switch.svelte";
  import StyleFields from "./StyleFields.svelte";

  // Header & footer section of the options panel: a 2x3 position picker; each spot keeps its own template and look.
  // Every change previews 300 ms later (edit.ts).
  const e = app.edit;
  const pos = $derived(e.hf.pos);
  const s = $derived(e.hf.spots[pos]);
  const NAMES = { tl: "hfPosTl", tc: "hfPosTc", tr: "hfPosTr", bl: "hfPosBl", bc: "hfPosBc", br: "hfPosBr" } as const;
  const CHIPS = [
    ["{page}", "hfPage"],
    ["{total}", "hfTotal"],
    ["{date_id}", "hfDateId"],
    ["{date_en}", "hfDateEn"],
    ["{file}", "hfFile"],
  ] as const;
  const rErr = $derived(rangeErr(s.allPages, s.ranges));
  let tin = $state<HTMLInputElement>();

  const set = (p: Partial<HfSpot>) => {
    Object.assign(s, p);
    hfChanged(pos);
  };
  const typed = (v: string) => set({ template: v, preset: presetOf(v) });
  function pickPreset(v: string) {
    if (v === "custom") {
      set({ preset: "custom" });
      tin?.focus();
    } else set({ preset: v, template: v });
  }
  async function insert(tok: string) {
    const v = s.template;
    const a = tin?.selectionStart ?? v.length;
    const b = tin?.selectionEnd ?? a;
    typed(v.slice(0, a) + tok + v.slice(b));
    await tick();
    tin?.focus();
    tin?.setSelectionRange(a + tok.length, a + tok.length);
  }
  const num = (ev: Event, d: number) => {
    const v = parseFloat((ev.currentTarget as HTMLInputElement).value);
    return Number.isFinite(v) ? v : d;
  };

  const lbl = "font-size:13px;font-weight:700;color:var(--text3);letter-spacing:.04em;text-transform:uppercase";
  const field = "height:40px;box-sizing:border-box;border-radius:10px;border:1px solid var(--line2);background:var(--surface);color:var(--text);font:inherit;padding:0 10px;min-width:0";
  const row = "display:flex;flex-direction:column;gap:6px";
  const btn = "display:inline-flex;align-items:center;gap:6px;min-height:40px;padding:0 14px;border-radius:10px;border:1px solid var(--line2);background:var(--surface);color:var(--text);font-size:15px;cursor:pointer;white-space:nowrap";
</script>

<section aria-label={t("hfTitle")} style="display:flex;flex-direction:column;gap:16px">
  <div style="display:flex;align-items:center;justify-content:space-between;gap:8px">
    <div style={lbl}>{t("hfTitle")}</div>
    <button style="{btn};min-height:34px;padding:0 12px;font-size:14px;font-weight:600" onclick={() => (e.panel = null)}>{t("editDone")}</button>
  </div>

  <div style={row}>
    <span style="font-weight:600" id="hf-pos">{t("hfPosition")}</span>
    <div style="display:flex;gap:14px;align-items:center">
      <div
        role="group"
        aria-labelledby="hf-pos"
        style="flex-shrink:0;width:132px;height:120px;box-sizing:border-box;padding:7px;border-radius:6px;background:var(--page);box-shadow:0 0 0 1px var(--pageedge),var(--shadow);display:grid;grid-template-columns:repeat(3,1fr);grid-template-rows:1fr 1fr 1fr;gap:5px"
      >
        {#each HF_POS as p (p)}
          {@const on = pos === p}
          {@const has = e.hf.spots[p].id !== null}
          <button
            aria-pressed={on}
            aria-label={t(NAMES[p]) + (has ? " •" : "")}
            title={t(NAMES[p])}
            onclick={() => (e.hf.pos = p as HfPos)}
            style="grid-column:{'lcr'.indexOf(p[1]) + 1};grid-row:{p[0] === 't' ? 1 : 3};position:relative;border-radius:5px;cursor:pointer;padding:0;border:{on ? '2px solid var(--accent)' : '1px dashed #9a9aa3'};background:{on ? 'color-mix(in srgb, var(--accent) 30%, #fff)' : 'transparent'}"
          >
            {#if has}<span aria-hidden="true" style="position:absolute;top:50%;left:50%;width:8px;height:8px;margin:-4px 0 0 -4px;border-radius:50%;background:var(--accent)"></span>{/if}
          </button>
        {/each}
      </div>
      <div style="min-width:0;display:flex;flex-direction:column;gap:2px">
        <span style="font-weight:600;overflow-wrap:anywhere">{t(NAMES[pos])}</span>
        {#if s.id !== null}<span style="font-size:14px;color:var(--text2)">{s.template}</span>{/if}
      </div>
    </div>
  </div>

  <label style={row}>
    <span style="font-weight:600">{t("hfPreset")}</span>
    <select style={field} value={s.preset} onchange={(ev) => pickPreset(ev.currentTarget.value)}>
      {#each HF_PRESETS() as p (p.tpl)}<option value={p.tpl}>{p.label}</option>{/each}
      <option value="custom">{t("hfCustom")}</option>
    </select>
  </label>

  <div style={row}>
    <label for="hf-tpl" style="font-weight:600">{t("hfTemplate")}</label>
    <input id="hf-tpl" bind:this={tin} type="text" maxlength="200" value={s.template} oninput={(ev) => typed(ev.currentTarget.value)} style="{field};height:44px;padding:0 12px" />
    <div role="group" aria-label={t("hfInsert")} style="display:flex;gap:6px;flex-wrap:wrap;align-items:center">
      <span style="font-size:14px;color:var(--text2)">{t("hfInsert")}</span>
      {#each CHIPS as [tok, key] (tok)}
        <button
          title={tok}
          onclick={() => insert(tok)}
          style="display:inline-flex;align-items:center;gap:4px;min-height:30px;padding:0 10px;border-radius:7px;border:1px solid var(--line2);background:var(--surface2);color:var(--text);font:inherit;font-size:14px;cursor:pointer;white-space:nowrap"
          ><i class="ph ph-plus" style="font-size:12px"></i>{t(key)}</button
        >
      {/each}
    </div>
  </div>

  <StyleFields {s} onpatch={(p) => set(p)} size italic />

  <div style="display:flex;gap:12px">
    <label style="{row};flex:1;min-width:0;font-weight:600">
      {t("hfMargin")}
      <span style="display:flex;align-items:center;gap:6px">
        <input type="number" min="0" max="300" step="1" value={s.margin} oninput={(ev) => set({ margin: num(ev, 28) })} style="{field};width:100%" />
        <span style="font-weight:400;color:var(--text2)">pt</span>
      </span>
    </label>
    <label style="{row};flex:1;min-width:0;font-weight:600">
      {t("hfStart")}
      <input type="number" min="0" max="99999" step="1" value={s.start} oninput={(ev) => set({ start: Math.round(num(ev, 1)) })} style={field} />
    </label>
  </div>

  <Switch on={s.skip} label={t("hfSkipFirst")} onchange={(v) => set({ skip: v })} />

  <div style={row}>
    <span style="font-weight:600">{t("wmPages")}</span>
    <Seg label={t("wmPages")} value={s.allPages} options={[{ v: true, label: t("wmAll") }, { v: false, label: t("wmRange") }]} onpick={(v) => set({ allPages: v })} />
    {#if !s.allPages}
      <input
        type="text"
        aria-label={t("wmRange")}
        placeholder="1-3, 5"
        value={s.ranges}
        aria-invalid={!!rErr}
        oninput={(ev) => set({ ranges: ev.currentTarget.value })}
        style="{field};height:44px;padding:0 12px;border-color:{rErr ? 'var(--err)' : 'var(--line2)'}"
      />
      {#if rErr}<div role="alert" style="color:var(--err);font-size:14px;overflow-wrap:anywhere">{rErr}</div>
      {:else}<div style="color:var(--text2);font-size:14px">{t("rangesHint")}</div>{/if}
    {/if}
  </div>

  {#if s.dup}
    <div role="note" style="display:flex;gap:8px;padding:10px 12px;border-radius:12px;background:var(--warn-soft);color:var(--warn);font-size:14px">
      <i class="ph ph-warning" style="font-size:18px;flex-shrink:0;margin-top:1px"></i>{t("hfSamePos")}
    </div>
  {/if}
  {#if s.err}<div role="alert" style="padding:10px 12px;border-radius:12px;background:var(--err-soft);font-size:14.5px;overflow-wrap:anywhere">{s.err}</div>{/if}

  {#if s.id !== null}
    <button style="{btn};color:var(--err);align-self:flex-start" onclick={() => clearHf(pos)} disabled={e.busy}><i class="ph ph-trash" style="font-size:16px"></i>{t("hfClear")}</button>
  {/if}
</section>

<style>
  button:disabled {
    opacity: 0.5;
    cursor: not-allowed !important;
  }
</style>
