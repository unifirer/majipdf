<script lang="ts" generics="T extends string | number | boolean">
  // Segmented control (handoff style): track --surface2, active = --surface + shadow. Full width, 36 px buttons.
  let { value, options, onpick, label }: { value: T | null; options: { v: T; label: string }[]; onpick: (v: T) => void; label: string } = $props();
</script>

<div role="group" aria-label={label} style="display:flex;padding:3px;border-radius:10px;background:var(--surface2);gap:2px">
  {#each options as o (String(o.v))}
    {@const on = value === o.v}
    <button
      aria-pressed={on}
      onclick={() => onpick(o.v)}
      style="flex:1;min-width:0;min-height:36px;padding:0 8px;border-radius:7px;border:0;cursor:pointer;font:inherit;font-size:14px;font-weight:600;line-height:1.15;color:{on ? 'var(--accent-text)' : 'var(--text2)'};background:{on ? 'var(--surface)' : 'transparent'};box-shadow:{on ? '0 1px 2px rgba(0,0,0,.12)' : 'none'}"
      >{o.label}</button
    >
  {/each}
</div>
