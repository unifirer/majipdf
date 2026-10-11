<script lang="ts">
  // Handoff switch: the whole row is the button.
  let { on, label, onchange }: { on: boolean; label: string; onchange: (v: boolean) => void } = $props();
</script>

<button
  role="switch"
  aria-checked={on}
  onclick={() => onchange(!on)}
  style="display:flex;align-items:center;justify-content:space-between;gap:12px;padding:0;border:0;background:none;text-align:left;color:var(--text);cursor:pointer;font:inherit"
>
  <span style="font-weight:600">{label}</span>
  <span style="position:relative;width:42px;height:24px;flex-shrink:0;border-radius:12px;background:{on ? 'var(--accent)' : 'var(--line2)'}">
    <span style="position:absolute;top:3px;left:{on ? 21 : 3}px;width:18px;height:18px;border-radius:50%;background:#fff;transition:left .15s"></span>
  </span>
</button>
