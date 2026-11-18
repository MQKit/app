<script lang="ts">
  import { onMount } from "svelte";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import { api } from "./lib/api";
  import { app, init, openWithFiles } from "./lib/store.svelte";
  import { t } from "./lib/i18n.svelte";
  import TitleBar from "./components/TitleBar.svelte";
  import Sidebar from "./components/Sidebar.svelte";
  import Home from "./views/Home.svelte";
  import ToolView from "./views/ToolView.svelte";
  import History from "./views/History.svelte";
  import Settings from "./views/Settings.svelte";
  import Support from "./views/Support.svelte";
  import UpdateBanner from "./components/UpdateBanner.svelte";

  /** The open tool registers here to receive dropped files. */
  let toolDrop: ((paths: string[]) => void) | null = $state(null);

  onMount(() => {
    init();
    let unlisten: (() => void) | undefined;
    getCurrentWebview()
      .onDragDropEvent(async (e) => {
        const p = e.payload;
        if (p.type === "enter" || p.type === "over") app.dragging = true;
        else if (p.type === "leave") app.dragging = false;
        else if (p.type === "drop") {
          app.dragging = false;
          if (!p.paths.length) return;
          if (app.route.name === "tool" && toolDrop) toolDrop(p.paths);
          else openWithFiles(await api.expand(p.paths));
        }
      })
      .then((u) => (unlisten = u));
    const block = (e: Event) => e.preventDefault();
    window.addEventListener("contextmenu", block);
    return () => {
      unlisten?.();
      window.removeEventListener("contextmenu", block);
    };
  });
</script>

<div class="window">
  <TitleBar />
  {#if app.ready}
    <div class="body">
      <Sidebar />
      <main class="layer">
        <UpdateBanner />
        {#if app.route.name === "tool"}
          {#key app.route.id + (app.route.files?.length ?? 0)}
            <ToolView id={app.route.id} initialFiles={app.route.files ?? []} bind:onDrop={toolDrop} />
          {/key}
        {:else if app.route.name === "home"}
          <Home />
        {:else if app.route.name === "history"}
          <History />
        {:else if app.route.name === "settings"}
          <Settings />
        {:else if app.route.name === "support"}
          <Support />
        {/if}
      </main>
    </div>
  {/if}
  {#if app.dragging}
    <div class="drop-overlay"><div><i class="ti ti-upload"></i>{t(app.busy ? "busyDrop" : "dropHere")}</div></div>
  {/if}
</div>

<style>
  .window {
    height: 100%;
    display: flex;
    flex-direction: column;
    position: relative;
  }
  .body {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: 212px minmax(0, 1fr);
  }
  /* The content layer: lighter than the navigation, rounded where they meet. */
  .layer {
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
    background: var(--layer-solid);
    border-top: 1px solid var(--layer-stroke);
    border-left: 1px solid var(--layer-stroke);
    border-top-left-radius: var(--radius);
    overflow: hidden;
  }
  :global(:root[data-mica="true"]) .layer {
    background: var(--layer);
  }
  .drop-overlay {
    position: absolute;
    inset: 40px 6px 6px 6px;
    background: color-mix(in srgb, var(--accent-fill) 10%, transparent);
    border: 2px dashed var(--accent-border);
    border-radius: var(--radius-lg);
    display: grid;
    place-items: center;
    pointer-events: none;
    z-index: 50;
  }
  .drop-overlay div {
    background: var(--layer-solid);
    padding: 14px 22px;
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow);
    font-size: 15px;
    font-weight: 600;
    display: flex;
    gap: 10px;
    align-items: center;
    color: var(--accent);
  }
  :global(:root[data-mica="true"]) .drop-overlay div {
    background: var(--base);
  }
  .drop-overlay .ti {
    font-size: 22px;
  }
</style>
