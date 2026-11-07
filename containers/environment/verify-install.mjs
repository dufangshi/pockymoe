// Build-time file checks only: do not start a harness or create a CLI home.
import assert from "node:assert/strict";
import { constants } from "node:fs";
import { access, open, readFile, symlink } from "node:fs/promises";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";

assert.equal(process.platform, "linux");
assert.equal(process.arch, "x64");
assert.equal(process.version, "v24.22.2");

const root = dirname(new URL(import.meta.url).pathname);
const require = createRequire(join(root, "package.json"));
const manifest = JSON.parse(await readFile(join(root, "package.json"), "utf8"));
for (const [name, version] of Object.entries(manifest.dependencies)) {
  const installed = JSON.parse(
    await readFile(join(root, "node_modules", name, "package.json"), "utf8"),
  );
  assert.equal(installed.version, version, name);
}

const codex = join(
  dirname(require.resolve("@openai/codex-linux-x64/package.json")),
  "vendor/x86_64-unknown-linux-musl/bin/codex",
);
// Use the SDK's own native CLI for both ACP and the `claude` command. This
// avoids installing a second copy of the large standalone Claude binary.
const sdkRequire = createRequire(require.resolve("@anthropic-ai/claude-agent-sdk"));
const claude = sdkRequire.resolve("@anthropic-ai/claude-agent-sdk-linux-x64/claude");
for (const path of [codex, claude]) {
  await access(path, constants.X_OK);
  const file = await open(path, "r");
  try {
    const header = Buffer.alloc(20);
    const { bytesRead } = await file.read(header, 0, header.length, 0);
    assert.equal(bytesRead, header.length, path);
    assert.equal(header.subarray(0, 4).toString("hex"), "7f454c46", path);
    assert.equal(header[4], 2, "64-bit ELF required");
    assert.equal(header[5], 1, "little-endian ELF required");
    assert.equal(header.readUInt16LE(18), 62, "x86_64 ELF required");
  } finally {
    await file.close();
  }
}

const commands = {
  codex: join(root, "node_modules/.bin/codex"),
  "codex-acp": join(root, "node_modules/.bin/codex-acp"),
  "claude-agent-acp": join(root, "node_modules/.bin/claude-agent-acp"),
  claude,
};
for (const [name, path] of Object.entries(commands)) {
  await access(path, constants.X_OK);
  await symlink(path, `/usr/local/bin/${name}`);
}
