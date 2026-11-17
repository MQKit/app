<script lang="ts">
  import { t, type Key } from "../lib/i18n.svelte";
  import { app, go } from "../lib/store.svelte";

  const cats: { id?: string; key: Key; icon: string }[] = [
    { key: "all", icon: "layout-grid" },
    { id: "favorites", key: "favorites", icon: "star" },
    { id: "pdf", key: "cat_pdf", icon: "file-type-pdf" },
    { id: "image", key: "cat_image", icon: "photo" },
    { id: "video", key: "cat_video", icon: "movie" },
    { id: "audio", key: "cat_audio", icon: "music" },
    { id: "document", key: "cat_document", icon: "file-text" },
  ];
  const bottom: { name: "history" | "settings" | "support"; key: Key; icon: string }[] = [
    { name: "history", key: "history", icon: "history" },
    { name: "settings", key: "settings", icon: "settings" },
    { name: "support", key: "support", icon: "heart" },
  ];
  const inTool = (c?: string) => app.route.name === "tool" && c === undefined;
  const isCat = (id?: string) => app.route.name === "home" && (app.route.category ?? undefined) === id;
</script>

<nav class="side">
  {#each cats as c}
    <button class="item" class:on={isCat(c.id) || inTool(c.id)} onclick={() => go({ name: "home", category: c.id })}>
      <i class="ti ti-{c.icon}"></i>{t(c.key)}
    </button>
  {/each}
  <div class="sep"></div>
  {#each bottom as b}
    <button class="item" class:on={app.route.name === b.name} onclick={() => go({ name: b.name })}>
      <i class="ti ti-{b.icon}"></i>{t(b.key)}
    </button>
  {/each}
  <p class="privacy" title={t("privacy")}><i class="ti ti-shield-lock"></i><span>{t("privacyShort")}</span></p>
</nav>

<style>
  .side {
    padding: 8px 10px 12px;
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-height: 0;
    overflow-y: auto;
  }
  .item {
    position: relative;
    border: 0;
    background: transparent;
    text-align: left;
    padding: 7px 10px;
    color: var(--text-nav);
    display: flex;
    align-items: center;
    gap: 11px;
    border-radius: var(--radius);
    font-size: 13.5px;
  }
  .item:hover {
    background: var(--subtle-hover) !important;
  }
  .item .ti {
    font-size: 18px;
    opacity: 0.8;
  }
  .item.on {
    background: var(--selected) !important;
    box-shadow: var(--shadow-sm);
    color: var(--text);
    font-weight: 600;
  }
  /* Fluent's selection pill */
  .item.on::before {
    content: "";
    position: absolute;
    left: 0;
    top: 50%;
    width: 3px;
    height: 16px;
    border-radius: 2px;
    background: var(--accent-fill);
    transform: translateY(-50%);
  }
  .item.on .ti {
    color: var(--accent);
    opacity: 1;
  }
  .sep {
    margin: auto 6px 6px;
    height: 1px;
    background: var(--layer-stroke);
  }
  .privacy {
    display: flex;
    align-items: center;
    gap: 7px;
    margin: 10px 2px 0;
    padding: 6px 10px;
    border-radius: 999px;
    font-size: 12px;
    line-height: 16px;
    color: var(--ok-text);
    background: var(--ok-bg);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    cursor: default;
  }
  .privacy span {
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .privacy .ti {
    font-size: 15px;
    flex: none;
  }
</style>
