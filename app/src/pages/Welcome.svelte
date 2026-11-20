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

<main class="desk">
  <section class="window" aria-label="Welcome to KAHF">
    <div class="titlebar">Welcome to KAHF</div>
    <div class="cols">
      <aside class="side">
        <div class="brand">KAHF</div>
        <p>Messages, mail, files and voice over amateur radio.</p>
      </aside>
      <div class="content">
        <h1>No internet needed</h1>
        <p class="lead">
          KAHF connects your computer to an amateur radio and sends what you write to other people's computers, even
          where there is no phone signal at all.
        </p>
        <ol class="how">
          <li><b>You write</b><span class="muted">a message, a voice note or a file, to someone's callsign.</span></li>
          <li><b>Your radio sends it</b><span class="muted">as a short burst of sound, through the air.</span></li>
          <li><b>They receive it</b><span class="muted">on their screen. If they are far, a relay passes it on.</span></li>
        </ol>
        <fieldset class="need">
          <legend>You will need</legend>
          <ul>
            <li>An amateur radio licence to transmit. Listening needs none.</li>
            <li>A radio for the shortwave bands, or an SDR receiver.</li>
            <li>A cable or sound-card interface between the radio and this computer.</li>
          </ul>
        </fieldset>
      </div>
    </div>
    <div class="buttons">
      <button class="btn" onclick={explore}>Just look around</button>
      <button class="btn primary" onclick={() => app.go("setup")}>Set up KAHF <Icon name="arrow" size={14} /></button>
    </div>
  </section>
</main>

<style>
  .desk {
    height: 100%;
    overflow: auto;
    display: grid;
    place-items: center;
    padding: 24px;
  }
  .window {
    width: min(860px, 100%);
    background: var(--bg);
    border: 1px solid var(--line-strong);
    border-radius: 8px;
    box-shadow: var(--shadow);
    overflow: hidden;
  }
  .cols {
    display: grid;
    grid-template-columns: 220px 1fr;
  }
  .side {
    padding: 22px 18px;
    color: var(--title-ink);
    background: linear-gradient(170deg, var(--title-top), var(--title-bot));
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .side p {
    font-size: 13px;
    opacity: 0.92;
  }
  .brand {
    font-weight: 700;
    letter-spacing: 0.2em;
    font-size: 20px;
  }
  .content {
    background: var(--panel);
    padding: 20px 24px 18px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .lead {
    line-height: 1.5;
  }
  .how {
    list-style: none;
    margin: 0;
    padding: 0;
    border: 1px solid var(--line);
    border-radius: 6px;
  }
  .how li {
    display: grid;
    grid-template-columns: 140px 1fr;
    gap: 12px;
    padding: 8px 12px;
    border-bottom: 1px solid var(--line);
  }
  .how li:last-child {
    border-bottom: 0;
  }
  .need {
    margin: 0;
    border: 1px solid var(--line);
    border-radius: 6px;
    padding: 4px 14px 10px;
  }
  .need legend {
    padding: 0 6px;
    font-weight: 600;
    color: var(--muted);
  }
  .need ul {
    margin: 4px 0 0;
    padding-left: 18px;
    color: var(--muted);
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .buttons {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    padding: 10px 14px;
    border-top: 1px solid var(--line-strong);
  }
  @media (max-width: 560px) {
    .cols {
      grid-template-columns: 1fr;
    }
  }
</style>
