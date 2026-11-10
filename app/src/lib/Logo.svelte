<script lang="ts">
  import darkText from "../assets/logo-dark-text.png";
  import lightText from "../assets/logo-light-text.png";

  /** Height of the logo in pixels; the width follows. */
  let { height = 22 }: { height?: number } = $props();
</script>

<!-- Both versions are loaded; the theme shows the readable one. -->
<span class="logo" style:height="{height}px" role="img" aria-label="Capshot">
  <img class="on-light" src={darkText} alt="" draggable="false" />
  <img class="on-dark" src={lightText} alt="" draggable="false" />
</span>

<style>
  .logo {
    display: inline-flex;
    flex: none;
    user-select: none;
  }
  img {
    height: 100%;
    width: auto;
  }
  .on-dark {
    display: none;
  }
  :global(:root[data-theme="dark"]) .on-light {
    display: none;
  }
  :global(:root[data-theme="dark"]) .on-dark {
    display: block;
  }
</style>
