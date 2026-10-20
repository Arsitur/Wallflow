import { readFileSync } from 'node:fs';

const json = path => JSON.parse(readFileSync(new URL(path, import.meta.url), 'utf8'));
const version = json('../package.json').version;
const lock = json('../package-lock.json');
const config = json('../src-tauri/tauri.conf.json');
const cargo = readFileSync(new URL('../src-tauri/Cargo.toml', import.meta.url), 'utf8').match(/^version\s*=\s*"([^"]+)"/m)?.[1];
const cargoLock = readFileSync(new URL('../src-tauri/Cargo.lock', import.meta.url), 'utf8').split('[[package]]')
  .find(section => /^name = "wallflow"$/m.test(section))?.match(/^version = "([^"]+)"/m)?.[1];
for (const [file, actual] of Object.entries({ 'package-lock.json': lock.version, 'package-lock.json root': lock.packages[''].version, 'tauri.conf.json': config.version, 'Cargo.toml': cargo, 'Cargo.lock': cargoLock })) {
  if (actual !== version) throw new Error(`${file} has version ${actual}; expected ${version}. Update all version files before releasing.`);
}
if (process.env.RELEASE_TAG && process.env.RELEASE_TAG !== `v${version}`) {
  throw new Error(`Tag ${process.env.RELEASE_TAG} does not match app version v${version}.`);
}
console.log(`Release version verified: v${version}`);
