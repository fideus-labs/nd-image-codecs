// nd-image-codecs bench viewer — scaffold.
//
// Reads BenchRecord JSON (see bench/rs/ndic-bench-core) from the records
// directory this site is generated next to, and renders per-benchmark history
// and config overlays. Fleshed out alongside roadmap Phase 1 workloads; full
// feature set (ref lanes, rate–distortion) in Phase 5.

export async function loadRecords(indexUrl = "./records/index.json") {
  const res = await fetch(indexUrl);
  if (!res.ok) throw new Error(`failed to load ${indexUrl}: ${res.status}`);
  return res.json();
}
