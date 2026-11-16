// Inventaire des licences des bibliothèques livrées avec Vortex Mail.
//   npm run licenses
// Produit :
//   - src/assets/licenses.json        (affiché dans Paramètres › À propos)
//   - THIRD_PARTY_LICENSES.txt        (texte complet, à joindre aux distributions)
// Côté JavaScript : dépendances de production (pas les outils de build).
// Côté Rust : dépendances normales de l'application (via `cargo metadata`).
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const LICENSE_FILE = /^(licen[cs]e|copying|notice)([-._].*)?$/i;

/** Textes de licence trouvés dans un dossier de paquet. */
function licenseTexts(dir) {
  if (!existsSync(dir)) return [];
  return readdirSync(dir)
    .filter((f) => LICENSE_FILE.test(f))
    .sort()
    .map((f) => readFileSync(join(dir, f), "utf8").replace(/\r\n/g, "\n").trim())
    .filter(Boolean);
}

function repoUrl(repo) {
  const url = typeof repo === "string" ? repo : repo?.url;
  if (!url) return "";
  return url.replace(/^git\+/, "").replace(/\.git$/, "").replace(/^git:\/\//, "https://").replace(/^github:/, "https://github.com/");
}

// ---------------------------------------------------------------- npm

function npmPackages() {
  const pkg = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
  const seen = new Map();
  /** Résolution façon Node : node_modules du parent, puis ceux des ancêtres. */
  const resolve = (name, fromDir) => {
    let dir = fromDir;
    for (;;) {
      const candidate = join(dir, "node_modules", name);
      if (existsSync(join(candidate, "package.json"))) return candidate;
      if (dir === root) return null;
      const up = dirname(dir);
      dir = up.endsWith("node_modules") ? dirname(up) : up;
      if (!dir.startsWith(root)) return null;
    }
  };
  const visit = (name, fromDir) => {
    const dir = resolve(name, fromDir);
    if (!dir) return;
    const p = JSON.parse(readFileSync(join(dir, "package.json"), "utf8"));
    const key = `${p.name}@${p.version}`;
    if (seen.has(key)) return;
    const license = typeof p.license === "string" ? p.license : p.license?.type ?? (p.licenses ?? []).map((l) => l.type).join(" OR ");
    seen.set(key, { name: p.name, version: p.version, license: license || "inconnue", url: repoUrl(p.repository) || p.homepage || "", texts: licenseTexts(dir) });
    for (const dep of Object.keys({ ...p.dependencies, ...p.optionalDependencies })) visit(dep, dir);
  };
  for (const dep of Object.keys(pkg.dependencies ?? {})) visit(dep, root);
  return [...seen.values()];
}

// ---------------------------------------------------------------- Rust

/** Plateforme visée : celle de la machine, sauf `LICENSES_TARGET=<triplet>` (ex. x86_64-pc-windows-msvc). */
function targetTriple() {
  if (process.env.LICENSES_TARGET) return process.env.LICENSES_TARGET;
  try {
    return /host: (\S+)/.exec(execFileSync("rustc", ["-vV"], { encoding: "utf8" }))?.[1] ?? null;
  } catch {
    return null;
  }
}

function rustPackages() {
  let meta;
  const target = targetTriple();
  try {
    const out = execFileSync(
      "cargo",
      [
        "metadata",
        "--format-version",
        "1",
        "--manifest-path",
        join(root, "src-tauri", "Cargo.toml"),
        // Seulement les crates compilées pour cette plateforme (pas celles de Windows, macOS, Android…).
        ...(target ? ["--filter-platform", target] : []),
      ],
      { encoding: "utf8", maxBuffer: 256 * 1024 * 1024, stdio: ["ignore", "pipe", "inherit"] },
    );
    meta = JSON.parse(out);
  } catch (e) {
    console.warn(`cargo metadata indisponible (${e.message.split("\n")[0]}) : partie Rust ignorée.`);
    return null;
  }
  const byId = new Map(meta.packages.map((p) => [p.id, p]));
  const nodes = new Map(meta.resolve.nodes.map((n) => [n.id, n]));
  const members = new Set(meta.workspace_members);
  const keep = new Set();
  // Dépendances « normales » depuis l'application (ni build ni dev).
  const stack = [...members];
  while (stack.length) {
    const id = stack.pop();
    for (const d of nodes.get(id)?.deps ?? []) {
      if (!d.dep_kinds.some((k) => k.kind === null)) continue;
      if (keep.has(d.pkg)) continue;
      keep.add(d.pkg);
      stack.push(d.pkg);
    }
  }
  return [...keep]
    .filter((id) => !members.has(id))
    .map((id) => byId.get(id))
    .filter((p) => p && p.source) // paquets du registre (pas les crates du projet)
    .map((p) => ({
      name: p.name,
      version: p.version,
      license: (p.license ?? (p.license_file ? "voir le fichier de licence" : "inconnue")).replace(/\//g, " OR ").replace(/\s+/g, " ").trim(),
      url: p.repository || p.homepage || `https://crates.io/crates/${p.name}`,
      texts: licenseTexts(dirname(p.manifest_path)),
    }));
}

// ---------------------------------------------------------------- sortie

const byName = (a, b) => a.name.localeCompare(b.name) || a.version.localeCompare(b.version, undefined, { numeric: true });
const npm = npmPackages().sort(byName);
const previous = existsSync(join(root, "src/assets/licenses.json"))
  ? JSON.parse(readFileSync(join(root, "src/assets/licenses.json"), "utf8"))
  : null;
const rustNew = rustPackages();
// Sans cargo (ex. aperçu), on garde la partie Rust du précédent inventaire.
const rust = rustNew ? rustNew.sort(byName) : null;

// Textes dédoublonnés : beaucoup de paquets partagent la même licence mot pour mot.
const texts = [];
const textIndex = new Map();
const compact = (list) =>
  list.map(({ texts: t, ...rest }) => ({
    ...rest,
    texts: t.map((text) => {
      if (!textIndex.has(text)) {
        textIndex.set(text, texts.length);
        texts.push(text);
      }
      return textIndex.get(text);
    }),
  }));

let output;
if (rust || !previous) {
  output = { generated: new Date().toISOString().slice(0, 10), npm: compact(npm), rust: compact(rust ?? []), texts };
} else {
  const prevRust = previous.rust.map((p) => ({ ...p, texts: p.texts.map((i) => previous.texts[i]) }));
  output = { generated: new Date().toISOString().slice(0, 10), npm: compact(npm), rust: compact(prevRust), texts };
}
writeFileSync(join(root, "src/assets/licenses.json"), JSON.stringify(output));

const section = (title, list) =>
  `${"=".repeat(78)}\n${title}\n${"=".repeat(78)}\n\n` +
  list
    .map(
      (p) =>
        `${p.name} ${p.version} — ${p.license}\n${p.url}\n\n` +
        (p.texts.length ? p.texts.map((i) => output.texts[i]).join("\n\n---\n\n") : "(aucun fichier de licence fourni par le paquet ; licence déclarée : " + p.license + ")") +
        `\n\n${"-".repeat(78)}\n`,
    )
    .join("\n");
writeFileSync(
  join(root, "THIRD_PARTY_LICENSES.txt"),
  `Vortex Mail — licences des composants tiers\nGénéré le ${output.generated} par scripts/licenses.mjs\n\n` +
    section("Interface (bibliothèques JavaScript)", output.npm) +
    "\n" +
    section("Application (bibliothèques Rust)", output.rust),
);

const count = (list) => list.length;
const ids = new Map();
for (const p of [...output.npm, ...output.rust]) ids.set(p.license, (ids.get(p.license) ?? 0) + 1);
console.log(`${count(output.npm)} paquets JavaScript, ${count(output.rust)} crates Rust${rust ? ` (plateforme ${targetTriple() ?? "toutes"})` : ""}.`);
console.log([...ids].sort((a, b) => b[1] - a[1]).map(([l, n]) => `  ${n.toString().padStart(4)}  ${l}`).join("\n"));

// Licences à réciprocité (même faible) sans alternative permissive : à connaître avant de distribuer.
const permissive = /\b(MIT|MIT-0|Apache-2\.0|BSD-[23]-Clause|ISC|Zlib|0BSD|Unlicense|CC0-1\.0|Unicode-3\.0|BSL-1\.0)\b/;
const flagged = [...output.npm, ...output.rust].filter((p) =>
  p.license
    .split(/\s+AND\s+/)
    .some((part) => !part.split(/\s+OR\s+/).some((choice) => permissive.test(choice))),
);
if (flagged.length) {
  console.log("\nÀ vérifier (pas d'option permissive) :");
  for (const p of flagged) console.log(`  ${p.name} ${p.version} — ${p.license}`);
}
