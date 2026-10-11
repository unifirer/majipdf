<script lang="ts">
  import { app } from "../state.svelte";
  import { t } from "../i18n";

  // Compact font / size / colour / bold / italic row for the watermark and header & footer panels.
  type S = { font: string; size?: number; color: string; bold: boolean; italic?: boolean };
  let { s, onpatch, size = false, italic = false }: { s: S; onpatch: (p: Partial<S>) => void; size?: boolean; italic?: boolean } = $props();
  const e = app.edit;
  const COLORS = ["#000000", "#808080", "#1F3864", "#C00000"];
  const families = $derived(e.fonts.includes(s.font) ? e.fonts : [s.font, ...e.fonts]);
  const field = "height:40px;box-sizing:border-box;border-radius:10px;border:1px solid var(--line2);background:var(--surface);color:var(--text);font:inherit;padding:0 10px;min-width:0";
  const tog = (on: boolean) =>
    `flex:1;min-height:36px;border-radius:7px;border:0;cursor:pointer;display:inline-flex;align-items:center;justify-content:center;font:inherit;font-weight:600;color:${on ? "var(--accent-text)" : "var(--text2)"};background:${on ? "var(--surface)" : "transparent"};box-shadow:${on ? "0 1px 2px rgba(0,0,0,.12)" : "none"}`;
  const setSize = (ev: Event) => {
    const v = parseFloat((ev.currentTarget as HTMLInputElement).value);
    if (Number.isFinite(v) && v >= 6 && v <= 72) onpatch({ size: Math.round(v * 2) / 2 });
  };
</script>

<div style="display:flex;flex-direction:column;gap:12px">
  <label style="display:flex;flex-direction:column;gap:6px;font-weight:600">
    {t("fmtFont")}
    <select style={field} value={s.font} onchange={(ev) => onpatch({ font: ev.currentTarget.value })}>
      {#each families as fam (fam)}<option value={fam}>{fam}</option>{/each}
    </select>
  </label>
  <div style="display:flex;gap:12px;align-items:flex-end">
    {#if size}
      <label style="display:flex;flex-direction:column;gap:6px;font-weight:600;flex:1;min-width:0">
        {t("fmtSize")}
        <input type="number" min="6" max="72" step="0.5" style={field} value={s.size} oninput={setSize} />
      </label>
    {/if}
    <div style="display:flex;padding:3px;border-radius:10px;background:var(--surface2);gap:2px;height:40px;box-sizing:border-box;flex:1;min-width:0">
      <button style={tog(s.bold)} aria-pressed={s.bold} aria-label={t("fmtBold")} title={t("fmtBold")} onclick={() => onpatch({ bold: !s.bold })}
        ><i class="ph ph-text-b" style="font-size:18px"></i></button
      >
      {#if italic}
        <button style={tog(!!s.italic)} aria-pressed={!!s.italic} aria-label={t("fmtItalic")} title={t("fmtItalic")} onclick={() => onpatch({ italic: !s.italic })}
          ><i class="ph ph-text-italic" style="font-size:18px"></i></button
        >
      {/if}
    </div>
  </div>
  <div style="display:flex;flex-direction:column;gap:6px">
    <span style="font-weight:600" id="sf-color">{t("fmtColor")}</span>
    <div role="group" aria-labelledby="sf-color" style="display:flex;gap:8px;align-items:center;flex-wrap:wrap">
      {#each COLORS as c (c)}
        {@const on = s.color.toLowerCase() === c.toLowerCase()}
        <button
          aria-label={c}
          title={c}
          aria-pressed={on}
          onclick={() => onpatch({ color: c })}
          style="width:32px;height:32px;border-radius:50%;cursor:pointer;background:{c};border:2px solid var(--surface);box-shadow:0 0 0 {on ? '2px var(--accent)' : '1px var(--line2)'}"
        ></button>
      {/each}
      <input
        type="color"
        aria-label={t("fmtColor")}
        value={s.color}
        oninput={(ev) => onpatch({ color: ev.currentTarget.value.toUpperCase() })}
        style="width:40px;height:32px;padding:0;border:1px solid var(--line2);border-radius:8px;background:var(--surface);cursor:pointer"
      />
    </div>
  </div>
</div>
