/**
 * Generate `crates/phonix-web/src/icons/generated.rs` from Lucide.
 *
 * Lucide ships every icon as a standalone 24x24 SVG whose children are plain
 * geometry (`path`, `circle`, `rect`, `line`, `polyline`) and whose stroke is
 * `currentColor`. That is the whole reason it was chosen: an icon is a string
 * of markup, not a component, so the Rust side is one enum and one `<svg>`
 * wrapper rather than 2000 generated functions.
 *
 * This runs by hand, not in the build:
 *
 *     npm install lucide-static
 *     node tools/generate-icons.mjs
 *
 * The output is committed. Contributors do not need node to build the app -
 * only to add an icon, which is a deliberate edit to `tools/icons.txt`.
 */

import { readFileSync, writeFileSync, existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const listFile = join(root, "tools", "icons.txt");
const outFile = join(root, "crates", "phonix-web", "src", "icons", "generated.rs");

// Resolved relative to wherever `lucide-static` was installed. Checked
// explicitly so a missing dependency is a sentence rather than a stack trace.
const candidates = [
    join(root, "node_modules", "lucide-static"),
    process.env.LUCIDE_STATIC ?? "",
].filter(Boolean);

const lucide = candidates.find((dir) => existsSync(join(dir, "package.json")));
if (!lucide) {
    console.error(
        "lucide-static not found. Run `npm install lucide-static` in the repo root,\n" +
        "or set LUCIDE_STATIC to an existing installation.",
    );
    process.exit(1);
}

const { version, license } = JSON.parse(
    readFileSync(join(lucide, "package.json"), "utf8"),
);

const names = readFileSync(listFile, "utf8")
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line && !line.startsWith("#"));

const seen = new Set();
for (const name of names) {
    if (seen.has(name)) {
        console.error(`duplicate icon in tools/icons.txt: ${name}`);
        process.exit(1);
    }
    seen.add(name);
}

/** `chevron-right` -> `ChevronRight`. */
const toVariant = (name) =>
    name.split("-").map((part) => part[0].toUpperCase() + part.slice(1)).join("");

/**
 * Everything between `<svg ...>` and `</svg>`, collapsed onto one line.
 *
 * The wrapper is dropped on purpose: size, stroke width and colour are decided
 * by the Rust component, so an icon that carried its own would be immune to
 * them.
 */
function body(name) {
    const file = join(lucide, "icons", `${name}.svg`);
    if (!existsSync(file)) {
        console.error(`no such lucide icon: ${name} (looked in ${file})`);
        process.exit(1);
    }

    const svg = readFileSync(file, "utf8");
    const open = svg.indexOf(">", svg.indexOf("<svg"));
    const close = svg.lastIndexOf("</svg>");
    const inner = svg.slice(open + 1, close);

    const flat = inner.replace(/\s+/g, " ").trim();
    if (!flat) {
        console.error(`lucide icon ${name} has no geometry`);
        process.exit(1);
    }
    if (flat.includes("script") || flat.includes("<use")) {
        console.error(`lucide icon ${name} contains markup we will not inline`);
        process.exit(1);
    }
    return flat;
}

const rows = names.map((name) => ({ name, variant: toVariant(name), body: body(name) }));
rows.sort((a, b) => a.variant.localeCompare(b.variant));

const rust = `//! Icon geometry, generated from Lucide v${version} (${license}).
//!
//! DO NOT EDIT. Add a name to \`tools/icons.txt\` and run:
//!
//! \`\`\`text
//! npm install lucide-static
//! node tools/generate-icons.mjs
//! \`\`\`
//!
//! Each body is the inside of Lucide's 24x24 \`<svg>\`, with the wrapper
//! stripped - see [\`super::Icon\`] for why.

/// Every icon the application can draw.
///
/// The variants are the whole vocabulary: a screen cannot reach for an icon
/// that has not been through \`tools/icons.txt\`, which is what keeps the bundle
/// from quietly growing an icon at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Icon {
${rows.map((r) => `    /// Lucide \`${r.name}\`.\n    ${r.variant},`).join("\n")}
}

impl Icon {
    /// The icon's Lucide name, e.g. \`"chevron-right"\`.
    ///
    /// Stable enough to persist: it is what a data-driven menu would store.
    pub const fn key(self) -> &'static str {
        match self {
${rows.map((r) => `            Self::${r.variant} => "${r.name}",`).join("\n")}
        }
    }

    /// The SVG geometry, without the \`<svg>\` wrapper.
    pub const fn body(self) -> &'static str {
        match self {
${rows.map((r) => `            Self::${r.variant} => r#"${r.body}"#,`).join("\n")}
        }
    }

    /// Every icon, in variant order. Drives the icon gallery and the tests.
    pub const ALL: &'static [Icon] = &[
${rows.map((r) => `        Icon::${r.variant},`).join("\n")}
    ];
}

impl core::str::FromStr for Icon {
    type Err = ();

    /// Parse a Lucide name back into a variant.
    ///
    /// For the day menus come out of a database rather than out of
    /// \`navigation::tree\`: an unknown name is an error, never a blank space.
    fn from_str(name: &str) -> Result<Self, Self::Err> {
        match name {
${rows.map((r) => `            "${r.name}" => Ok(Self::${r.variant}),`).join("\n")}
            _ => Err(()),
        }
    }
}
`;

writeFileSync(outFile, rust);
console.log(`wrote ${rows.length} icons from lucide-static ${version} -> ${outFile}`);
