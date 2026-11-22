// guesthouse.ruxn.com: Guesthouse's web page, as static files in public/, with no code of its own.
// Deployed from this folder with `cf deploy public`. The Windows installer itself is on
// files.ruxn.com, and the Linux release on GitHub. A plain object, so the folder needs no packages
// installed.
export default {
  worker: {
    name: "guesthouse-web",
    compatibilityDate: "2026-11-01",
    domains: ["guesthouse.ruxn.com"],
    workersDev: false,
  },
};
