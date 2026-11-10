/* ath browser relay: run in an https://www.athletic.net tab, as a bookmarklet or pasted into the console.
   Each loop asks ath (on 127.0.0.1) for the next request, performs it in this page with the
   browser's own session, and posts the status and body back. It only performs same-origin
   requests and knows nothing else about Athletic.net. When ath is not running it keeps
   polling quietly, so one click serves every later ath command until the tab is reloaded.
   Only one tab serves at a time; another tab running this waits on standby and takes over
   when the serving tab goes quiet. */
(() => {
  const ATH = "__ATH_RELAY__";
  const LOOPS = 5; /* at most ath's default concurrency, and under Chrome's 6 connections per host */
  if (window.__athRelay) {
    console.log("ath relay: already running in this tab", window.__athRelay);
    return;
  }
  if (location.origin !== "https://www.athletic.net") {
    alert("ath relay: open https://www.athletic.net first, then run this again there.");
    return;
  }
  const TAB = crypto.randomUUID();
  const state = window.__athRelay = { relay: ATH, tab: TAB, mode: "starting", handled: 0 };
  const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
  const MODES = {
    waiting: "waiting for ath to run (this is normal)",
    standby: "on standby, another tab is serving ath",
    serving: "serving ath from this tab; leave it open",
  };
  function setMode(mode) {
    if (state.mode === mode) return;
    state.mode = mode;
    console.log(`ath relay: ${MODES[mode]}`);
  }

  async function perform(job) {
    try {
      const url = new URL(job.url, location.href);
      if (url.origin !== location.origin) {
        return { id: job.id, error: `refused: ${url.origin} is not this page's origin` };
      }
      const r = await fetch(url, {
        method: job.method,
        headers: job.headers || {},
        body: job.body == null ? undefined : job.body,
        credentials: "include",
      });
      return { id: job.id, status: r.status, body: await r.text() };
    } catch (e) {
      return { id: job.id, error: String(e) };
    }
  }

  /* A lost result would leave ath waiting, so try a few times. */
  async function report(result) {
    for (let attempt = 1; attempt <= 4; attempt++) {
      try {
        await fetch(`${ATH}/result`, {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify(result),
        });
        state.handled++;
        return;
      } catch (e) {
        await sleep(500 * attempt);
      }
    }
  }

  async function loop(n) {
    let wait = 500;
    for (;;) {
      /* Unless this tab is serving, only the first loop polls, so the console stays quiet. */
      if (n > 0 && state.mode !== "serving") {
        await sleep(1000);
        continue;
      }
      let r;
      try {
        r = await fetch(`${ATH}/next?tab=${TAB}&loop=${n}`, { cache: "no-store" });
      } catch (e) {
        setMode("waiting");
        await sleep(wait);
        wait = Math.min(wait * 2, 3000);
        continue;
      }
      wait = 500;
      if (r.status === 204) {
        setMode("serving");
        continue;
      }
      let job = null;
      try {
        if (r.status === 200) job = await r.json();
      } catch (e) {
        /* Unreadable: when this loop polls again, ath hands the job out again. */
      }
      if (!job) {
        await sleep(1000);
        continue;
      }
      if (job.standby) {
        setMode("standby");
        await sleep(2000);
        continue;
      }
      setMode("serving");
      await report(await perform(job));
    }
  }

  for (let n = 0; n < LOOPS; n++) loop(n);
})();
