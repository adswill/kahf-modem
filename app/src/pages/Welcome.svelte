<script lang="ts">
  import { app } from "../lib/state.svelte";
  import Icon from "../components/Icon.svelte";

  async function explore() {
    // Look around without transmitting.
    await app.saveSettings({ listenOnly: true, setupDone: true });
    await app.startEngine();
    app.page = "chats";
  }
</script>

<main class="welcome">
  <div class="brand mono">KAHF</div>
  <h1>Messages, mail, files and voice,<br />by radio. No internet needed.</h1>
  <p class="lead muted">
    KAHF connects your computer to an amateur radio and sends what you write to other people's computers, even
    where there is no phone signal at all.
  </p>

  <ol class="how">
    <li><b>You write</b><span class="muted">a message, a voice note or a file, to someone's callsign.</span></li>
    <li><b>Your radio sends it</b><span class="muted">as a short burst of sound, through the air.</span></li>
    <li><b>They receive it</b><span class="muted">on their screen. If they are far, a relay passes it on.</span></li>
  </ol>

  <div class="row">
    <button class="btn primary lg" onclick={() => app.go("setup")}>Set up KAHF <Icon name="arrow" size={16} /></button>
    <button class="btn lg quiet" onclick={explore}>Just look around first</button>
  </div>

  <div class="need">
    <div class="label">You will need</div>
    <ul>
      <li>An amateur radio licence to transmit. Listening needs none.</li>
      <li>A radio for the shortwave bands, or an SDR receiver.</li>
      <li>A cable or sound-card interface between the radio and this computer.</li>
    </ul>
  </div>
</main>

<style>
  .welcome {
    height: 100%;
    overflow: auto;
    max-width: 720px;
    margin: 0 auto;
    padding: 9vh 32px 40px;
    display: flex;
    flex-direction: column;
    gap: 22px;
  }
  .brand {
    font-weight: 800;
    letter-spacing: 0.3em;
    color: var(--accent);
  }
  h1 {
    font-size: clamp(26px, 4vw, 36px);
    line-height: 1.2;
    letter-spacing: -0.02em;
  }
  .lead {
    font-size: 16px;
    line-height: 1.55;
    max-width: 56ch;
  }
  .how {
    list-style: none;
    margin: 0;
    padding: 0;
    border-top: 1px solid var(--line);
  }
  .how li {
    display: grid;
    grid-template-columns: 180px 1fr;
    gap: 16px;
    padding: 13px 0;
    border-bottom: 1px solid var(--line);
  }
  .need ul {
    margin: 8px 0 0;
    padding-left: 18px;
    color: var(--muted);
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
</style>
