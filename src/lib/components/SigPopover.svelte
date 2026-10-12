<script lang="ts">
  import { app } from "../state.svelte";
  import { t } from "../i18n";
  import SigLibrary from "./SigLibrary.svelte";

  // Signature / stamp library popover. position:fixed under the toolbar button so the scroller never clips it.
  let { anchor }: { anchor: HTMLElement } = $props();
  const e = app.edit;

  let box = $state<HTMLDivElement>();
  let pos = $state({ left: 8, top: 8, maxH: 400 });

  function place() {
    const r = anchor.getBoundingClientRect();
    const w = Math.min(380, innerWidth - 16);
    const left = Math.max(8, Math.min(r.left, innerWidth - w - 8));
    pos = { left, top: r.bottom + 6, maxH: Math.max(160, innerHeight - r.bottom - 14) };
  }
  $effect(() => {
    place();
    addEventListener("resize", place);
    const sc = anchor.closest("[role=toolbar]")?.parentElement;
    sc?.addEventListener("scroll", place, { passive: true });
    return () => {
      removeEventListener("resize", place);
      sc?.removeEventListener("scroll", place);
    };
  });

  // Outside click closes (the tool button toggles it itself).
  function onpointerdown(ev: PointerEvent) {
    const tg = ev.target as Node;
    if (box && !box.contains(tg) && !anchor.contains(tg)) e.sigOpen = false;
  }
</script>

<svelte:window {onpointerdown} />

<div
  bind:this={box}
  role="dialog"
  aria-label={t("editToolSig")}
  style="position:fixed;z-index:40;left:{pos.left}px;top:{pos.top}px;width:{Math.min(380, innerWidth - 16)}px;max-height:{pos.maxH}px;box-sizing:border-box;overflow:auto;border-radius:16px;background:var(--surface);border:1px solid var(--line);box-shadow:0 12px 40px rgba(0,0,0,.18);padding:14px"
>
  <SigLibrary />
</div>
