<script lang="ts">
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { t } from "../lib/i18n.svelte";
  import { go } from "../lib/store.svelte";
  import Logo from "./Logo.svelte";

  const win = getCurrentWindow();
  let maximized = $state(false);
  win.isMaximized().then((m) => (maximized = m));
  win.onResized(async () => (maximized = await win.isMaximized()));
</script>

<header class="bar" data-tauri-drag-region>
  <button class="brand" onclick={() => go({ name: "home" })} tabindex="-1">
    <Logo size={18} />
    {t("appName")}
  </button>
  <div class="spacer" data-tauri-drag-region></div>
  <div class="controls">
    <button title={t("minimize")} onclick={() => win.minimize()}><i class="ti ti-minus"></i></button>
    <button title={t("maximize")} onclick={() => win.toggleMaximize()}>
      <i class="ti {maximized ? 'ti-copy' : 'ti-square'}"></i>
    </button>
    <button class="close" title={t("close")} onclick={() => win.close()}><i class="ti ti-x"></i></button>
  </div>
</header>

<style>
  .bar {
    height: 40px;
    flex: none;
    display: flex;
    align-items: center;
  }
  .brand {
    border: 0;
    display: flex;
    align-items: center;
    gap: 10px;
    font-weight: 600;
    font-size: 13px;
    padding: 0 16px;
    background: transparent;
    height: 100%;
    border-radius: 0;
  }
  .brand:hover {
    background: none !important;
  }
  .spacer {
    flex: 1;
    height: 100%;
  }
  .controls {
    display: flex;
    height: 100%;
  }
  .controls button {
    border: 0;
    border-radius: 0;
    width: 46px;
    height: 100%;
    background: transparent;
    color: var(--text-2);
    font-size: 15px;
    display: grid;
    place-items: center;
  }
  .controls button:hover {
    background: var(--subtle-hover) !important;
  }
  .controls .close:hover {
    background: #c42b1c !important;
    color: #fff;
  }
</style>
