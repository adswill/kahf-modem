<script lang="ts">
  import { app } from "../lib/state.svelte";
  import Icon from "../components/Icon.svelte";

  async function explore() {
    // Look around without transmitting: listening only, nothing to configure.
    await app.saveSettings({ listenOnly: true, setupDone: false });
    app.page = "home";
  }
</script>

<main class="welcome">
  <div class="hero">
    <div class="logo" aria-hidden="true">K</div>
    <h1>Send messages by radio.<br />No internet. No phone network.</h1>
    <p class="lead muted">
      KAHF turns your computer and an amateur radio into a way to send messages, mail and files to other
      people, even where there is no signal at all.
    </p>
    <div class="row wrap cta">
      <button class="btn primary big" onclick={() => app.go("setup")}>
        Set up in 2 minutes <Icon name="arrow" />
      </button>
      <button class="btn ghost big" onclick={explore}>Just look around first</button>
    </div>
  </div>

  <section class="how" aria-label="How it works">
    <div class="step">
      <div class="num">1</div>
      <h3>You write</h3>
      <p class="muted">Type a message to someone's callsign, like a text message. That is all you do.</p>
    </div>
    <div class="step">
      <div class="num">2</div>
      <h3>Your radio sends it</h3>
      <p class="muted">
        KAHF turns the words into a short burst of sound and gives it to your radio. It travels through
        the air, even across the world.
      </p>
    </div>
    <div class="step">
      <div class="num">3</div>
      <h3>They receive it</h3>
      <p class="muted">Their KAHF hears it and shows it on their screen. If they are too far, a relay passes it on.</p>
    </div>
  </section>

  <section class="need card flat">
    <h2>What you need</h2>
    <ul>
      <li><strong>An amateur radio licence</strong> to transmit. Listening needs no licence at all.</li>
      <li><strong>A radio</strong> that can send and receive on the shortwave bands, or an SDR receiver.</li>
      <li><strong>A cable or sound-card interface</strong> between the radio and this computer.</li>
    </ul>
    <p class="small muted">
      No radio yet? Choose "Just look around" and KAHF will only listen. You can set the rest up later.
    </p>
  </section>
</main>

<style>
  .welcome {
    height: 100%;
    overflow: auto;
    padding: 6vh 8vw 40px;
    display: flex;
    flex-direction: column;
    gap: 40px;
    max-width: 1100px;
    margin: 0 auto;
  }
  .hero {
    display: flex;
    flex-direction: column;
    gap: 20px;
    align-items: flex-start;
  }
  .logo {
    width: 56px;
    height: 56px;
    border-radius: 16px;
    background: var(--accent);
    color: var(--accent-ink);
    display: grid;
    place-items: center;
    font-weight: 800;
    font-size: 30px;
  }
  h1 {
    font-size: clamp(30px, 4.4vw, 46px);
  }
  .lead {
    font-size: 18px;
    max-width: 640px;
  }
  .cta {
    margin-top: 6px;
  }
  .how {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(240px, 1fr));
    gap: 16px;
  }
  .step {
    background: var(--panel);
    border: 1px solid var(--line-soft);
    border-radius: 16px;
    padding: 22px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .num {
    width: 30px;
    height: 30px;
    border-radius: 50%;
    background: var(--accent-soft);
    color: var(--accent);
    font-weight: 700;
    display: grid;
    place-items: center;
  }
  .need ul {
    margin: 12px 0;
    padding-left: 20px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
</style>
