// guesthouse.ruxn.com: Guesthouse's web page, as static files in public/, with no code of its own.
// Deployed by .github/workflows/site-deploy.yml when site/ changes on main, which runs `cf deploy public`
// in this folder, or by hand the same way. The Windows installer itself is on files.ruxn.com, and the
// Linux release on GitHub. The deploy tool is the one version in package.json and package-lock.json, installed with `npm ci --ignore-scripts`; this file is a plain object and imports nothing.
export default {
  worker: {
    name: "guesthouse-web",
    compatibilityDate: "2026-11-01",
    domains: ["guesthouse.ruxn.com"],
    workersDev: false,
  },
};
