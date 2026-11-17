<script lang="ts">
  // The app's logo. The drawing lives in src-tauri/icons/logo.svg (the icons are made from it);
  // ui/src/assets/logo.svg is a copy of it for the window.
  import logo from "../assets/logo.svg";
  let { size = 18 }: { size?: number } = $props();
</script>

<img src={logo} width={size} height={size} alt="" draggable="false" />
