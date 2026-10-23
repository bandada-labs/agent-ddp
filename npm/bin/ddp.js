#!/usr/bin/env node
// Runs the native ddp built for this machine. The package carries one binary per platform.
const { spawnSync } = require("child_process");
const { existsSync } = require("fs");
const path = require("path");

const name = `ddp-${process.platform}-${process.arch}${process.platform === "win32" ? ".exe" : ""}`;
const binary = path.join(__dirname, name);
if (!existsSync(binary)) {
  console.error(`ddp: no build for ${process.platform}-${process.arch} in this package.`);
  console.error("next: cargo install --git https://github.com/sebospc/agent-ddp agent-ddp");
  process.exit(1);
}
const result = spawnSync(binary, process.argv.slice(2), { stdio: "inherit" });
if (result.error) {
  console.error(`ddp: ${result.error.message}`);
  process.exit(1);
}
process.exit(result.status ?? 1);
