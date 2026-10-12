<script lang="ts">
  import { app } from "../state.svelte";
  import { t } from "../i18n";
  import SigLibrary from "./SigLibrary.svelte";

  // Settings → "Manage…": the signature/stamp library in its own scrolling window, so Settings stays short.
  let closeBtn = $state<HTMLButtonElement>();
  const close = () => (app.sigManageOpen = false);
  $effect(() => closeBtn?.focus());
</script>

<svelte:window onkeydown={(e) => e.key === "Escape" && !app.edit.drawOpen && close()} />

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div onclick={close} style="position:fixed;inset:0;z-index:40;background:var(--scrim);display:grid;place-items:center;padding:16px">
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div
    role="dialog"
    aria-modal="true"
    aria-labelledby="sigman-title"
    tabindex="-1"
    onclick={(e) => e.stopPropagation()}
    style="width:560px;max-width:100%;max-height:calc(100vh - 32px);box-sizing:border-box;display:flex;flex-direction:column;gap:14px;padding:18px 20px 20px;border-radius:20px;background:var(--surface);border:1px solid var(--line2);box-shadow:0 24px 60px rgba(0,0,0,.35)"
  >
    <div style="display:flex;align-items:center;gap:8px">
      <i class="ph ph-signature" style="font-size:20px;color:var(--accent-text)"></i>
      <h2 id="sigman-title" style="margin:0;flex:1;font-size:17px;font-weight:700">{t("setSigs")}</h2>
      <button bind:this={closeBtn} onclick={close} aria-label={t("close")} style="width:34px;height:34px;border-radius:9px;border:0;background:none;color:var(--text2);cursor:pointer;display:grid;place-items:center">
        <i class="ph ph-x" style="font-size:18px"></i>
      </button>
    </div>
    <div style="min-height:0;overflow:auto">
      <SigLibrary manage />
    </div>
  </div>
</div>
