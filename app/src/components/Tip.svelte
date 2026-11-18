<script lang="ts">
  // A small "?" that explains something when pointed at or focused. Keeps the screen quiet
  // while the answer is one move away.
  let { text }: { text: string } = $props();
</script>

<button type="button" class="tip" aria-label={text}>
  ?
  <span class="pop">{text}</span>
</button>

<style>
  .tip {
    position: relative;
    display: inline-grid;
    place-items: center;
    width: 15px;
    height: 15px;
    border-radius: 50%;
    border: 1px solid var(--line-strong);
    color: var(--faint);
    font-size: 10px;
    font-weight: 700;
    cursor: help;
    vertical-align: middle;
    background: none;
    padding: 0;
    font-family: inherit;
  }
  .tip:hover,
  .tip:focus {
    color: var(--accent);
    border-color: var(--accent);
  }
  .pop {
    display: none;
    position: absolute;
    z-index: 50;
    left: 50%;
    top: 22px;
    transform: translateX(-50%);
    width: 240px;
    padding: 9px 11px;
    background: var(--panel-2);
    border: 1px solid var(--line-strong);
    border-radius: 6px;
    box-shadow: var(--shadow);
    color: var(--text);
    font-size: 12.5px;
    font-weight: 400;
    line-height: 1.4;
    text-align: left;
  }
  .tip:hover .pop,
  .tip:focus .pop {
    display: block;
  }
</style>
