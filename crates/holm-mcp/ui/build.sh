#!/usr/bin/env bash
# One file, nothing fetched at render time: a host's sandbox allows the page only what
# the resource declared, and a script host is one more thing to declare and to trust.
set -euo pipefail
cd "$(dirname "$0")"

npm ci --silent --no-audit --no-fund
npx esbuild src/app.js --bundle --minify --format=esm --target=es2022 \
  --outfile=dist/app.js --log-level=warning

node - <<'JS'
const fs = require("fs");
const html = fs.readFileSync("src/screen.html", "utf8");
if (!html.includes("<!--APP-->")) throw new Error("src/screen.html has no <!--APP--> marker");
// A closing tag inside a string would end the inline script early.
const js = fs.readFileSync("dist/app.js", "utf8").replace(/<\/script/gi, "<\\/script");
fs.writeFileSync("screen.html", html.replace("<!--APP-->", () => `<script type="module">${js}</script>`));
JS

printf 'screen.html: %s bytes\n' "$(wc -c < screen.html | tr -d ' ')"
